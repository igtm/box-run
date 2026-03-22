use std::env;
use std::io::{self, IsTerminal, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;

use tempfile::NamedTempFile;
use tracing::debug;

use crate::cli::{FsLayout, HelperArgs, NetMode, StdinMode, TtyMode};
use crate::error::{BoxRunError, Result};
use crate::landlock_support;
use crate::policy::{LandlockPlan, MountKind, Policy};

const HELPER_PATH: &str = "/tmp/box-run-helper";

pub fn run(policy: &Policy) -> Result<ExitCode> {
    validate_tcp_landlock_support(policy)?;

    let bwrap = which::which("bwrap").map_err(|source| BoxRunError::MissingProgram {
        program: "bwrap",
        source,
    })?;
    let current_exe = env::current_exe().map_err(BoxRunError::CurrentExe)?;

    let mut command = Command::new(&bwrap);
    let stdio_mode = configure_stdio(&mut command, policy)?;

    let _artifacts = build_bwrap_command(&mut command, &current_exe, policy)?;
    debug!(program = %bwrap.display(), args = ?command.get_args().collect::<Vec<_>>());

    let status = match stdio_mode {
        StdioMode::Direct => command.status().map_err(|source| BoxRunError::Spawn {
            program: bwrap.clone(),
            source,
        })?,
        StdioMode::Forwarded { copy_stdin } => {
            spawn_with_forwarded_stdio(command, &bwrap, copy_stdin)?
        }
    };

    Ok(exit_code_from_status(status))
}

pub fn run_helper(args: HelperArgs) -> Result<ExitCode> {
    set_no_new_privs()?;
    landlock_support::apply(&LandlockPlan {
        ro_paths: args.landlock_ro,
        rw_paths: args.landlock_rw,
        tcp_connect_ports: args.landlock_tcp_connect,
        tcp_bind_ports: args.landlock_tcp_bind,
    })?;

    let (program, command_args) = args
        .command
        .split_first()
        .ok_or_else(|| BoxRunError::Message("missing target command".to_owned()))?;

    let error = Command::new(program).args(command_args).exec();
    Err(BoxRunError::ExecTarget(error))
}

pub fn doctor() -> Result<ExitCode> {
    println!("platform: linux");

    match which::which("bwrap") {
        Ok(path) => {
            println!("bubblewrap: found at {}", path.display());
            match Command::new(&path).arg("--version").output() {
                Ok(output) => {
                    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    if version.is_empty() {
                        println!("bubblewrap version: unknown");
                    } else {
                        println!("bubblewrap version: {version}");
                    }
                }
                Err(error) => {
                    println!("bubblewrap version: unavailable ({error})");
                }
            }
        }
        Err(error) => {
            println!("bubblewrap: unavailable ({error})");
        }
    }

    match landlock_support::detect_abi() {
        Some(abi) => {
            println!("landlock: available (ABI {abi})");
            println!("landlock filesystem mode: enabled for strict layouts");
            if abi >= landlock_support::TCP_RULES_MIN_ABI {
                println!("landlock tcp allowlists: enabled");
            } else {
                println!(
                    "landlock tcp allowlists: unavailable (requires ABI {}+)",
                    landlock_support::TCP_RULES_MIN_ABI
                );
            }
        }
        None => {
            println!("landlock: unavailable");
            println!("landlock filesystem mode: disabled");
            println!("landlock tcp allowlists: disabled");
        }
    }

    Ok(ExitCode::SUCCESS)
}

struct BuildArtifacts {
    hidden_files: Vec<NamedTempFile>,
}

enum StdioMode {
    Direct,
    Forwarded { copy_stdin: bool },
}

fn build_bwrap_command(
    command: &mut Command,
    current_exe: &Path,
    policy: &Policy,
) -> Result<BuildArtifacts> {
    command
        .arg("--new-session")
        .arg("--die-with-parent")
        .arg("--unshare-pid")
        .arg("--unshare-ipc")
        .arg("--unshare-uts");

    if matches!(policy.net, NetMode::None) {
        command.arg("--unshare-net");
    }

    if policy.env.clear {
        command.arg("--clearenv");
    }

    for (key, value) in &policy.env.vars {
        command.arg("--setenv").arg(key).arg(value);
    }

    if matches!(policy.fs_layout, FsLayout::HostRo) {
        command.arg("--ro-bind").arg("/").arg("/");
    }

    for tmpfs in &policy.tmpfs_mounts {
        command.arg("--tmpfs").arg(tmpfs);
    }

    add_mount(
        command,
        current_exe,
        Path::new(HELPER_PATH),
        MountKind::File,
        true,
    );

    for mount in &policy.ro_mounts {
        add_mount(command, &mount.src, &mount.dest, mount.kind, true);
    }

    for mount in &policy.rw_mounts {
        add_mount(command, &mount.src, &mount.dest, mount.kind, false);
    }

    command.arg("--proc").arg("/proc");
    command.arg("--dev").arg("/dev");
    let mut artifacts = BuildArtifacts {
        hidden_files: Vec::new(),
    };
    apply_hidden_paths(command, policy, &mut artifacts)?;

    command.arg("--chdir").arg(&policy.cwd);
    command.arg("--").arg(HELPER_PATH).arg("__helper");

    for path in &policy.landlock.ro_paths {
        command.arg("--landlock-ro").arg(path);
    }
    for path in &policy.landlock.rw_paths {
        command.arg("--landlock-rw").arg(path);
    }
    for port in &policy.landlock.tcp_connect_ports {
        command.arg("--landlock-tcp-connect").arg(port.to_string());
    }
    for port in &policy.landlock.tcp_bind_ports {
        command.arg("--landlock-tcp-bind").arg(port.to_string());
    }

    command.arg("--");
    command.args(&policy.command);
    Ok(artifacts)
}

fn apply_hidden_paths(
    command: &mut Command,
    policy: &Policy,
    artifacts: &mut BuildArtifacts,
) -> Result<()> {
    for hidden_path in &policy.hidden_paths {
        match hidden_path.kind {
            MountKind::Dir => {
                command.arg("--tmpfs").arg(&hidden_path.dest);
                command.arg("--remount-ro").arg(&hidden_path.dest);
            }
            MountKind::File => {
                let hidden_file = NamedTempFile::new().map_err(|source| BoxRunError::Spawn {
                    program: Path::new("tempfile").to_path_buf(),
                    source,
                })?;
                command
                    .arg("--ro-bind")
                    .arg(hidden_file.path())
                    .arg(&hidden_path.dest);
                artifacts.hidden_files.push(hidden_file);
            }
        }
    }

    Ok(())
}

fn add_mount(command: &mut Command, src: &Path, dest: &Path, kind: MountKind, readonly: bool) {
    create_mount_destination(command, dest, kind);

    let bind_flag = match readonly {
        true => "--ro-bind",
        false => "--bind",
    };

    command.arg(bind_flag).arg(src).arg(dest);
}

fn create_mount_destination(command: &mut Command, dest: &Path, kind: MountKind) {
    match kind {
        MountKind::Dir => {
            command.arg("--dir").arg(dest);
        }
        MountKind::File => {
            if let Some(parent) = dest.parent() {
                command.arg("--dir").arg(parent);
            }
        }
    }
}

fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }

    if let Some(signal) = status.signal() {
        let code = 128u16.saturating_add(signal as u16).min(u8::MAX as u16) as u8;
        return ExitCode::from(code);
    }

    ExitCode::from(1)
}

