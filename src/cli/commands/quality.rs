use cargo_metadata::{Metadata, MetadataCommand};
use serde::Deserialize;
use std::fs;

use crate::cli::commands::deps;
use crate::core::analyzers::{
    audit::run_vulnerability_audit, clippy::run_clippy, unsafe_code::run_unsafe, unused::run_unused,
};
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{
    print_json, AuditOutput, ClippyOutput, DepsOutput, QualityOutput, SkipReason, UnsafeOutput,
    UnusedOutput,
};
use crate::core::scorers::quality::{
    score_quality, Availability, ClippySummary, DependencyFreshness, MsrvStatus, QualityInputs,
    SecuritySummary, UnsafeSummary, UnusedSummary,
};

pub async fn run(json: bool) -> Result<()> {
    let deps_future = deps::analyze(false);
    let audit_future = run_blocking("audit", run_vulnerability_audit);
    let clippy_future = run_clippy();
    let msrv_future = check_msrv();
    let unused_future = run_unused();
    let unsafe_future = run_unsafe();

    let (deps_result, audit_result, clippy_result, msrv_result, unused_result, unsafe_result) = tokio::join!(
        deps_future,
        audit_future,
        clippy_future,
        msrv_future,
        unused_future,
        unsafe_future
    );

    let output = build_quality_output(
        deps_result,
        audit_result,
        clippy_result,
        msrv_result,
        unused_result,
        unsafe_result,
    );

    emit_output(json, &output)
}

fn emit_output(json: bool, output: &QualityOutput) -> Result<()> {
    if json {
        print_json(output)
    } else {
        println!("{output}");
        Ok(())
    }
}

/// Converts an analyzer result into scorer input.
///
/// A failure becomes [`Availability::Unavailable`] rather than a synthesized
/// healthy summary. Every substitution that used to happen here — a
/// zero-vulnerability `SecuritySummary`, a `None` that the scorer read as 100 —
/// presented "we could not check" as "we checked and it was clean".
///
/// Dependency freshness does not use this: an `Ok` from `deps::analyze` can
/// still mean nothing was checked, so it needs [`dependency_freshness`].
fn availability<T, U>(result: Result<T>, map: impl FnOnce(T) -> U) -> Availability<U> {
    match result {
        Ok(value) => Availability::Measured(map(value)),
        Err(err) => unavailable_from(&err),
    }
}

/// Separates "the optional tool is not installed" from "the analyzer failed".
///
/// Only the former is the user's to act on, and neither says anything about
/// project health. This relies on `ErrorCode::MissingTool` actually firing,
/// which is why `is_missing_subcommand` has to track cargo's wording.
///
/// The error message already carries the install hint for a missing tool, and
/// `UnavailableMetric::name` already says which metric, so the error's own text
/// is the whole detail.
fn unavailable_from<T>(err: &UpkeepError) -> Availability<T> {
    if err.code() == ErrorCode::MissingTool {
        Availability::not_installed(err.to_string())
    } else {
        Availability::failed(err.to_string())
    }
}

/// Builds the freshness input over the dependencies that were actually checked.
///
/// When crates.io is unreachable `analyze` still returns `Ok`: every dependency
/// lands in `skipped_packages` with [`SkipReason::RegistryUnavailable`] and
/// `outdated` is 0. Scoring that as `(N - 0) / N * 100 = 100` with
/// `complete: true` is an offline run printing a full-weight `A`. So a
/// dependency that could not be checked leaves the denominator instead of
/// counting as up to date, and when that leaves nothing checked the metric is
/// unavailable rather than perfect.
///
/// **The denominator is [`DepsOutput::checked`], never `total - skipped`.**
/// `total` counts dependency *edges* — raw declarations, so a crate in both
/// `[dependencies]` and `[dev-dependencies]` is 2 — while `outdated` and the
/// skip lists count *groups*, where that crate is 1. Subtracting one from the
/// other left a positive `checked` on a run where nothing was compared at all,
/// which scored a perfect 100 at full weight: the very bug this function was
/// written to fix, reintroduced by the arithmetic used to fix it. `analyze` is
/// the only place that can see both units, so it derives the denominator and
/// this reads it.
///
/// `checked` already accounts for the skips that mean "not applicable" —
/// `NonRegistry`, `TargetSpecific`, `OptionalNotActivated`. There is no newer
/// version for a git or path dependency to be behind, so dropping them would
/// misreport the denominator in the other direction. Unsupported registries and
/// successful responses without version metadata remain unanswered comparisons,
/// as does a declared dependency missing from Cargo's resolve graph — or present
/// in it under a package name that resolved to several distinct versions, which
/// `deps` refuses to guess between.
fn dependency_freshness(result: Result<DepsOutput>) -> Availability<DependencyFreshness> {
    let output = match result {
        Ok(output) => output,
        Err(err) => return unavailable_from(&err),
    };

    let registry_unavailable = output
        .skipped_packages
        .iter()
        .any(|skipped| skipped.reason == SkipReason::RegistryUnavailable);
    let unsupported_registry = output
        .skipped_packages
        .iter()
        .any(|skipped| skipped.reason == SkipReason::UnsupportedRegistry);
    let registry_metadata_missing = output
        .skipped_packages
        .iter()
        .any(|skipped| skipped.reason == SkipReason::RegistryMetadataMissing);
    // An ambiguous resolve is an unanswered comparison for the same reason a missing
    // one is: the declaration was there and no version could be compared. Leaving it
    // out here would let a run whose only dependency was ambiguous report a measured
    // freshness of 100 over an empty denominator.
    let unanswered_resolve = output.skipped_packages.iter().any(|skipped| {
        matches!(
            skipped.reason,
            SkipReason::MissingResolve | SkipReason::AmbiguousResolve
        )
    });

    // `checked == 0` with nothing skipped is a project with no dependencies,
    // which genuinely scores 100. An unanswered registry comparison makes it
    // unmeasured instead, whether it failed, lacked metadata, or used a registry
    // whose API cargo-upkeep does not support.
    if output.checked == 0
        && (registry_unavailable
            || unsupported_registry
            || registry_metadata_missing
            || unanswered_resolve)
    {
        let detail = if registry_unavailable
            && !unsupported_registry
            && !registry_metadata_missing
            && !unanswered_resolve
        {
            "the crates.io registry was unavailable"
        } else if unsupported_registry
            && !registry_unavailable
            && !registry_metadata_missing
            && !unanswered_resolve
        {
            "no supported registry comparison was available"
        } else {
            "no supported registry comparison could be completed"
        };
        return Availability::failed(format!(
            "{detail}; none of the {} declared dependencies could be checked for newer versions",
            output.total
        ));
    }

    Availability::Measured(DependencyFreshness {
        total: output.checked,
        outdated: output.outdated,
    })
}

