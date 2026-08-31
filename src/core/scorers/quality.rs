use crate::core::output::{
    Grade, MetricScore, QualityOutput, UnavailableMetric, UnavailableReason,
};

// === Metric Weights ===
// These weights determine how much each metric contributes to the overall quality score.
// They must sum to 1.0 (100%); `weights_sum_to_one` in the tests below enforces that.

pub const WEIGHT_DEPENDENCY_FRESHNESS: f32 = 0.20;
pub const WEIGHT_SECURITY: f32 = 0.25;
pub const WEIGHT_UNUSED_DEPS: f32 = 0.15;
pub const WEIGHT_UNSAFE_CODE: f32 = 0.15;
pub const WEIGHT_CLIPPY: f32 = 0.15;
pub const WEIGHT_MSRV: f32 = 0.10;

/// Total weight across all six metrics, used to express `measured_weight` as a
/// fraction. Derived rather than hardcoded so it stays correct if a weight moves.
const TOTAL_WEIGHT: f32 = WEIGHT_DEPENDENCY_FRESHNESS
    + WEIGHT_SECURITY
    + WEIGHT_UNUSED_DEPS
    + WEIGHT_UNSAFE_CODE
    + WEIGHT_CLIPPY
    + WEIGHT_MSRV;

// === Metric Names ===
// Shared between the breakdown, the unavailable list, and `recommendations_for`,
// which dispatches on them.

pub const METRIC_DEPENDENCY_FRESHNESS: &str = "Dependency freshness";
pub const METRIC_SECURITY: &str = "Security";
pub const METRIC_UNUSED_DEPS: &str = "Unused dependencies";
pub const METRIC_UNSAFE_CODE: &str = "Unsafe code";
pub const METRIC_CLIPPY: &str = "Clippy";
pub const METRIC_MSRV: &str = "MSRV";

// === Security Penalty Multipliers ===
// These values define how much each severity level reduces the security score.
// The penalties are designed to reflect the urgency of addressing issues:
// - Critical vulnerabilities have immediate exploitation risk and should block releases
// - High vulnerabilities have significant impact but may require specific conditions
// - Moderate vulnerabilities have limited impact or require unusual circumstances
// - Low vulnerabilities are informational or have minimal impact

/// Points deducted per critical severity vulnerability (25 points each)
const SECURITY_PENALTY_CRITICAL: u64 = 25;
/// Points deducted per high severity vulnerability (15 points each)
const SECURITY_PENALTY_HIGH: u64 = 15;
/// Points deducted per moderate severity vulnerability (5 points each)
const SECURITY_PENALTY_MODERATE: u64 = 5;
/// Points deducted per low severity vulnerability (2 points each)
const SECURITY_PENALTY_LOW: u64 = 2;

// === Clippy Penalty Multipliers ===
// An error is weighted five times a warning. A clippy `error` is a lint the
// analyzed project's own configuration puts at deny level: the correctness
// group by default, or anything escalated with `-D`, a crate-level
// `deny`/`forbid` attribute, or `[lints.clippy]`.
//
// The weight is a heuristic, not an inference from severity. Under a blanket
// `-D warnings` — which `interpret_clippy_run` already expects to encounter —
// every advisory lint becomes an error, so an error does not necessarily name a
// rule the project singled out as unshippable. `push_driver_error` also counts
// a synthesized `clippy::driver` entry as an error, and that is a non-zero exit
// status rather than a lint at all.

/// Points deducted per clippy warning (2 points each)
const CLIPPY_PENALTY_WARNING: u64 = 2;
/// Points deducted per clippy error (10 points each)
const CLIPPY_PENALTY_ERROR: u64 = 10;

/// Dependency freshness over the dependencies that were *actually checked*.
///
/// `total` is deliberately not "every dependency declared": a dependency whose
/// latest version could not be fetched was not compared against anything, and
/// counting it in the denominator scores it as up to date. See
/// `dependency_freshness` in `cli::commands::quality` for the renormalization.
#[derive(Debug)]
pub struct DependencyFreshness {
    pub total: usize,
    pub outdated: usize,
}

