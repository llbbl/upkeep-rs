//! CLI parsing and logging setup.

pub mod commands;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "cargo-upkeep",
    version,
    about = "Unified Rust project maintenance CLI",
    arg_required_else_help = true
)]
pub struct Cli {
    #[arg(short, long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub log_level: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Upkeep(UpkeepCommand),
    Detect,
    Audit,
    Deps {
        #[arg(long)]
        security: bool,
    },
    Quality,
    Unused,
    #[command(name = "unsafe-code", alias = "unsafe")]
    UnsafeCode,
    Tree(TreeArgs),
}

#[derive(Debug, Subcommand)]
pub enum UpkeepCommand {
    Detect,
    Audit,
    Deps {
        #[arg(long)]
        security: bool,
    },
    Quality,
    Unused,
    #[command(name = "unsafe-code", alias = "unsafe")]
    UnsafeCode,
    Tree(TreeArgs),
}

#[derive(Debug, Args, Clone)]
pub struct TreeArgs {
    #[arg(long)]
    pub depth: Option<usize>,
    #[arg(long)]
    pub duplicates: bool,
    #[arg(long)]
    pub invert: Option<String>,
    #[arg(long)]
    pub features: bool,
    #[arg(long = "no-dev")]
    pub no_dev: bool,
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
    use super::{Cli, Command, TreeArgs, UpkeepCommand};
    use clap::Parser;

    #[test]
    fn parses_upkeep_subcommand() {
        let cli = Cli::try_parse_from(["cargo-upkeep", "upkeep", "detect"]).unwrap();
        match cli.command {
            Command::Upkeep(UpkeepCommand::Detect) => {}
            _ => panic!("unexpected subcommand"),
        }
    }

    #[test]
    fn parses_direct_subcommand() {
        let cli = Cli::try_parse_from(["cargo-upkeep", "detect"]).unwrap();
        match cli.command {
            Command::Detect => {}
            _ => panic!("unexpected subcommand"),
        }
    }

    #[test]
    fn parses_tree_flags() {
        let cli = Cli::try_parse_from([
            "cargo-upkeep",
            "tree",
            "--depth",
            "2",
            "--duplicates",
            "--invert",
            "serde",
            "--features",
            "--no-dev",
        ])
        .unwrap();

        match cli.command {
            Command::Tree(args) => {
                assert_eq!(args.depth, Some(2));
                assert!(args.duplicates);
                assert_eq!(args.invert.as_deref(), Some("serde"));
                assert!(args.features);
                assert!(args.no_dev);
            }
            _ => panic!("unexpected subcommand"),
        }
    }

    #[test]
    fn parses_tree_upkeep_flags() {
        let cli = Cli::try_parse_from([
            "cargo-upkeep",
            "upkeep",
            "tree",
            "--depth",
            "1",
        ])
        .unwrap();

        match cli.command {
            Command::Upkeep(UpkeepCommand::Tree(TreeArgs { depth, .. })) => {
                assert_eq!(depth, Some(1));
            }
            _ => panic!("unexpected subcommand"),
        }
    }
}