fn build_quality_output(
    deps_result: Result<DepsOutput>,
    audit_result: Result<AuditOutput>,
    clippy_result: Result<ClippyOutput>,
    msrv_result: Result<MsrvStatus>,
    unused_result: Result<UnusedOutput>,
    unsafe_result: Result<UnsafeOutput>,
) -> QualityOutput {
    score_quality(QualityInputs {
        dependency_freshness: dependency_freshness(deps_result),
        security: availability(audit_result, |output| SecuritySummary {
            critical: output.summary.critical,
            high: output.summary.high,
            moderate: output.summary.moderate,
            low: output.summary.low,
        }),
        unused: availability(unused_result, |output| UnusedSummary {
            unused_count: output.unused.len(),
        }),
        unsafe_code: availability(unsafe_result, |output| UnsafeSummary {
            total_unsafe: output.summary.total_unsafe,
        }),
        clippy: availability(clippy_result, |output| ClippySummary {
            warnings: output.warnings,
            errors: output.errors,
        }),
        // `MsrvStatus::Missing` stays a real finding scoring 50: the project
        // genuinely did not declare `rust-version`. Only a *failed* MSRV check
        // is unavailable — the two used to be conflated here.
        msrv: availability(msrv_result, |status| status),
    })
}

async fn check_msrv() -> Result<MsrvStatus> {
    run_blocking("MSRV check", || {
        let metadata = MetadataCommand::new().exec().map_err(|err| {
            UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
        })?;
        msrv_status(&metadata)
    })
    .await
}

#[derive(Debug, Deserialize)]
struct WorkspaceManifest {
    workspace: Option<WorkspaceTable>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceTable {
    package: Option<WorkspacePackage>,
}

#[derive(Debug, Deserialize)]
struct WorkspacePackage {
    #[serde(rename = "rust-version")]
    rust_version: Option<String>,
}

fn msrv_status(metadata: &Metadata) -> Result<MsrvStatus> {
    if let Some(root) = metadata.root_package() {
        return Ok(if root.rust_version.is_some() {
            MsrvStatus::Valid
        } else {
            MsrvStatus::Missing
        });
    }

    // Cargo resolves `rust-version.workspace = true` onto each member package in
    // metadata. When every member exposes a version, the virtual workspace has a
    // complete member-level MSRV declaration and needs no manifest fallback. Requiring
    // every member avoids treating a partially declared workspace as healthy.
    let workspace_packages = metadata.workspace_packages();
    if !workspace_packages.is_empty()
        && workspace_packages
            .iter()
            .all(|package| package.rust_version.is_some())
    {
        return Ok(MsrvStatus::Valid);
    }

    // `[workspace.package]` values are not emitted in cargo metadata unless a member
    // inherits them. Read the virtual workspace manifest itself so a declared
    // workspace-wide MSRV still counts even before members opt into inheritance.
    let manifest_path = metadata.workspace_root.join("Cargo.toml");
    let contents = fs::read_to_string(&manifest_path).map_err(|err| {
        UpkeepError::context(
            ErrorCode::Metadata,
            format!("failed to read workspace manifest {manifest_path}"),
            err,
        )
    })?;
    let manifest: WorkspaceManifest = toml::from_str(&contents).map_err(|err| {
        UpkeepError::context(
            ErrorCode::Metadata,
            format!("failed to parse workspace manifest {manifest_path}"),
            err,
        )
    })?;

    Ok(
        if manifest
            .workspace
            .and_then(|workspace| workspace.package)
            .and_then(|package| package.rust_version)
            .is_some()
        {
            MsrvStatus::Valid
        } else {
            MsrvStatus::Missing
        },
    )
}

async fn run_blocking<T, F>(label: &str, func: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(func).await.map_err(|err| {
        UpkeepError::message(ErrorCode::TaskFailed, format!("{label} task failed: {err}"))
    })?
}

#[cfg(test)]
mod tests {
    use super::{build_quality_output, check_msrv, msrv_status, run_blocking, MsrvStatus};
    use crate::core::analyzers::clippy::parse_diagnostics;
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::{
        AuditOutput, AuditSummary, AuditWarning, AuditWarningKind, ClippyOutput, Confidence,
        DependencyType, DepsOutput, Grade, MetricScore, OutdatedPackage, QualityOutput, Severity,
        SkipReason, SkippedDependency, UnavailableMetric, UnavailableReason, UnsafeOutput,
        UnsafePackage, UnsafeSummary as UnsafeOutputSummary, UnusedDep, UnusedOutput, UpdateType,
        Vulnerability,
    };
    use crate::core::scorers::quality::{
        score_quality, Availability, QualityInputs, SecuritySummary, UnsafeSummary, UnusedSummary,
        METRIC_CLIPPY, METRIC_DEPENDENCY_FRESHNESS, METRIC_MSRV, METRIC_SECURITY,
        METRIC_UNSAFE_CODE, METRIC_UNUSED_DEPS, WEIGHT_UNUSED_DEPS,
    };
    use cargo_metadata::{Metadata, MetadataCommand};
    use serde_json::Value;
    use std::fs;
    use tempfile::TempDir;

    const FLOAT_TOLERANCE: f32 = 0.01;

    fn err() -> UpkeepError {
        UpkeepError::message(ErrorCode::TaskFailed, "boom")
    }

    /// A `DepsOutput` shaped the way `deps::analyze` actually shapes one.
    ///
    /// **`declared` and `checked` are different units, and the gap between them
    /// is the point of this signature.** `declared` lands in `total` and counts
    /// dependency *edges*: raw declarations, no deduplication, dev and build
    /// kinds included. `checked` counts *groups* — edges merged by
    /// `(name, resolved version)` — which is also the unit of `outdated` and of
    /// the deduplicated `skipped_packages`. One crate in both `[dependencies]`
    /// and `[dev-dependencies]` is 2 declared and 1 checked.
    ///
    /// The earlier helper took only `declared` and set `total: declared` while
    /// generating `declared` *distinct* names, so it could only ever model
    /// `total == groups` — the one case where reconstructing the denominator as
    /// `total - registry_skipped` happens to be right. Production violates that
    /// constantly, and the tests passed anyway. Hence the assertions below:
    /// the helper now refuses to build a `DepsOutput` that `analyze` could not
    /// produce.
    fn deps_output(
        declared: usize,
        checked: usize,
        outdated: usize,
        registry_skipped: usize,
    ) -> DepsOutput {
        assert!(
            outdated <= checked,
            "outdated ({outdated}) cannot exceed checked ({checked}): a dependency is only \
             reported outdated after its latest version was compared"
        );
        assert!(
            checked + registry_skipped <= declared,
            "checked ({checked}) + registry_skipped ({registry_skipped}) cannot exceed declared \
             edges ({declared}): grouping and skip deduplication only ever shrink the count"
        );

        let packages: Vec<OutdatedPackage> = (0..outdated)
            .map(|index| OutdatedPackage {
                name: format!("outdated-{index}"),
                alias: None,
                current: "1.0.0".to_string(),
                latest: "2.0.0".to_string(),
                required: "1.0".to_string(),
                update_type: UpdateType::Major,
                dependency_type: DependencyType::Normal,
                members: vec!["demo".to_string()],
            })
            .collect();

        let skipped_packages: Vec<SkippedDependency> = (0..registry_skipped)
            .map(|index| SkippedDependency {
                name: format!("unchecked-{index}"),
                alias: None,
                required: "1.0".to_string(),
                reason: SkipReason::RegistryUnavailable,
                dependency_type: DependencyType::Normal,
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                target: None,
            })
            .collect();

        DepsOutput {
            total: declared,
            checked,
            outdated: packages.len(),
            major: packages.len(),
            minor: 0,
            patch: 0,
            packages,
            skipped: skipped_packages.len(),
            skipped_packages,
            warnings: Vec::new(),
            security: None,
            workspace: false,
            members: vec!["demo".to_string()],
            skipped_members: Vec::new(),
        }
    }

