use super::run_with::run_with_output;
use crate::core::analyzers::audit::run_audit;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    run_with_output(
        json,
        async {
            tokio::task::spawn_blocking(run_audit)
                .await
                .map_err(map_audit_join_error)?
        },
        print_json,
        |output| {
            println!("{output}");
            Ok(())
        },
    )
    .await
}

fn map_audit_join_error(err: tokio::task::JoinError) -> UpkeepError {
    let message = if err.is_panic() {
        format!("audit task panicked: {err}")
    } else {
        format!("audit task was cancelled: {err}")
    };
    UpkeepError::message(ErrorCode::TaskFailed, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::output::{AuditOutput, AuditSummary, Severity, Vulnerability};
    use serde_json::Value;

    #[tokio::test]
    async fn map_audit_join_error_handles_panic() {
        let handle = tokio::spawn(async { panic!("boom") });
        let join_error = handle.await.unwrap_err();
        let mapped = map_audit_join_error(join_error);
        assert_eq!(mapped.code(), ErrorCode::TaskFailed);
        assert!(mapped.to_string().contains("panicked"));
    }

    #[tokio::test]
    async fn map_audit_join_error_handles_cancelled() {
        use tokio::time::{sleep, Duration};

        let handle = tokio::spawn(async {
            sleep(Duration::from_secs(30)).await;
        });
        handle.abort();
        let join_error = handle.await.unwrap_err();
        let mapped = map_audit_join_error(join_error);
        assert_eq!(mapped.code(), ErrorCode::TaskFailed);
        assert!(mapped.to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn run_with_output_json_shape() {
        let output = AuditOutput {
            vulnerabilities: vec![Vulnerability {
                id: "RUSTSEC-0000-0000".to_string(),
                package: "serde".to_string(),
                package_version: "1.0.0".to_string(),
                severity: Severity::High,
                title: "Example".to_string(),
                path: vec!["root".to_string()],
                fix_available: true,
            }],
            summary: AuditSummary {
                critical: 0,
                high: 1,
                moderate: 0,
                low: 0,
                total: 1,
            },
        };

        run_with_output(
            true,
            async { Ok(output) },
            |output| {
                let value = serde_json::to_value(output)?;
                assert_eq!(value["summary"]["total"], Value::Number(1.into()));
                assert_eq!(
                    value["vulnerabilities"][0]["severity"],
                    Value::String("high".into())
                );
                Ok(())
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn run_with_output_propagates_error() {
        let err = run_with_output(
            true,
            async { Err(UpkeepError::message(ErrorCode::TaskFailed, "boom")) },
            |_: &AuditOutput| Ok(()),
            |_: &AuditOutput| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::TaskFailed);
    }
}
