//! Command dispatch and handlers.

mod audit;
mod deps;
mod detect;
mod unused;
mod tree;

use anyhow::Result;

use crate::cli::UpkeepCommand;

pub async fn handle(command: UpkeepCommand, json: bool) -> Result<()> {
    match command {
        UpkeepCommand::Detect => detect::run(json).await,
        UpkeepCommand::Audit => audit::run(json).await,
        UpkeepCommand::Deps { security } => deps::run(json, security).await,
        UpkeepCommand::Quality => {
            tracing::info!("quality not implemented");
            Ok(())
        }
        UpkeepCommand::Unused => {
            unused::run(json).await
        }
        UpkeepCommand::UnsafeCode => {
            tracing::info!("unsafe-code not implemented");
            Ok(())
        }
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
        let commands = [
            UpkeepCommand::Detect,
            UpkeepCommand::Quality,
            UpkeepCommand::UnsafeCode,
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
