use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(author, version, about = "Run commands inside a lightweight sandbox")]
pub struct Cli {
    #[arg(short, long, action = ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Run(RunArgs),
    Doctor,
    #[command(name = "__helper", hide = true)]
    Helper(HelperArgs),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum FsLayout {
    #[value(name = "host-ro")]
    HostRo,
    #[value(name = "strict")]
    Strict,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum NetMode {
    #[value(name = "none")]
    None,
    #[value(name = "host")]
    Host,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum StdinMode {
    #[value(name = "inherit")]
    Inherit,
    #[value(name = "null")]
    Null,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum TtyMode {
    #[value(name = "auto")]
    Auto,
    #[value(name = "force")]
    Force,
    #[value(name = "disable")]
    Disable,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[arg(long, value_enum)]
    pub fs_layout: Option<FsLayout>,

    #[arg(long = "ro", value_name = "SRC[:DEST]")]
    pub ro: Vec<String>,

    #[arg(long = "rw", value_name = "SRC[:DEST]")]
    pub rw: Vec<String>,

    #[arg(long = "tmpfs", value_name = "DEST")]
    pub tmpfs: Vec<PathBuf>,

    #[arg(long = "hide", value_name = "PATH")]
    pub hide: Vec<PathBuf>,

    #[arg(long, value_enum)]
    pub net: Option<NetMode>,

    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,

    #[arg(long = "env-pass", value_name = "KEY")]
    pub env_pass: Vec<String>,

    #[arg(
        long,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true",
        value_name = "BOOL",
        conflicts_with = "inherit_env"
    )]
    pub env_clear: Option<bool>,

    #[arg(
        long,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true",
        value_name = "BOOL"
    )]
    pub inherit_env: Option<bool>,

    #[arg(long, value_name = "PATH")]
    pub cwd: Option<PathBuf>,

    #[arg(long, value_enum)]
    pub stdin: Option<StdinMode>,

    #[arg(long, value_enum)]
    pub tty: Option<TtyMode>,

    #[arg(long, value_name = "PORT")]
    pub allow_tcp_connect: Vec<u16>,

    #[arg(long, value_name = "PORT")]
    pub allow_tcp_bind: Vec<u16>,

    #[arg(
        long,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true",
        value_name = "BOOL"
    )]
    pub best_effort: Option<bool>,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<OsString>,
}

#[derive(Debug, Args)]
pub struct HelperArgs {
    #[arg(long = "landlock-ro", value_name = "PATH")]
    pub landlock_ro: Vec<PathBuf>,

    #[arg(long = "landlock-rw", value_name = "PATH")]
    pub landlock_rw: Vec<PathBuf>,

    #[arg(long = "landlock-tcp-connect", value_name = "PORT")]
    pub landlock_tcp_connect: Vec<u16>,

    #[arg(long = "landlock-tcp-bind", value_name = "PORT")]
    pub landlock_tcp_bind: Vec<u16>,

    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<OsString>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Commands, TtyMode};

    #[test]
    fn bool_flags_are_none_when_omitted() {
        let cli = Cli::try_parse_from(["box-run", "run", "--", "true"]).unwrap();
        let Commands::Run(args) = cli.command else {
            panic!("expected run args");
        };

        assert_eq!(args.best_effort, None);
        assert_eq!(args.env_clear, None);
        assert_eq!(args.inherit_env, None);
    }

    #[test]
    fn bool_flags_parse_as_true_when_present_without_value() {
        let cli = Cli::try_parse_from([
            "box-run",
            "run",
            "--best-effort",
            "--inherit-env",
            "--",
            "true",
        ])
        .unwrap();
        let Commands::Run(args) = cli.command else {
            panic!("expected run args");
        };

        assert_eq!(args.best_effort, Some(true));
        assert_eq!(args.inherit_env, Some(true));
    }

    #[test]
    fn tty_mode_parses() {
        let cli =
            Cli::try_parse_from(["box-run", "run", "--tty", "disable", "--", "true"]).unwrap();
        let Commands::Run(args) = cli.command else {
            panic!("expected run args");
        };

        assert_eq!(args.tty, Some(TtyMode::Disable));
    }
}