#[derive(Debug)]
pub struct SecuritySummary {
    pub critical: usize,
    pub high: usize,
    pub moderate: usize,
    pub low: usize,
}

#[derive(Debug)]
pub struct ClippySummary {
    pub warnings: usize,
    pub errors: usize,
}

#[derive(Debug)]
pub enum MsrvStatus {
    Valid,
    Missing,
    #[allow(dead_code)] // Will be used when MSRV validation is implemented
    Invalid,
}

#[derive(Debug)]
pub struct UnusedSummary {
    pub unused_count: usize,
}

#[derive(Debug)]
pub struct UnsafeSummary {
    pub total_unsafe: usize,
}

/// Whether a metric's analyzer actually produced data.
///
/// This exists so an analyzer that did not run cannot be encoded as healthy
/// data. Every one of the six inputs is wrapped, including freshness and
/// security, which previously fabricated an empty-but-valid summary on failure
/// and scored a perfect 100.
#[derive(Debug)]
pub enum Availability<T> {
    /// The analyzer ran and produced this data.
    Measured(T),
    /// The analyzer did not produce data, and this is why.
    Unavailable {
        reason: UnavailableReason,
        detail: String,
    },
}

impl<T> Availability<T> {
    /// Constructor for an analyzer whose optional tool is absent.
    pub fn not_installed(detail: impl Into<String>) -> Self {
        Availability::Unavailable {
            reason: UnavailableReason::NotInstalled,
            detail: detail.into(),
        }
    }

    /// Constructor for an analyzer that ran and failed, or that ran but could
    /// not check anything.
    pub fn failed(detail: impl Into<String>) -> Self {
        Availability::Unavailable {
            reason: UnavailableReason::Failed,
            detail: detail.into(),
        }
    }
}

pub struct QualityInputs {
    pub dependency_freshness: Availability<DependencyFreshness>,
    pub security: Availability<SecuritySummary>,
    pub unused: Availability<UnusedSummary>,
    pub unsafe_code: Availability<UnsafeSummary>,
    pub clippy: Availability<ClippySummary>,
    pub msrv: Availability<MsrvStatus>,
}

/// One metric after scoring, before it is split into `breakdown` / `unavailable`.
struct EvaluatedMetric {
    name: &'static str,
    weight: f32,
    outcome: Result<f32, (UnavailableReason, String)>,
}

fn evaluate<T>(
    name: &'static str,
    weight: f32,
    input: Availability<T>,
    score_fn: impl FnOnce(&T) -> f32,
) -> EvaluatedMetric {
    let outcome = match input {
        Availability::Measured(value) => Ok(score_fn(&value)),
        Availability::Unavailable { reason, detail } => Err((reason, detail)),
    };
    EvaluatedMetric {
        name,
        weight,
        outcome,
    }
}

