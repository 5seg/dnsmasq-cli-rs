# dnsmasq-cli-rs

Alpine 上の dnsmasq クエリログを SQLite に取り込み、ターミナル (TUI) で
ライブ表示・統計・検索・ホワイトリスト管理を行うツール。Rust 製の単一静的バイナリ。

VPS (`vlproxy`) に配置して `ssh vlproxy` した先で TUI を起動する使い方を想定している。

## 全体像

```
dnsmasq (root)
  └─ log-queries=extra + log-facility=/var/log/dnsmasq.log
        │  (テキスト行を tail)
        ▼
dnsmasq-cli-rs ingest        ← OpenRC 常駐サービス (command_user=user)
        │  parse → マイクロバッチ (SQLite WAL)
        ▼
/var/lib/dnsmasq-cli-rs/logs.db (SQLite)
        ▲  read-only
dnsmasq-cli-rs (TUI, user)   ← ssh 先で起動

管理操作 ([w] whitelist / [R] reload) は sudo -n 経由で:
  /etc/dnsmasq-cli-rs/whitelist.txt に追記
  /etc/dnsmasq.d/adblock.conf から該当行を削除
  rc-service dnsmasq restart
```

## 必要要件

- Rust 1.85+ (`rust` / `cargo`)。Alpine なら `doas apk add rust cargo build-base`。
  bundled SQLite のビルドに C コンパイラが必要。
- VPS: Alpine + OpenRC、dnsmasq、`user` のパスワードレス sudo。

## ビルド

```bash
./build.sh            # ホスト (musl) 向け静的リリースビルド
./build.sh --deploy   # ビルドして vlproxy:/usr/local/bin/dnsmasq-cli-rs に配置
```

VPS 側の初期セットアップ (冪等):

```bash
scp deploy/setup.sh vlproxy:/tmp/
ssh vlproxy 'sudo sh /tmp/setup.sh'
```

## サブコマンド

| コマンド | 内容 |
| --- | --- |
| `dnsmasq-cli-rs` | TUI (既定) |
| `dnsmasq-cli-rs ingest` | ログを tail して SQLite に取り込む (常駐サービス用) |
| `dnsmasq-cli-rs info` | DB 概要 (件数 / 期間 / outcome 内訳) |
| `dnsmasq-cli-rs prune` | 保持期間を超えた行を削除 (既定は dry-run) |

主なオプション:

| オプション | 既定 | 説明 |
| --- | --- | --- |
| `--log <PATH>` | `/var/log/dnsmasq.log` | dnsmasq クエリログ |
| `--db <PATH>` | `/var/lib/dnsmasq-cli-rs/logs.db` | SQLite DB |
| `--retention-days <N>` | `30` | 保持日数 (ingest が定期削除) |
| `--poll-ms <MS>` | `1000` | ingest のポーリング間隔 |
| `--top <N>` | `10` | 統計/TUI の上位件数 |
| `--older-than <DUR>` | `30d` | prune 対象 (例 `12h`, `90m`) |
| `--apply` / `--vacuum` | — | prune を実行 / 実行後に VACUUM |

## TUI キーバインド

| キー | 動作 |
| --- | --- |
| `Tab` | Live ⇄ Stats |
| `↑↓` / `PgUp` `PgDn` / `Home` `End` | スクロール・選択 |
| `/` | ドメイン検索 (前方一致ではなく部分一致) |
| `c` | クライアントで絞り込み |
| `b` | ブロック行のみ |
| `p` | 一時停止 (取り込み表示を止める) |
| `Esc` | フィルタをリセット |
| `1` `2` `3` | 統計期間 1h / 24h / 7d |
| `w` | 選択行のドメインを whitelist に追加 + dnsmasq リロード |
| `R` | dnsmasq をリロード |
| `q` | 終了 |

## ログ形式 (取り込み仕様)

`log-queries=extra` を前提に以下を解釈する (`dnsmasq[PID]:` 接頭辞は任意):

```
<seq> <client>/<port> query[A] <domain> from <client>
<seq> <client>/<port> forwarded <domain> to <server>
<seq> <client>/<port> reply <domain> is <ip>
<seq> <client>/<port> cached <domain> is <ip|NXDOMAIN|NODATA>
<seq> <client>/<port> config <domain> is <ip>      # 0.0.0.0 / :: は blocked
<seq> <client>/<port> /etc/hosts <domain> is <ip>
```

- `query[...]` 到着時に pending に積み、対応する outcome 行で確定する。
- dnsmasq の PID が変わったら (再起動) pending を確定する (seq がリセットされるため)。
- ログ行にタイムスタンプが無いため、**取り込み時刻**を `ts` として保存する。
- 挿入と tail オフセット更新を 1 トランザクションで行い、クラッシュ時も重複・欠落しない。
- ローテーション (inode 変化) と truncate (サイズ縮小) を検知して追従する。

## DB スキーマ

```sql
CREATE TABLE queries (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts      INTEGER NOT NULL,   -- unix millis (取り込み時刻)
  pid     INTEGER,            -- dnsmasq PID
  seq     INTEGER,            -- log-queries=extra の連番
  client  TEXT    NOT NULL,
  domain  TEXT    NOT NULL,
  qtype   TEXT,
  outcome TEXT    NOT NULL,   -- blocked|forwarded|cached|reply|hosts|unknown|...
  answer  TEXT,
  raw     TEXT
);
-- ts / domain / client / outcome に索引
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);  -- tail 位置, schema_version
```

## ホワイトリストの仕組み

- `/etc/dnsmasq-cli-rs/whitelist.txt` に 1 行 1 ドメインで記載する。
- 週次の `/etc/periodic/weekly/update-adblock.sh` がこのファイルを読み、
  該当ドメインを `adblock.conf` から除外する。
- TUI の `w` は、whitelist への追記 + `adblock.conf` の該当行削除 + dnsmasq リロードを
  まとめて行うため、次回の週次更新を待たずに即時解除される。

## 運用

- 取り込みサービス: `rc-service dnsmasq-cli-rs {start,stop,restart,status}`。
- ログ: `/var/lib/dnsmasq-cli-rs/ingest.log` (サービスの出力)。
- 手動 prune (dry-run → 実行):

  ```bash
  dnsmasq-cli-rs prune --older-than 30d
  dnsmasq-cli-rs prune --older-than 30d --apply --vacuum
  ```

- 権限: DB は `user` 所有。TUI は `user` で実行する (sudo 不要)。
  管理操作 (`w` / `R`) のみ `sudo -n` を使うため、`user` にパスワードレス sudo が必要。

## トラブルシューティング

- **`/var/log/dnsmasq.log` が生成されない**: `logging.conf` 追加後に dnsmasq を再起動したか確認。
- **TUI が文字化けする**: VPS に terminfo が無い場合は `apk add ncurses-terminfo`。
- **`w` が `whitelist failed: ...` になる**: `user` のパスワードレス sudo を確認。
- **DB が読めない**: `logs.db` と `-wal`/`-shm` の所有者が `user` か確認。

## 開発

| コマンド | 内容 |
| --- | --- |
| `cargo build` | デバッグビルド |
| `./build.sh` | 静的リリースビルド |
| `cargo build --release` | (静的化なしの) リリースビルド |

方針としてテストコードは追加しない (グローバル `../AGENTS.md` の指示)。
