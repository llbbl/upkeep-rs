use crate::core::error::Result;
use crate::core::output::UnsafeOutput;
use std::future::Future;

use crate::core::analyzers::unsafe_code::run_unsafe;
use crate::core::output::print_json;

pub async fn run(json: bool) -> Result<()> {
    run_with(
        json,
        run_unsafe,
        print_json,
        |output| {
            println!("{output}");
            Ok(())
        },
    )
    .await
}

async fn run_with<F, Fut, J, P>(
    json: bool,
    runner: F,
    json_printer: J,
    text_printer: P,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<UnsafeOutput>>,
    J: FnOnce(&UnsafeOutput) -> Result<()>,
    P: FnOnce(&UnsafeOutput) -> Result<()>,
{
    let output = runner().await?;
    if json {
        json_printer(&output)
    } else {
        text_printer(&output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::{ErrorCode, UpkeepError};
    use crate::core::output::{UnsafePackage, UnsafeSummary};
    use std::cell::Cell;

    fn sample_output() -> UnsafeOutput {
        UnsafeOutput {
            summary: UnsafeSummary {
                packages: 1,
                unsafe_functions: 1,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 1,
                unsafe_expressions: 0,
                total_unsafe: 2,
            },
            packages: vec![UnsafePackage {
                name: "unsafe-crate".to_string(),
                version: "0.1.0".to_string(),
                package_id: None,
                unsafe_functions: 1,
                unsafe_impls: 0,
                unsafe_traits: 0,
                unsafe_blocks: 1,
                unsafe_expressions: 0,
                total_unsafe: 2,
            }],
        }
    }

    #[tokio::test]
    async fn run_with_json_uses_json_printer() {
        let called_json = Cell::new(false);
        let called_text = Cell::new(false);

        run_with(
            true,
            || async { Ok(sample_output()) },
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
            || async { Ok(sample_output()) },
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
    async fn run_with_propagates_runner_error() {
        let err = run_with(
            true,
            || async {
                Err(UpkeepError::message(
                    ErrorCode::MissingTool,
                    "cargo-geiger is not installed",
                ))
            },
            |_| Ok(()),
            |_| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::MissingTool);
    }

    #[tokio::test]
    async fn run_with_propagates_json_printer_error() {
        let err = run_with(
            true,
            || async { Ok(sample_output()) },
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
            || async { Ok(sample_output()) },
            |_| Ok(()),
            |_| Err(UpkeepError::message(ErrorCode::Io, "text write failed")),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Io);
        assert!(err.to_string().contains("text write failed"));
    }
}
