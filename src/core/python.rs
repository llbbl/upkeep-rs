//! The Python maintenance output contract.
//!
//! These types are the schema documented in [`docs/python-schema.md`], defined
//! before any manager adapter exists so that each backend normalizes *into* one
//! deliberately designed shape rather than inheriting whichever tool was
//! implemented first.
//!
//! The version in [`PYTHON_SCHEMA_VERSION`] is ours. It is never a passthrough
//! of a manager's own schema field — `uv` labels its JSON `"schema": {"version":
//! "preview"}` and warns that `uv audit` may change without notice, so an
//! upstream break has to be absorbed here, by the adapter, instead of reaching
//! a caller's CI gate.
//!
//! # Set-valued fields
//!
//! Python sources disagree about which attributes they expose at all, so several
//! fields must distinguish "this source does not report it" from "it is empty".
//! Both here and at the top level that distinction is `null` versus the empty
//! value: `"security": null` is an unmeasured capability, `"extras": null` is an
//! unreported attribute, and `[]` in either position means reported-and-empty.
//! One encoding, one rule, at every level of the payload.
//!
//! [`PythonMarker`] is the sole exception, and only because it is single-valued
//! and needs a third state; its doc comment says so.
//!
//! None of these fields carry `skip_serializing_if`. An omitted key cannot be
//! told apart from a key a future version stopped emitting, so the `null` is
//! always written.
//!
//! [`docs/python-schema.md`]: https://github.com/llbbl/upkeep-rs/blob/main/docs/python-schema.md

// Every item below is constructed only from `#[cfg(test)]` code until the first
// adapter lands (#71). `cargo clippy --all-targets` also compiles the binary
// *without* `cfg(test)`, where these are genuinely dead, and `-D warnings` makes
// that a hard failure. `ErrorCode::Internal` in `crate::core::error` carries the
// same allow for the same reason. Remove this attribute with #71, which gives
// every type a production caller.
#![allow(dead_code)]

use serde::Serialize;

/// The version of the Python maintenance contract this crate emits.
///
/// Additive changes — a new field, a new enum variant, a new capability — do not
/// bump this; consumers are required to ignore unknown keys and unrecognized
/// enum strings. Removals, renames, type changes, redefinitions of an existing
/// value's meaning, and changes to what an exit code means do bump it. The full
/// rule is in `docs/python-schema.md`.
pub const PYTHON_SCHEMA_VERSION: u32 = 1;

/// A normalized Python maintenance report.
///
/// The capability vocabulary here — `complete`, `unavailable`, and the
/// `reason`/`detail` pair — is deliberately the same one [`QualityOutput`] uses,
/// so that a Python payload reads like the rest of this CLI.
///
/// [`QualityOutput`]: crate::core::output::QualityOutput
#[derive(Debug, Serialize)]
pub struct PythonOutput {
    /// Always [`PYTHON_SCHEMA_VERSION`], and always the first key.
    pub schema_version: u32,
    pub manager: PythonManager,
    /// True only when every capability in `capabilities` was measured.
    pub complete: bool,
    /// One entry per capability on every run, so a caller never has to infer
    /// coverage from a missing key.
    pub capabilities: Vec<PythonCapabilityCoverage>,
    /// The capabilities that did not run, and why. Empty when `complete` is true.
    pub unavailable: Vec<PythonUnavailableCapability>,
    /// `None` when the outdated capability was not measured.
    ///
    /// Serialized as an explicit `null`, never omitted: an absent key cannot be
    /// told apart from a key some future version stopped emitting. An empty
    /// `packages` list is the opposite claim — the check ran and found nothing —
    /// and conflating the two is the defaulted-to-healthy bug `quality` already
    /// shipped once (#10, #34).
    pub outdated: Option<PythonOutdatedReport>,
    /// `None` when the security capability was not measured. See
    /// [`PythonOutput::outdated`] for why `null` and `[]` are not interchangeable.
    pub security: Option<PythonSecurityReport>,
    /// Non-fatal notes, including disclaimers about upstream tool instability.
    pub warnings: Vec<String>,
}

/// The Python manager a report was normalized from.
#[derive(Debug, Serialize)]
pub struct PythonManager {
    pub name: PythonManagerName,
    /// The manager's own version string, or `None` when it could not be
    /// determined. Reported verbatim: these are not all PEP 440 versions and
    /// this crate does not parse them.
    pub version: Option<String>,
}

