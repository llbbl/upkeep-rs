use anyhow::Result;

use crate::core::analyzers::unused::run_unused;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    let output = run_unused().await?;
    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}
