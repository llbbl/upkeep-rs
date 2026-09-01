//! `cargo upkeep python` — normalized Python maintenance reporting.
//!
//! The contract this implements is `docs/python-schema.md`. Two things about it
//! shape everything below.
//!
//! **A capability that did not run is `null`, never an empty result.** An empty
//! `findings` list means the scanner ran and found nothing; `"security": null`
//! means nobody looked. Collapsing those is the bug `quality` already shipped
//! once (#10, #34), where an unmeasured metric defaulted to a healthy value and a
//! broken toolchain graded as an `A`.
//!
//! **Findings are not failures.** Outdated packages exit 0 and so do
//! vulnerabilities. The report is the deliverable; a pipeline that wants a gate
//! asks for one with `--require-complete` or `--fail-on-vulnerability`.

use crate::cli::{CapabilityArg, PythonArgs, ThresholdArg};
use crate::core::analyzers::uv::{Capability, ScopeIndex, Uv};
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::print_json;
use crate::core::python::{
    PythonCapability, PythonCapabilityCoverage, PythonManager, PythonManagerName, PythonOutput,
    PythonSeverity, PythonUnavailableCapability, PythonUnavailableReason, PYTHON_SCHEMA_VERSION,
};

/// The disclaimer every payload carries.
///
/// `uv tree --format json` and `uv audit --output-format json` both print an
/// experimental-output warning to stderr and both self-declare
/// `"schema": {"version": "preview"}`. Absorbing that here is the point of owning
/// a schema: the instability is a normalization problem for this adapter, not
/// something a caller's CI gate should have to see on stderr and interpret.
const UPSTREAM_DISCLAIMER: &str =
    "uv documents its own JSON output as unstable; this report is normalized into cargo-upkeep \
     schema_version 1";

pub async fn run(json: bool, args: PythonArgs) -> Result<()> {
    let policy = ExitPolicy::from(&args);

    let working_directory = std::env::current_dir().map_err(|err| {
        UpkeepError::context(ErrorCode::Io, "failed to read the working directory", err)
    })?;

    // A manager that could not be detected is one of the two documented no-report
    // failures. It is an error rather than an empty payload because there is
    // nothing for `complete` to qualify, and it deliberately carries no
    // `schema_version`: this path emits an error object, not a `PythonOutput`.
    let uv = Uv::detect(&working_directory).await?;
    let output = build_output(&uv).await;

    // The report is printed before the policy runs, so a failing exit status never
    // costs the caller the analysis that explains it. `quality` establishes the
    // ordering and `tests/cli.rs` pins it there; the same test exists for this
    // command because the ordering is only observable from outside the process.
    emit_output(json, &output)?;
    enforce_exit_policy(&output, &policy)
}

fn emit_output(json: bool, output: &PythonOutput) -> Result<()> {
    if json {
        print_json(output)
    } else {
        println!("{output}");
        Ok(())
    }
}

/// Probes each capability, runs the ones that answered, and assembles the payload.
///
/// Both capabilities are probed and run in sequence rather than concurrently:
/// `uv audit` borrows direct-versus-transitive scope from the dependency graph
/// `uv tree` produces, and uv itself locks its cache across concurrent
/// invocations, so there is nothing to win by overlapping them.
async fn build_output(uv: &Uv) -> PythonOutput {
    let mut warnings = vec![UPSTREAM_DISCLAIMER.to_string()];
    let mut unavailable = Vec::new();

    let (outdated, scopes) = match uv.probe_outdated().await {
        Capability::Available => match uv.outdated().await {
            Ok((report, scopes)) => (Some(report), scopes),
            Err(err) => {
                unavailable.push(gap(
                    PythonCapability::Outdated,
                    PythonUnavailableReason::Failed,
                    err.to_string(),
                ));
                (None, ScopeIndex::default())
            }
        },
        Capability::Unavailable { reason, detail } => {
            unavailable.push(gap(PythonCapability::Outdated, reason, detail));
            (None, ScopeIndex::default())
        }
    };

    let security = match uv.probe_security().await {
        Capability::Available => match uv.security(&scopes).await {
            Ok((report, notes)) => {
                warnings.extend(notes);
                Some(report)
            }
            Err(err) => {
                unavailable.push(gap(
                    PythonCapability::Security,
                    PythonUnavailableReason::Failed,
                    err.to_string(),
                ));
                None
            }
        },
        Capability::Unavailable { reason, detail } => {
            unavailable.push(gap(PythonCapability::Security, reason, detail));
            None
        }
    };

    let capabilities = coverage(&unavailable);

    PythonOutput {
        schema_version: PYTHON_SCHEMA_VERSION,
        manager: PythonManager {
            name: PythonManagerName::Uv,
            version: uv.version().map(str::to_string),
        },
        complete: unavailable.is_empty(),
        capabilities,
        unavailable,
        outdated,
        security,
        warnings,
    }
}

