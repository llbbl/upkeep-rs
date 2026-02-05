//! Command dispatch and handlers.

mod audit;
mod deps;
mod detect;
mod quality;
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
                UpkeepError::message(ErrorCode::TaskFailed, format!("detect task failed: {err}"))
            })?,
        UpkeepCommand::Audit => audit::run(json).await,
        UpkeepCommand::Deps { security } => deps::run(json, security).await,
        UpkeepCommand::Quality => quality::run(json).await,
        UpkeepCommand::Unused => unused::run(json).await,
        UpkeepCommand::UnsafeCode => unsafe_code::run(json).await,
        UpkeepCommand::Tree(args) => tree::run(json, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::cli::UpkeepCommand;

    #[tokio::test]
    async fn handlers_return_ok() {
        // Only test commands that don't require network access.
        // Audit requires fetching rustsec advisory database.
        // Deps requires fetching crate info from crates.io.
        // Unused requires cargo-machete to be installed.
        // Unsafe code requires cargo-geiger to be installed.
        let commands = [
            UpkeepCommand::Detect,
            UpkeepCommand::Quality,
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
}
