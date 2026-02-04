//! cargo-upkeep entrypoint.

mod cli;
mod core;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::CargoCli::parse();
    match cli.command {
        cli::CargoCommand::Upkeep(upkeep) => {
            cli::init_logging(upkeep.verbose, upkeep.log_level.as_deref())?;
            cli::commands::handle(upkeep.command).await
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_module_smoke() {
        assert!(true);
    }
}
