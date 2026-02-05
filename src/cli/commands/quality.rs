use cargo_metadata::MetadataCommand;

use crate::cli::commands::deps;
use crate::core::analyzers::{
    audit::run_audit, clippy::run_clippy, unsafe_code::run_unsafe, unused::run_unused,
};
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::{
    print_json, AuditOutput, ClippyOutput, DepsOutput, QualityOutput, UnsafeOutput, UnusedOutput,
};
use crate::core::scorers::quality::{
    score_quality, ClippySummary, DependencyFreshness, MsrvStatus, QualityInputs, SecuritySummary,
    UnsafeSummary, UnusedSummary,
};

pub async fn run(json: bool) -> Result<()> {
    let deps_future = deps::analyze(false);
    let audit_future = run_blocking("audit", run_audit);
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

    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}

fn build_quality_output(
    deps_result: Result<DepsOutput>,
    audit_result: Result<AuditOutput>,
    clippy_result: Result<ClippyOutput>,
    msrv_result: Result<MsrvStatus>,
    unused_result: Result<UnusedOutput>,
    unsafe_result: Result<UnsafeOutput>,
) -> QualityOutput {
    let mut extra_recommendations = Vec::new();

    let dependency_freshness = match deps_result {
        Ok(deps_output) => DependencyFreshness {
            total: deps_output.total,
            outdated: deps_output.outdated,
        },
        Err(err) => {
            extra_recommendations.push(format!("Dependency freshness unavailable: {err}"));
            DependencyFreshness {
                total: 0,
                outdated: 0,
            }
        }
    };

    let security = match audit_result {
        Ok(output) => SecuritySummary {
            critical: output.summary.critical,
            high: output.summary.high,
            moderate: output.summary.moderate,
            low: output.summary.low,
        },
        Err(err) => {
            extra_recommendations.push(format!("Security scan unavailable: {err}"));
            SecuritySummary {
                critical: 0,
                high: 0,
                moderate: 0,
                low: 0,
            }
        }
    };

    let clippy = match clippy_result {
        Ok(output) => Some(ClippySummary {
            warnings: output.warnings,
            errors: output.errors,
        }),
        Err(err) => {
            extra_recommendations.push(format!("Clippy unavailable: {err}"));
            None
        }
    };

    let unused = match unused_result {
        Ok(output) => Some(UnusedSummary {
            unused_count: output.unused.len(),
        }),
        Err(err) => {
            extra_recommendations.push(format!("Unused dependencies unavailable: {err}"));
            None
        }
    };

    let unsafe_code = match unsafe_result {
        Ok(output) => Some(UnsafeSummary {
            total_unsafe: output.summary.total_unsafe,
        }),
        Err(err) => {
            extra_recommendations.push(format!("Unsafe code scan unavailable: {err}"));
            None
        }
    };

    let msrv = match msrv_result {
        Ok(status) => status,
        Err(err) => {
            extra_recommendations.push(format!("MSRV check unavailable: {err}"));
            MsrvStatus::Missing
        }
    };

    let mut output = score_quality(&QualityInputs {
        dependency_freshness,
        security,
        unused,
        unsafe_code,
        clippy,
        msrv,
    });

    output.recommendations.extend(extra_recommendations);

    output
}

async fn check_msrv() -> Result<MsrvStatus> {
    let metadata = run_blocking("cargo metadata", || {
        MetadataCommand::new().exec().map_err(|err| {
            UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)
        })
    })
    .await?;
    let root = metadata.root_package().ok_or_else(|| {
        UpkeepError::message(
            ErrorCode::InvalidData,
            "no root package found (virtual workspaces are not supported yet)",
        )
    })?;

    match root.rust_version.as_ref() {
        Some(_) => Ok(MsrvStatus::Valid),
        None => Ok(MsrvStatus::Missing),
    }
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
    use super::{build_quality_output, check_msrv, run_blocking, MsrvStatus};
    use crate::core::error::{ErrorCode, UpkeepError};

    fn err() -> UpkeepError {
        UpkeepError::message(ErrorCode::TaskFailed, "boom")
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
    fn build_quality_output_adds_recommendations_for_failures() {
        let output = build_quality_output(
            Err(err()),
            Err(err()),
            Err(err()),
            Ok(MsrvStatus::Valid),
            Err(err()),
            Err(err()),
        );

        assert_eq!(
            output.recommendations,
            vec![
                "Dependency freshness unavailable: boom".to_string(),
                "Security scan unavailable: boom".to_string(),
                "Clippy unavailable: boom".to_string(),
                "Unused dependencies unavailable: boom".to_string(),
                "Unsafe code scan unavailable: boom".to_string(),
            ]
        );
    }
}