/// A supported Python manager backend.
///
/// Adding a variant is an additive change and does not bump
/// [`PYTHON_SCHEMA_VERSION`], which is why consumers must not fail on a value
/// they do not recognize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonManagerName {
    Uv,
    Poetry,
    PipTools,
}

/// One question this adapter can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonCapability {
    Outdated,
    Security,
}

/// Whether one capability was measured on this run.
#[derive(Debug, Serialize)]
pub struct PythonCapabilityCoverage {
    pub name: PythonCapability,
    /// False here must be accompanied by a matching
    /// [`PythonUnavailableCapability`] entry and a `null` report field. The
    /// three move together or the payload is lying.
    pub measured: bool,
}

/// A capability that could not be measured, and why.
#[derive(Debug, Serialize)]
pub struct PythonUnavailableCapability {
    pub name: PythonCapability,
    pub reason: PythonUnavailableReason,
    /// Human-readable explanation, including an install hint when the cause is
    /// a missing tool.
    pub detail: String,
}

/// Why a capability could not be measured.
///
/// `not_installed` and `failed` share their serialized labels with
/// [`UnavailableReason`] on purpose — the distinction callers have to act on is
/// the same one. `Unsupported` has no counterpart there: every Rust metric has a
/// tool that *could* be installed, whereas some Python managers simply do not
/// expose a capability at all and no install will change that. Widening
/// [`UnavailableReason`] itself would have changed an existing Rust contract.
///
/// [`UnavailableReason`]: crate::core::output::UnavailableReason
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonUnavailableReason {
    /// The tool that would answer this is not installed. Actionable by the user,
    /// and says nothing about the project.
    NotInstalled,
    /// The tool ran and did not produce a usable result.
    Failed,
    /// The detected manager cannot answer this at all. A different tool is
    /// needed; installing something is not the fix.
    Unsupported,
}

/// Normalized outdated-dependency results.
#[derive(Debug, Serialize)]
pub struct PythonOutdatedReport {
    /// Distinct packages the freshness question was actually settled for, and
    /// the only valid denominator.
    ///
    /// Not a count of declarations, and not `total - skipped`. The Rust `deps`
    /// output documents at length why mixing those units credits comparisons
    /// that never happened; the same reasoning applies here.
    pub checked: usize,
    pub outdated: usize,
    pub counts: PythonUpdateCounts,
    pub packages: Vec<PythonOutdatedPackage>,
}

/// `outdated` broken down by classification, so a pipeline can gate without
/// walking every entry.
///
/// `unclassified` is part of the summary rather than buried in the entries: a
/// run where the classifier gave up should be visible from the counts alone.
#[derive(Debug, Serialize)]
pub struct PythonUpdateCounts {
    pub epoch: usize,
    pub major: usize,
    pub minor: usize,
    pub patch: usize,
    pub qualifier: usize,
    pub unclassified: usize,
}

/// One package with a newer version available.
#[derive(Debug, Serialize)]
pub struct PythonOutdatedPackage {
    /// PEP 503 normalized name: lowercase, with runs of `-`, `_`, and `.`
    /// collapsed to a single `-`.
    pub name: String,
    pub current: String,
    pub latest: String,
    pub update_type: PythonUpdateType,
    pub scope: PythonDependencyScope,
    /// Dependency groups this package belongs to, or `null` when the source does
    /// not report groups at all. See the module note on set-valued fields.
    pub groups: Option<Vec<String>>,
    /// Extras this package was pulled in for, or `null` when the source does not
    /// report extras at all.
    pub extras: Option<Vec<String>>,
    pub marker: PythonMarker,
}

