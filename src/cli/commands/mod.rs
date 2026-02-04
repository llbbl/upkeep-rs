//! Command dispatch and handlers.

use anyhow::Result;

use crate::cli::UpkeepCommand;

pub async fn handle(command: UpkeepCommand) -> Result<()> {
    match command {
        UpkeepCommand::Detect => {
            tracing::info!("detect not implemented");
        }
        UpkeepCommand::Audit => {
            tracing::info!("audit not implemented");
        }
        UpkeepCommand::Deps => {
            tracing::info!("deps not implemented");
        }
        UpkeepCommand::Quality => {
            tracing::info!("quality not implemented");
        }
        UpkeepCommand::Unused => {
            tracing::info!("unused not implemented");
        }
        UpkeepCommand::Unsafe => {
            tracing::info!("unsafe not implemented");
        }
        UpkeepCommand::Tree => {
            tracing::info!("tree not implemented");
        }
    }

    Ok(())
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
            UpkeepCommand::Unsafe,
            UpkeepCommand::Tree,
        ];

        for command in commands {
            handle(command).await.unwrap();
        }
    }
}
