# box-run

`box-run` is a Linux-first CLI for running one command inside a lightweight sandbox backed by `bubblewrap`.

Japanese README: [README.ja.md](README.ja.md)

Current MVP features:

- host filesystem exposed read-only by default
- current working directory re-mounted read-write
- optional strict filesystem layout with explicit `--ro` / `--rw` mounts
- hidden path overlays with `--hide`
- network namespace isolation with `--net none`
- clean environment by default with a small allowlist (`PATH`, `TERM`, locale vars)
- explicit `--env-clear`, `--stdin`, and `--tty` process controls
- hidden helper process that applies `PR_SET_NO_NEW_PRIVS` before `exec`
- Landlock filesystem rules applied for `strict` filesystem layouts on a best-effort basis
- optional TCP bind/connect allowlists backed by Landlock on kernels with ABI 4+
- `doctor` command for backend inspection

## Install

Runtime requirement:

- Linux with `bubblewrap` (`bwrap`) available on `PATH`

Common package examples:

```bash
# Ubuntu / Debian
sudo apt-get install bubblewrap

# Fedora
sudo dnf install bubblewrap

# Arch Linux
sudo pacman -S bubblewrap
```

Install from Git:

```bash
cargo install --git https://github.com/igtm/box-run box-run
```

Install from a local checkout:

```bash
cargo build
# or
cargo install --path .
```

After installation, verify the backend:

```bash
box-run doctor
```

## Examples

```bash
# Default sandbox: host read-only, cwd read-write, network off
./target/debug/box-run run -- make test

# Strict root with explicit mounts
./target/debug/box-run run \
  --fs-layout strict \
  --ro /usr \
  --ro /bin \
  --ro /lib \
  --ro /lib64 \
  --rw "$PWD:/workspace" \
  --cwd /workspace \
  -- python3 script.py

# Pass one extra environment variable
./target/debug/box-run run --env-pass HOME -- my-command

# Drop stdin and disable TTY detection inside the sandbox
./target/debug/box-run run --stdin null --tty disable -- my-command

# Hide a file or directory inside the sandbox
./target/debug/box-run run --hide /etc/hosts --hide /etc/ssh -- my-command

# Allow outbound TCP only to port 443 when Landlock ABI 4+ is available
./target/debug/box-run run --net host --allow-tcp-connect 443 -- curl https://example.com

# Load defaults from a TOML file
./target/debug/box-run run --config sandbox.toml

# Inspect backend availability
./target/debug/box-run doctor
```

Example `sandbox.toml`:

```toml
best_effort = true
command = ["python3", "script.py"]

[fs]
layout = "strict"
ro = ["/usr", "/bin", "/lib", "/lib64"]
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
API_KEY = "dummy"

[process]
cwd = "/workspace"
stdin = "inherit"
tty = "auto"
```

## Platform Support

- Linux: fully supported backend
- macOS: builds and `doctor` works, but sandbox backend is not implemented yet
- Windows: builds and `doctor` works, but sandbox backend is not implemented yet

TCP allowlists are enforced only when Landlock ABI 4+ is available. On older kernels, `--best-effort`
keeps the command runnable and logs that the TCP policy could not be enforced.

Design notes live in [docs/box-run-spec.md](docs/box-run-spec.md).
