use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, BoxRunError>;

#[derive(Debug, Error)]
pub enum BoxRunError {
    #[error("{0}")]
    Message(String),

    #[error("missing required program `{program}`")]
    MissingProgram {
        program: &'static str,
        #[source]
        source: which::Error,
    },

    #[error("failed to read current working directory")]
    CurrentDir(#[source] std::io::Error),

    #[error("failed to read current executable path")]
    CurrentExe(#[source] std::io::Error),

    #[error("failed to resolve source path `{path}`")]
    CanonicalizePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to inspect path `{path}`")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid bind spec `{spec}`; expected SRC[:DEST]")]
    InvalidBindSpec { spec: String },

    #[error("sandbox destination `{path}` must be absolute")]
    NonAbsoluteSandboxPath { path: PathBuf },

    #[error("invalid env assignment `{spec}`; expected KEY=VALUE")]
    InvalidEnvAssignment { spec: String },

    #[error("env key `{key}` must not be empty")]
    EmptyEnvKey { key: String },

    #[error("missing target command; pass it after `--` or set `command = [...]` in config")]
    MissingCommand,

    #[error("unsupported option: {message}")]
    UnsupportedOption { message: String },

    #[error("failed to read config file `{path}`")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file `{path}`")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to execute `{program}`")]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed while waiting for `{program}`")]
    Wait {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("sandbox helper failed to exec target command")]
    ExecTarget(#[source] std::io::Error),

    #[error("failed to configure process hardening")]
    ProcessHardening(#[source] std::io::Error),

    #[error("failed to forward sandbox {stream}")]
    IoForward {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("sandbox {stream} forwarding thread panicked")]
    IoForwardPanic { stream: &'static str },

    #[cfg(target_os = "linux")]
    #[error("failed to configure Landlock restrictions")]
    Landlock(#[from] landlock::RulesetError),
}
