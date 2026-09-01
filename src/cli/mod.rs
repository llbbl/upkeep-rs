//! CLI parsing and logging setup.

pub mod commands;

use crate::core::error::{ErrorCode, Result, UpkeepError};
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
    #[command(subcommand, about = "Run upkeep subcommands")]
    Upkeep(UpkeepCommand),
    #[command(about = "Detect workspace, tooling, and CI")]
    Detect,
    #[command(about = "Report RustSec vulnerabilities")]
    Audit,
    Deps {
        #[arg(
            long,
            help = "Include RustSec advisories for direct workspace deps (requires Cargo.lock)"
        )]
        security: bool,
    },
    #[command(about = "Compute project quality score")]
    Quality {
        #[arg(
            long,
            help = "Exit nonzero when any metric could not be measured (a run that measured nothing always exits nonzero)"
        )]
        require_complete: bool,
    },
    #[command(about = "Find unused dependencies")]
    Unused,
    #[command(
        name = "unsafe-code",
        alias = "unsafe",
        about = "Report unsafe code usage"
    )]
    UnsafeCode,
    #[command(about = "Render dependency tree with filters")]
    Tree(TreeArgs),
    #[command(about = "Report outdated and vulnerable Python dependencies")]
    Python(PythonArgs),
}

#[derive(Debug, Subcommand)]
pub enum UpkeepCommand {
    #[command(about = "Detect workspace, tooling, and CI")]
    Detect,
    #[command(about = "Report RustSec vulnerabilities")]
    Audit,
    Deps {
        #[arg(
            long,
            help = "Include RustSec advisories for direct workspace deps (requires Cargo.lock)"
        )]
        security: bool,
    },
    #[command(about = "Compute project quality score")]
    Quality {
        #[arg(
            long,
            help = "Exit nonzero when any metric could not be measured (a run that measured nothing always exits nonzero)"
        )]
        require_complete: bool,
    },
    #[command(about = "Find unused dependencies")]
    Unused,
    #[command(
        name = "unsafe-code",
        alias = "unsafe",
        about = "Report unsafe code usage"
    )]
    UnsafeCode,
    #[command(about = "Render dependency tree with filters")]
    Tree(TreeArgs),
    #[command(about = "Report outdated and vulnerable Python dependencies")]
    Python(PythonArgs),
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    #[arg(long, help = "Limit recursion depth")]
    pub depth: Option<usize>,
    #[arg(long, help = "Only show duplicate crates")]
    pub duplicates: bool,
    #[arg(long, help = "Invert tree to show reverse dependencies")]
    pub invert: Option<String>,
    #[arg(long, help = "Include enabled features")]
    pub features: bool,
    #[arg(long = "no-dev", help = "Exclude dev-dependencies")]
    pub no_dev: bool,
}

// The two opt-in policy gates on `python`.
//
// One struct shared by both `Command::Python` and `UpkeepCommand::Python`,
// deliberately. Those enums declare every other subcommand's flags twice, and a
// flag added to one of the two copies exists under only one invocation form
// while compiling and unit-testing clean (#34). Sharing the struct makes that
// particular divergence unrepresentable; the variant still has to be added to
// both enums and to `main`'s mapping, which
// `cli_python_flags_are_plumbed_through_both_invocation_forms` covers.
//
// Written as `//` rather than `///` on purpose, here and on the fields below:
// clap's derive turns a doc comment into user-facing help text, so a rationale
// aimed at the next maintainer ends up printed by `--help`.
#[derive(Debug, Args)]
pub struct PythonArgs {
    // `None` is the flag not passed; `Some(vec![])` is the bare form, meaning
    // every capability. The bare form is the interactive one: adding a
    // capability is an additive change that does not bump `schema_version`, so
    // a bare `--require-complete` starts failing on unchanged code the first
    // time a release adds a capability whose tool the runner lacks. Naming the
    // capabilities pins the gate to what the pipeline asked for.
    #[arg(
        long,
        value_name = "CAPABILITIES",
        value_enum,
        value_delimiter = ',',
        num_args = 0..,
        require_equals = true,
        help = "Exit nonzero when a capability was not measured; optionally name which (--require-complete=outdated,security)"
    )]
    pub require_complete: Option<Vec<CapabilityArg>>,

    #[arg(
        long,
        value_name = "THRESHOLD",
        value_enum,
        help = "Exit nonzero when any finding is at or above this severity (an unknown severity satisfies every threshold)"
    )]
    pub fail_on_vulnerability: Option<ThresholdArg>,
}

