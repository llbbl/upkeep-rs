#![allow(dead_code)]

use crate::core::output::{Grade, MetricScore, QualityOutput};

pub const WEIGHT_DEPENDENCY_FRESHNESS: f32 = 0.20;
pub const WEIGHT_SECURITY: f32 = 0.25;
pub const WEIGHT_UNUSED_DEPS: f32 = 0.15;
pub const WEIGHT_UNSAFE_CODE: f32 = 0.15;
pub const WEIGHT_CLIPPY: f32 = 0.15;
pub const WEIGHT_MSRV: f32 = 0.10;

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
    Invalid,
}

pub struct QualityInputs {
    pub dependency_freshness: DependencyFreshness,
    pub security: SecuritySummary,
    pub clippy: Option<ClippySummary>,
    pub msrv: MsrvStatus,
}

pub fn score_quality(inputs: &QualityInputs) -> QualityOutput {
    let freshness_score = dependency_freshness_score(&inputs.dependency_freshness);
    let security_score = security_score(&inputs.security);
    let unused_score = 100.0;
    let unsafe_score = 100.0;
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
        .saturating_mul(25)
        .saturating_add((summary.high as u64).saturating_mul(15))
        .saturating_add((summary.moderate as u64).saturating_mul(5))
        .saturating_add((summary.low as u64).saturating_mul(2));
    100u64.saturating_sub(penalty) as f32
}

fn clippy_score(summary: &ClippySummary) -> f32 {
    let penalty = (summary.warnings as u64)
        .saturating_mul(2)
        .saturating_add((summary.errors as u64).saturating_mul(10));
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
