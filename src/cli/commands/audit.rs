use anyhow::Result;

use crate::core::analyzers::audit::run_audit;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    let output = tokio::task::spawn_blocking(run_audit).await.map_err(|e| {
        if e.is_panic() {
            anyhow::anyhow!("audit task panicked: {e}")
        } else {
            anyhow::anyhow!("audit task was cancelled: {e}")
        }
    })??;

    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}