/// How `latest` differs from `current` under PEP 440.
///
/// Deliberately *not* the Cargo rule used by [`UpdateType`]. PEP 440 has no
/// compatibility convention to encode: there is no "leftmost non-zero component"
/// boundary, and pretending otherwise would fabricate a promise Python does not
/// make. This enum describes the version numbers and nothing more.
///
/// [`UpdateType`]: crate::core::output::UpdateType
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonUpdateType {
    /// The PEP 440 epoch differs. Its own class rather than a flavour of
    /// `Major`, because an epoch bump declares the project's whole versioning
    /// scheme changed and there is no comparison to make across it.
    Epoch,
    /// Same epoch, first release component differs.
    ///
    /// This is not a prediction of breakage. A calendar-versioned project bumps
    /// its first component every January, and there is no reliable way to detect
    /// CalVer, so this crate does not guess.
    Major,
    /// First release component equal, second differs.
    Minor,
    /// First two release components equal, a later one differs. Release segments
    /// are zero-padded to equal length before comparison, so `1.4` to `1.4.1` is
    /// a patch.
    Patch,
    /// Release segments are identical; only a pre-release, post-release, dev, or
    /// local segment differs.
    Qualifier,
    /// One of the two versions is not valid PEP 440.
    ///
    /// A first-class outcome, not an error. Returning an honest "we do not know"
    /// beats defaulting to `Patch`, which a caller would read as safe.
    Unclassified,
}

/// Whether a package is depended on directly or pulled in by something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonDependencyScope {
    Direct,
    Transitive,
    /// The source does not distinguish the two. Several genuinely do not, and an
    /// entry that cannot be established as direct must not be filed as
    /// transitive just to fill the field.
    Unknown,
}

/// Normalized security results.
#[derive(Debug, Serialize)]
pub struct PythonSecurityReport {
    pub summary: PythonSecuritySummary,
    /// Empty means the scanner ran and found nothing. That is a different claim
    /// from [`PythonOutput::security`] being `None`.
    pub findings: Vec<PythonVulnerability>,
}

/// Finding counts by severity.
///
/// `unknown` has its own bucket rather than being folded into `low`: a summary
/// whose four graded buckets are zero while `unknown` is not must not read as
/// clean.
#[derive(Debug, Serialize)]
pub struct PythonSecuritySummary {
    pub critical: usize,
    pub high: usize,
    pub moderate: usize,
    pub low: usize,
    pub unknown: usize,
    pub total: usize,
}

/// One advisory matched against one installed package.
#[derive(Debug, Serialize)]
pub struct PythonVulnerability {
    /// The advisory identifier as issued, such as a `GHSA-` or `PYSEC-` id.
    pub id: String,
    /// Other identifiers for the same advisory, or `null` when the source does
    /// not publish alias sets.
    ///
    /// Alias publication is source-dependent, so this is one of the set-valued
    /// fields: `[]` says the source publishes aliases and this advisory has none.
    /// Collapsing the two would let a consumer that deduplicates findings by
    /// alias intersection report one CVE as two distinct vulnerabilities.
    pub aliases: Option<Vec<String>>,
    /// PEP 503 normalized package name.
    pub package: String,
    pub installed_version: String,
    pub severity: PythonSeverity,
    /// The advisory's summary line, or `null` when the source publishes none.
    /// Never an empty string, which would read as a title rather than an absence.
    pub title: Option<String>,
    pub scope: PythonDependencyScope,
    /// Versions the advisory names as fixed, or `null` when the source reports no
    /// fix information.
    ///
    /// A set rather than a `fix_available` boolean, because the list answers the
    /// same question without collapsing "the advisory names no fix" into "no fix
    /// information was reported".
    pub fixed_versions: Option<Vec<String>>,
}

/// Advisory severity.
///
/// [`Severity`] on the Rust side has no `Unknown`, and adding one would have
/// changed an existing contract. Python advisory sources frequently publish no
/// severity at all, and such a finding is reported as `Unknown` rather than
/// downgraded into `Low`.
///
/// [`Severity`]: crate::core::output::Severity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonSeverity {
    Critical,
    High,
    Moderate,
    Low,
    /// The source established no severity for this finding. Under
    /// `--fail-on-vulnerability` this satisfies every threshold: a severity that
    /// was never established cannot be shown to be below the bar.
    Unknown,
}

