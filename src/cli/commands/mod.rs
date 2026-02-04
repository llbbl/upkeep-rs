//! Command dispatch and handlers.

mod audit;
mod deps;
mod detect;

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
            tracing::info!("unused not implemented");
            Ok(())
        }
        UpkeepCommand::UnsafeCode => {
            tracing::info!("unsafe-code not implemented");
            Ok(())
        }
        UpkeepCommand::Tree => {
            tracing::info!("tree not implemented");
            Ok(())
        }
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
        let commands = [
            UpkeepCommand::Detect,
            UpkeepCommand::Quality,
            UpkeepCommand::Unused,
            UpkeepCommand::UnsafeCode,
            UpkeepCommand::Tree,
        ];

        for command in commands {
            handle(command, false).await.unwrap();
        }
    }
}
