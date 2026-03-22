use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cli::{FsLayout, NetMode, StdinMode, TtyMode};
use crate::error::{BoxRunError, Result};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    #[serde(default)]
    pub fs: FsConfig,
    #[serde(default)]
    pub net: NetConfig,
    #[serde(default)]
    pub env: EnvConfig,
    #[serde(default)]
    pub process: ProcessConfig,
    #[serde(default)]
    pub command: Vec<String>,
    pub best_effort: Option<bool>,
}

impl SandboxConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).map_err(|source| BoxRunError::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&raw).map_err(|source| BoxRunError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FsConfig {
    pub layout: Option<FsLayout>,
    #[serde(default)]
    pub ro: Vec<String>,
    #[serde(default)]
    pub rw: Vec<String>,
    #[serde(default)]
    pub tmpfs: Vec<PathBuf>,
    #[serde(default)]
    pub hide: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetConfig {
    pub mode: Option<NetMode>,
    #[serde(default)]
    pub allow_tcp_connect: Vec<u16>,
    #[serde(default)]
    pub allow_tcp_bind: Vec<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnvConfig {
    pub clear: Option<bool>,
    #[serde(default)]
    pub pass: Vec<String>,
    #[serde(default)]
    pub set: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessConfig {
    pub cwd: Option<PathBuf>,
    pub stdin: Option<StdinMode>,
    pub tty: Option<TtyMode>,
}

#[cfg(test)]
mod tests {
    use crate::cli::{StdinMode, TtyMode};

    use super::SandboxConfig;

    #[test]
    fn parse_full_config() {
        let config = toml::from_str::<SandboxConfig>(
            r#"
best_effort = true
command = ["sh", "-c", "echo config"]

[fs]
layout = "strict"
ro = ["/usr", "/bin"]
rw = ["./workspace:/workspace"]
tmpfs = ["/tmp"]
hide = ["/workspace/.env"]

[net]
mode = "none"
allow_tcp_connect = [443]
allow_tcp_bind = [8080]

[env]
clear = true
pass = ["PATH", "TERM"]

[env.set]
FOO = "bar"

[process]
cwd = "/workspace"
stdin = "inherit"
tty = "auto"
"#,
        )
        .unwrap();

        assert_eq!(config.command, vec!["sh", "-c", "echo config"]);
        assert_eq!(config.net.allow_tcp_connect, vec![443]);
        assert_eq!(config.net.allow_tcp_bind, vec![8080]);
        assert_eq!(config.env.set.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(config.process.stdin, Some(StdinMode::Inherit));
        assert_eq!(config.process.tty, Some(TtyMode::Auto));
    }
}
