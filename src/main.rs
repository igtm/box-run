mod cli;
mod config;
mod error;
#[cfg(target_os = "linux")]
mod landlock_support;
#[cfg(target_os = "linux")]
mod linux;
mod platform;
mod policy;

use std::error::Error as _;
use std::process::ExitCode;

use clap::Parser;
use tracing::level_filters::LevelFilter;

use crate::cli::{Cli, Commands};
use crate::error::Result;

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match dispatch(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            let mut source = error.source();
            while let Some(next) = source {
                eprintln!("caused by: {next}");
                source = next.source();
            }
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Commands::Run(args) => platform::run(args),
        Commands::Doctor => platform::doctor(),
        Commands::Helper(args) => platform::run_helper(args),
    }
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .without_time()
        .with_target(false)
        .try_init();
}
