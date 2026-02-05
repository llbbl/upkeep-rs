use super::run_with::run_with_output;
use crate::core::error::Result;

use crate::core::analyzers::unused::run_unused;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    run_with_output(json, run_unused(), print_json, |output| {
        println!("{output}");
        Ok(())
    })
    .await
}
