# box-run 仕様書ドラフト

## 1. 目的

`box-run` は、任意のコマンドを「壊しにくい」「漏らしにくい」実行環境で起動するための Rust 製 CLI ツールである。

主な用途:

- ビルドスクリプトや生成系コマンドの安全な試行
- 信頼度の低いツールやエージェントの実行制約
- CI やローカル開発での再現性の高いプロセス実行

このツールはコンテナランタイムの代替ではなく、単発のプロセス実行を軽量に囲うことを目的とする。

## 2. 脅威モデル

`box-run` が守りたいもの:

- ホストファイルの意図しない変更
- 外部ネットワークへの意図しない接続
- ホスト環境変数の意図しない継承
- 子プロセスからのプロセスツリー暴走

`box-run` が v1 で守らないもの:

- カーネル脆弱性や権限昇格 exploit への耐性
- root 権限や管理者権限を持つ攻撃者からの防御
- ホスト上の全秘密情報の完全秘匿
- ドメイン単位の厳密な egress 制御

重要な前提:

- 「ホスト全体を read-only で見せる」構成は破壊防止には有効だが、閲覧防止には不十分である。
- より強い隔離が必要な場合は、空の rootfs に明示 bind する strict モードを使う。

## 3. サポート方針

### v1

- Linux を first-class support とする
- `bubblewrap` を利用したラッパー実装を採用する
- Landlock は利用可能な場合のみ best-effort で追加適用する

### v2 以降

- Linux ネイティブ実装への移行を検討する
- seccomp を任意 hardening として追加する
- プロキシベースのドメイン許可を追加する

### macOS / Windows

v1 では正式サポートしない。理由は以下。

- macOS の公開ドキュメント上の App Sandbox は、署名済み app bundle と entitlement を前提としている
- Windows の AppContainer は有力だが、Job Object や restricted token と組み合わせた設計が Linux より重い
- どちらも「任意コマンドを ad-hoc に包む CLI」としては、Linux より先に安定化させる価値が低い

方針としては、v1 で無理に cross-platform を名乗らず、`doctor` で未対応を明示する。

## 4. UX と CLI

基本形:

```bash
box-run run [OPTIONS] -- <command> [args...]
```

将来の補助コマンド:

```bash
box-run doctor
box-run explain --policy sandbox.toml
```

### 実用的なデフォルト

`box-run run -- <cmd>` のデフォルトは次とする。

- ネットワーク: 無効
- 環境変数: `env_clear`
- ファイルシステム: `host-ro` プロファイル
- 作業ディレクトリ: 現在の `cwd` を read-write bind
- 子プロセス終了: 親終了時に巻き添えで終了

`host-ro` は利便性優先のため、ホスト全体を read-only で露出する。
秘密情報の読み取りまで防ぎたい場合は `--fs-layout strict` を使う。

### 主なオプション案

```bash
# 完全な既定動作
box-run run -- make test

# strict ルートで /usr と workspace だけ見せる
box-run run \
  --fs-layout strict \
  --ro /usr \
  --ro /bin \
  --ro /lib \
  --ro /lib64 \
  --rw "$PWD:/workspace" \
  --cwd /workspace \
  -- python script.py

# 環境変数を最小限だけ渡す
box-run run --env-pass TERM --env-pass PATH --env API_KEY=dummy -- node app.js

# ネットワークを host に戻す
box-run run --net host -- cargo test
```

### オプション詳細

- `--fs-layout <host-ro|strict>`
- `--ro <src[:dest]>`
- `--rw <src[:dest]>`
- `--tmpfs <dest>`
- `--hide <dest>`
- `--cwd <path>`
- `--net <none|host>`
- `--allow-tcp-connect <port>`
- `--allow-tcp-bind <port>`
- `--env KEY=VALUE`
- `--env-pass KEY`
- `--env-clear`
- `--stdin <inherit|null>`
- `--tty <auto|force|disable>`
- `--best-effort`

備考:

- `--allow-tcp-connect` と `--allow-tcp-bind` は Linux + Landlock ABI 対応時のみ有効
- ドメイン許可は v1 に入れない。名前解決と接続先検証が必要で、単なる sandbox backend の責務を超えるため

## 5. 設定ファイル

CLI と等価な `TOML` 設定を読めるようにする。

```toml
[fs]
layout = "strict"
ro = ["/usr", "/bin", "/lib", "/lib64"]
rw = ["./workspace:/workspace"]
tmpfs = ["/tmp"]
hide = ["/home/me/.ssh", "/home/me/.aws"]

[net]
mode = "none"
allow_tcp_connect = []
allow_tcp_bind = []

[env]
clear = true
pass = ["TERM", "PATH"]

[process]
cwd = "/workspace"
tty = "auto"
```

優先順位は `CLI > config file > default`。

## 6. 実行モデル

### 6.1 高レベルフロー