fn configure_stdio(command: &mut Command, policy: &Policy) -> Result<StdioMode> {
    match policy.tty {
        TtyMode::Auto => {
            command.stdin(stdin_stdio(policy.stdin));
            command.stdout(Stdio::inherit());
            command.stderr(Stdio::inherit());
            Ok(StdioMode::Direct)
        }
        TtyMode::Force => {
            ensure_forced_tty(policy.stdin)?;
            command.stdin(stdin_stdio(policy.stdin));
            command.stdout(Stdio::inherit());
            command.stderr(Stdio::inherit());
            Ok(StdioMode::Direct)
        }
        TtyMode::Disable => {
            match policy.stdin {
                StdinMode::Inherit => command.stdin(Stdio::piped()),
                StdinMode::Null => command.stdin(Stdio::null()),
            };
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());
            Ok(StdioMode::Forwarded {
                copy_stdin: matches!(policy.stdin, StdinMode::Inherit),
            })
        }
    }
}

fn stdin_stdio(mode: StdinMode) -> Stdio {
    match mode {
        StdinMode::Inherit => Stdio::inherit(),
        StdinMode::Null => Stdio::null(),
    }
}

fn ensure_forced_tty(stdin_mode: StdinMode) -> Result<()> {
    let stdin_ok = matches!(stdin_mode, StdinMode::Null) || io::stdin().is_terminal();
    if stdin_ok && io::stdout().is_terminal() && io::stderr().is_terminal() {
        return Ok(());
    }

    Err(BoxRunError::UnsupportedOption {
        message: "--tty force requires the current process to be attached to a terminal".to_owned(),
    })
}

fn spawn_with_forwarded_stdio(
    mut command: Command,
    program: &Path,
    copy_stdin: bool,
) -> Result<std::process::ExitStatus> {
    let mut child = command.spawn().map_err(|source| BoxRunError::Spawn {
        program: program.to_path_buf(),
        source,
    })?;

    if copy_stdin {
        start_stdin_forwarder(&mut child);
    }

    let stdout_thread = child.stdout.take().map(|mut reader| {
        thread::spawn(move || -> std::io::Result<()> {
            let mut writer = io::stdout().lock();
            std::io::copy(&mut reader, &mut writer)?;
            writer.flush()
        })
    });
    let stderr_thread = child.stderr.take().map(|mut reader| {
        thread::spawn(move || -> std::io::Result<()> {
            let mut writer = io::stderr().lock();
            std::io::copy(&mut reader, &mut writer)?;
            writer.flush()
        })
    });

    let status = child.wait().map_err(|source| BoxRunError::Wait {
        program: program.to_path_buf(),
        source,
    })?;

    finish_copy_thread("stdout", stdout_thread)?;
    finish_copy_thread("stderr", stderr_thread)?;
    Ok(status)
}