/// A PEP 508 environment marker, which a Python source may not expose at all.
///
/// Every other "the source may not report this" field on this schema is an
/// `Option<Vec<String>>`: `null` for not reported, `[]` for reported-and-empty.
/// A marker is single-valued, so that encoding cannot carry it — `null` would
/// have to mean both "markers are not reported" and "this dependency has no
/// marker", which are different facts with different consequences. Three states
/// need a tag, so this one field is tagged and the rest are not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "expression", rename_all = "snake_case")]
pub enum PythonMarker {
    /// The source does not report markers at all.
    NotReported,
    /// The source reports markers and this dependency has none.
    Absent,
    Reported(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::doc_examples::DocumentedExamples;
    use serde_json::Value;

    /// The richest shape: both capabilities measured, and every
    /// reported/not-reported state exercised at least once.
    pub(crate) fn measured_output() -> PythonOutput {
        PythonOutput {
            schema_version: PYTHON_SCHEMA_VERSION,
            manager: PythonManager {
                name: PythonManagerName::Uv,
                version: Some("0.0.0".to_string()),
            },
            complete: true,
            capabilities: vec![
                PythonCapabilityCoverage {
                    name: PythonCapability::Outdated,
                    measured: true,
                },
                PythonCapabilityCoverage {
                    name: PythonCapability::Security,
                    measured: true,
                },
            ],
            unavailable: Vec::new(),
            outdated: Some(PythonOutdatedReport {
                checked: 12,
                outdated: 2,
                counts: PythonUpdateCounts {
                    epoch: 0,
                    major: 1,
                    minor: 0,
                    patch: 0,
                    qualifier: 1,
                    unclassified: 0,
                },
                packages: vec![
                    PythonOutdatedPackage {
                        name: "example-http".to_string(),
                        current: "1.4.2".to_string(),
                        latest: "2.0.0".to_string(),
                        update_type: PythonUpdateType::Major,
                        scope: PythonDependencyScope::Direct,
                        groups: Some(vec!["main".to_string()]),
                        extras: Some(vec!["socks".to_string()]),
                        marker: PythonMarker::Reported("python_version >= '3.10'".to_string()),
                    },
                    PythonOutdatedPackage {
                        name: "example-parser".to_string(),
                        current: "0.9.0".to_string(),
                        latest: "0.9.0.post1".to_string(),
                        update_type: PythonUpdateType::Qualifier,
                        scope: PythonDependencyScope::Transitive,
                        // Reported-and-empty next to not-reported in the same
                        // entry: the documented example is the place that
                        // difference has to be visible.
                        groups: Some(Vec::new()),
                        extras: None,
                        marker: PythonMarker::Absent,
                    },
                ],
            }),
            security: Some(PythonSecurityReport {
                summary: PythonSecuritySummary {
                    critical: 0,
                    high: 1,
                    moderate: 0,
                    low: 0,
                    unknown: 1,
                    total: 2,
                },
                findings: vec![
                    PythonVulnerability {
                        id: "GHSA-0000-0000-0000".to_string(),
                        aliases: Some(vec!["CVE-0000-00000".to_string()]),
                        package: "example-http".to_string(),
                        installed_version: "1.4.2".to_string(),
                        severity: PythonSeverity::High,
                        title: Some("Example advisory".to_string()),
                        scope: PythonDependencyScope::Direct,
                        fixed_versions: Some(vec!["1.4.3".to_string(), "2.0.0".to_string()]),
                    },
                    PythonVulnerability {
                        id: "PYSEC-0000-0000".to_string(),
                        aliases: None,
                        package: "example-parser".to_string(),
                        installed_version: "0.9.0".to_string(),
                        severity: PythonSeverity::Unknown,
                        title: None,
                        scope: PythonDependencyScope::Transitive,
                        fixed_versions: None,
                    },
                ],
            }),
            warnings: vec![
                "uv documents its own JSON output as unstable; this report is normalized into \
                 cargo-upkeep schema_version 1"
                    .to_string(),
            ],
        }
    }

    /// A manager that cannot answer one capability at all.
    pub(crate) fn capability_gap_output() -> PythonOutput {
        PythonOutput {
            schema_version: PYTHON_SCHEMA_VERSION,
            manager: PythonManager {
                name: PythonManagerName::Poetry,
                version: None,
            },
            complete: false,
            capabilities: vec![
                PythonCapabilityCoverage {
                    name: PythonCapability::Outdated,
                    measured: true,
                },
                PythonCapabilityCoverage {
                    name: PythonCapability::Security,
                    measured: false,
                },
            ],
            unavailable: vec![PythonUnavailableCapability {
                name: PythonCapability::Security,
                reason: PythonUnavailableReason::Unsupported,
                detail: "the detected manager reports no vulnerability data; run a dedicated \
                         scanner and gate on that instead"
                    .to_string(),
            }],
            outdated: Some(PythonOutdatedReport {
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
            }),
            security: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn documented_json_examples_match_output_contract() {
        let documentation = DocumentedExamples::load("docs/python-schema.md");

        for (example, output) in [
            ("python", measured_output()),
            ("python-capability-gap", capability_gap_output()),
        ] {
            assert_eq!(
                documentation.example(example),
                crate::core::doc_examples::serialized_value(&output),
                "{example}: docs/python-schema.md JSON example drifted from its serialized \
                 output contract"
            );
        }
    }

    #[test]
    fn schema_version_is_emitted_verbatim() {
        // The constant is the contract, so the payload must carry it rather than
        // a literal that can drift from it.
        let value = crate::core::doc_examples::serialized_value(&measured_output());
        assert_eq!(value["schema_version"], Value::from(PYTHON_SCHEMA_VERSION));
    }

    #[test]
    fn unmeasured_capability_serializes_as_explicit_null() {
        // The whole point of the schema: a capability nobody ran must not be
        // representable as an empty result, and must not vanish from the payload
        // either. `null` is load-bearing, so an accidental
        // `skip_serializing_if = "Option::is_none"` has to fail here.
        let value = crate::core::doc_examples::serialized_value(&capability_gap_output());

        assert!(
            value.as_object().expect("object").contains_key("security"),
            "an unmeasured capability must stay present as null, not be omitted"
        );
        assert_eq!(value["security"], Value::Null);
        assert_eq!(value["complete"], Value::Bool(false));
        assert_eq!(value["unavailable"][0]["name"], Value::from("security"));
        assert_eq!(
            value["unavailable"][0]["reason"],
            Value::from("unsupported")
        );

        // A measured-but-empty result is the opposite claim and must not look
        // the same.
        assert_eq!(value["outdated"]["outdated"], Value::from(0));
        assert_eq!(value["outdated"]["packages"], Value::Array(Vec::new()));
    }

    /// A set-valued field must never let "the source does not report this" be
    /// read as "there are none" — the defaulted-to-healthy shape of #10/#34.
    /// `null` and `[]` carry that distinction at every level of the payload.
    #[test]
    fn set_valued_fields_distinguish_empty_from_unreported() {
        let reported_empty: Option<Vec<String>> = Some(Vec::new());
        let not_reported: Option<Vec<String>> = None;

        assert_eq!(
            serde_json::to_value(&reported_empty).unwrap(),
            Value::Array(Vec::new())
        );
        assert_eq!(serde_json::to_value(&not_reported).unwrap(), Value::Null);
        assert_ne!(
            serde_json::to_value(&reported_empty).unwrap(),
            serde_json::to_value(&not_reported).unwrap()
        );
    }

    /// Every set-valued field on the two fixtures uses that encoding, and none of
    /// them is omitted. A `skip_serializing_if` added later would make an
    /// unreported attribute indistinguishable from a retired key.
    #[test]
    fn unreported_attributes_stay_present_as_null() {
        let value = crate::core::doc_examples::serialized_value(&measured_output());
        let package = &value["outdated"]["packages"][1];
        for field in ["groups", "extras"] {
            assert!(
                package
                    .as_object()
                    .expect("package object")
                    .contains_key(field),
                "{field} must stay present even when unreported"
            );
        }
        assert_eq!(package["extras"], Value::Null, "unreported reads as null");
        assert_eq!(
            package["groups"],
            Value::Array(Vec::new()),
            "reported-and-empty reads as [], not null"
        );

        let finding = &value["security"]["findings"][1];
        assert!(
            finding
                .as_object()
                .expect("finding object")
                .contains_key("fixed_versions"),
            "fixed_versions must stay present even when unreported"
        );
        assert_eq!(finding["fixed_versions"], Value::Null);
    }

    /// A marker is single-valued, so `null` cannot carry both "markers are not
    /// reported" and "this dependency has none". It is the one tagged field, and
    /// its three states must stay distinct.
    #[test]
    fn python_marker_has_three_distinct_states() {
        assert_eq!(
            serde_json::to_value(PythonMarker::Reported("sys_platform == 'win32'".into())).unwrap(),
            serde_json::json!({
                "status": "reported",
                "expression": "sys_platform == 'win32'"
            })
        );
        assert_eq!(
            serde_json::to_value(PythonMarker::Absent).unwrap(),
            serde_json::json!({ "status": "absent" })
        );
        assert_eq!(
            serde_json::to_value(PythonMarker::NotReported).unwrap(),
            serde_json::json!({ "status": "not_reported" })
        );
    }

    /// `counts` and `summary` exist so a pipeline can gate without walking every
    /// entry. That is only safe if they agree with the entries they summarize, so
    /// the invariants are asserted rather than left to each adapter to interpret.
    #[test]
    fn summaries_agree_with_the_entries_they_summarize() {
        let output = measured_output();
        let outdated = output.outdated.as_ref().expect("outdated measured");
        let counts = &outdated.counts;
        let bucketed = counts.epoch
            + counts.major
            + counts.minor
            + counts.patch
            + counts.qualifier
            + counts.unclassified;

        assert_eq!(
            outdated.outdated,
            outdated.packages.len(),
            "`outdated` counts the entries in `packages`; it is never truncated"
        );
        assert_eq!(
            bucketed, outdated.outdated,
            "every entry lands in one bucket"
        );
        assert!(
            outdated.checked >= outdated.outdated,
            "an outdated package was necessarily checked"
        );

        let security = output.security.as_ref().expect("security measured");
        let summary = &security.summary;
        assert_eq!(
            summary.total,
            security.findings.len(),
            "`total` counts findings, not deduplicated advisories"
        );
        assert_eq!(
            summary.critical + summary.high + summary.moderate + summary.low + summary.unknown,
            summary.total,
            "every finding lands in exactly one severity bucket"
        );
    }

    #[test]
    fn enum_labels_are_pinned() {
        // These strings are the public contract; a rename is a schema_version
        // bump, so it must not be possible to do one silently.
        for (value, label) in [
            (
                serde_json::to_value(PythonManagerName::PipTools),
                "pip_tools",
            ),
            (serde_json::to_value(PythonUpdateType::Epoch), "epoch"),
            (serde_json::to_value(PythonUpdateType::Major), "major"),
            (serde_json::to_value(PythonUpdateType::Minor), "minor"),
            (serde_json::to_value(PythonUpdateType::Patch), "patch"),
            (
                serde_json::to_value(PythonUpdateType::Qualifier),
                "qualifier",
            ),
            (
                serde_json::to_value(PythonUpdateType::Unclassified),
                "unclassified",
            ),
            (
                serde_json::to_value(PythonDependencyScope::Unknown),
                "unknown",
            ),
            (serde_json::to_value(PythonSeverity::Critical), "critical"),
            (serde_json::to_value(PythonSeverity::High), "high"),
            (serde_json::to_value(PythonSeverity::Moderate), "moderate"),
            (serde_json::to_value(PythonSeverity::Low), "low"),
            (serde_json::to_value(PythonSeverity::Unknown), "unknown"),
            (
                serde_json::to_value(PythonUnavailableReason::NotInstalled),
                "not_installed",
            ),
            (
                serde_json::to_value(PythonUnavailableReason::Unsupported),
                "unsupported",
            ),
            (serde_json::to_value(PythonCapability::Security), "security"),
        ] {
            assert_eq!(value.unwrap(), Value::String(label.to_string()));
        }
    }

    #[test]
    fn unavailable_reason_labels_match_the_rust_side() {
        // `not_installed` and `failed` are shared vocabulary with `quality`'s
        // `UnavailableReason`, not a coincidence. If either side is relabelled,
        // callers reading both payloads see two names for one distinction.
        use crate::core::output::UnavailableReason;

        assert_eq!(
            serde_json::to_value(PythonUnavailableReason::NotInstalled).unwrap(),
            serde_json::to_value(UnavailableReason::NotInstalled).unwrap()
        );
        assert_eq!(
            serde_json::to_value(PythonUnavailableReason::Failed).unwrap(),
            serde_json::to_value(UnavailableReason::Failed).unwrap()
        );
    }
}
