use super::run_with::run_with_output;
use crate::core::error::Result;

use crate::core::analyzers::unsafe_code::run_unsafe;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    run_with_output(json, run_unsafe(), print_json, |output| {
        println!("{output}");
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::run_with_output;
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::{UnsafeOutput, UnsafePackage, UnsafeSummary};
    use serde_json::Value;

    #[tokio::test]
    async fn run_with_output_json_shape() {
        let output = UnsafeOutput {
            summary: UnsafeSummary {
                packages: 1,
                unsafe_functions: 1,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 2,
                unsafe_expressions: 0,
                total_unsafe: 3,
            },
            packages: vec![UnsafePackage {
                name: "ffi".to_string(),
                version: "0.1.0".to_string(),
                package_id: None,
                unsafe_functions: 1,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 2,
                unsafe_expressions: 0,
                total_unsafe: 3,
            }],
        };

        run_with_output(
            true,
            async { Ok(output) },
            |output| {
                let value = serde_json::to_value(output)?;
                assert_eq!(value["summary"]["total_unsafe"], Value::Number(3.into()));
                assert_eq!(value["packages"][0]["name"], Value::String("ffi".into()));
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
            |_: &UnsafeOutput| Ok(()),
            |_: &UnsafeOutput| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::TaskFailed);
    }
}
