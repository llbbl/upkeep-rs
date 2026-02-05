use crate::core::analyzers::audit::run_audit;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::AuditOutput;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    run_with(
        json,
        run_audit,
        print_json,
        |output| {
            println!("{output}");
            Ok(())
        },
    )
    .await
}

async fn run_with<F, J, P>(json: bool, audit_fn: F, json_printer: J, text_printer: P) -> Result<()>
where
    F: FnOnce() -> Result<AuditOutput> + Send + 'static,
    J: FnOnce(&AuditOutput) -> Result<()>,
    P: FnOnce(&AuditOutput) -> Result<()>,
{
    let output = tokio::task::spawn_blocking(audit_fn)
        .await
        .map_err(map_audit_join_error)??;

    if json {
        json_printer(&output)
    } else {
        text_printer(&output)
    }
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
    use crate::core::output::{AuditSummary, Severity, Vulnerability};
    use std::cell::Cell;

    fn sample_output() -> AuditOutput {
        AuditOutput {
            vulnerabilities: vec![Vulnerability {
                package: "serde".to_string(),
                package_version: "1.0.0".to_string(),
                advisory_id: "RUSTSEC-0000-0000".to_string(),
                severity: Severity::High,
                title: "Example".to_string(),
                path: vec!["root".to_string(), "serde".to_string()],
                fix_available: true,
            }],
            summary: AuditSummary {
                critical: 0,
                high: 1,
                moderate: 0,
                low: 0,
                total: 1,
            },
        }
    }

    #[tokio::test]
    async fn run_with_json_uses_json_printer() {
        let called_json = Cell::new(false);
        let called_text = Cell::new(false);

        run_with(
            true,
            || Ok(sample_output()),
            |_| {
                called_json.set(true);
                Ok(())
            },
            |_| {
                called_text.set(true);
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(called_json.get());
        assert!(!called_text.get());
    }

    #[tokio::test]
    async fn run_with_text_uses_text_printer() {
        let called_json = Cell::new(false);
        let called_text = Cell::new(false);

        run_with(
            false,
            || Ok(sample_output()),
            |_| {
                called_json.set(true);
                Ok(())
            },
            |_| {
                called_text.set(true);
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(!called_json.get());
        assert!(called_text.get());
    }

    #[tokio::test]
    async fn run_with_propagates_audit_error() {
        let err = run_with(
            true,
            || Err(UpkeepError::message(ErrorCode::InvalidData, "no lockfile")),
            |_| Ok(()),
            |_| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::InvalidData);
    }

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
    async fn run_with_propagates_json_printer_error() {
        let err = run_with(
            true,
            || Ok(sample_output()),
            |_| Err(UpkeepError::message(ErrorCode::Io, "json write failed")),
            |_| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Io);
        assert!(err.to_string().contains("json write failed"));
    }

    #[tokio::test]
    async fn run_with_propagates_text_printer_error() {
        let err = run_with(
            false,
            || Ok(sample_output()),
            |_| Ok(()),
            |_| Err(UpkeepError::message(ErrorCode::Io, "text write failed")),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Io);
        assert!(err.to_string().contains("text write failed"));
    }
}
