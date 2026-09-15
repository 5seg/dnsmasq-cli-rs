# AGENTS.md

Context & instructions for AI coding agents working on `dnsmasq-cli-rs`.
Cross-repository guidelines: see `../AGENTS.md`.

> [!NOTE]
> Single source of truth: `README.md` for spec/operations, code comments for
> protocol/format details. Keep this file focused on agent-specific gotchas.

---

## 1. Overview

`dnsmasq-cli-rs`: Alpine/OpenRC 上の dnsmasq クエリログを取り込み (ingest)、
SQLite に保存し、TUI で閲覧・統計・ホワイトリスト管理する単一バイナリ。

- Runtime: Rust (edition 2024)。Alpine ネイティブ musl、`crt-static` で完全静的ビルド。
- Deps: `ratatui` + `crossterm` (TUI)、`rusqlite` (bundled SQLite)、`anyhow`。
  tokio / chrono / clap は**使わない** (軽量・単一バイナリ重視)。
- 実行場所: バイナリは VPS (`vlproxy`) に配置し、`ssh` 先で TUI を起動する。

## 2. Commands

| Command | Description |
| --- | --- |
| `cargo build` | デバッグビルド |
| `cargo build --release` | リリースビルド (静的化なし) |
| `./build.sh` | `crt-static` 付き静的リリースビルド |
| `./build.sh --deploy` | ビルド + `vlproxy:/usr/local/bin/` へ配置 |
| `ssh vlproxy 'sudo sh /tmp/setup.sh'` | VPS 側の冪等セットアップ |

> [!IMPORTANT]
> 変更後は `cargo build` が warnings 無しで通ることを確認する。
> テストコードは追加しない (グローバル `../AGENTS.md` の指示)。

## 3. Module Layout

```
src/
  main.rs       エントリ / サブコマンド分岐 (info, prune)
  cli.rs        手書き引数パース (clap 不使用)
  model.rs      QueryEvent / Outcome
  parser.rs     dnsmasq 行パーサ (純粋関数)
  store.rs      rusqlite: schema, バッチ挿入と tail 状態の原子的コミット, 集計
  ingest.rs     tail + follow + ローテーション追従 + pending 相関
  util.rs       時刻整形 (TZ オフセット) / 期間・オフセットパース
  tui/
    mod.rs      App 状態 + イベントループ + キー処理
    ui.rs       ratatui 描画 (Live / Stats)
    manage.rs   whitelist 追加 / dnsmasq リロード (sudo -n)
deploy/
  setup.sh      VPS 側セットアップ (自己完結・冪等)
```

## 4. Gotchas

- **取り込みの正しさ**: `store::commit_batch` は「イベント挿入」と「tail オフセット更新」を
  1 トランザクションで行う。この不変条件を崩すと重複/欠落が起きる。
  保存する offset は pending が指す最も古い行の先頭 (`ingest.rs` の `safe`) であること。
- **seq は dnsmasq 再起動で 1 に戻る**: PID 変化を検知して pending をフラッシュする。
- **ログにタイムスタンプが無い**: `ts` は取り込み時刻。`spec` 上の制約であり仕様。
  したがって ingest が止まっていた区間の行は、再開時刻でまとめて記録され、日別グラフに
  偽のスパイクが出る。これは直せない (発生源に時刻が無い) ので、止めないことが重要。
- **ログの読み取り権限**: dnsmasq は `/var/log/dnsmasq.log` を `dnsmasq:dnsmasq 0640` で
  作る。取り込みユーザーが `dnsmasq` グループに入っていないと読めず ingest が停止する。
  `setup.sh` のグループ追加を消さないこと。`ingest.rs` は開けない間 1 回だけ警告を出す。
- **表示は UTC 既定**: `util.rs` の `TZ_OFFSET_MS` (既定 0) を `--tz-offset` /
  `DNSMASQ_CLI_RS_TZ_OFFSET` で設定し、`breakdown` と `store::daily` に適用する。
  DB の `ts` は UTC のまま。日別集計の `day_ms` は「ローカル日の先頭に対応する UTC millis」
  (`day * 86_400_000 - offset`) で、`fmt_date` が再びオフセットを足す前提。
- **ブロック判定**: `config`/`reply` の応答が `0.0.0.0` / `::` なら `blocked`。
- **バージョン**: ratatui 0.30 は `crossterm` 0.29 を内部利用。二重依存を避けるため
  直接依存も 0.29 に合わせる。
- **crt-static**: `RUSTFLAGS` で全体に付けると proc-macro が壊れる。`build.sh` の通り
  ターゲット個別の `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` で付けること。
- **管理操作は `sudo -n`**: 対話 sudo を避け、TUI をブロックしない。パスワードが必要な
  環境では即失敗してステータス行に出る。
- **whitelist 連携**: `deploy/setup.sh` が `update-adblock.sh` を whitelist 対応版に
  書き換える。手で編集する場合は 2 ファイル awk (`FNR==NR`) のイディオムを保つ
  (busybox awk で `getline < file` に依存しないため)。

## 5. Verification

- ローカル: 合成ログで `ingest` → `info` を確認。`script -qec "stty rows 40 cols 140; ..."` で
  TUI の描画を非対話確認できる (`--db` でテスト用 DB を指定)。
- VPS: `setup.sh` 後に `getent hosts <domain>` でブロック/解除を確認し、
  `dnsmasq-cli-rs info` で取り込みを確認する。
