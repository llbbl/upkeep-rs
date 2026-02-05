use crate::core::output::{Grade, MetricScore, QualityOutput};

// === Metric Weights ===
// These weights determine how much each metric contributes to the overall quality score.
// They must sum to 1.0 (100%).

pub const WEIGHT_DEPENDENCY_FRESHNESS: f32 = 0.20;
pub const WEIGHT_SECURITY: f32 = 0.25;
pub const WEIGHT_UNUSED_DEPS: f32 = 0.15;
pub const WEIGHT_UNSAFE_CODE: f32 = 0.15;
pub const WEIGHT_CLIPPY: f32 = 0.15;
pub const WEIGHT_MSRV: f32 = 0.10;

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

pub struct DependencyFreshness {
    pub total: usize,
    pub outdated: usize,
}

pub struct SecuritySummary {
    pub critical: usize,
    pub high: usize,
    pub moderate: usize,
    pub low: usize,
}

pub struct ClippySummary {
    pub warnings: usize,
    pub errors: usize,
}

pub enum MsrvStatus {
    Valid,
    Missing,
    #[allow(dead_code)] // Will be used when MSRV validation is implemented
    Invalid,
}

pub struct UnusedSummary {
    pub unused_count: usize,
}

pub struct UnsafeSummary {
    pub total_unsafe: usize,
}

pub struct QualityInputs {
    pub dependency_freshness: DependencyFreshness,
    pub security: SecuritySummary,
    pub unused: Option<UnusedSummary>,
    pub unsafe_code: Option<UnsafeSummary>,
    pub clippy: Option<ClippySummary>,
    pub msrv: MsrvStatus,
}

pub fn score_quality(inputs: &QualityInputs) -> QualityOutput {
    let freshness_score = dependency_freshness_score(&inputs.dependency_freshness);
    let security_score = security_score(&inputs.security);
    let unused_score = inputs
        .unused
        .as_ref()
        .map(unused_deps_score)
        .unwrap_or(100.0);
    let unsafe_score = inputs
        .unsafe_code
        .as_ref()
        .map(unsafe_code_score)
        .unwrap_or(100.0);
    let clippy_score = inputs.clippy.as_ref().map(clippy_score).unwrap_or(100.0);
    let msrv_score = msrv_score(&inputs.msrv);

    let breakdown = vec![
        MetricScore {
            name: "Dependency freshness".to_string(),
            score: freshness_score,
            weight: WEIGHT_DEPENDENCY_FRESHNESS,
        },
        MetricScore {
            name: "Security".to_string(),
            score: security_score,
            weight: WEIGHT_SECURITY,
        },
        MetricScore {
            name: "Unused dependencies".to_string(),
            score: unused_score,
            weight: WEIGHT_UNUSED_DEPS,
        },
        MetricScore {
            name: "Unsafe code".to_string(),
            score: unsafe_score,
            weight: WEIGHT_UNSAFE_CODE,
        },
        MetricScore {
            name: "Clippy".to_string(),
            score: clippy_score,
            weight: WEIGHT_CLIPPY,
        },
        MetricScore {
            name: "MSRV".to_string(),
            score: msrv_score,
            weight: WEIGHT_MSRV,
        },
    ];

    let score = breakdown
        .iter()
        .map(|metric| metric.score * metric.weight)
        .sum::<f32>();

    let grade = grade_for(score);
    let recommendations = recommendations_for(&breakdown);

    QualityOutput {
        score,
        grade,
        breakdown,
        recommendations,
    }
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

fn clippy_score(summary: &ClippySummary) -> f32 {
    let penalty = (summary.warnings as u64)
        .saturating_mul(2)
        .saturating_add((summary.errors as u64).saturating_mul(10));
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

fn recommendations_for(metrics: &[MetricScore]) -> Vec<String> {
    let mut items = Vec::new();

    for metric in metrics {
        let message = match metric.name.as_str() {
            "Dependency freshness" if metric.score < 90.0 => {
                Some("Update outdated dependencies.".to_string())
            }
            "Security" if metric.score < 90.0 => Some("Address security advisories.".to_string()),
            "Unused dependencies" if metric.score < 100.0 => {
                Some("Remove unused dependencies.".to_string())
            }
            "Unsafe code" if metric.score < 100.0 => Some("Reduce unsafe code usage.".to_string()),
            "Clippy" if metric.score < 90.0 => Some("Fix clippy warnings and errors.".to_string()),
            "MSRV" if metric.score < 100.0 => {
                Some("Declare a valid MSRV in Cargo.toml.".to_string())
            }
            _ => None,
        };

        if let Some(message) = message {
            let impact = (100.0 - metric.score) * metric.weight;
            items.push((impact, message));
        }
    }

    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    items.into_iter().map(|(_, message)| message).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
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
            MetricScore {
                name: "Dependency freshness".to_string(),
                score: 60.0,
                weight: WEIGHT_DEPENDENCY_FRESHNESS,
            },
            MetricScore {
                name: "Security".to_string(),
                score: 70.0,
                weight: WEIGHT_SECURITY,
            },
            MetricScore {
                name: "Clippy".to_string(),
                score: 80.0,
                weight: WEIGHT_CLIPPY,
            },
            MetricScore {
                name: "Unused dependencies".to_string(),
                score: 90.0,
                weight: WEIGHT_UNUSED_DEPS,
            },
            MetricScore {
                name: "Unsafe code".to_string(),
                score: 95.0,
                weight: WEIGHT_UNSAFE_CODE,
            },
            MetricScore {
                name: "MSRV".to_string(),
                score: 50.0,
                weight: WEIGHT_MSRV,
            },
        ];

        let recommendations = recommendations_for(&metrics);
        assert_eq!(recommendations[0], "Update outdated dependencies.");
        assert_eq!(recommendations[1], "Address security advisories.");
        assert_eq!(recommendations[2], "Declare a valid MSRV in Cargo.toml.");
        assert_eq!(recommendations[3], "Fix clippy warnings and errors.");
        assert_eq!(recommendations[4], "Remove unused dependencies.");
        assert_eq!(recommendations[5], "Reduce unsafe code usage.");
    }
}