/// Derives `capabilities[]` from the gaps that were recorded.
///
/// Derived rather than tracked alongside, because the three unavailability
/// signals — `measured: false`, an `unavailable[]` entry, and a `null` report —
/// have to agree or the payload is lying, and three independently maintained
/// lists is how they stop agreeing. Every capability appears on every run, so a
/// caller never infers coverage from a missing key.
fn coverage(unavailable: &[PythonUnavailableCapability]) -> Vec<PythonCapabilityCoverage> {
    [PythonCapability::Outdated, PythonCapability::Security]
        .into_iter()
        .map(|name| PythonCapabilityCoverage {
            name,
            measured: !unavailable.iter().any(|entry| entry.name == name),
        })
        .collect()
}

fn gap(
    name: PythonCapability,
    reason: PythonUnavailableReason,
    detail: String,
) -> PythonUnavailableCapability {
    PythonUnavailableCapability {
        name,
        reason,
        detail,
    }
}

/// What the caller asked to fail on.
#[derive(Debug, Default)]
struct ExitPolicy {
    /// `None` when the flag was absent. `Some(&[])` is the bare form, meaning
    /// every capability this version knows about.
    require_complete: Option<Vec<PythonCapability>>,
    fail_on_vulnerability: Option<ThresholdArg>,
}

impl From<&PythonArgs> for ExitPolicy {
    fn from(args: &PythonArgs) -> Self {
        Self {
            require_complete: args.require_complete.as_ref().map(|capabilities| {
                capabilities
                    .iter()
                    .map(|capability| match capability {
                        CapabilityArg::Outdated => PythonCapability::Outdated,
                        CapabilityArg::Security => PythonCapability::Security,
                    })
                    .collect()
            }),
            fail_on_vulnerability: args.fail_on_vulnerability,
        }
    }
}

/// Turns an already-printed report into the process exit status.
///
/// A pure function over the finished output, for the reason `quality`'s
/// equivalent is one: `build_output` shells out to `uv` and reaches the network,
/// so it is no place to test a decision this small.
///
/// The order of the three checks is the order of severity of what went wrong. A
/// run with no reports at all fails first and without any flag, because there is
/// nothing for `complete` to qualify — exactly as `quality` treats `score: null`.
/// Coverage comes next, and findings last: a caller told their scanner never ran
/// should hear that before hearing what the scanner that did run found.
fn enforce_exit_policy(output: &PythonOutput, policy: &ExitPolicy) -> Result<()> {
    if output.outdated.is_none() && output.security.is_none() {
        return Err(UpkeepError::message(
            ErrorCode::IncompleteAnalysis,
            format!(
                "python analysis measured nothing: all {} capabilities were unavailable, so there \
                 is no report to stand on; each entry in `unavailable` says why",
                output.capabilities.len()
            ),
        ));
    }

    if let Some(required) = &policy.require_complete {
        let missing: Vec<String> = if required.is_empty() {
            output
                .unavailable
                .iter()
                .map(|gap| gap.name.to_string())
                .collect()
        } else {
            required
                .iter()
                .filter(|name| output.unavailable.iter().any(|gap| gap.name == **name))
                .map(ToString::to_string)
                .collect()
        };

        if !missing.is_empty() {
            // A coverage gate says something about the runner, never about the
            // project: install what you intend to measure before gating on it.
            return Err(UpkeepError::message(
                ErrorCode::IncompleteAnalysis,
                format!(
                    "python analysis incomplete: {} was not measured; --require-complete treats \
                     that as a failure, and the report says why",
                    missing.join(", ")
                ),
            ));
        }
    }

    if let Some(threshold) = policy.fail_on_vulnerability {
        // No security report is not a clean one, but it is also not a finding.
        // `--require-complete` is the gate for that; this one is only about what
        // was found.
        if let Some(security) = &output.security {
            let matched = security
                .findings
                .iter()
                .filter(|finding| meets_threshold(finding.severity, threshold))
                .count();
            if matched > 0 {
                // The all-unknown case is not an edge case: `uv audit` publishes
                // no severity field at all, so today it is *every* uv run. Saying
                // "at or above critical" there is false, and this line is the one
                // thing that reaches a CI summary or an alert. Explain the match
                // where it bites rather than only in the docs.
                let all_unknown = security
                    .findings
                    .iter()
                    .all(|finding| finding.severity == PythonSeverity::Unknown);
                let reason = if all_unknown {
                    "every finding has an unknown severity, which satisfies every \
                     threshold because it cannot be shown to be below the bar"
                        .to_string()
                } else {
                    format!("they are at or above `{}`", threshold_label(threshold))
                };
                return Err(UpkeepError::message(
                    ErrorCode::PolicyViolation,
                    format!(
                        "{matched} of {} vulnerabilities match `{}`; {reason}; \
                         --fail-on-vulnerability treats that as a failure",
                        security.summary.total,
                        threshold_label(threshold),
                    ),
                ));
            }
        }
    }

    Ok(())
}

