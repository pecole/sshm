# sshm

SSH接続マネージャー。`~/.ssh/config` をパースし、fzf風のファジー選択TUIで接続。ホストの追加・編集・削除もCLIから。

## ビルド

```sh
cargo build --release
cp target/release/sshm ~/.local/bin/   # PATHの通った場所へ
```

## 使い方

```sh
sshm                # ファジー選択して接続（デフォルト）
sshm connect web1   # エイリアス直指定で接続
sshm list           # ホスト一覧
sshm add            # 対話形式でホスト追加
sshm edit [alias]   # ホスト編集（省略時はピッカー）
sshm remove [alias] # ホスト削除（確認あり）
```

## 仕様メモ

- `~/.ssh/config` を直接読み書き。書き込み前に `config.bak.<timestamp>` へ自動バックアップ
- ワイルドカードHost（`*` など）は一覧から除外（設定ファイル上は保持される）
- `Host a b c` のように複数エイリアスを持つブロックは、一覧・接続はOKだが edit/remove は安全のため拒否（手動編集を案内）
- 接続は `exec ssh <alias>` でプロセス置換するので、ssh_config の全オプション（ProxyJump等）がそのまま効く
