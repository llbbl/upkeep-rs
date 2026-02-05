use crate::core::analyzers::audit::run_audit;
use crate::core::error::{ErrorCode, Result, UpkeepError};
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    let output = tokio::task::spawn_blocking(run_audit).await.map_err(|e| {
        let message = if e.is_panic() {
            format!("audit task panicked: {e}")
        } else {
            format!("audit task was cancelled: {e}")
        };
        UpkeepError::message(ErrorCode::TaskFailed, message)
    })??;

    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}
