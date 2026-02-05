use crate::core::error::Result;

use crate::core::analyzers::unsafe_code::run_unsafe;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    let output = run_unsafe().await?;
    if json {
        print_json(&output)
    } else {
        println!("{output}");
        Ok(())
    }
}
