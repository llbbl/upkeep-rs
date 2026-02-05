use crate::core::error::Result;
use std::future::Future;

pub(super) async fn run_with_output<Output, Fut, J, P>(
    json: bool,
    runner: Fut,
    json_printer: J,
    text_printer: P,
) -> Result<()>
where
    Fut: Future<Output = Result<Output>>,
    J: FnOnce(&Output) -> Result<()>,
    P: FnOnce(&Output) -> Result<()>,
{
    let output = runner.await?;
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
    use std::cell::Cell;

    #[derive(Debug)]
    #[allow(dead_code)] // Field used for test data construction
    struct TestOutput {
        value: i32,
    }

    fn sample_output() -> TestOutput {
        TestOutput { value: 42 }
    }

    #[tokio::test]
    async fn run_with_json_uses_json_printer() {
        let called_json = Cell::new(false);
        let called_text = Cell::new(false);

        run_with_output(
            true,
            async { Ok(sample_output()) },
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

        run_with_output(
            false,
            async { Ok(sample_output()) },
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
        let err = run_with_output(
            true,
            async {
                Err(UpkeepError::message(
                    ErrorCode::InvalidData,
                    "runner failed",
                ))
            },
            |_: &TestOutput| Ok(()),
            |_: &TestOutput| Ok(()),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::InvalidData);
    }

    #[tokio::test]
    async fn run_with_propagates_json_printer_error() {
        let err = run_with_output(
            true,
            async { Ok(sample_output()) },
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
        let err = run_with_output(
            false,
            async { Ok(sample_output()) },
            |_| Ok(()),
            |_| Err(UpkeepError::message(ErrorCode::Io, "text write failed")),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Io);
        assert!(err.to_string().contains("text write failed"));
    }
}
