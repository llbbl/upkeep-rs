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

#[cfg(test)]
mod tests {
    use super::run_with_output;
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::{Confidence, DependencyType, UnusedDep, UnusedOutput};
    use serde_json::Value;

    #[tokio::test]
    async fn run_with_output_json_shape() {
        let output = UnusedOutput {
            unused: vec![UnusedDep {
                name: "tokio".to_string(),
                dependency_type: DependencyType::Dev,
                confidence: Confidence::High,
            }],
            possibly_unused: vec!["serde".to_string()],
        };

        run_with_output(
            true,
            async { Ok(output) },
            |output| {
                let value = serde_json::to_value(output)?;
                assert_eq!(value["unused"][0]["name"], Value::String("tokio".into()));
                assert_eq!(
                    value["unused"][0]["confidence"],
                    Value::String("high".into())
                );
                assert_eq!(value["possibly_unused"][0], Value::String("serde".into()));
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
            async { Err(UpkeepError::message(ErrorCode::InvalidData, "bad")) },
            |_: &UnusedOutput| Ok(()),
            |_: &UnusedOutput| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::InvalidData);
    }
}