    fn clean_audit() -> AuditOutput {
        AuditOutput {
            vulnerabilities: Vec::new(),
            warnings: Vec::new(),
            summary: AuditSummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
                total: 0,
            },
        }
    }

    fn warning_only_audit() -> AuditOutput {
        AuditOutput {
            vulnerabilities: Vec::new(),
            warnings: vec![AuditWarning {
                kind: AuditWarningKind::Unmaintained,
                package: "legacy".to_string(),
                package_version: "1.0.0".to_string(),
                advisory_id: Some("RUSTSEC-2099-0002".to_string()),
                title: Some("Legacy is unmaintained".to_string()),
                path: vec!["demo".to_string(), "legacy".to_string()],
                fix_available: Some(false),
            }],
            summary: AuditSummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
                total: 0,
            },
        }
    }

    fn clean_clippy() -> ClippyOutput {
        ClippyOutput {
            warnings: 0,
            errors: 0,
            warnings_by_lint: Default::default(),
            details: Vec::new(),
            score: 100.0,
        }
    }

    fn clean_unused() -> UnusedOutput {
        UnusedOutput {
            unused: Vec::new(),
            possibly_unused: Vec::new(),
        }
    }

    fn clean_unsafe() -> UnsafeOutput {
        UnsafeOutput {
            packages: Vec::new(),
            summary: UnsafeOutputSummary {
                packages: 0,
                unsafe_functions: 0,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 0,
                unsafe_expressions: 0,
                total_unsafe: 0,
            },
        }
    }

    // === Asymmetric fixtures ===
    //
    // The `clean_*` fixtures above are all zeros or all empty, which is why a
    // swapped or misread field in the `build_quality_output` mappings is
    // invisible to every test that uses them (#51). These are the same shapes
    // with values chosen so a misread field changes the score. They are
    // additions, not replacements: changing a `clean_*` fixture would move the
    // expected score of every test that already depends on it being clean.

    /// The four severity counts `asymmetric_audit` reports, in the field order
    /// of `SecuritySummary`: `[critical, high, moderate, low]`.
    ///
    /// **They must stay pairwise distinct.** Swapping the mapping's fields `i`
    /// and `j` moves the penalty by `(weight_i - weight_j) * (count_j - count_i)`,
    /// so distinct counts are necessary — and, while the four severity weights
    /// stay distinct from each other, sufficient.
    ///
    /// Necessary and sufficient for the *penalty*, that is. A moved penalty can
    /// still land on an equal *score*, because the score clamps at 0 and 100:
    /// three of the six swaps here exceed a penalty of 100 and all report 0.
    /// They are still caught, because each is compared against the unclamped
    /// baseline rather than against each other.
    ///
    /// Neither premise is assumed. The weights are private to the scorer, so
    /// `security_summary_mapping_is_not_swappable` runs every pairwise swap
    /// through the real scorer and fails if any of them is invisible.
    const SECURITY_COUNTS: [usize; 4] = [1, 2, 3, 4];

    fn vulnerability(index: usize, severity: Severity) -> Vulnerability {
        Vulnerability {
            id: format!("RUSTSEC-2024-{index:04}"),
            package: format!("vulnerable-{index}"),
            package_version: "1.0.0".to_string(),
            severity,
            title: "example advisory".to_string(),
            path: vec!["demo".to_string(), format!("vulnerable-{index}")],
            fix_available: true,
        }
    }

    /// An `AuditOutput` carrying `SECURITY_COUNTS`.
    ///
    /// Built the way `audit::summarize` builds one — every counted
    /// vulnerability is in `vulnerabilities` and `total` is that list's length
    /// — so this is a value `run_audit` could actually return rather than a
    /// summary invented independently of its evidence.
    fn asymmetric_audit() -> AuditOutput {
        let [critical, high, moderate, low] = SECURITY_COUNTS;
        // `Severity` is neither `Copy` nor `Clone`, so each entry is built from
        // a constructor rather than cloned from one value per severity.
        let severities: [(usize, fn() -> Severity); 4] = [
            (critical, || Severity::Critical),
            (high, || Severity::High),
            (moderate, || Severity::Moderate),
            (low, || Severity::Low),
        ];

        let mut vulnerabilities = Vec::new();
        for (count, severity) in severities {
            for _ in 0..count {
                let index = vulnerabilities.len();
                vulnerabilities.push(vulnerability(index, severity()));
            }
        }

        AuditOutput {
            summary: AuditSummary {
                critical,
                high,
                moderate,
                low,
                total: vulnerabilities.len(),
            },
            vulnerabilities,
            warnings: Vec::new(),
        }
    }

    /// How many entries `asymmetric_unused` puts in each of its two lists.
    ///
    /// Different from each other, and their sum differs from both, so reading
    /// `possibly_unused` — or summing the two — scores differently from reading
    /// `unused`. `unused_summary_mapping_reads_the_confirmed_list` checks that
    /// against the real scorer.
    const UNUSED_CONFIRMED: usize = 2;
    const UNUSED_POSSIBLE: usize = 5;

    fn asymmetric_unused() -> UnusedOutput {
        UnusedOutput {
            unused: (0..UNUSED_CONFIRMED)
                .map(|index| UnusedDep {
                    name: format!("unused-{index}"),
                    dependency_type: DependencyType::Normal,
                    confidence: Confidence::High,
                })
                .collect(),
            possibly_unused: (0..UNUSED_POSSIBLE)
                .map(|index| format!("possibly-unused-{index}"))
                .collect(),
        }
    }

    fn unsafe_package(name: &str, counts: [usize; 5]) -> UnsafePackage {
        let [unsafe_functions, unsafe_impls, unsafe_traits, unsafe_blocks, unsafe_expressions] =
            counts;
        UnsafePackage {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            package_id: None,
            unsafe_functions,
            unsafe_impls,
            unsafe_traits,
            unsafe_blocks,
            unsafe_expressions,
            // `parse_geiger_output` derives the per-package total the same way.
            total_unsafe: counts.iter().sum(),
        }
    }

    /// An `UnsafeOutput` whose seven summary fields all hold different counts.
    ///
    /// The summary is accumulated from the packages exactly as
    /// `unsafe_code::summarize` accumulates it, so the totals stay consistent
    /// with their evidence. The per-package counts are chosen to make the six
    /// sibling fields distinct from `total_unsafe`, which is the field the
    /// mapping is supposed to read.
    fn asymmetric_unsafe() -> UnsafeOutput {
        let packages = vec![
            unsafe_package("alpha", [2, 1, 3, 2, 4]),
            unsafe_package("beta", [1, 0, 1, 3, 2]),
        ];

        let mut summary = UnsafeOutputSummary {
            packages: packages.len(),
            unsafe_functions: 0,
            unsafe_impls: 0,
            unsafe_traits: 0,
            unsafe_blocks: 0,
            unsafe_expressions: 0,
            total_unsafe: 0,
        };
        for package in &packages {
            summary.unsafe_functions += package.unsafe_functions;
            summary.unsafe_impls += package.unsafe_impls;
            summary.unsafe_traits += package.unsafe_traits;
            summary.unsafe_blocks += package.unsafe_blocks;
            summary.unsafe_expressions += package.unsafe_expressions;
            summary.total_unsafe += package.total_unsafe;
        }

        UnsafeOutput { packages, summary }
    }

    /// The error an uninstalled optional cargo tool actually produces, once
    /// `is_missing_subcommand` classifies current cargo's stderr correctly.
    fn missing_tool(tool: &str) -> UpkeepError {
        UpkeepError::message(
            ErrorCode::MissingTool,
            format!("cargo-{tool} is not installed; install with `cargo install cargo-{tool}`"),
        )
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < FLOAT_TOLERANCE,
            "expected {expected}, got {actual}"
        );
    }

    fn score_of(output: &QualityOutput, name: &str) -> Option<f32> {
        output
            .breakdown
            .iter()
            .find(|metric| metric.name == name)
            .unwrap_or_else(|| panic!("missing breakdown entry for {name}"))
            .score
    }

    /// `QualityInputs` with every metric unavailable, to be overridden one
    /// field at a time with `..`.
    ///
    /// A mapping test scores exactly one metric so the assertion cannot be
    /// satisfied by some other metric moving the aggregate.
    fn nothing_measured() -> QualityInputs {
        QualityInputs {
            dependency_freshness: Availability::failed("not under test"),
            security: Availability::failed("not under test"),
            unused: Availability::failed("not under test"),
            unsafe_code: Availability::failed("not under test"),
            clippy: Availability::failed("not under test"),
            msrv: Availability::failed("not under test"),
        }
    }

    /// The Security breakdown score the real scorer gives these counts, in
    /// `[critical, high, moderate, low]` order.
    ///
    /// This is the reference side of the security mapping test: the scorer is
    /// production code, so the test pins the `AuditOutput` -> `SecuritySummary`
    /// correspondence without restating a penalty weight or a literal score.
    fn security_reference_score(counts: [usize; 4]) -> Option<f32> {
        let [critical, high, moderate, low] = counts;
        let output = score_quality(QualityInputs {
            security: Availability::Measured(SecuritySummary {
                critical,
                high,
                moderate,
                low,
            }),
            ..nothing_measured()
        });
        score_of(&output, METRIC_SECURITY)
    }

    fn unused_reference_score(unused_count: usize) -> Option<f32> {
        let output = score_quality(QualityInputs {
            unused: Availability::Measured(UnusedSummary { unused_count }),
            ..nothing_measured()
        });
        score_of(&output, METRIC_UNUSED_DEPS)
    }

    fn unsafe_reference_score(total_unsafe: usize) -> Option<f32> {
        let output = score_quality(QualityInputs {
            unsafe_code: Availability::Measured(UnsafeSummary { total_unsafe }),
            ..nothing_measured()
        });
        score_of(&output, METRIC_UNSAFE_CODE)
    }

    /// Asserts a fixture lands where a misread field can still move it.
    ///
    /// Every penalty here is `100u64.saturating_sub(..)`, so a fixture scoring
    /// 100 (nothing to penalize) or 0 (clamped) can absorb a wrong count
    /// without changing the number. A mapping test whose fixture sat at either
    /// end would pass while proving nothing.
    ///
    /// The `expect` below is load-bearing rather than defensive. The callers
    /// compare two `Option<f32>`, which would agree vacuously if the metric were
    /// unavailable on both sides — a fixture wired into the wrong argument slot
    /// scores `None`, and `None == None`. Unwrapping here forces the expected
    /// side to be a real measurement before any comparison is reached.
    fn assert_sensitive(score: Option<f32>) {
        let score = score.expect("the metric under test must be measured");
        assert!(
            score > 0.0 && score < 100.0,
            "fixture scores {score}, where a misread count could be absorbed by the \
             0/100 clamp; choose counts that land strictly inside the range"
        );
    }

    fn unavailable_entry<'a>(output: &'a QualityOutput, name: &str) -> &'a UnavailableMetric {
        output
            .unavailable
            .iter()
            .find(|metric| metric.name == name)
            .unwrap_or_else(|| panic!("expected {name} to be unavailable"))
    }

    fn virtual_workspace_metadata(
        workspace_rust_version: Option<&str>,
        inherit_rust_version: bool,
    ) -> (TempDir, Metadata) {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let root = temp_dir.path();
        let member_dir = root.join("member");
        fs::create_dir_all(member_dir.join("src")).expect("create member src");

        let workspace_package = workspace_rust_version
            .map(|version| format!("\n[workspace.package]\nrust-version = \"{version}\"\n"))
            .unwrap_or_default();
        fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nresolver = \"2\"\nmembers = [\"member\"]\n{workspace_package}"),
        )
        .expect("write workspace manifest");

        let inherited = if inherit_rust_version {
            "rust-version.workspace = true\n"
        } else {
            ""
        };
        fs::write(
            member_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{inherited}"
            ),
        )
        .expect("write member manifest");
        fs::write(member_dir.join("src/lib.rs"), "pub fn stub() {}\n")
            .expect("write member source");

        let mut command = MetadataCommand::new();
        command.manifest_path(root.join("Cargo.toml"));
        let metadata = command.exec().expect("load fixture metadata");
        (temp_dir, metadata)
    }

    #[tokio::test]
    async fn run_blocking_returns_ok_value() {
        let value = run_blocking("ok", || Ok(42)).await.unwrap();
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn run_blocking_propagates_inner_error() {
        let err = run_blocking::<u8, _>("fail", || {
            Err(UpkeepError::message(ErrorCode::InvalidData, "nope"))
        })
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::InvalidData);
    }

    #[tokio::test]
    async fn check_msrv_returns_valid_when_set() {
        let status = check_msrv().await.unwrap();
        assert!(matches!(
            status,
            crate::core::scorers::quality::MsrvStatus::Valid
        ));
    }

    #[test]
    fn virtual_workspace_level_msrv_is_valid_without_member_inheritance() {
        let (_temp_dir, metadata) = virtual_workspace_metadata(Some("1.70"), false);
        assert!(metadata.root_package().is_none());
        assert!(metadata
            .workspace_packages()
            .iter()
            .all(|package| package.rust_version.is_none()));

        assert!(matches!(
            msrv_status(&metadata).expect("MSRV status"),
            MsrvStatus::Valid
        ));
    }

    #[test]
    fn virtual_workspace_inherited_msrv_is_valid() {
        let (_temp_dir, metadata) = virtual_workspace_metadata(Some("1.70"), true);
        assert!(metadata.root_package().is_none());
        assert!(metadata
            .workspace_packages()
            .iter()
            .all(|package| package.rust_version.is_some()));

        assert!(matches!(
            msrv_status(&metadata).expect("MSRV status"),
            MsrvStatus::Valid
        ));
    }

    #[test]
    fn virtual_workspace_without_msrv_is_missing() {
        let (_temp_dir, metadata) = virtual_workspace_metadata(None, false);
        assert!(metadata.root_package().is_none());

        assert!(matches!(
            msrv_status(&metadata).expect("MSRV status"),
            MsrvStatus::Missing
        ));
    }

    /// Failures are reported as structured `unavailable` entries, not as
    /// free-text recommendations.
    ///
    /// This replaces a test that asserted the opposite. The old strings were
    /// only ever visible under `--json`, and mixing "we could not check this"
    /// into the same list as "fix your dependencies" made the two
    /// indistinguishable to any consumer.
    #[test]
    fn build_quality_output_reports_failures_as_unavailable_metrics() {
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Ok(MsrvStatus::Valid),
            Err(err()),
            Err(err()),
        );

        let reported: Vec<&str> = output
            .unavailable
            .iter()
            .map(|metric| metric.name.as_str())
            .collect();
        assert_eq!(
            reported,
            vec![
                METRIC_DEPENDENCY_FRESHNESS,
                METRIC_SECURITY,
                METRIC_UNUSED_DEPS,
                METRIC_UNSAFE_CODE,
                METRIC_CLIPPY,
            ]
        );
        assert!(output
            .unavailable
            .iter()
            .all(|metric| metric.reason == UnavailableReason::Failed && metric.detail == "boom"));

        // Only MSRV was measured, so the score is that metric alone.
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.10);
        assert_close(output.score.expect("score"), 100.0);
        assert_eq!(score_of(&output, METRIC_MSRV), Some(100.0));

        // The failure text is gone from recommendations entirely.
        assert!(output.recommendations.is_empty());
    }

    /// `cargo upkeep clippy` and `cargo upkeep quality` must report the same
    /// clippy score for the same lints.
    ///
    /// The penalty formula used to exist twice — once in `analyzers::clippy`,
    /// once in `scorers::quality` — so changing a weight in one desynchronised
    /// the two commands (#37). It is one function now, which makes a *unit*
    /// test of the formula unable to catch a future split. This drives the two
    /// real reporting paths end to end instead: one `parse_diagnostics` feeds
    /// `ClippyOutput.score` on one side, and the same `ClippyOutput` goes
    /// through the real `build_quality_output` to the Clippy breakdown entry on
    /// the other.
    ///
    /// It lives here, not in `analyzers::clippy`, because the desync can also
    /// be reintroduced *below* the formula: `build_quality_output` maps
    /// `ClippyOutput` to `ClippySummary` field by field, and swapping the two
    /// there would weight warnings at 10 and errors at 2 on the quality side
    /// only. Every other `build_quality_output` test passes `clean_clippy()`,
    /// which is `0, 0` and symmetric, so nothing else in the suite can see that
    /// swap. Counts of 3 and 1 are deliberately unequal for the same reason.
    ///
    /// Deliberately not asserting a literal: the point is that the two agree,
    /// whatever the weights are. `parse_clippy_output_counts_and_details` in
    /// `analyzers::clippy` and `clippy_score_applies_penalties` in the scorer
    /// pin the values, so a weight change stays a visible, deliberate edit.
    ///
    /// The comparison is exact `f32` equality, which is sound only because
    /// `clippy_score` returns `100u64.saturating_sub(..) as f32` and is always
    /// integer-valued, making the quality side's `round_hundredths` an identity.
    /// A formula that returned a fractional score would need a tolerance here —
    /// otherwise this fails while the two paths still agree, and reads as a
    /// desync that isn't one.
    #[test]
    fn clippy_command_and_quality_breakdown_agree() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"avoid needless return","spans":[{"file_name":"src/lib.rs","line_start":10,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::redundant_clone"},"message":"redundant clone","spans":[{"file_name":"src/lib.rs","line_start":20,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"avoid needless return","spans":[{"file_name":"src/lib.rs","line_start":30,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"error","code":{"code":"clippy::panic"},"message":"do not panic","spans":[{"file_name":"src/main.rs","line_start":42,"is_primary":true}]}}"#;

        let analyzed = parse_diagnostics(stdout);
        assert_eq!(analyzed.warnings, 3);
        assert_eq!(analyzed.errors, 1);

        // What `cargo upkeep clippy` prints, captured before the output is
        // handed to the quality path.
        let reported_by_clippy = analyzed.score;

        // What `cargo upkeep quality` prints. Every other metric fails so the
        // Clippy entry is the only one under test; the mapping and the scoring
        // are production code, not restated here.
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Ok(analyzed),
            Err(err()),
            Err(err()),
            Err(err()),
        );

        assert_eq!(score_of(&output, METRIC_CLIPPY), Some(reported_by_clippy));
    }

    /// `build_quality_output` must map `AuditOutput`'s severity counts onto the
    /// matching `SecuritySummary` fields.
    ///
    /// This is the #37 hole in its worst form (#51). `SecuritySummary` has four
    /// same-typed `usize` fields feeding four different penalty weights, so any
    /// transposition compiles and silently reweights the metric — swapping
    /// `critical` and `low` turns a 25-point deduction into a 2-point one. The
    /// suite could not see it because `clean_audit()` is four zeros, and every
    /// other `build_quality_output` test passes it.
    ///
    /// Two halves, and the first is what makes the second worth anything:
    ///
    /// 1. **The fixture is sensitive.** Each of the six pairwise swaps is run
    ///    through the real scorer and must produce a different score. That is
    ///    the property the test rests on, checked rather than assumed: the
    ///    penalty weights are private to the scorer, so a note here asserting
    ///    "the counts are distinct, therefore any swap shows" would go stale
    ///    the moment a weight moved or a count was edited.
    /// 2. **The mapping agrees.** The genuine `AuditOutput` goes through the
    ///    genuine `build_quality_output`, and its Security entry must equal
    ///    what the scorer produces for the summary those counts are supposed to
    ///    become.
    ///
    /// No literal score and no weight appears here, for the reason
    /// `clippy_command_and_quality_breakdown_agree` gives: the claim is that
    /// the two sides agree, whatever the weights are.
    /// `security_score_applies_penalties` in the scorer pins the values.
    ///
    /// Exact `f32` equality is sound because `security_score` returns
    /// `100u64.saturating_sub(..) as f32`, which is always integer-valued, so
    /// the scorer's `round_hundredths` is an identity on both sides.
    #[test]
    fn security_summary_mapping_is_not_swappable() {
        let expected = security_reference_score(SECURITY_COUNTS);
        assert_sensitive(expected);

        for (left, right) in [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
            let mut swapped = SECURITY_COUNTS;
            swapped.swap(left, right);
            assert_ne!(
                security_reference_score(swapped),
                expected,
                "SECURITY_COUNTS is not sensitive to swapping fields {left} and {right} of \
                 [critical, high, moderate, low]; a mapping that transposed them would score \
                 the same and this test could not see it"
            );
        }

        let output = build_quality_output(
            Err(err()),
            Ok(asymmetric_audit()),
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
        );

        assert_eq!(score_of(&output, METRIC_SECURITY), expected);
    }

    #[test]
    fn audit_warnings_do_not_affect_the_quality_grade() {
        let clean = build_quality_output(
            Ok(deps_output(0, 0, 0, 0)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );
        let warned = build_quality_output(
            Ok(deps_output(0, 0, 0, 0)),
            Ok(warning_only_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(score_of(&warned, METRIC_SECURITY), Some(100.0));
        assert_eq!(warned.score, clean.score);
        assert_eq!(warned.grade, clean.grade);
    }

    /// `build_quality_output` must count the *confirmed* unused dependencies.
    ///
    /// `UnusedSummary` has one field, so there is no transposition to make —
    /// but `UnusedOutput` has two lists, and `unused_count` reads the length of
    /// one of them. Reading `possibly_unused` instead, or summing the two,
    /// penalizes the project for dependencies `cargo-machete` only suspected.
    /// `clean_unused()` is empty on both lists, so that defect is invisible to
    /// the rest of the suite for exactly the same reason the security swap is.
    ///
    /// The two lengths and their sum are pairwise distinct, checked below
    /// rather than asserted in prose.
    #[test]
    fn unused_summary_mapping_reads_the_confirmed_list() {
        let expected = unused_reference_score(UNUSED_CONFIRMED);
        assert_sensitive(expected);

        for wrong in [UNUSED_POSSIBLE, UNUSED_CONFIRMED + UNUSED_POSSIBLE] {
            assert_ne!(
                unused_reference_score(wrong),
                expected,
                "a mapping counting {wrong} entries would score the same as the correct \
                 {UNUSED_CONFIRMED}; the fixture cannot tell the two apart"
            );
        }

        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Ok(asymmetric_unused()),
            Err(err()),
        );

        assert_eq!(score_of(&output, METRIC_UNUSED_DEPS), expected);
    }

    /// `build_quality_output` must read `total_unsafe`, not one of its six
    /// siblings.
    ///
    /// `UnsafeSummary` is single-field, so this is the wrong-source-field case
    /// rather than a swap: `UnsafeOutput`'s summary carries seven same-typed
    /// `usize` counts and the mapping picks one. Reading `unsafe_blocks` — or
    /// `packages`, which is not a count of unsafe anything — compiles and
    /// reweights the metric. `clean_unsafe()` is seven zeros, so nothing else
    /// in the suite can see which one was picked.
    ///
    /// The loop is the sensitivity check: substituting any sibling field must
    /// change the score. It fails loudly if the fixture's counts are ever
    /// edited into a shape where two fields coincide, or where the scorer's
    /// diminishing-penalty plateau flattens two of them onto the same value.
    #[test]
    fn unsafe_summary_mapping_reads_the_total() {
        let unsafe_output = asymmetric_unsafe();
        let summary = &unsafe_output.summary;

        let expected = unsafe_reference_score(summary.total_unsafe);
        assert_sensitive(expected);

        for (field, count) in [
            ("packages", summary.packages),
            ("unsafe_functions", summary.unsafe_functions),
            ("unsafe_impls", summary.unsafe_impls),
            ("unsafe_traits", summary.unsafe_traits),
            ("unsafe_blocks", summary.unsafe_blocks),
            ("unsafe_expressions", summary.unsafe_expressions),
        ] {
            assert_ne!(
                unsafe_reference_score(count),
                expected,
                "a mapping reading `{field}` ({count}) instead of `total_unsafe` ({}) would \
                 score the same; the fixture cannot tell the two apart",
                summary.total_unsafe
            );
        }

        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Ok(unsafe_output),
        );

        assert_eq!(score_of(&output, METRIC_UNSAFE_CODE), expected);
    }

    #[test]
    fn build_quality_output_marks_missing_machete_as_not_installed() {
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Ok(MsrvStatus::Valid),
            Err(missing_tool("machete")),
            Err(err()),
        );

        let entry = unavailable_entry(&output, METRIC_UNUSED_DEPS);
        assert_eq!(entry.reason, UnavailableReason::NotInstalled);
        assert_close(entry.weight, WEIGHT_UNUSED_DEPS);
        assert!(entry.detail.contains("cargo install cargo-machete"));
        assert_eq!(score_of(&output, METRIC_UNUSED_DEPS), None);

        // A genuine analyzer failure is still classified separately.
        assert_eq!(
            unavailable_entry(&output, METRIC_SECURITY).reason,
            UnavailableReason::Failed
        );
    }

    #[test]
    fn build_quality_output_marks_missing_geiger_as_not_installed() {
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Ok(MsrvStatus::Valid),
            Err(err()),
            Err(missing_tool("geiger")),
        );

        let entry = unavailable_entry(&output, METRIC_UNSAFE_CODE);
        assert_eq!(entry.reason, UnavailableReason::NotInstalled);
        assert!(entry.detail.contains("cargo install cargo-geiger"));
        assert_eq!(score_of(&output, METRIC_UNSAFE_CODE), None);
    }

    /// A failed MSRV check is unavailable; it must not be reported as
    /// `MsrvStatus::Missing`, which is a real finding about the project.
    ///
    /// Everything else is measured here so the assertion isolates MSRV. The
    /// version of this test that failed all six analyzers was byte-identical to
    /// `build_quality_output_total_failure_yields_no_grade` and so proved
    /// nothing that test did not already prove.
    #[test]
    fn build_quality_output_failed_msrv_check_is_unavailable_not_missing() {
        let output = build_quality_output(
            Ok(deps_output(10, 10, 0, 0)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Err(err()),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(
            unavailable_entry(&output, METRIC_MSRV).reason,
            UnavailableReason::Failed
        );
        assert_eq!(score_of(&output, METRIC_MSRV), None);
        assert_eq!(output.unavailable.len(), 1);

        // `MsrvStatus::Missing` would have scored 50 and produced this advice.
        // A check that did not run produces neither.
        assert!(!output
            .recommendations
            .contains(&"Declare a valid MSRV in Cargo.toml.".to_string()));

        // The other five metrics are perfect, so the renormalized score is 100
        // over 0.90 of the weight — not 100 over all of it.
        assert_close(output.score.expect("score"), 100.0);
        assert_eq!(output.grade, Some(Grade::A));
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.90);
    }

    /// A registry-unavailable run may not report perfect freshness.
    ///
    /// `analyze` returns `Ok` when crates.io is unreachable: every dependency
    /// lands in `skipped_packages` as `registry_unavailable` and `outdated` is
    /// 0. Reading `total` and `outdated` alone computed `(10 - 0) / 10 * 100`,
    /// scored the metric 100 at its full 0.20 weight, left `unavailable` empty
    /// and asserted `complete: true` — an offline run printing a full-weight
    /// `A` and telling CI that all six metrics were measured.
    #[test]
    fn build_quality_output_registry_unavailable_is_not_perfect_freshness() {
        let output = build_quality_output(
            Ok(deps_output(10, 0, 0, 10)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(score_of(&output, METRIC_DEPENDENCY_FRESHNESS), None);
        let entry = unavailable_entry(&output, METRIC_DEPENDENCY_FRESHNESS);
        assert_eq!(entry.reason, UnavailableReason::Failed);
        assert!(
            entry.detail.contains("crates.io registry was unavailable"),
            "unexpected detail: {}",
            entry.detail
        );

        // The claim that everything was measured is the dangerous part: the
        // README tells CI to gate on it.
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.80);

        // And no advice is offered about dependencies that were never checked.
        assert!(!output
            .recommendations
            .contains(&"Update outdated dependencies.".to_string()));
    }

    #[test]
    fn build_quality_output_all_unsupported_registries_is_unavailable() {
        let mut deps = deps_output(2, 0, 0, 0);
        deps.skipped_packages.push(SkippedDependency {
            name: "private-crate".to_string(),
            alias: None,
            required: "1.0".to_string(),
            reason: SkipReason::UnsupportedRegistry,
            dependency_type: DependencyType::Normal,
            source: Some("registry+https://packages.example.com/index".to_string()),
            target: Some("cfg(unix)".to_string()),
        });
        deps.skipped = deps.skipped_packages.len();

        let output = build_quality_output(
            Ok(deps),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(score_of(&output, METRIC_DEPENDENCY_FRESHNESS), None);
        let entry = unavailable_entry(&output, METRIC_DEPENDENCY_FRESHNESS);
        assert_eq!(entry.reason, UnavailableReason::Failed);
        assert!(entry.detail.contains("no supported registry comparison"));
        assert!(!output.complete);
    }

    #[test]
    fn build_quality_output_all_missing_resolve_is_unavailable() {
        let mut deps = deps_output(1, 0, 0, 0);
        deps.skipped_packages.push(SkippedDependency {
            name: "unresolved".to_string(),
            alias: None,
            required: "1.0".to_string(),
            reason: SkipReason::MissingResolve,
            dependency_type: DependencyType::Normal,
            source: None,
            target: None,
        });
        deps.skipped = 1;

        let output = build_quality_output(
            Ok(deps),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(score_of(&output, METRIC_DEPENDENCY_FRESHNESS), None);
        assert!(!output.complete);
    }

    /// An ambiguous resolve is as unanswered as a missing one.
    ///
    /// The crate exists on crates.io and a comparison was owed; `deps` declined to
    /// guess which resolved instance the declaration meant. Treating that as
    /// anything other than unmeasured would score a run where nothing was compared
    /// as a measured 100 over an empty denominator.
    #[test]
    fn build_quality_output_all_ambiguous_resolve_is_unavailable() {
        let mut deps = deps_output(1, 0, 0, 0);
        deps.skipped_packages.push(SkippedDependency {
            name: "wasm-bindgen".to_string(),
            alias: None,
            required: "=0.2.100".to_string(),
            reason: SkipReason::AmbiguousResolve,
            dependency_type: DependencyType::Normal,
            source: None,
            target: None,
        });
        deps.skipped = 1;

        let output = build_quality_output(
            Ok(deps),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(score_of(&output, METRIC_DEPENDENCY_FRESHNESS), None);
        assert!(!output.complete);
    }

    /// A re-declared crate does not manufacture a comparison that never happened.
    ///
    /// The minimal real case, reproduced against the built binary: one crate
    /// declared in both `[dependencies]` and `[dev-dependencies]`. `analyze`
    /// reports `total: 2` — two edges — but `resolve_dependencies` merges them
    /// into a single group keyed `(name, resolved version)`, and `SkippedCollector`
    /// deduplicates the skip, so an offline run yields exactly one
    /// `registry_unavailable` entry.
    ///
    /// Reconstructing the denominator as `total - unchecked` gave `2 - 1 = 1`,
    /// which slipped past the `checked == 0` guard and scored
    /// `(1 - 0) / 1 * 100` — a measured 100 at the full 0.20 weight on a run
    /// where nothing was compared. `complete` stayed false only because other
    /// analyzers were down; with the registry as the sole failure it would have
    /// printed a complete `A`.
    ///
    /// The trigger is ordinary: any crate in both `[dependencies]` and
    /// `[dev-dependencies]` at the same requirement, or any two workspace
    /// members sharing a dependency.
    #[test]
    fn build_quality_output_registry_outage_is_unmeasured_when_edges_exceed_groups() {
        // 2 declared edges, 1 group, that group unreachable: nothing compared.
        let output = build_quality_output(
            Ok(deps_output(2, 0, 0, 1)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_eq!(
            score_of(&output, METRIC_DEPENDENCY_FRESHNESS),
            None,
            "freshness must be unavailable, not a perfect score over a comparison never made"
        );
        let entry = unavailable_entry(&output, METRIC_DEPENDENCY_FRESHNESS);
        assert_eq!(entry.reason, UnavailableReason::Failed);
        assert!(
            entry.detail.contains("crates.io registry was unavailable"),
            "unexpected detail: {}",
            entry.detail
        );
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.80);
    }

    /// A partial outage where edges exceed groups scores over the groups.
    ///
    /// 10 declared edges collapsing to 8 groups, 2 of them unreachable, 3 of the
    /// remaining 6 outdated. The honest score is 3/6 current = 50.0. Deriving the
    /// denominator from the edge count gave `(10 - 2 - 3) / (10 - 2) = 62.5` —
    /// inflated by the two edges that were never separate comparisons.
    #[test]
    fn build_quality_output_partial_outage_scores_over_groups_not_edges() {
        let output = build_quality_output(
            Ok(deps_output(10, 6, 3, 2)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_close(
            score_of(&output, METRIC_DEPENDENCY_FRESHNESS).expect("freshness measured"),
            50.0,
        );
        assert!(output.complete);
    }

    /// A partial registry outage scores over the checked subset only.
    ///
    /// 10 dependencies declared, 4 unreachable, 3 of the remaining 6 outdated.
    /// The honest score is 3/6 current = 50.0. Counting the 4 unchecked as
    /// current gives 6/10 = 60.0 — credit for four comparisons never made.
    #[test]
    fn build_quality_output_partial_registry_outage_scores_the_checked_subset() {
        let output = build_quality_output(
            Ok(deps_output(10, 6, 3, 4)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_close(
            score_of(&output, METRIC_DEPENDENCY_FRESHNESS).expect("freshness measured"),
            50.0,
        );

        // The metric was measured, so it carries its full weight and the run is
        // complete: a partial outage is not a missing metric.
        assert!(output.complete);
        assert_close(output.measured_weight, 1.0);
        assert!(output
            .recommendations
            .contains(&"Update outdated dependencies.".to_string()));
    }

    /// Skips that mean "not applicable" stay in the denominator.
    ///
    /// A git, path or inactive-optional dependency has no registry version to
    /// be behind, so excluding it would shrink the denominator for a comparison
    /// that was never owed — misreporting freshness in the other direction.
    #[test]
    fn build_quality_output_non_registry_skips_do_not_shrink_the_denominator() {
        let mut deps = deps_output(10, 10, 2, 0);
        for (index, reason) in [
            SkipReason::NonRegistry,
            SkipReason::TargetSpecific,
            SkipReason::OptionalNotActivated,
        ]
        .into_iter()
        .enumerate()
        {
            deps.skipped_packages.push(SkippedDependency {
                name: format!("not-applicable-{index}"),
                alias: None,
                required: "1.0".to_string(),
                reason,
                dependency_type: DependencyType::Normal,
                source: None,
                target: None,
            });
        }
        deps.skipped = deps.skipped_packages.len();

        let output = build_quality_output(
            Ok(deps),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        // 8 of 10 current, not 8 of 7.
        assert_close(
            score_of(&output, METRIC_DEPENDENCY_FRESHNESS).expect("freshness measured"),
            80.0,
        );
        assert!(output.complete);
    }

    /// A project with no dependencies is still perfectly fresh.
    ///
    /// `checked == 0` only means "unmeasured" when something was skipped for
    /// registry unavailability; with nothing declared there is nothing to check
    /// and 100 is the honest answer.
    #[test]
    fn build_quality_output_no_dependencies_is_measured_at_one_hundred() {
        let output = build_quality_output(
            Ok(deps_output(0, 0, 0, 0)),
            Ok(clean_audit()),
            Ok(clean_clippy()),
            Ok(MsrvStatus::Valid),
            Ok(clean_unused()),
            Ok(clean_unsafe()),
        );

        assert_close(
            score_of(&output, METRIC_DEPENDENCY_FRESHNESS).expect("freshness measured"),
            100.0,
        );
        assert!(output.complete);
    }

    /// The headline regression: with every analyzer down, the command used to
    /// print `Score: 100.0 / Grade: A`.
    #[test]
    fn build_quality_output_total_failure_yields_no_grade() {
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
        );

        assert_eq!(output.score, None);
        assert_eq!(output.grade, None);
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.0);
        assert_eq!(output.unavailable.len(), 6);
        assert!(output.breakdown.iter().all(|metric| metric.score.is_none()));

        // And nothing in the rendered text can be read as a passing grade.
        let text = format!("{output}");
        assert!(text.contains("Analysis incomplete: 6 of 6 metrics"));
        assert!(text.contains("Score: unavailable"));
        assert!(text.contains("Grade: unavailable"));
        assert!(!text.contains("Grade: A"));
    }

    #[test]
    fn emit_output_json_shape() {
        let output = QualityOutput {
            score: Some(92.5),
            grade: Some(Grade::A),
            complete: false,
            measured_weight: 0.75,
            breakdown: vec![
                MetricScore {
                    name: METRIC_SECURITY.to_string(),
                    score: Some(90.0),
                    weight: 0.25,
                },
                MetricScore {
                    name: METRIC_UNUSED_DEPS.to_string(),
                    score: None,
                    weight: 0.15,
                },
            ],
            unavailable: vec![UnavailableMetric {
                name: METRIC_UNUSED_DEPS.to_string(),
                weight: 0.15,
                reason: UnavailableReason::NotInstalled,
                detail: "cargo-machete is not installed".to_string(),
            }],
            recommendations: vec!["Address advisories".to_string()],
        };

        let value = serde_json::to_value(&output).expect("serialize");
        assert_eq!(value["grade"], Value::String("A".into()));
        assert_eq!(value["complete"], Value::Bool(false));
        assert_eq!(
            value["breakdown"][0]["name"],
            Value::String("Security".into())
        );
        // An unmeasured metric serializes as null, never as a number.
        assert_eq!(value["breakdown"][1]["score"], Value::Null);
        assert_eq!(
            value["unavailable"][0]["reason"],
            Value::String("not_installed".into())
        );
    }

    /// Derived from a real total failure rather than hand-built.
    ///
    /// The literal this replaced set `complete: false` beside an empty
    /// `unavailable`, a pair `score_quality` cannot produce — `complete` *is*
    /// `unavailable.is_empty()`. A fixture that models an impossible state
    /// cannot catch a regression in how the real states serialize.
    #[test]
    fn emit_output_json_nulls_score_and_grade_when_nothing_measured() {
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
            Err(err()),
        );

        let value = serde_json::to_value(&output).expect("serialize");
        assert_eq!(value["score"], Value::Null);
        assert_eq!(value["grade"], Value::Null);
        assert_eq!(value["complete"], Value::Bool(false));
        // Every breakdown entry is present but unscored: the six metrics are
        // still enumerated, none of them with a substituted number.
        assert_eq!(value["breakdown"].as_array().expect("breakdown").len(), 6);
        assert!(value["breakdown"]
            .as_array()
            .expect("breakdown")
            .iter()
            .all(|metric| metric["score"] == Value::Null));
    }
}