fn start_stdin_forwarder(child: &mut Child) {
    let Some(mut writer) = child.stdin.take() else {
        return;
    };

    thread::spawn(move || {
        let mut reader = io::stdin().lock();
        let _ = std::io::copy(&mut reader, &mut writer);
        let _ = writer.flush();
    });
}

fn finish_copy_thread(
    stream: &'static str,
    handle: Option<thread::JoinHandle<std::io::Result<()>>>,
) -> Result<()> {
    let Some(handle) = handle else {
        return Ok(());
    };

    match handle.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(source)) => Err(BoxRunError::IoForward { stream, source }),
        Err(_) => Err(BoxRunError::IoForwardPanic { stream }),
    }
}

fn set_no_new_privs() -> Result<()> {
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(BoxRunError::ProcessHardening(
            std::io::Error::last_os_error(),
        ))
    }
}

fn validate_tcp_landlock_support(policy: &Policy) -> Result<()> {
    if !policy.landlock.has_tcp_rules() {
        return Ok(());
    }

    match landlock_support::detect_abi() {
        Some(abi) if abi >= landlock_support::TCP_RULES_MIN_ABI => Ok(()),
        Some(abi) if policy.best_effort => {
            tracing::warn!(
                landlock_abi = abi,
                "TCP allowlists require Landlock ABI {}+, continuing because best-effort mode is enabled",
                landlock_support::TCP_RULES_MIN_ABI
            );
            Ok(())
        }
        Some(abi) => Err(BoxRunError::UnsupportedOption {
            message: format!(
                "TCP allowlists require Landlock ABI {}+, but detected ABI {abi}; rerun with --best-effort to continue without enforcement",
                landlock_support::TCP_RULES_MIN_ABI
            ),
        }),
        None if policy.best_effort => {
            tracing::warn!(
                "TCP allowlists require Landlock support, continuing because best-effort mode is enabled"
            );
            Ok(())
        }
        None => Err(BoxRunError::UnsupportedOption {
            message: format!(
                "TCP allowlists require Landlock ABI {}+, but Landlock is unavailable; rerun with --best-effort to continue without enforcement",
                landlock_support::TCP_RULES_MIN_ABI
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::cli::{FsLayout, NetMode, StdinMode, TtyMode};
    use crate::linux::build_bwrap_command;
    use crate::policy::{EnvPlan, LandlockPlan, Mount, MountKind, Policy};

    #[test]
    fn build_command_includes_expected_linux_flags() {
        let policy = Policy {
            fs_layout: FsLayout::Strict,
            ro_mounts: vec![Mount {
                src: PathBuf::from("/usr"),
                dest: PathBuf::from("/usr"),
                kind: MountKind::Dir,
            }],
            rw_mounts: vec![Mount {
                src: PathBuf::from("/work"),
                dest: PathBuf::from("/work"),
                kind: MountKind::Dir,
            }],
            tmpfs_mounts: vec![PathBuf::from("/tmp")],
            hidden_paths: vec![],
            landlock: LandlockPlan {
                ro_paths: vec![PathBuf::from("/"), PathBuf::from("/proc")],
                rw_paths: vec![
                    PathBuf::from("/dev"),
                    PathBuf::from("/tmp"),
                    PathBuf::from("/work"),
                ],
                tcp_connect_ports: vec![443],
                tcp_bind_ports: vec![8080],
            },
            cwd: PathBuf::from("/work"),
            net: NetMode::None,
            stdin: StdinMode::Inherit,
            tty: TtyMode::Auto,
            env: EnvPlan {
                clear: true,
                vars: vec![("PATH".to_owned(), OsString::from("/usr/bin"))],
            },
            command: vec![
                OsString::from("sh"),
                OsString::from("-c"),
                OsString::from("true"),
            ],
            best_effort: false,
        };

        let mut command = Command::new("bwrap");
        let _artifacts =
            build_bwrap_command(&mut command, Path::new("/host/box-run"), &policy).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.contains(&"--ro-bind".to_owned()));
        assert!(args.contains(&"--unshare-net".to_owned()));
        assert!(args.contains(&"--clearenv".to_owned()));
        assert!(args.contains(&"/tmp/box-run-helper".to_owned()));
        assert!(args.contains(&"/host/box-run".to_owned()));
        assert!(args.contains(&"--landlock-ro".to_owned()));
        assert!(args.contains(&"--landlock-rw".to_owned()));
        assert!(args.contains(&"--landlock-tcp-connect".to_owned()));
        assert!(args.contains(&"--landlock-tcp-bind".to_owned()));
    }
}
