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
}
