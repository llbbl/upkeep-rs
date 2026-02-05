use cargo_metadata::MetadataCommand;

use crate::cli::commands::deps;
use crate::core::analyzers::{
    audit::run_audit, clippy::run_clippy, unsafe_code::run_unsafe, unused::run_unused,
};
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::print_json;
use crate::core::scorers::quality::{
    score_quality, ClippySummary, DependencyFreshness, MsrvStatus, QualityInputs, SecuritySummary,
};

pub async fn run(json: bool) -> Result<()> {
    let deps_future = deps::analyze(false);
    let audit_future = run_blocking("audit", run_audit);
    let clippy_future = run_blocking("clippy", run_clippy);
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

    if let Err(err) = unused_result {
        extra_recommendations.push(format!("Unused dependencies unavailable: {err}"));
    }

    if let Err(err) = unsafe_result {
        extra_recommendations.push(format!("Unsafe code scan unavailable: {err}"));
    }

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
        clippy,
        msrv,
    });

    output.recommendations.extend(extra_recommendations);

    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
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