/// Scores a project over the metrics that could actually be measured.
///
/// Unmeasured metrics are **excluded and the remainder renormalized**: their
/// weight leaves the denominator entirely rather than contributing a defaulted
/// value. A user who has not installed `cargo-geiger` does not have a worse
/// project, so there is no penalty — but neither do they get credit for a check
/// that never ran, which is what the old `unwrap_or(100.0)` handed out.
///
/// When nothing at all could be measured, `score` and `grade` are `None`: there
/// is no honest number to report, and any number would be read as a grade.
pub fn score_quality(inputs: QualityInputs) -> QualityOutput {
    let metrics = [
        evaluate(
            METRIC_DEPENDENCY_FRESHNESS,
            WEIGHT_DEPENDENCY_FRESHNESS,
            inputs.dependency_freshness,
            dependency_freshness_score,
        ),
        evaluate(
            METRIC_SECURITY,
            WEIGHT_SECURITY,
            inputs.security,
            security_score,
        ),
        evaluate(
            METRIC_UNUSED_DEPS,
            WEIGHT_UNUSED_DEPS,
            inputs.unused,
            unused_deps_score,
        ),
        evaluate(
            METRIC_UNSAFE_CODE,
            WEIGHT_UNSAFE_CODE,
            inputs.unsafe_code,
            unsafe_code_score,
        ),
        evaluate(METRIC_CLIPPY, WEIGHT_CLIPPY, inputs.clippy, clippy_score),
        evaluate(METRIC_MSRV, WEIGHT_MSRV, inputs.msrv, msrv_score),
    ];

    let mut breakdown = Vec::with_capacity(metrics.len());
    let mut unavailable = Vec::new();
    let mut measured_weight = 0.0f32;
    let mut weighted_total = 0.0f32;

    for metric in metrics {
        let score = match metric.outcome {
            Ok(score) => {
                measured_weight += metric.weight;
                weighted_total += score * metric.weight;
                // Weighted with the raw value, reported rounded: every number
                // this type exposes — `score`, `measured_weight`, and each
                // breakdown entry — is in hundredths, so the JSON and the text
                // rendering of the same metric cannot disagree.
                Some(round_hundredths(score))
            }
            Err((reason, detail)) => {
                unavailable.push(UnavailableMetric {
                    name: metric.name.to_string(),
                    weight: metric.weight,
                    reason,
                    detail,
                });
                None
            }
        };

        breakdown.push(MetricScore {
            name: metric.name.to_string(),
            score,
            weight: metric.weight,
        });
    }

    // Derived from the list, not from float equality on the summed weights:
    // `0.20 + 0.25 + ... == 1.0` is not reliable in f32.
    let complete = unavailable.is_empty();

    // Renormalize over the weight that ran. The guard is the division-by-zero
    // protection: when every metric is unavailable `measured_weight` is 0.0,
    // and `0.0 / 0.0` is NaN, which would serialize as `null` by accident
    // rather than by the deliberate "nothing could be measured" path below.
    let score = (measured_weight > 0.0).then(|| round_hundredths(weighted_total / measured_weight));
    // Grade the same number that is reported. Both the JSON `score` and the
    // text rendering are this rounded value, so the letter is derived from the
    // figure the reader sees rather than from a raw value that could round
    // across a grade boundary. Rounding first can hand out at most 0.005 of
    // undeserved credit exactly at a boundary; number/letter agreement is
    // worth more than that.
    let grade = score.map(grade_for);
    let recommendations = recommendations_for(&breakdown);

    QualityOutput {
        score,
        grade,
        complete,
        measured_weight: round_hundredths(measured_weight / TOTAL_WEIGHT),
        breakdown,
        unavailable,
        recommendations,
    }
}

/// Rounds to two decimal places to keep f32 accumulation noise out of the
/// serialized contract.
///
/// Renormalization divides by a summed weight, so a flawless partial run would
/// otherwise emit `"score": 99.99999` — which reads as "not quite perfect" and
/// defeats any consumer comparing against a threshold exactly.
fn round_hundredths(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
}

fn dependency_freshness_score(input: &DependencyFreshness) -> f32 {
    if input.total == 0 {
        return 100.0;
    }
    let up_to_date = input.total.saturating_sub(input.outdated) as f32;
    let total = input.total as f32;
    (up_to_date / total) * 100.0
}

fn security_score(summary: &SecuritySummary) -> f32 {
    let penalty = (summary.critical as u64)
        .saturating_mul(SECURITY_PENALTY_CRITICAL)
        .saturating_add((summary.high as u64).saturating_mul(SECURITY_PENALTY_HIGH))
        .saturating_add((summary.moderate as u64).saturating_mul(SECURITY_PENALTY_MODERATE))
        .saturating_add((summary.low as u64).saturating_mul(SECURITY_PENALTY_LOW));
    100u64.saturating_sub(penalty) as f32
}