/// Whether one finding trips one threshold.
///
/// **An `unknown` severity satisfies every threshold.** A finding whose severity
/// was never established cannot be shown to be below the bar, and silently
/// excluding it would turn a missing severity into a passing build. That is not a
/// corner case for `uv`, which publishes no severity at all — under this adapter
/// it is *every* finding. A pipeline that finds it too noisy is told to gate on
/// the parsed `summary.unknown` bucket instead, so the decision is deliberate.
fn meets_threshold(severity: PythonSeverity, threshold: ThresholdArg) -> bool {
    if severity == PythonSeverity::Unknown {
        return true;
    }
    match threshold {
        ThresholdArg::Critical => severity == PythonSeverity::Critical,
        ThresholdArg::High => matches!(severity, PythonSeverity::Critical | PythonSeverity::High),
        ThresholdArg::Moderate => matches!(
            severity,
            PythonSeverity::Critical | PythonSeverity::High | PythonSeverity::Moderate
        ),
        // Every graded severity is at or above `low`, so these two accept the same
        // set today. Both are kept because they state different intent.
        ThresholdArg::Low | ThresholdArg::Any => true,
    }
}

fn threshold_label(threshold: ThresholdArg) -> &'static str {
    match threshold {
        ThresholdArg::Critical => "critical",
        ThresholdArg::High => "high",
        ThresholdArg::Moderate => "moderate",
        ThresholdArg::Low => "low",
        ThresholdArg::Any => "any",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::python::{
        PythonDependencyScope, PythonOutdatedReport, PythonSecurityReport, PythonSecuritySummary,
        PythonUpdateCounts, PythonVulnerability,
    };

    fn empty_outdated() -> PythonOutdatedReport {
        PythonOutdatedReport {
            checked: 4,
            outdated: 0,
            counts: PythonUpdateCounts {
                epoch: 0,
                major: 0,
                minor: 0,
                patch: 0,
                qualifier: 0,
                unclassified: 0,
            },
            packages: Vec::new(),
        }
    }

    fn finding(severity: PythonSeverity) -> PythonVulnerability {
        PythonVulnerability {
            id: "GHSA-0000-0000-0000".to_string(),
            aliases: None,
            package: "example".to_string(),
            installed_version: "1.0.0".to_string(),
            severity,
            title: None,
            scope: PythonDependencyScope::Unknown,
            fixed_versions: None,
        }
    }

    fn security_report(severities: &[PythonSeverity]) -> PythonSecurityReport {
        let mut summary = PythonSecuritySummary {
            critical: 0,
            high: 0,
            moderate: 0,
            low: 0,
            unknown: 0,
            total: severities.len(),
        };
        for severity in severities {
            match severity {
                PythonSeverity::Critical => summary.critical += 1,
                PythonSeverity::High => summary.high += 1,
                PythonSeverity::Moderate => summary.moderate += 1,
                PythonSeverity::Low => summary.low += 1,
                PythonSeverity::Unknown => summary.unknown += 1,
            }
        }
        PythonSecurityReport {
            summary,
            findings: severities.iter().copied().map(finding).collect(),
        }
    }

    fn output(
        outdated: Option<PythonOutdatedReport>,
        security: Option<PythonSecurityReport>,
    ) -> PythonOutput {
        let unavailable: Vec<PythonUnavailableCapability> = [
            (PythonCapability::Outdated, outdated.is_none()),
            (PythonCapability::Security, security.is_none()),
        ]
        .into_iter()
        .filter(|(_, missing)| *missing)
        .map(|(name, _)| {
            gap(
                name,
                PythonUnavailableReason::NotInstalled,
                "not under test".to_string(),
            )
        })
        .collect();

        PythonOutput {
            schema_version: PYTHON_SCHEMA_VERSION,
            manager: PythonManager {
                name: PythonManagerName::Uv,
                version: Some("0.0.0".to_string()),
            },
            complete: unavailable.is_empty(),
            // Built by the production function, so a payload assembled here
            // cannot disagree with one the command emits.
            capabilities: coverage(&unavailable),
            unavailable,
            outdated,
            security,
            warnings: Vec::new(),
        }
    }

    fn policy(
        require_complete: Option<Vec<PythonCapability>>,
        fail_on_vulnerability: Option<ThresholdArg>,
    ) -> ExitPolicy {
        ExitPolicy {
            require_complete,
            fail_on_vulnerability,
        }
    }

    /// Findings are not failures. This is the default every pipeline inherits, so
    /// it is asserted before any gate is.
    #[test]
    fn findings_alone_exit_zero() {
        let result = enforce_exit_policy(
            &output(
                Some(empty_outdated()),
                Some(security_report(&[PythonSeverity::Critical])),
            ),
            &ExitPolicy::default(),
        );
        assert!(
            result.is_ok(),
            "vulnerabilities alone must not fail the run"
        );
    }

    /// A run where every capability is unavailable fails without any flag.
    ///
    /// One of the two documented no-report conditions: `complete` is false and
    /// both reports are `null`, so there is nothing for it to qualify.
    #[test]
    fn a_run_with_no_reports_fails_without_a_flag() {
        let err = enforce_exit_policy(&output(None, None), &ExitPolicy::default())
            .expect_err("nothing was measured, so this must not report success");
        assert_eq!(err.code(), ErrorCode::IncompleteAnalysis);
        assert!(err.to_string().contains("measured nothing"), "{err}");
    }

    /// A partial run still exits zero unless the caller opts in. That default is
    /// what an existing pipeline relies on.
    #[test]
    fn a_partial_run_exits_zero_without_require_complete() {
        assert!(enforce_exit_policy(
            &output(Some(empty_outdated()), None),
            &ExitPolicy::default()
        )
        .is_ok());
    }

    /// The bare form gates on every capability this version knows about.
    #[test]
    fn bare_require_complete_fails_on_any_gap() {
        let err = enforce_exit_policy(
            &output(Some(empty_outdated()), None),
            &policy(Some(Vec::new()), None),
        )
        .expect_err("security was not measured");
        assert_eq!(err.code(), ErrorCode::IncompleteAnalysis);
        assert!(err.to_string().contains("security"), "{err}");

        assert!(
            enforce_exit_policy(
                &output(Some(empty_outdated()), Some(security_report(&[]))),
                &policy(Some(Vec::new()), None)
            )
            .is_ok(),
            "a complete run satisfies the bare form"
        );
    }

    /// A named list gates on exactly what the pipeline asked for.
    ///
    /// This is the whole reason the list exists. Adding a capability is an
    /// additive change that does not bump `schema_version`, so a bare
    /// `--require-complete` would start failing on unchanged code the first time
    /// a release adds one — "unmeasured reads as broken", arriving unannounced.
    #[test]
    fn a_named_require_complete_ignores_capabilities_it_did_not_name() {
        assert!(
            enforce_exit_policy(
                &output(Some(empty_outdated()), None),
                &policy(Some(vec![PythonCapability::Outdated]), None)
            )
            .is_ok(),
            "only `outdated` was required, and `outdated` was measured"
        );

        let err = enforce_exit_policy(
            &output(Some(empty_outdated()), None),
            &policy(Some(vec![PythonCapability::Security]), None),
        )
        .expect_err("security was required and not measured");
        assert!(err.to_string().contains("security"), "{err}");
    }

    /// Every threshold, against every severity.
    ///
    /// The table is the point: `unknown` has to satisfy all five, and a graded
    /// severity has to be compared against the bar rather than ranked by enum
    /// order. `low` and `any` accepting the same set is asserted rather than
    /// assumed, so a future severity below `low` separates them loudly.
    #[test]
    fn thresholds_accept_at_or_above_and_always_accept_unknown() {
        use PythonSeverity::{Critical, High, Low, Moderate, Unknown};
        use ThresholdArg as T;

        for (severity, expected) in [
            (Critical, [true, true, true, true, true]),
            (High, [false, true, true, true, true]),
            (Moderate, [false, false, true, true, true]),
            (Low, [false, false, false, true, true]),
            (Unknown, [true, true, true, true, true]),
        ] {
            for (threshold, expected) in [T::Critical, T::High, T::Moderate, T::Low, T::Any]
                .into_iter()
                .zip(expected)
            {
                assert_eq!(
                    meets_threshold(severity, threshold),
                    expected,
                    "{severity} against {}",
                    threshold_label(threshold)
                );
            }
        }
    }

    /// The gate fires on the finding list, and its message names the threshold.
    #[test]
    fn fail_on_vulnerability_uses_the_policy_violation_code() {
        let err = enforce_exit_policy(
            &output(
                Some(empty_outdated()),
                Some(security_report(&[PythonSeverity::Moderate])),
            ),
            &policy(None, Some(ThresholdArg::Moderate)),
        )
        .expect_err("a moderate finding trips a moderate threshold");
        assert_eq!(
            err.code(),
            ErrorCode::PolicyViolation,
            "an opt-in gate rejecting a result is not an incomplete analysis"
        );
        assert!(err.to_string().contains("moderate"), "{err}");

        assert!(
            enforce_exit_policy(
                &output(
                    Some(empty_outdated()),
                    Some(security_report(&[PythonSeverity::Moderate]))
                ),
                &policy(None, Some(ThresholdArg::High))
            )
            .is_ok(),
            "a moderate finding is below a high threshold"
        );
    }

    /// Under this adapter every finding is `unknown`, so every threshold fires.
    ///
    /// Not a curiosity: `uv audit` publishes no severity, so this is the ordinary
    /// case rather than the edge one. A gate that silently dropped unknown
    /// findings would pass on a project with 51 real vulnerabilities.
    #[test]
    fn an_unknown_severity_trips_even_the_critical_threshold() {
        let err = enforce_exit_policy(
            &output(
                Some(empty_outdated()),
                Some(security_report(&[PythonSeverity::Unknown])),
            ),
            &policy(None, Some(ThresholdArg::Critical)),
        )
        .expect_err("a severity that was never established cannot be shown to be below the bar");
        assert_eq!(err.code(), ErrorCode::PolicyViolation);
    }

    /// An unmeasured scanner is a coverage problem, not a finding.
    ///
    /// `--fail-on-vulnerability` alone must not turn "nobody looked" into a
    /// failure, and must not turn it into a pass either — it simply has nothing
    /// to say. `--require-complete` is the gate that has something to say.
    #[test]
    fn fail_on_vulnerability_says_nothing_about_an_unmeasured_scanner() {
        assert!(enforce_exit_policy(
            &output(Some(empty_outdated()), None),
            &policy(None, Some(ThresholdArg::Any))
        )
        .is_ok());

        let err = enforce_exit_policy(
            &output(Some(empty_outdated()), None),
            &policy(
                Some(vec![PythonCapability::Security]),
                Some(ThresholdArg::Any),
            ),
        )
        .expect_err("the coverage gate is the one that fires");
        assert_eq!(err.code(), ErrorCode::IncompleteAnalysis);
    }

    /// Coverage is reported before findings when both gates would fire.
    #[test]
    fn coverage_failures_are_reported_before_findings() {
        let err = enforce_exit_policy(
            &output(None, Some(security_report(&[PythonSeverity::Critical]))),
            &policy(Some(Vec::new()), Some(ThresholdArg::Critical)),
        )
        .expect_err("both gates would fire");
        assert_eq!(
            err.code(),
            ErrorCode::IncompleteAnalysis,
            "a caller whose scanner never ran should hear that first"
        );
    }

    /// The three unavailability signals move together, and every capability is
    /// listed on every run.
    ///
    /// A caller must never have to infer coverage from a missing key, so an
    /// unmeasured capability stays in `capabilities[]` with `measured: false`
    /// rather than dropping out of it.
    #[test]
    fn every_capability_is_listed_and_a_gap_marks_it_unmeasured() {
        let none_missing = coverage(&[]);
        assert_eq!(none_missing.len(), 2);
        assert!(none_missing.iter().all(|capability| capability.measured));

        let security_missing = coverage(&[gap(
            PythonCapability::Security,
            PythonUnavailableReason::NotInstalled,
            "not under test".to_string(),
        )]);
        assert_eq!(
            security_missing.len(),
            2,
            "an unmeasured capability must stay listed, not vanish"
        );
        assert!(
            security_missing
                .iter()
                .any(|capability| capability.name == PythonCapability::Outdated
                    && capability.measured)
        );
        assert!(security_missing.iter().any(|capability| capability.name
            == PythonCapability::Security
            && !capability.measured));

        // And the assembled payload keeps the other two signals in step with it.
        let output = output(Some(empty_outdated()), None);
        assert!(output.security.is_none());
        assert!(!output.complete);
        assert_eq!(output.unavailable[0].name, PythonCapability::Security);
    }
}
