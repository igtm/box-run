use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{FsLayout, NetMode, RunArgs, StdinMode, TtyMode};
use crate::config::SandboxConfig;
use crate::error::{BoxRunError, Result};

const DEFAULT_ENV_KEYS: &[&str] = &["PATH", "TERM", "LANG", "LC_ALL", "LC_CTYPE"];
const LANDLOCK_RW_DEVICE_PATHS: &[&str] = &[
    "/dev/null",
    "/dev/full",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/pts",
    "/dev/ptmx",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub fs_layout: FsLayout,
    pub ro_mounts: Vec<Mount>,
    pub rw_mounts: Vec<Mount>,
    pub tmpfs_mounts: Vec<PathBuf>,
    pub hidden_paths: Vec<HiddenPath>,
    pub landlock: LandlockPlan,
    pub cwd: PathBuf,
    pub net: NetMode,
    pub stdin: StdinMode,
    pub tty: TtyMode,
    pub env: EnvPlan,
    pub command: Vec<OsString>,
    pub best_effort: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mount {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub kind: MountKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MountKind {
    File,
    Dir,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HiddenPath {
    pub dest: PathBuf,
    pub kind: MountKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvPlan {
    pub clear: bool,
    pub vars: Vec<(String, OsString)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LandlockPlan {
    pub ro_paths: Vec<PathBuf>,
    pub rw_paths: Vec<PathBuf>,
    pub tcp_connect_ports: Vec<u16>,
    pub tcp_bind_ports: Vec<u16>,
}

impl LandlockPlan {
    pub fn is_empty(&self) -> bool {
        self.ro_paths.is_empty()
            && self.rw_paths.is_empty()
            && self.tcp_connect_ports.is_empty()
            && self.tcp_bind_ports.is_empty()
    }

    pub fn has_tcp_rules(&self) -> bool {
        !self.tcp_connect_ports.is_empty() || !self.tcp_bind_ports.is_empty()
    }

    pub fn has_fs_rules(&self) -> bool {
        !self.ro_paths.is_empty() || !self.rw_paths.is_empty()
    }
}

impl Policy {
    pub fn from_args(args: RunArgs) -> Result<Self> {
        let config = match args.config.as_ref() {
            Some(path) => SandboxConfig::load(path)?,
            None => SandboxConfig::default(),
        };
        let input = PolicyInput::from_sources(config, args)?;

        let host_cwd = env::current_dir().map_err(BoxRunError::CurrentDir)?;
        let host_cwd = canonicalize_host_path(&host_cwd, &host_cwd)?;

        let mut ro_mounts = parse_mounts(&input.ro, &host_cwd)?;
        let mut rw_mounts = parse_mounts(&input.rw, &host_cwd)?;

        if !rw_mounts.iter().any(|mount| mount.dest == host_cwd) {
            rw_mounts.push(mount_from_paths(host_cwd.clone(), host_cwd.clone())?);
        }

        ro_mounts.sort_by(|left, right| left.dest.cmp(&right.dest));
        rw_mounts.sort_by(|left, right| left.dest.cmp(&right.dest));

        let cwd = match input.cwd {
            Some(cwd) => require_absolute_sandbox_path(cwd)?,
            None => host_cwd.clone(),
        };

        let tmpfs_mounts = normalize_tmpfs_mounts(input.tmpfs)?;
        let hidden_paths = resolve_hidden_paths(input.hide, &ro_mounts, &rw_mounts, &host_cwd)?;
        let landlock = build_landlock_plan(
            input.fs_layout,
            &ro_mounts,
            &rw_mounts,
            &tmpfs_mounts,
            &input.allow_tcp_connect,
            &input.allow_tcp_bind,
        );
        let env = resolve_env_plan(input.env_clear, &input.env_pass, input.env_set)?;

        Ok(Self {
            fs_layout: input.fs_layout,
            ro_mounts,
            rw_mounts,
            tmpfs_mounts,
            hidden_paths,
            landlock,
            cwd,
            net: input.net,
            stdin: input.stdin,
            tty: input.tty,
            env,
            command: input.command,
            best_effort: input.best_effort,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PolicyInput {
    fs_layout: FsLayout,
    ro: Vec<String>,
    rw: Vec<String>,
    tmpfs: Vec<PathBuf>,
    hide: Vec<PathBuf>,
    cwd: Option<PathBuf>,
    net: NetMode,
    stdin: StdinMode,
    tty: TtyMode,
    allow_tcp_connect: Vec<u16>,
    allow_tcp_bind: Vec<u16>,
    env_clear: bool,
    env_pass: Vec<String>,
    env_set: BTreeMap<String, OsString>,
    command: Vec<OsString>,
    best_effort: bool,
}

impl PolicyInput {
    fn from_sources(config: SandboxConfig, args: RunArgs) -> Result<Self> {
        let mut env_set = config
            .env
            .set
            .into_iter()
            .map(|(key, value)| (key, OsString::from(value)))
            .collect::<BTreeMap<_, _>>();
        for (key, value) in parse_explicit_env(&args.env)? {
            env_set.insert(key, value);
        }

        let command = if args.command.is_empty() {
            config
                .command
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        } else {
            args.command
        };
        if command.is_empty() {
            return Err(BoxRunError::MissingCommand);
        }

        let input = Self {
            fs_layout: args
                .fs_layout
                .or(config.fs.layout)
                .unwrap_or(FsLayout::HostRo),
            ro: merge_vecs(config.fs.ro, args.ro),
            rw: merge_vecs(config.fs.rw, args.rw),
            tmpfs: merge_vecs(config.fs.tmpfs, args.tmpfs),
            hide: merge_vecs(config.fs.hide, args.hide),
            cwd: args.cwd.or(config.process.cwd),
            net: args.net.or(config.net.mode).unwrap_or(NetMode::None),
            stdin: args
                .stdin
                .or(config.process.stdin)
                .unwrap_or(StdinMode::Inherit),
            tty: args.tty.or(config.process.tty).unwrap_or(TtyMode::Auto),
            allow_tcp_connect: merge_vecs(config.net.allow_tcp_connect, args.allow_tcp_connect),
            allow_tcp_bind: merge_vecs(config.net.allow_tcp_bind, args.allow_tcp_bind),
            env_clear: args
                .env_clear
                .or(args.inherit_env.map(|inherit| !inherit))
                .or(config.env.clear)
                .unwrap_or(true),
            env_pass: merge_vecs(config.env.pass, args.env_pass),
            env_set,
            command,
            best_effort: args.best_effort.or(config.best_effort).unwrap_or(false),
        };

        tracing::debug!(
            fs_layout = ?input.fs_layout,
            net = ?input.net,
            tty = ?input.tty,
            stdin = ?input.stdin,
            command = ?input.command,
            env_clear = input.env_clear,
            "merged CLI and config inputs"
        );

        Ok(input)
    }
}

fn resolve_env_plan(
    clear_env: bool,
    explicit_pass: &[String],
    explicit_set: BTreeMap<String, OsString>,
) -> Result<EnvPlan> {
    if !clear_env {
        return Ok(EnvPlan {
            clear: false,
            vars: explicit_set.into_iter().collect(),
        });
    }

    let mut vars = BTreeMap::<String, OsString>::new();

    for key in DEFAULT_ENV_KEYS
        .iter()
        .copied()
        .chain(explicit_pass.iter().map(String::as_str))
    {
        if let Some(value) = env::var_os(key) {
            vars.insert(key.to_owned(), value);
        }
    }

    for (key, value) in explicit_set {
        vars.insert(key, value);
    }

    Ok(EnvPlan {
        clear: true,
        vars: vars.into_iter().collect(),
    })
}

fn merge_vecs<T>(mut left: Vec<T>, right: Vec<T>) -> Vec<T> {
    left.extend(right);
    left
}

fn resolve_hidden_paths(
    raw_paths: Vec<PathBuf>,
    ro_mounts: &[Mount],
    rw_mounts: &[Mount],
    host_cwd: &Path,
) -> Result<Vec<HiddenPath>> {
    let mut hidden_paths = Vec::new();

    for raw_path in raw_paths {
        let dest = require_absolute_sandbox_path(raw_path)?;
        let kind = resolve_hidden_path_kind(&dest, ro_mounts, rw_mounts, host_cwd)?;
        hidden_paths.push(HiddenPath { dest, kind });
    }

    hidden_paths.sort_by(|left, right| left.dest.cmp(&right.dest));
    hidden_paths.dedup_by(|left, right| left.dest == right.dest);
    Ok(hidden_paths)
}

fn resolve_hidden_path_kind(
    dest: &Path,
    ro_mounts: &[Mount],
    rw_mounts: &[Mount],
    host_cwd: &Path,
) -> Result<MountKind> {
    for mount in ro_mounts.iter().chain(rw_mounts.iter()) {
        if let Some(kind) = resolve_path_kind_from_mount(dest, mount)? {
            return Ok(kind);
        }
    }

    let host_path = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        host_cwd.join(dest)
    };
    let metadata = fs::metadata(&host_path).map_err(|source| BoxRunError::Metadata {
        path: host_path,
        source,
    })?;

    Ok(if metadata.is_dir() {
        MountKind::Dir
    } else {
        MountKind::File
    })
}

fn resolve_path_kind_from_mount(dest: &Path, mount: &Mount) -> Result<Option<MountKind>> {
    match mount.kind {
        MountKind::File => {
            if dest == mount.dest {
                Ok(Some(MountKind::File))
            } else {
                Ok(None)
            }
        }
        MountKind::Dir => {
            if dest == mount.dest {
                return Ok(Some(MountKind::Dir));
            }

            let Ok(relative) = dest.strip_prefix(&mount.dest) else {
                return Ok(None);
            };
            let candidate = mount.src.join(relative);
            let metadata = fs::metadata(&candidate).map_err(|source| BoxRunError::Metadata {
                path: candidate,
                source,
            })?;

            Ok(Some(if metadata.is_dir() {
                MountKind::Dir
            } else {
                MountKind::File
            }))
        }
    }
}

fn build_landlock_plan(
    fs_layout: FsLayout,
    ro_mounts: &[Mount],
    rw_mounts: &[Mount],
    tmpfs_mounts: &[PathBuf],
    allow_tcp_connect: &[u16],
    allow_tcp_bind: &[u16],
) -> LandlockPlan {
    let tcp_connect_ports = normalize_tcp_ports(allow_tcp_connect);
    let tcp_bind_ports = normalize_tcp_ports(allow_tcp_bind);

    if matches!(fs_layout, FsLayout::HostRo) {
        return LandlockPlan {
            ro_paths: vec![],
            rw_paths: vec![],
            tcp_connect_ports,
            tcp_bind_ports,
        };
    }

    let mut ro_paths = BTreeSet::new();
    let mut rw_paths = BTreeSet::new();

    for mount in ro_mounts {
        ro_paths.insert(mount.dest.clone());
    }

    ro_paths.insert(PathBuf::from("/proc"));
    ro_paths.insert(PathBuf::from("/dev"));

    for mount in rw_mounts {
        rw_paths.insert(mount.dest.clone());
    }

    for mount in tmpfs_mounts {
        rw_paths.insert(mount.clone());
    }

    for path in LANDLOCK_RW_DEVICE_PATHS {
        rw_paths.insert(PathBuf::from(path));
    }

    LandlockPlan {
        ro_paths: ro_paths.into_iter().collect(),
        rw_paths: rw_paths.into_iter().collect(),
        tcp_connect_ports,
        tcp_bind_ports,
    }
}

fn normalize_tcp_ports(ports: &[u16]) -> Vec<u16> {
    ports
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn parse_explicit_env(explicit_set: &[String]) -> Result<Vec<(String, OsString)>> {
    explicit_set
        .iter()
        .map(|spec| {
            let (key, value) = spec
                .split_once('=')
                .ok_or_else(|| BoxRunError::InvalidEnvAssignment { spec: spec.clone() })?;
            if key.is_empty() {
                return Err(BoxRunError::EmptyEnvKey {
                    key: key.to_owned(),
                });
            }

            Ok((key.to_owned(), OsString::from(value)))
        })
        .collect()
}

fn normalize_tmpfs_mounts(raw_paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut mounts = BTreeSet::new();
    mounts.insert(PathBuf::from("/tmp"));

    for raw_path in raw_paths {
        mounts.insert(require_absolute_sandbox_path(raw_path)?);
    }

    Ok(mounts.into_iter().collect())
}

fn parse_mounts(specs: &[String], host_cwd: &Path) -> Result<Vec<Mount>> {
    specs
        .iter()
        .map(|spec| parse_mount(spec, host_cwd))
        .collect()
}

fn parse_mount(spec: &str, host_cwd: &Path) -> Result<Mount> {
    let (src, dest, explicit_dest) = match spec.split_once(':') {
        Some((src, dest)) if !src.is_empty() && !dest.is_empty() => {
            (PathBuf::from(src), PathBuf::from(dest), true)
        }
        None => {
            let src = PathBuf::from(spec);
            let dest = if src.is_absolute() {
                src.clone()
            } else {
                host_cwd.join(&src)
            };
            (src, dest, false)
        }
        _ => {
            return Err(BoxRunError::InvalidBindSpec {
                spec: spec.to_owned(),
            });
        }
    };

    let src = canonicalize_host_path(&src, host_cwd)?;
    let dest = if !explicit_dest {
        dest
    } else if dest.is_absolute() {
        dest
    } else {
        return Err(BoxRunError::NonAbsoluteSandboxPath { path: dest });
    };

    mount_from_paths(src, require_absolute_sandbox_path(dest)?)
}

fn mount_from_paths(src: PathBuf, dest: PathBuf) -> Result<Mount> {
    let metadata = fs::metadata(&src).map_err(|source| BoxRunError::Metadata {
        path: src.clone(),
        source,
    })?;

    let kind = if metadata.is_dir() {
        MountKind::Dir
    } else {
        MountKind::File
    };

    Ok(Mount { src, dest, kind })
}

fn canonicalize_host_path(path: &Path, host_cwd: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        host_cwd.join(path)
    };

    path.canonicalize()
        .map_err(|source| BoxRunError::CanonicalizePath { path, source })
}

fn require_absolute_sandbox_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(BoxRunError::NonAbsoluteSandboxPath { path })
    }
}

#[cfg(test)]
use std::ffi::OsStr;

#[cfg(test)]
pub fn format_env_arg(key: &str, value: &OsStr) -> OsString {
    let mut merged = OsString::from(key);
    merged.push("=");
    merged.push(value);
    merged
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{PolicyInput, format_env_arg, parse_explicit_env, resolve_env_plan};
    use crate::cli::RunArgs;
    use crate::cli::{FsLayout, NetMode, StdinMode, TtyMode};
    use crate::config::{EnvConfig, FsConfig, NetConfig, ProcessConfig, SandboxConfig};
    use crate::policy::{LandlockPlan, Mount, MountKind, Policy};

    #[test]
    fn explicit_env_parses_assignments() {
        let vars = parse_explicit_env(&["FOO=bar".to_owned(), "A=B=C".to_owned()]).unwrap();
        assert_eq!(
            vars,
            vec![
                ("FOO".to_owned(), OsString::from("bar")),
                ("A".to_owned(), OsString::from("B=C"))
            ]
        );
    }

    #[test]
    fn explicit_env_rejects_missing_separator() {
        let err = parse_explicit_env(&["BROKEN".to_owned()]).unwrap_err();
        assert!(err.to_string().contains("invalid env assignment"));
    }

    #[test]
    fn mount_dest_must_be_absolute_when_explicit() {
        let cwd = std::env::current_dir().unwrap();
        let err = super::parse_mount(".:relative", &cwd).unwrap_err();
        assert!(err.to_string().contains("must be absolute"));
    }

    #[test]
    fn clear_env_plan_keeps_explicit_override() {
        let mut env_set = BTreeMap::new();
        env_set.insert("PATH".to_owned(), OsString::from("/custom/bin"));
        let plan = resolve_env_plan(true, &[], env_set).unwrap();
        assert!(plan.clear);
        assert_eq!(
            plan.vars
                .iter()
                .find(|(key, _)| key == "PATH")
                .map(|(_, value)| value.clone()),
            Some(OsString::from("/custom/bin"))
        );
    }

    #[test]
    fn format_env_argument_joins_key_and_value() {
        let arg = format_env_arg("TERM", &OsString::from("xterm"));
        assert_eq!(arg, PathBuf::from("TERM=xterm").into_os_string());
    }

    #[test]
    fn inherit_env_disables_clear_behavior() {
        let mut env_set = BTreeMap::new();
        env_set.insert("FOO".to_owned(), OsString::from("bar"));
        let plan = resolve_env_plan(false, &["PATH".to_owned()], env_set).unwrap();
        assert!(!plan.clear);
        assert_eq!(plan.vars, vec![("FOO".to_owned(), OsString::from("bar"))]);
    }

    #[test]
    fn host_ro_landlock_plan_is_disabled() {
        let policy = Policy {
            fs_layout: FsLayout::HostRo,
            ro_mounts: vec![],
            rw_mounts: vec![Mount {
                src: PathBuf::from("/work"),
                dest: PathBuf::from("/work"),
                kind: MountKind::Dir,
            }],
            tmpfs_mounts: vec![PathBuf::from("/tmp")],
            hidden_paths: vec![],
            landlock: LandlockPlan {
                ro_paths: vec![],
                rw_paths: vec![],
                tcp_connect_ports: vec![],
                tcp_bind_ports: vec![],
            },
            cwd: PathBuf::from("/work"),
            net: NetMode::None,
            stdin: StdinMode::Inherit,
            tty: TtyMode::Auto,
            env: super::EnvPlan {
                clear: true,
                vars: vec![],
            },
            command: vec![],
            best_effort: false,
        };

        let plan = super::build_landlock_plan(
            policy.fs_layout,
            &policy.ro_mounts,
            &policy.rw_mounts,
            &policy.tmpfs_mounts,
            &[],
            &[],
        );
        assert!(plan.is_empty());
    }

    #[test]
    fn strict_landlock_plan_uses_explicit_ro_mounts() {
        let plan = super::build_landlock_plan(
            FsLayout::Strict,
            &[Mount {
                src: PathBuf::from("/usr"),
                dest: PathBuf::from("/usr"),
                kind: MountKind::Dir,
            }],
            &[Mount {
                src: PathBuf::from("/workspace"),
                dest: PathBuf::from("/workspace"),
                kind: MountKind::Dir,
            }],
            &[PathBuf::from("/tmp"), PathBuf::from("/scratch")],
            &[],
            &[],
        );

        assert_eq!(
            plan,
            LandlockPlan {
                ro_paths: vec![
                    PathBuf::from("/dev"),
                    PathBuf::from("/proc"),
                    PathBuf::from("/usr")
                ],
                rw_paths: vec![
                    PathBuf::from("/dev/full"),
                    PathBuf::from("/dev/null"),
                    PathBuf::from("/dev/ptmx"),
                    PathBuf::from("/dev/pts"),
                    PathBuf::from("/dev/random"),
                    PathBuf::from("/dev/tty"),
                    PathBuf::from("/dev/urandom"),
                    PathBuf::from("/dev/zero"),
                    PathBuf::from("/scratch"),
                    PathBuf::from("/tmp"),
                    PathBuf::from("/workspace")
                ],
                tcp_connect_ports: vec![],
                tcp_bind_ports: vec![],
            }
        );
    }

    #[test]
    fn landlock_plan_normalizes_tcp_ports() {
        let plan = super::build_landlock_plan(
            FsLayout::HostRo,
            &[],
            &[],
            &[],
            &[443, 80, 443],
            &[0, 8080, 0],
        );

        assert_eq!(plan.tcp_connect_ports, vec![80, 443]);
        assert_eq!(plan.tcp_bind_ports, vec![0, 8080]);
        assert!(plan.has_tcp_rules());
    }

    #[test]
    fn implicit_absolute_mount_preserves_requested_destination_path() {
        let tmp = std::env::temp_dir().join(format!("box-run-policy-test-{}", std::process::id()));
        let real = tmp.join("real");
        let link = tmp.join("link");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mount = super::parse_mount(
            link.to_string_lossy().as_ref(),
            PathBuf::from("/").as_path(),
        )
        .unwrap();
        assert_eq!(mount.dest, link);
        assert_eq!(mount.src, real.canonicalize().unwrap());

        fs::remove_file(&link).unwrap();
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn config_and_cli_are_merged_with_cli_priority() {
        let input = PolicyInput::from_sources(
            SandboxConfig {
                fs: FsConfig {
                    layout: Some(FsLayout::Strict),
                    ro: vec!["/usr".to_owned()],
                    rw: vec!["./workspace:/workspace".to_owned()],
                    tmpfs: vec![PathBuf::from("/cache")],
                    hide: vec![],
                },
                net: NetConfig {
                    mode: Some(NetMode::Host),
                    allow_tcp_connect: vec![443],
                    allow_tcp_bind: vec![8080],
                },
                env: EnvConfig {
                    clear: Some(true),
                    pass: vec!["PATH".to_owned()],
                    set: BTreeMap::from([("FROM_CONFIG".to_owned(), "1".to_owned())]),
                },
                process: ProcessConfig {
                    cwd: Some(PathBuf::from("/workspace")),
                    stdin: Some(StdinMode::Inherit),
                    tty: Some(TtyMode::Auto),
                },
                command: vec!["echo".to_owned(), "from-config".to_owned()],
                best_effort: Some(true),
            },
            RunArgs {
                config: None,
                fs_layout: Some(FsLayout::HostRo),
                ro: vec!["/bin".to_owned()],
                rw: vec![],
                tmpfs: vec![PathBuf::from("/tmp")],
                hide: vec![PathBuf::from("/etc/hosts")],
                net: Some(NetMode::None),
                env: vec!["FROM_CLI=1".to_owned()],
                env_pass: vec!["TERM".to_owned()],
                env_clear: None,
                inherit_env: Some(true),
                cwd: None,
                stdin: Some(StdinMode::Null),
                tty: Some(TtyMode::Disable),
                allow_tcp_connect: vec![8443],
                allow_tcp_bind: vec![8080, 9090],
                best_effort: Some(false),
                command: vec![OsString::from("echo"), OsString::from("from-cli")],
            },
        )
        .unwrap();

        assert_eq!(input.fs_layout, FsLayout::HostRo);
        assert_eq!(input.net, NetMode::None);
        assert!(!input.env_clear);
        assert_eq!(input.stdin, StdinMode::Null);
        assert_eq!(input.tty, TtyMode::Disable);
        assert_eq!(
            input.command,
            vec![OsString::from("echo"), OsString::from("from-cli")]
        );
        assert!(!input.best_effort);
        assert_eq!(
            input.env_set.get("FROM_CONFIG").cloned(),
            Some(OsString::from("1"))
        );
        assert_eq!(
            input.env_set.get("FROM_CLI").cloned(),
            Some(OsString::from("1"))
        );
        assert_eq!(input.ro, vec!["/usr".to_owned(), "/bin".to_owned()]);
        assert_eq!(input.hide, vec![PathBuf::from("/etc/hosts")]);
        assert_eq!(input.allow_tcp_connect, vec![443, 8443]);
        assert_eq!(input.allow_tcp_bind, vec![8080, 8080, 9090]);
    }

    #[test]
    fn hide_target_kind_is_resolved_from_mounted_directory() {
        let tmp = std::env::temp_dir().join(format!("box-run-hide-test-{}", std::process::id()));
        let source = tmp.join("source");
        let secret_dir = source.join("secret");
        let secret_file = source.join("secret.txt");
        fs::create_dir_all(&secret_dir).unwrap();
        fs::write(&secret_file, b"secret").unwrap();

        let mounts = vec![Mount {
            src: source.clone(),
            dest: PathBuf::from("/workspace"),
            kind: MountKind::Dir,
        }];

        assert_eq!(
            super::resolve_hidden_path_kind(
                Path::new("/workspace/secret"),
                &mounts,
                &[],
                Path::new("/")
            )
            .unwrap(),
            MountKind::Dir
        );
        assert_eq!(
            super::resolve_hidden_path_kind(
                Path::new("/workspace/secret.txt"),
                &mounts,
                &[],
                Path::new("/")
            )
            .unwrap(),
            MountKind::File
        );

        fs::remove_file(secret_file).unwrap();
        fs::remove_dir_all(tmp).unwrap();
    }
}