1. CLI で policy を構築
2. backend capability を検出
3. policy を backend 向けに compile
4. sandbox 内部で helper を起動
5. helper が追加 hardening を適用して target command を `exec`

### 6.2 Linux v1 backend

`bubblewrap` を使って以下を構成する。

- mount namespace
- user namespace
- pid namespace
- ipc namespace
- uts namespace
- optional network namespace
- `/proc`, `/dev`, `tmpfs` の最小構成
- `--die-with-parent`
- `--new-session`

代表的な `bwrap` 引数イメージ:

```text
bwrap
  --unshare-user
  --unshare-pid
  --unshare-ipc
  --unshare-uts
  --new-session
  --die-with-parent
  --proc /proc
  --dev /dev
  ...
  -- <box-run internal helper> -- <target>
```

### 6.3 helper プロセスの責務

helper は sandbox 内で最後に実行される薄い Rust バイナリまたは内部サブコマンドで、以下を担当する。

- `PR_SET_NO_NEW_PRIVS`
- Landlock の best-effort 適用
- 将来の seccomp 適用
- 環境変数最終調整
- ターゲットコマンドへの `exec`

これにより、mount namespace 構築と LSM/seccomp 適用を分離できる。

## 7. capability モデル

backends は「できること」を宣言し、policy compile 時に検証する。

```rust
struct BackendCapabilities {
    fs_host_ro: bool,
    fs_strict: bool,
    hide_paths: bool,
    net_none: bool,
    landlock_fs: bool,
    landlock_net_ports: bool,
    seccomp: bool,
}
```

失敗方針:

- 未対応オプションは通常エラー
- `--best-effort` 指定時のみ、警告を出して弱い構成へフォールバック

## 8. モジュール構成案

単一 crate で始めてもよいが、早めに責務分離した方が後で楽になる。

```text
crates/
  box-run-cli/        # clap, entrypoint
  box-run-policy/     # policy 型, config load, validation
  box-run-backend/    # trait, capability negotiation
  box-run-linux/      # bwrap command builder, helper, Landlock
```

小さく始めるなら `box-run-cli` と `box-run-linux` を一つにまとめてもよい。

## 9. 推奨クレート

### 必須

- `clap`: CLI 定義
- `serde`: policy 構造体
- `toml`: 設定ファイル
- `tracing`, `tracing-subscriber`: ログ
- `thiserror`: エラー型
- `which`: `bwrap` 検出
- `tempfile`: 一時ファイルや一時ディレクトリ

### Linux 実装

- `landlock`: Landlock ルール適用
- `rustix`: `prctl` や低レベル syscall 系の薄いラッパー
- `seccompiler`: seccomp 導入時の候補

`nix` でも実装できるが、v1 は `bwrap` ラッパー寄りなので `rustix` の方が依存を抑えやすい。

### 将来機能

- `tokio`: プロキシや非同期入出力が必要になったら導入
- `hyper`: HTTP proxy 実装時
- `windows`: Windows backend 導入時

重要:

- v1 では `tokio` を前提にしない
- v1 では `libc` 直叩きを避け、必要最低限のみ `rustix` か既存 crate に寄せる

## 10. 実装ロードマップ

### Phase 1: Linux wrapper MVP

- `box-run run -- <cmd>` を実装
- `host-ro` と `strict` の 2 レイアウトを実装
- `--ro`, `--rw`, `--tmpfs`, `--cwd`, `--env*`, `--net none|host` を実装
- exit code, signal, stdio, tty を自然に透過させる
- `doctor` で `bwrap` と Landlock 可否を表示

### Phase 2: Hardening

- helper 経由の `PR_SET_NO_NEW_PRIVS`
- Landlock filesystem 制約
- Landlock TCP port 制約
- `--best-effort` と capability warning

### Phase 3: Optional proxy mode

- `--net proxy`
- 明示 allowlist された domain/port のみ許可
- DNS と接続先検証を proxy 側で実施

### Phase 4: Native backend 検討

- `bwrap` 非依存の Linux 実装を PoC
- 保守負荷と監査容易性を比較し、置換か併存かを決める

## 11. Gemini 案からの修正点

- cross-platform を v1 の目標から外した
- macOS の `sandbox-exec` 前提を採用しない
- Windows backend は Job Object と restricted token の設計検証を先送りした
- ドメイン許可を core sandbox ではなく proxy mode に分離した
- Landlock は filesystem 専用ではなく、対応 kernel では TCP port 制約にも使う
- `tokio` は v1 必須ではない
- threat model と capability negotiation を明示した

## 12. 最低限の受け入れ基準

- `box-run run -- sh -c 'touch /etc/box-run-test'` が失敗し、ホスト `/etc` を書き換えない
- `box-run run --net none -- curl https://example.com` が失敗する
- `box-run run --env-clear -- env` にホスト秘密情報が出ない
- `box-run run -- false` の exit code が 1 で返る
- `box-run doctor` が backend と制限事項を説明できる
