# dnsmasq-cli-rs

**dnsmasq のクエリログを、ターミナル上のライブで検索可能なダッシュボードに。**

`dnsmasq-cli-rs` は dnsmasq が出力するクエリログを tail してローカルの SQLite に取り込み、
キーボード操作の TUI で DNS トラフィックをリアルタイムに眺められるようにするツールです。
ライブのクエリ表示、ドメイン/クライアント別のランキング、ブロック率の統計、部分一致検索、
キー一発のホワイトリスト追加までを備えます。ランタイム依存のない軽量な Rust 単一バイナリで、
小さな VPS や家庭内ゲートウェイに向いています。

[English README is here / 英語版はこちら](README.md)

---

## 目次

- [特徴](#特徴)
- [仕組み](#仕組み)
- [必要要件](#必要要件)
- [ビルド](#ビルド)
- [クイックスタート](#クイックスタート)
- [サブコマンド](#サブコマンド)
- [オプション](#オプション)
- [TUI キーバインド](#tui-キーバインド)
- [ログ形式](#ログ形式)
- [DB スキーマ](#db-スキーマ)
- [ホワイトリスト管理](#ホワイトリスト管理)
- [サービスとして動かす](#サービスとして動かす)
- [トラブルシューティング](#トラブルシューティング)
- [開発](#開発)
- [ライセンス](#ライセンス)

## 特徴

- **ライブ表示** — 解決されたクエリが outcome ごとに色分けされて流れます
  (`blocked` / `forwarded` / `cached` / `reply` / `hosts` など)。
- **履歴を永続化** — すべて SQLite に保存するため、アプリやホストを再起動しても残ります。
- **統計** — ドメイン (ブロック済みを除く) / ブロックされたドメイン / クライアント /
  解決手段 (cached・reply・forwarded…) / クエリ種別の上位を、件数と割合つきで表示。
  1h・24h・7d のブロック率つき。
- **検索とフィルタ** — ドメインの部分一致検索、クライアント絞り込み、ブロックのみ表示。
- **ホワイトリスト管理** — TUI を離れずに選択ドメインをホワイトリストへ追加し dnsmasq を
  リロードできます (OpenRC 環境)。
- **クラッシュ耐性のある取り込み** — イベント挿入とログ tail 位置の更新を 1 トランザクションで
  行うため、クラッシュしても行の重複・欠落が起きません。
- **ローテーション追従** — inode 変化 (ローテーション) と truncate を検知して追従します。
- **単一の静的バイナリ** — 依存は `rusqlite` (bundled SQLite)・`ratatui`・`crossterm` のみ。
  `tokio`・`chrono`・`clap` は使いません。

## 仕組み

```
dnsmasq  (root)
  └─ log-queries=extra + log-facility=/var/log/dnsmasq.log
        │  テキスト行を tail
        ▼
dnsmasq-cli-rs ingest        ← 常駐サービス (非特権ユーザー)
        │  parse → マイクロバッチ (SQLite WAL)
        ▼
/var/lib/dnsmasq-cli-rs/logs.db  (SQLite)
        ▲  read-only
dnsmasq-cli-rs (TUI)         ← SSH 先で起動

管理キー ([w] whitelist / [R] reload) は `sudo -n` 経由で実行:
  - /etc/dnsmasq-cli-rs/whitelist.txt へ追記
  - /etc/dnsmasq.d/adblock.conf から該当行を削除
  - dnsmasq を再起動 (`rc-service`)
```

`ingest` と TUI は別プロセスで、同じ DB を共有します。取り込みはサービスとして常駐させ、
見たいときだけ TUI を開く、という使い方ができます。

## 必要要件

- **Rust 1.85 以上** (edition 2024)。bundled SQLite のビルドに C コンパイラが必要です。
  Alpine: `apk add rust cargo build-base`。Debian/Ubuntu:
  `apt install rustc cargo build-essential`。
- **dnsmasq** がクエリログを出力する設定になっていること ([ログ形式](#ログ形式) 参照)。
- **Linux (musl / glibc)。** リファレンスの配置先は Alpine + OpenRC ですが、ingest と TUI は
  移植可能です。ホワイトリスト / リロードの各キーは OpenRC を前提とします
  ([ホワイトリスト管理](#ホワイトリスト管理) 参照)。
- **terminfo** (TUI を動かすマシン)。最小構成の Alpine では `apk add ncurses-terminfo`。

## ビルド

```bash
cargo build --release          # 通常のリリースビルド
./build.sh                     # musl 静的リリースビルド (crt-static)
TARGET=x86_64-unknown-linux-musl ./build.sh
```

`build.sh` は `--deploy` でリモートホストへ配置できます (`HOST` / `DEST` は上書き可):

```bash
HOST=myhost ./build.sh --deploy
```

インストール手順は特に不要で、バイナリは自己完結しています。対象ホストの
`/usr/local/bin/dnsmasq-cli-rs` にコピーしてください。

## クイックスタート

1. **dnsmasq にクエリログを出力させる。** `/etc/dnsmasq.d/` に以下を追加します:

   ```conf
   log-queries=extra
   log-facility=/var/log/dnsmasq.log
   ```

   追加後、dnsmasq を再起動します。

2. **ログを取り込む** (常駐させます。サービス化は後述):

   ```bash
   dnsmasq-cli-rs ingest
   ```

3. **TUI を開く**:

   ```bash
   dnsmasq-cli-rs
   ```

   既定以外のパスを使う場合は両方に指定します:

   ```bash
   dnsmasq-cli-rs ingest --log ./dnsmasq.log --db ./logs.db
   dnsmasq-cli-rs        --db ./logs.db
   ```

## サブコマンド

| コマンド | 内容 |
| --- | --- |
| `dnsmasq-cli-rs` | TUI (既定)。`tui` を明示しても同じ。 |
| `dnsmasq-cli-rs ingest` | ログを tail して SQLite に取り込む。常駐サービス向け。 |
| `dnsmasq-cli-rs info` | DB 概要 (件数 / 期間 / outcome 内訳) を表示。 |
| `dnsmasq-cli-rs prune` | 指定より古い行を削除。既定は dry-run。 |
| `dnsmasq-cli-rs --help` | ヘルプ。 |
| `dnsmasq-cli-rs --version` | バージョン。 |

## オプション

| オプション | 既定 | 説明 |
| --- | --- | --- |
| `--log <PATH>` | `/var/log/dnsmasq.log` | 取り込む dnsmasq クエリログ。 |
| `--db <PATH>` | `/var/lib/dnsmasq-cli-rs/logs.db` | SQLite DB のパス。 |
| `--retention-days <N>` | `30` | `ingest` が定期削除する保持日数。 |
| `--poll-ms <MS>` | `1000` | `ingest` のポーリング間隔。 |
| `--top <N>` | `10` | TUI / 統計の上位件数。 |
| `--older-than <DUR>` | `30d` | `prune` の対象 (例 `12h`, `90m`, `2w`)。 |
| `--apply` | off | `prune` を実際に実行 (無指定は dry-run)。 |
| `--vacuum` | off | `prune` 後に `VACUUM`。 |

`--log` / `--db` は `--log=PATH` / `--db=PATH` の形式でも指定できます。

例 — 90 日より古い行を削除して領域を回収する:

```bash
dnsmasq-cli-rs prune --older-than 90d            # dry-run。削除対象件数を表示
dnsmasq-cli-rs prune --older-than 90d --apply --vacuum
```

## TUI キーバインド

| キー | 動作 |
| --- | --- |
| `Tab` | **Live** ⇄ **Stats** を切替。 |
| `↑` / `↓` (`k` / `j`) | 選択行を移動。 |
| `PgUp` / `PgDn` | ページ単位で移動。 |
| `Home` / `End` (`g` / `G`) | 先頭 / 末尾へ。 |
| `/` | ドメイン検索 (部分一致・大文字小文字を無視)。 |
| `c` | クライアントで絞り込み。 |
| `b` | ブロック表示を巡回: 全件 → ブロックのみ → ブロックを除外。 |
| `p` | ライブ表示を一時停止 / 再開。 |
| `Esc` | すべてのフィルタをリセット。 |
| `1` / `2` / `3` | 統計期間 1h / 24h / 7d。 |
| `T` | 統計期間を順に切替。 |
| `w` | 選択ドメインをホワイトリストに追加し dnsmasq をリロード (`y/n` 確認)。 |
| `R` | dnsmasq をリロード (`y/n` 確認)。 |
| `q` / `Ctrl-C` | 終了。 |

## ログ形式

取り込みは `log-queries=extra` が出力する形式を前提とします。`dnsmasq[PID]:` 接頭辞は
任意です (syslog を介さずファイルへ直接出力した行も扱えます):

```text
<seq> <client>/<port> query[A] <domain> from <client>
<seq> <client>/<port> forwarded <domain> to <server>
<seq> <client>/<port> reply <domain> is <ip>
<seq> <client>/<port> cached <domain> is <ip|NXDOMAIN|NODATA>
<seq> <client>/<port> config <domain> is <ip>      # 0.0.0.0 / :: ⇒ blocked
<seq> <client>/<port> /etc/hosts <domain> is <ip>
```

知っておくと良い点:

- `query[...]` 行は同じ `seq` を持つ outcome 行が来るまで *pending* として保持され、
  揃った時点で 1 行になります。
- dnsmasq の再起動で `seq` は 1 に戻ります。PID の変化を検知して pending を確定します。
- **専用ログにはタイムスタンプがありません。** 保存される `ts` は *取り込み時刻* であり、
  クエリの発生日時ではありません。これは意図した仕様です。
- イベントの挿入と tail オフセットの更新は **1 トランザクション**で行うため、クラッシュしても
  行の重複・欠落は起きません。
- ローテーション (inode 変化) と truncate (サイズ縮小) を検知して追従します。
- `config` / `reply` の応答が `0.0.0.0` / `::` の場合は `blocked` と判定します
  (広告ブロックリストがドメインを sinkhole する際の典型的な挙動です)。

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
  raw     TEXT                -- 元のログ行
);
-- ts / domain / client / outcome に索引

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
-- tail 位置, schema_version
```

ただの SQLite なので、任意のツールで直接クエリできます:

```bash
sqlite3 /var/lib/dnsmasq-cli-rs/logs.db \
  "SELECT domain, COUNT(*) c FROM queries WHERE outcome='blocked'
   GROUP BY domain ORDER BY c DESC LIMIT 20;"
```

## ホワイトリスト管理

TUI の `w` キーを使うと、ファイルを手で編集せずにドメインのブロックを解除できます:

1. `/etc/dnsmasq-cli-rs/whitelist.txt` にドメインを追記。
2. `/etc/dnsmasq.d/adblock.conf` から該当する `address=/<domain>/...` 行を削除。
3. dnsmasq を再起動し、即時反映。

同じホワイトリストを読む定期更新の広告ブロックリスト生成と組み合わせると便利です
(次回生成時に該当ドメインが除外されます)。同梱の `deploy/setup.sh` は、ホワイトリスト対応の
週次ジェネレータを例としてインストールします。

注意:

- 管理操作はすべて `sudo -n` (非対話) で実行します。パスワードが必要な環境では即座に失敗し、
  ステータス行に表示するため TUI が固まりません。
- リロードは `rc-service dnsmasq restart` を使うため、`w` / `R` は **OpenRC 専用**です。
  ingest・TUI・`info`・`prune` はどのシステムでも動作します。

## サービスとして動かす

Alpine/OpenRC では、サンプルの init スクリプト
[`deploy/dnsmasq-cli-rs.initd`](deploy/dnsmasq-cli-rs.initd) を利用できます。冪等なセットアップ
スクリプト [`deploy/setup.sh`](deploy/setup.sh) は、これに加えて dnsmasq のログ設定・logrotate・
ディレクトリ・ホワイトリスト対応ジェネレータをまとめて導入します:

```bash
sudo SVC_USER=myserviceuser sh deploy/setup.sh
```

サービスは非特権ユーザーで動作します:

```bash
rc-service dnsmasq-cli-rs {start|stop|restart|status}
```

systemd では、ingest を常駐ユニットとして動かせます (`User` とパスは適宜変更):

```ini
[Unit]
Description=dnsmasq query log ingester
After=dnsmasq.service
Requires=dnsmasq.service

[Service]
User=dnsmasq-log
ExecStart=/usr/local/bin/dnsmasq-cli-rs ingest \
  --log /var/log/dnsmasq.log \
  --db /var/lib/dnsmasq-cli-rs/logs.db \
  --retention-days 30
Restart=always

[Install]
WantedBy=multi-user.target
```

権限:

- ingest はログへの **読み取り** と DB ディレクトリへの **書き込み** が必要です。専用の
  非特権ユーザーで動かすのがおすすめです。
- TUI は DB を読むだけなので権限は不要です。`w` / `R` は OpenRC 環境で特定コマンドに対する
  パスワードレス sudo が必要です。

## トラブルシューティング

- **`/var/log/dnsmasq.log` が生成されない** — ログ設定を追加した後に dnsmasq を再起動したか
  確認してください。
- **TUI が文字化けする** — terminfo を導入してください (Alpine: `apk add ncurses-terminfo`)。
- **`w` が `whitelist failed: …` になる** — パスワードレス sudo と
  `/etc/dnsmasq-cli-rs/whitelist.txt` の存在を確認してください。
- **DB を開けない** — `logs.db` (および `-wal` / `-shm`) の所有者を確認してください。TUI と
  ingest が同じファイルにアクセスできる必要があります。
- **行が表示されない** — ログに想定形式の `log-queries=extra` 出力が含まれているか、`ingest`
  が動いているかを確認してください。

## 開発

```bash
cargo build                              # デバッグビルド
cargo build --release                    # リリースビルド
./build.sh                               # musl 静的リリースビルド
```

モジュール構成:

```
src/
  main.rs       エントリ / サブコマンド分岐
  cli.rs        手書き引数パース (clap 不使用)
  model.rs      QueryEvent / Outcome
  parser.rs     dnsmasq 行パーサ (純粋関数)
  store.rs      rusqlite: schema, バッチ挿入と tail 状態の原子的コミット, 集計
  ingest.rs     tail + follow + ローテーション追従 + pending 相関
  util.rs       UTC 時刻整形 / 期間パース
  tui/
    mod.rs      App 状態 + イベントループ + キー処理
    ui.rs       ratatui 描画 (Live / Stats)
    manage.rs   whitelist 追加 / dnsmasq リロード (sudo -n)
```

現時点でテストコードはありません。

## ライセンス

[MIT License](LICENSE) の下で公開しています。
