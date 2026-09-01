//! Command dispatch and handlers.

mod audit;
mod deps;
mod detect;
mod quality;
mod run_with;
mod tree;
mod unsafe_code;
mod unused;

use crate::core::error::{ErrorCode, Result, UpkeepError};

use crate::cli::UpkeepCommand;

pub async fn handle(command: UpkeepCommand, json: bool) -> Result<()> {
    match command {
        UpkeepCommand::Detect => tokio::task::spawn_blocking(move || detect::run(json))
            .await
            .map_err(|err| {
                // JoinError occurs when:
                // 1. The task panicked (is_panic() returns true)
                // 2. The task was cancelled (is_cancelled() returns true)
                // Note: Testing JoinError paths is complex as it requires injecting panics
                // into spawn_blocking tasks, which is not straightforward to do reliably.
                let reason = if err.is_panic() {
                    "task panicked"
                } else if err.is_cancelled() {
                    "task was cancelled"
                } else {
                    "task failed"
                };
                UpkeepError::message(ErrorCode::TaskFailed, format!("detect {reason}: {err}"))
            })?,
        UpkeepCommand::Audit => audit::run(json).await,
        UpkeepCommand::Deps { security } => deps::run(json, security).await,
        UpkeepCommand::Quality { require_complete } => quality::run(json, require_complete).await,
        UpkeepCommand::Unused => unused::run(json).await,
        UpkeepCommand::UnsafeCode => unsafe_code::run(json).await,
        UpkeepCommand::Tree(args) => tree::run(json, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::cli::UpkeepCommand;

    /// Dispatch reaches each handler, for the handlers that can run offline.
    ///
    /// Excluded, and why:
    /// - `Audit` fetches the RustSec advisory database.
    /// - `Quality` runs the audit and the crates.io lookups, so it does both.
    ///   It used to be listed here despite that, which meant this test wrote to
    ///   the shared `~/.cargo/advisory-db` on every `cargo test`. It is covered
    ///   end-to-end by `cli_quality_command_runs`, which goes through this same
    ///   dispatch with the advisory database pinned to a local fixture.
    /// - `Deps` fetches crate info from crates.io.
    /// - `Unused` and `UnsafeCode` need cargo-machete and cargo-geiger installed.
    #[tokio::test]
    async fn handlers_return_ok() {
        let commands = [
            UpkeepCommand::Detect,
            UpkeepCommand::Tree(crate::cli::TreeArgs {
                depth: Some(0),
                duplicates: false,
                invert: None,
                features: false,
                no_dev: false,
            }),
        ];

        for command in commands {
            handle(command, false).await.unwrap();
        }
    }

    #[tokio::test]
    async fn detect_handler_supports_json_output() {
        handle(UpkeepCommand::Detect, true).await.unwrap();
    }
}
