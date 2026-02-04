//! Command dispatch and handlers.

mod detect;

use anyhow::Result;

use crate::cli::UpkeepCommand;

pub async fn handle(command: UpkeepCommand, json: bool) -> Result<()> {
    match command {
        UpkeepCommand::Detect => detect::run(json).await,
        UpkeepCommand::Audit => {
            tracing::info!("audit not implemented");
            Ok(())
        }
        UpkeepCommand::Deps => {
            tracing::info!("deps not implemented");
            Ok(())
        }
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
        let commands = [
            UpkeepCommand::Detect,
            UpkeepCommand::Audit,
            UpkeepCommand::Deps,
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