/// The single definition of the clippy penalty, shared by both reporting paths.
///
/// `cargo upkeep clippy` puts this in `ClippyOutput.score` and `cargo upkeep
/// quality` puts it in the Clippy breakdown entry. The analyzer used to carry
/// its own copy of this arithmetic, so changing a weight here silently
/// desynchronised the two commands (#37). Scoring policy lives with the scorer,
/// alongside the weights and the sibling penalties, and
/// `clippy_command_and_quality_breakdown_agree` in `cli::commands::quality`
/// holds the two paths together — there rather than here because it also covers
/// the `ClippyOutput` -> `ClippySummary` mapping, where the same desync can be
/// reintroduced by swapping two fields.
pub(crate) fn clippy_score(summary: &ClippySummary) -> f32 {
    let penalty = (summary.warnings as u64)
        .saturating_mul(CLIPPY_PENALTY_WARNING)
        .saturating_add((summary.errors as u64).saturating_mul(CLIPPY_PENALTY_ERROR));
    100u64.saturating_sub(penalty) as f32
}

fn unused_deps_score(summary: &UnusedSummary) -> f32 {
    // Each unused dependency reduces the score by 5 points
    let penalty = (summary.unused_count as u64).saturating_mul(5);
    100u64.saturating_sub(penalty) as f32
}

fn unsafe_code_score(summary: &UnsafeSummary) -> f32 {
    // Unsafe code has diminishing penalty: first few are more impactful
    // 0 unsafe = 100, 1-5 = 95-75, 6+ = gradual decline
    let penalty = match summary.total_unsafe {
        0 => 0,
        1..=5 => summary.total_unsafe as u64 * 5,
        _ => 25 + (summary.total_unsafe.saturating_sub(5) as u64).min(25),
    };
    100u64.saturating_sub(penalty) as f32
}

fn msrv_score(status: &MsrvStatus) -> f32 {
    match status {
        MsrvStatus::Valid => 100.0,
        MsrvStatus::Missing => 50.0,
        MsrvStatus::Invalid => 0.0,
    }
}

fn grade_for(score: f32) -> Grade {
    if score >= 90.0 {
        Grade::A
    } else if score >= 80.0 {
        Grade::B
    } else if score >= 70.0 {
        Grade::C
    } else if score >= 60.0 {
        Grade::D
    } else {
        Grade::F
    }
}

