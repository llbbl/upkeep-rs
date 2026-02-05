//! cargo-upkeep entrypoint.

mod cli;
mod core;

use clap::Parser;
use std::process::ExitCode;

use crate::core::error::eprint_error_json;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    if let Err(err) = cli::init_logging(cli.verbose, cli.log_level.as_deref()) {
        return exit_with_error(&err, cli.json);
    }

    let command = match cli.command {
        cli::Command::Upkeep(command) => command,
        cli::Command::Detect => cli::UpkeepCommand::Detect,
        cli::Command::Audit => cli::UpkeepCommand::Audit,
        cli::Command::Deps { security } => cli::UpkeepCommand::Deps { security },
        cli::Command::Quality => cli::UpkeepCommand::Quality,
        cli::Command::Unused => cli::UpkeepCommand::Unused,
        cli::Command::UnsafeCode => cli::UpkeepCommand::UnsafeCode,
        cli::Command::Tree(args) => cli::UpkeepCommand::Tree(args),
    };

    match cli::commands::handle(command, cli.json).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => exit_with_error(&err, cli.json),
    }
}

fn exit_with_error(error: &core::error::UpkeepError, json: bool) -> ExitCode {
    if json {
        eprint_error_json(error);
    } else {
        eprintln!("{error}");
    }
    ExitCode::from(1)
}
