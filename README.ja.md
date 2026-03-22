# box-run

`box-run` は、`bubblewrap` を使って 1 つのコマンドを軽量なサンドボックスで実行する Linux first な CLI です。

English README: [README.md](README.md)

現在の主な機能:

- ホスト filesystem を既定で read-only 公開
- 現在の作業ディレクトリは read-write で再 mount
- `--ro` / `--rw` による strict filesystem layout
- `--hide` によるファイル・ディレクトリの隠蔽
- `--net none` による network namespace 分離
- 既定で最小限の環境変数だけを渡す clean environment
- `--env-clear`, `--stdin`, `--tty` による process 制御
- helper プロセスでの `PR_SET_NO_NEW_PRIVS` 適用
- strict layout 向けの Landlock filesystem hardening
- Landlock ABI 4+ 環境での TCP bind/connect allowlist
- backend 状態を確認する `doctor`

## インストール

実行時の前提:

- Linux 環境で `bubblewrap` (`bwrap`) が `PATH` 上にあること

代表的な package install 例:

```bash
# Ubuntu / Debian
sudo apt-get install bubblewrap

# Fedora
sudo dnf install bubblewrap

# Arch Linux
sudo pacman -S bubblewrap
```

GitHub から install:

```bash
cargo install --git https://github.com/igtm/box-run box-run
```

local checkout から install:

```bash
cargo build
# or
cargo install --path .
```

install 後は backend 状態を確認できます:

```bash
box-run doctor
```

## 例

```bash
# 既定: host read-only, cwd read-write, network off
./target/debug/box-run run -- make test

# strict root で明示 mount のみ許可
./target/debug/box-run run \
  --fs-layout strict \
  --ro /usr \
  --ro /bin \
  --ro /lib \
  --ro /lib64 \
  --rw "$PWD:/workspace" \
  --cwd /workspace \
  -- python3 script.py

# 環境変数を追加
./target/debug/box-run run --env-pass HOME -- my-command

# stdin を落として TTY も無効化
./target/debug/box-run run --stdin null --tty disable -- my-command

# ファイルやディレクトリを隠す
./target/debug/box-run run --hide /etc/hosts --hide /etc/ssh -- my-command

# Landlock ABI 4+ で outbound TCP を 443 のみに制限
./target/debug/box-run run --net host --allow-tcp-connect 443 -- curl https://example.com
```

## 設定ファイル例

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

## サポート方針

- Linux は実運用向け backend を実装済み
- macOS / Windows / その他 OS は、現状 `doctor` と明示的な unsupported message のみ

TCP allowlist は Landlock ABI 4+ でのみ enforcement されます。古い kernel では `--best-effort` を付けると警告付きで継続します。

設計メモは [docs/box-run-spec.md](docs/box-run-spec.md) にあります。