/// Builds the actionable recommendations, ordered by weighted impact.
///
/// Metrics with a `None` score are skipped entirely. An unmeasured metric is
/// not a finding about the project, and telling a user to "Remove unused
/// dependencies" because `cargo-machete` is missing is exactly the false signal
/// this module is meant to stop emitting. Unmeasured metrics are reported
/// through `QualityOutput::unavailable` instead.
fn recommendations_for(metrics: &[MetricScore]) -> Vec<String> {
    let mut items = Vec::new();

    for metric in metrics {
        let Some(score) = metric.score else {
            continue;
        };

        let message = match metric.name.as_str() {
            METRIC_DEPENDENCY_FRESHNESS if score < 90.0 => {
                Some("Update outdated dependencies.".to_string())
            }
            METRIC_SECURITY if score < 90.0 => Some("Address security advisories.".to_string()),
            METRIC_UNUSED_DEPS if score < 100.0 => Some("Remove unused dependencies.".to_string()),
            METRIC_UNSAFE_CODE if score < 100.0 => Some("Reduce unsafe code usage.".to_string()),
            METRIC_CLIPPY if score < 90.0 => Some("Fix clippy warnings and errors.".to_string()),
            METRIC_MSRV if score < 100.0 => Some("Declare a valid MSRV in Cargo.toml.".to_string()),
            _ => None,
        };

        if let Some(message) = message {
            let impact = (100.0 - score) * metric.weight;
            items.push((impact, message));
        }
    }

    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    items.into_iter().map(|(_, message)| message).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Maximum allowed difference between actual and expected values in floating-point comparisons.
    /// Set to 0.01 (1%) to account for floating-point arithmetic imprecision while still catching
    /// meaningful deviations in score calculations.
    const FLOAT_TOLERANCE: f32 = 0.01;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < FLOAT_TOLERANCE,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn dependency_freshness_score_handles_zero_total() {
        let input = DependencyFreshness {
            total: 0,
            outdated: 0,
        };
        assert_close(dependency_freshness_score(&input), 100.0);
    }

    #[test]
    fn dependency_freshness_score_computes_ratio() {
        let input = DependencyFreshness {
            total: 10,
            outdated: 2,
        };
        assert_close(dependency_freshness_score(&input), 80.0);
    }

    #[test]
    fn security_score_applies_penalties() {
        let summary = SecuritySummary {
            critical: 1,
            high: 2,
            moderate: 3,
            low: 4,
        };
        // Penalty: 25 + 30 + 15 + 8 = 78
        assert_close(security_score(&summary), 22.0);
    }

    #[test]
    fn clippy_score_applies_penalties() {
        let summary = ClippySummary {
            warnings: 3,
            errors: 1,
        };
        assert_close(clippy_score(&summary), 84.0);
    }

    #[test]
    fn unused_deps_score_applies_penalties() {
        let summary = UnusedSummary { unused_count: 4 };
        assert_close(unused_deps_score(&summary), 80.0);
    }

    #[test]
    fn unsafe_code_score_handles_buckets() {
        let zero = UnsafeSummary { total_unsafe: 0 };
        let few = UnsafeSummary { total_unsafe: 3 };
        let many = UnsafeSummary { total_unsafe: 8 };

        assert_close(unsafe_code_score(&zero), 100.0);
        assert_close(unsafe_code_score(&few), 85.0);
        assert_close(unsafe_code_score(&many), 72.0);
    }

    #[test]
    fn msrv_score_handles_status() {
        assert_close(msrv_score(&MsrvStatus::Valid), 100.0);
        assert_close(msrv_score(&MsrvStatus::Missing), 50.0);
        assert_close(msrv_score(&MsrvStatus::Invalid), 0.0);
    }

    #[test]
    fn grade_for_respects_boundaries() {
        assert!(matches!(grade_for(92.0), Grade::A));
        assert!(matches!(grade_for(89.99), Grade::B));
        assert!(matches!(grade_for(79.99), Grade::C));
        assert!(matches!(grade_for(69.99), Grade::D));
        assert!(matches!(grade_for(59.99), Grade::F));
    }

    #[test]
    fn recommendations_sorted_by_impact() {
        let metrics = vec![
            metric(METRIC_DEPENDENCY_FRESHNESS, Some(60.0)),
            metric(METRIC_SECURITY, Some(70.0)),
            metric(METRIC_CLIPPY, Some(80.0)),
            metric(METRIC_UNUSED_DEPS, Some(90.0)),
            metric(METRIC_UNSAFE_CODE, Some(95.0)),
            metric(METRIC_MSRV, Some(50.0)),
        ];

        let recommendations = recommendations_for(&metrics);
        assert_eq!(recommendations[0], "Update outdated dependencies.");
        assert_eq!(recommendations[1], "Address security advisories.");
        assert_eq!(recommendations[2], "Declare a valid MSRV in Cargo.toml.");
        assert_eq!(recommendations[3], "Fix clippy warnings and errors.");
        assert_eq!(recommendations[4], "Remove unused dependencies.");
        assert_eq!(recommendations[5], "Reduce unsafe code usage.");
    }

    // === Helpers for the availability tests ===

    fn weight_for(name: &str) -> f32 {
        match name {
            METRIC_DEPENDENCY_FRESHNESS => WEIGHT_DEPENDENCY_FRESHNESS,
            METRIC_SECURITY => WEIGHT_SECURITY,
            METRIC_UNUSED_DEPS => WEIGHT_UNUSED_DEPS,
            METRIC_UNSAFE_CODE => WEIGHT_UNSAFE_CODE,
            METRIC_CLIPPY => WEIGHT_CLIPPY,
            METRIC_MSRV => WEIGHT_MSRV,
            other => panic!("unknown metric {other}"),
        }
    }

    fn metric(name: &str, score: Option<f32>) -> MetricScore {
        MetricScore {
            name: name.to_string(),
            score,
            weight: weight_for(name),
        }
    }

    /// A project with nothing wrong with it: every metric measures a clean 100.
    fn perfect_inputs() -> QualityInputs {
        QualityInputs {
            dependency_freshness: Availability::Measured(DependencyFreshness {
                total: 10,
                outdated: 0,
            }),
            security: Availability::Measured(SecuritySummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
            }),
            unused: Availability::Measured(UnusedSummary { unused_count: 0 }),
            unsafe_code: Availability::Measured(UnsafeSummary { total_unsafe: 0 }),
            clippy: Availability::Measured(ClippySummary {
                warnings: 0,
                errors: 0,
            }),
            msrv: Availability::Measured(MsrvStatus::Valid),
        }
    }

    fn score_of(output: &QualityOutput, name: &str) -> Option<f32> {
        output
            .breakdown
            .iter()
            .find(|metric| metric.name == name)
            .unwrap_or_else(|| panic!("missing breakdown entry for {name}"))
            .score
    }

    fn unavailable_entry<'a>(output: &'a QualityOutput, name: &str) -> &'a UnavailableMetric {
        output
            .unavailable
            .iter()
            .find(|metric| metric.name == name)
            .unwrap_or_else(|| panic!("expected {name} to be unavailable"))
    }

    #[test]
    fn weights_sum_to_one() {
        assert_close(TOTAL_WEIGHT, 1.0);
    }

    /// The exact scenario the README documents under "Interpreting a partial
    /// result", so its printed numbers cannot drift from the scorer.
    ///
    /// That section exists to teach people to read the renormalized score, so
    /// arithmetic it cannot reproduce invites readers to distrust the
    /// renormalization itself. Regenerate the README block from this test's
    /// output rather than editing the numbers by hand.
    fn readme_partial_example() -> QualityInputs {
        QualityInputs {
            dependency_freshness: Availability::Measured(DependencyFreshness {
                total: 10,
                outdated: 2,
            }),
            security: Availability::Measured(SecuritySummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
            }),
            unused: Availability::not_installed(
                "cargo-machete is not installed; install with `cargo install cargo-machete`",
            ),
            unsafe_code: Availability::Measured(UnsafeSummary { total_unsafe: 2 }),
            clippy: Availability::Measured(ClippySummary {
                warnings: 12,
                errors: 0,
            }),
            msrv: Availability::Measured(MsrvStatus::Valid),
        }
    }

    #[test]
    fn readme_partial_example_matches_the_documented_output() {
        let output = score_quality(readme_partial_example());

        assert_eq!(score_of(&output, METRIC_DEPENDENCY_FRESHNESS), Some(80.0));
        assert_eq!(score_of(&output, METRIC_SECURITY), Some(100.0));
        assert_eq!(score_of(&output, METRIC_UNUSED_DEPS), None);
        assert_eq!(score_of(&output, METRIC_UNSAFE_CODE), Some(90.0));
        assert_eq!(score_of(&output, METRIC_CLIPPY), Some(76.0));
        assert_eq!(score_of(&output, METRIC_MSRV), Some(100.0));

        // 0.20*80 + 0.25*100 + 0.15*90 + 0.15*76 + 0.10*100 = 75.9
        // 75.9 / 0.85 = 89.29
        assert_eq!(output.score, Some(89.29));
        assert_eq!(output.grade, Some(Grade::B));
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.85);

        // Ordered by weighted impact: freshness (100-80)*0.20 = 4.0, then
        // clippy (100-76)*0.15 = 3.6, then unsafe (100-90)*0.15 = 1.5.
        assert_eq!(
            output.recommendations,
            vec![
                "Update outdated dependencies.".to_string(),
                "Fix clippy warnings and errors.".to_string(),
                "Reduce unsafe code usage.".to_string(),
            ]
        );
    }

    #[test]
    fn score_quality_is_complete_when_everything_measured() {
        let output = score_quality(perfect_inputs());

        assert!(output.complete);
        assert!(output.unavailable.is_empty());
        assert_close(output.measured_weight, 1.0);
        assert_close(output.score.expect("score"), 100.0);
        assert_eq!(output.grade, Some(Grade::A));
        assert!(output.breakdown.iter().all(|metric| metric.score.is_some()));
    }

    #[test]
    fn score_quality_excludes_unmeasured_metric_from_the_denominator() {
        // Everything perfect except clippy, which scores 0, and unused, which
        // is unmeasured. Renormalizing drops unused' 0.15 from the denominator:
        // measured weight 0.85, weighted total 0.85 - 0.15 (clippy) = 0.70.
        // 0.70 / 0.85 * 100 = 82.35.
        let output = score_quality(QualityInputs {
            clippy: Availability::Measured(ClippySummary {
                warnings: 100,
                errors: 0,
            }),
            unused: Availability::not_installed("cargo-machete is not installed"),
            ..perfect_inputs()
        });

        assert!(!output.complete);
        assert_close(output.measured_weight, 0.85);
        assert_close(output.score.expect("score"), 82.35);
        assert_eq!(output.grade, Some(Grade::B));

        // The old behaviour scored the unmeasured metric 100 and divided by a
        // full 1.0 denominator, giving 85.0 and hiding the exclusion entirely.
        assert!(output.score.expect("score") < 85.0);
    }

    #[test]
    fn score_quality_missing_machete_does_not_inflate_score() {
        let output = score_quality(QualityInputs {
            unused: Availability::not_installed(
                "cargo-machete is not installed; install with `cargo install cargo-machete`",
            ),
            ..perfect_inputs()
        });

        assert!(!output.complete);
        assert_eq!(score_of(&output, METRIC_UNUSED_DEPS), None);
        assert_close(output.measured_weight, 0.85);

        let entry = unavailable_entry(&output, METRIC_UNUSED_DEPS);
        assert_eq!(entry.reason, UnavailableReason::NotInstalled);
        assert_close(entry.weight, WEIGHT_UNUSED_DEPS);
        assert!(entry.detail.contains("cargo install cargo-machete"));

        // A clean project still grades cleanly — a missing optional tool is not
        // a health problem, so exclusion must not penalize.
        assert_close(output.score.expect("score"), 100.0);
        assert!(!output
            .recommendations
            .contains(&"Remove unused dependencies.".to_string()));
    }

    #[test]
    fn score_quality_missing_geiger_does_not_inflate_score() {
        let output = score_quality(QualityInputs {
            unsafe_code: Availability::not_installed(
                "cargo-geiger is not installed; install with `cargo install cargo-geiger`",
            ),
            ..perfect_inputs()
        });

        assert_eq!(score_of(&output, METRIC_UNSAFE_CODE), None);
        assert_eq!(
            unavailable_entry(&output, METRIC_UNSAFE_CODE).reason,
            UnavailableReason::NotInstalled
        );
        assert_close(output.measured_weight, 0.85);
        assert!(!output
            .recommendations
            .contains(&"Reduce unsafe code usage.".to_string()));
    }

    #[test]
    fn score_quality_failed_audit_is_excluded_not_assumed_clean() {
        let output = score_quality(QualityInputs {
            security: Availability::failed("advisory database unreachable"),
            ..perfect_inputs()
        });

        assert!(!output.complete);
        assert_eq!(score_of(&output, METRIC_SECURITY), None);
        assert_close(output.measured_weight, 0.75);

        let entry = unavailable_entry(&output, METRIC_SECURITY);
        assert_eq!(entry.reason, UnavailableReason::Failed);
        assert_close(entry.weight, WEIGHT_SECURITY);
        assert!(!output
            .recommendations
            .contains(&"Address security advisories.".to_string()));
    }

    #[test]
    fn score_quality_failed_deps_is_excluded_not_scored_as_empty() {
        // The old fallback was `total: 0`, which `dependency_freshness_score`
        // scores 100. Exclusion must not resurrect that.
        let output = score_quality(QualityInputs {
            dependency_freshness: Availability::failed("crates.io unreachable"),
            ..perfect_inputs()
        });

        assert_eq!(score_of(&output, METRIC_DEPENDENCY_FRESHNESS), None);
        assert_close(output.measured_weight, 0.80);
        assert_eq!(
            unavailable_entry(&output, METRIC_DEPENDENCY_FRESHNESS).reason,
            UnavailableReason::Failed
        );
        assert!(!output
            .recommendations
            .contains(&"Update outdated dependencies.".to_string()));
    }

    #[test]
    fn score_quality_mixed_success_and_failure() {
        // Measured: security (0.25) at 100, MSRV (0.10) at 50, freshness (0.20)
        // at 80. Unmeasured: unused (not installed), unsafe (not installed),
        // clippy (failed). Measured weight 0.55; weighted total
        // 0.25*100 + 0.10*50 + 0.20*80 = 25 + 5 + 16 = 46. 46 / 0.55 = 83.64.
        let output = score_quality(QualityInputs {
            dependency_freshness: Availability::Measured(DependencyFreshness {
                total: 10,
                outdated: 2,
            }),
            security: Availability::Measured(SecuritySummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
            }),
            unused: Availability::not_installed("cargo-machete is not installed"),
            unsafe_code: Availability::not_installed("cargo-geiger is not installed"),
            clippy: Availability::failed("clippy exited 101"),
            msrv: Availability::Measured(MsrvStatus::Missing),
        });

        assert!(!output.complete);
        assert_eq!(output.unavailable.len(), 3);
        assert_close(output.measured_weight, 0.55);
        assert_close(output.score.expect("score"), 83.64);
        assert_eq!(output.grade, Some(Grade::B));

        assert_eq!(
            unavailable_entry(&output, METRIC_UNUSED_DEPS).reason,
            UnavailableReason::NotInstalled
        );
        assert_eq!(
            unavailable_entry(&output, METRIC_CLIPPY).reason,
            UnavailableReason::Failed
        );

        // Only the measured findings produce advice, ordered by weighted
        // impact: MSRV loses (100-50)*0.10 = 5.0, freshness (100-80)*0.20 = 4.0.
        assert_eq!(
            output.recommendations,
            vec![
                "Declare a valid MSRV in Cargo.toml.".to_string(),
                "Update outdated dependencies.".to_string(),
            ]
        );
    }

    #[test]
    fn score_quality_reports_nothing_when_no_metric_measured() {
        let output = score_quality(QualityInputs {
            dependency_freshness: Availability::failed("boom"),
            security: Availability::failed("boom"),
            unused: Availability::not_installed("cargo-machete is not installed"),
            unsafe_code: Availability::not_installed("cargo-geiger is not installed"),
            clippy: Availability::failed("boom"),
            msrv: Availability::failed("boom"),
        });

        assert_eq!(output.score, None);
        assert_eq!(output.grade, None);
        assert!(!output.complete);
        assert_close(output.measured_weight, 0.0);
        assert_eq!(output.unavailable.len(), 6);
        assert!(output.breakdown.iter().all(|metric| metric.score.is_none()));
        assert!(output.recommendations.is_empty());
    }

    /// The bug this whole change exists to prevent: a total analysis failure
    /// used to print `Score: 100.0 / Grade: A`.
    #[test]
    fn total_failure_can_never_produce_a_passing_grade() {
        let output = score_quality(QualityInputs {
            dependency_freshness: Availability::failed("boom"),
            security: Availability::failed("boom"),
            unused: Availability::failed("boom"),
            unsafe_code: Availability::failed("boom"),
            clippy: Availability::failed("boom"),
            msrv: Availability::failed("boom"),
        });

        assert!(
            output.grade.is_none(),
            "a grade of {:?} was produced from zero measurements",
            output.grade
        );
        assert!(output.score.is_none());
        assert!(!output.complete);
    }
}
