//! cargo-upkeep entrypoint.

mod cli;
mod core;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    cli::init_logging(cli.verbose, cli.log_level.as_deref())?;

    let command = match cli.command {
        cli::Command::Upkeep(command) => command,
        cli::Command::Detect => cli::UpkeepCommand::Detect,
        cli::Command::Audit => cli::UpkeepCommand::Audit,
        cli::Command::Deps { security } => cli::UpkeepCommand::Deps { security },
        cli::Command::Quality => cli::UpkeepCommand::Quality,
        cli::Command::Unused => cli::UpkeepCommand::Unused,
        cli::Command::UnsafeCode => cli::UpkeepCommand::UnsafeCode,
        cli::Command::Tree => cli::UpkeepCommand::Tree,
    };

    cli::commands::handle(command, cli.json).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_module_smoke() {
        assert!(true);
    }
}