// A capability nameable on `--require-complete`.
//
// Mirrors `PythonCapability` rather than reusing it: that type is the serialized
// schema, and deriving `ValueEnum` on it would make a future rename of a CLI
// value look like a free change when it is a schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum CapabilityArg {
    Outdated,
    Security,
}

// A `--fail-on-vulnerability` threshold.
//
// `low` and `any` accept the same set today, because every graded severity is at
// or above `low` and `unknown` satisfies everything. Both names are kept because
// they say different things about intent, and a future severity below `low`
// would separate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum ThresholdArg {
    Critical,
    High,
    Moderate,
    Low,
    Any,
}

pub fn init_logging(verbose: bool, log_level: Option<&str>) -> Result<()> {
    let filter = match log_level {
        Some(level) => EnvFilter::try_new(level).map_err(|err| {
            UpkeepError::context(
                ErrorCode::Config,
                format!("invalid log level filter: {level}"),
                err,
            )
        })?,
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
        .map_err(|err| {
            UpkeepError::message(
                ErrorCode::Config,
                format!("failed to initialize logging: {err}"),
            )
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CapabilityArg, Cli, Command, ThresholdArg, TreeArgs, UpkeepCommand};
    use crate::core::error::ErrorCode;
    use clap::{error::ErrorKind, Parser};

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
        let cli = Cli::try_parse_from(["cargo-upkeep", "upkeep", "tree", "--depth", "1"]).unwrap();

        match cli.command {
            Command::Upkeep(UpkeepCommand::Tree(TreeArgs { depth, .. })) => {
                assert_eq!(depth, Some(1));
            }
            _ => panic!("unexpected subcommand"),
        }
    }

    #[test]
    fn parses_global_flags() {
        let cli = Cli::try_parse_from([
            "cargo-upkeep",
            "--json",
            "--verbose",
            "--log-level",
            "debug",
            "detect",
        ])
        .unwrap();

        assert!(cli.json);
        assert!(cli.verbose);
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
    }

    /// `Command` and `UpkeepCommand` declare `quality` separately, so a flag
    /// added to one silently exists only on `cargo-upkeep quality` or only on
    /// `cargo upkeep quality`. Both forms are asserted here for that reason.
    #[test]
    fn parses_quality_require_complete_in_both_forms() {
        let cli = Cli::try_parse_from(["cargo-upkeep", "quality", "--require-complete"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Quality {
                require_complete: true
            }
        ));

        let cli = Cli::try_parse_from(["cargo-upkeep", "upkeep", "quality", "--require-complete"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Command::Upkeep(UpkeepCommand::Quality {
                require_complete: true
            })
        ));

        // Opt-in: absent unless asked for.
        let cli = Cli::try_parse_from(["cargo-upkeep", "quality"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Quality {
                require_complete: false
            }
        ));
    }

    /// `--require-complete` has three states, and the bare one is not "absent".
    ///
    /// `None` is the flag not passed. `Some(vec![])` is the bare form, meaning
    /// every capability. `Some(vec![..])` names the gate's subjects. Collapsing
    /// the first two — the shape `Option<Vec<_>>` invites — would make the bare
    /// form a no-op and silently disable the gate.
    #[test]
    fn parses_python_require_complete_in_all_three_states() {
        let args = |argv: &[&str]| match Cli::try_parse_from(argv).unwrap().command {
            Command::Python(args) => args,
            _ => panic!("unexpected subcommand"),
        };

        assert_eq!(args(&["cargo-upkeep", "python"]).require_complete, None);
        assert_eq!(
            args(&["cargo-upkeep", "python", "--require-complete"]).require_complete,
            Some(Vec::new()),
            "the bare form means every capability, which is not the same as absent"
        );
        assert_eq!(
            args(&[
                "cargo-upkeep",
                "python",
                "--require-complete=outdated,security"
            ])
            .require_complete,
            Some(vec![CapabilityArg::Outdated, CapabilityArg::Security])
        );
        assert_eq!(
            args(&["cargo-upkeep", "python", "--require-complete=security"]).require_complete,
            Some(vec![CapabilityArg::Security])
        );
    }

    /// A capability list is a value list, so a space-separated capability must
    /// not be swallowed as one.
    ///
    /// `require_equals` is what enforces that. Without it, `--require-complete`
    /// followed by a positional would eat it, and `num_args = 0..` would make the
    /// bare form greedy over anything that followed.
    #[test]
    fn python_require_complete_requires_an_equals_sign() {
        let err = Cli::try_parse_from(["cargo-upkeep", "python", "--require-complete", "outdated"])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    /// Both gates reject a value they do not know, which is exit 2 rather than a
    /// silently ignored flag.
    #[test]
    fn python_gates_reject_unknown_values() {
        for argv in [
            &["cargo-upkeep", "python", "--require-complete=bogus"][..],
            &["cargo-upkeep", "python", "--fail-on-vulnerability", "bogus"][..],
        ] {
            let err = Cli::try_parse_from(argv).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidValue, "{argv:?}");
        }
    }

    /// `Command` and `UpkeepCommand` declare `python` separately, so the parse
    /// has to be asserted under both invocation forms — the same reason
    /// `parses_quality_require_complete_in_both_forms` exists.
    #[test]
    fn parses_python_gates_in_both_forms() {
        let cli = Cli::try_parse_from([
            "cargo-upkeep",
            "python",
            "--require-complete=security",
            "--fail-on-vulnerability",
            "high",
        ])
        .unwrap();
        match cli.command {
            Command::Python(args) => {
                assert_eq!(args.require_complete, Some(vec![CapabilityArg::Security]));
                assert_eq!(args.fail_on_vulnerability, Some(ThresholdArg::High));
            }
            _ => panic!("unexpected subcommand"),
        }

        let cli = Cli::try_parse_from([
            "cargo-upkeep",
            "upkeep",
            "python",
            "--require-complete=security",
            "--fail-on-vulnerability",
            "high",
        ])
        .unwrap();
        match cli.command {
            Command::Upkeep(UpkeepCommand::Python(args)) => {
                assert_eq!(args.require_complete, Some(vec![CapabilityArg::Security]));
                assert_eq!(args.fail_on_vulnerability, Some(ThresholdArg::High));
            }
            _ => panic!("unexpected subcommand"),
        }
    }

    #[test]
    fn parses_unsafe_aliases() {
        let cli = Cli::try_parse_from(["cargo-upkeep", "unsafe"]).unwrap();
        assert!(matches!(cli.command, Command::UnsafeCode));

        let cli = Cli::try_parse_from(["cargo-upkeep", "unsafe-code"]).unwrap();
        assert!(matches!(cli.command, Command::UnsafeCode));
    }

    #[test]
    fn missing_subcommand_returns_error() {
        let err = Cli::try_parse_from(["cargo-upkeep"]).unwrap_err();
        assert_eq!(
            err.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn unknown_flag_returns_error() {
        let err = Cli::try_parse_from(["cargo-upkeep", "--nope", "detect"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn unknown_subcommand_returns_error() {
        let err = Cli::try_parse_from(["cargo-upkeep", "DETECT"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn init_logging_invalid_level_returns_error() {
        let err = super::init_logging(false, Some("info=bogus")).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Config);
        assert!(err
            .to_string()
            .contains("invalid log level filter: info=bogus"));
    }
}
