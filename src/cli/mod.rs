//! CLI parsing and logging setup.

pub mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "cargo-upkeep",
    version,
    about = "Unified Rust project maintenance CLI",
    arg_required_else_help = true
)]
pub struct CargoCli {
    #[command(subcommand)]
    pub command: CargoCommand,
}

#[derive(Debug, Subcommand)]
pub enum CargoCommand {
    Upkeep(UpkeepCli),
}

#[derive(Debug, Parser)]
#[command(
    about = "Unified Rust project maintenance CLI",
    version,
    arg_required_else_help = true
)]
pub struct UpkeepCli {
    #[arg(short, long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub log_level: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: UpkeepCommand,
}

#[derive(Debug, Subcommand)]
pub enum UpkeepCommand {
    Detect,
    Audit,
    Deps,
    Quality,
    Unused,
    Unsafe,
    Tree,
}

pub fn init_logging(verbose: bool, log_level: Option<&str>) -> Result<()> {
    let filter = match log_level {
        Some(level) => EnvFilter::try_new(level)?,
        None => {
            if verbose {
                EnvFilter::new("info")
            } else {
                EnvFilter::new("warn")
            }
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|err| anyhow::anyhow!(err))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CargoCli, CargoCommand, UpkeepCommand};
    use clap::Parser;

    #[test]
    fn parses_upkeep_subcommand() {
        let cli = CargoCli::try_parse_from(["cargo-upkeep", "upkeep", "detect"]).unwrap();
        match cli.command {
            CargoCommand::Upkeep(upkeep) => match upkeep.command {
                UpkeepCommand::Detect => {}
                _ => panic!("unexpected subcommand"),
            },
        }
    }
}
