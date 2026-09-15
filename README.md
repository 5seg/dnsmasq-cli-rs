# dnsmasq-cli-rs

**Turn your dnsmasq query log into a live, searchable dashboard — right in the terminal.**

`dnsmasq-cli-rs` tails the query log produced by `dnsmasq`, ingests it into a local
SQLite database, and gives you a keyboard-driven TUI for watching DNS traffic in real
time: live queries, top domains and clients, block-rate statistics, substring search,
and one-keystroke whitelisting. It is a single, lightweight Rust binary with no runtime
dependencies — a good fit for a small VPS or a home gateway.

[日本語版の README はこちら / Japanese README](README_JA.md)

---

## Table of contents

- [Features](#features)
- [How it works](#how-it-works)
- [Requirements](#requirements)
- [Build](#build)
- [Quick start](#quick-start)
- [Commands](#commands)
- [Options](#options)
- [TUI key bindings](#tui-key-bindings)
- [Log format](#log-format)
- [Database schema](#database-schema)
- [Whitelist management](#whitelist-management)
- [Running as a service](#running-as-a-service)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [License](#license)

## Features

- **Live query stream** — resolutions appear as they happen, color-coded by outcome
  (`blocked`, `forwarded`, `cached`, `reply`, `hosts`, …).
- **Persistent history** — every query is stored in SQLite, so the view survives
  restarts of both the app and the host.
- **Statistics** — top domains (excluding blocked), top blocked domains, top clients,
  resolution outcomes (cached/reply/forwarded/…), and query types, each with counts and
  percentages, plus the block rate over a 1 h / 24 h / 7 d window.
- **Daily breakdown** — a per-day stacked bar chart for the last 7 days with query
  counts (cached/reply/forwarded) and blocked counts, each beside a per-day numeric
  list (newest first).
- **Search and filters** — substring domain search, client filter, and a
  blocked-only toggle, all applied instantly.
- **Whitelist management** — add the selected domain to a whitelist and reload dnsmasq
  without leaving the TUI (OpenRC systems).
- **Crash-safe ingestion** — events and the log-tail offset are committed in a single
  transaction, so a crash never duplicates or drops lines.
- **Rotation aware** — follows log rotation (inode change) and truncation.
- **One static binary** — `rusqlite` (bundled SQLite), `ratatui`, and `crossterm` only.
  No `tokio`, no `chrono`, no `clap`.

## How it works

```
dnsmasq  (root)
  └─ log-queries=extra + log-facility=/var/log/dnsmasq.log
        │  text lines, tailed
        ▼
dnsmasq-cli-rs ingest        ← long-running service (unprivileged user)
        │  parse → micro-batches (SQLite WAL)
        ▼
/var/lib/dnsmasq-cli-rs/logs.db  (SQLite)
        ▲  read-only
dnsmasq-cli-rs (TUI)         ← run over SSH

Management keys ([w] whitelist / [R] reload) shell out via `sudo -n`:
  - append to /etc/dnsmasq-cli-rs/whitelist.txt
  - remove matching lines from /etc/dnsmasq.d/adblock.conf
  - restart dnsmasq (`rc-service`)
```

`ingest` and the TUI are separate processes sharing one database. You can run the
ingester as a service and open the TUI only when you want to look at something.

## Requirements

- **Rust 1.85 or newer** (edition 2024) with a C compiler for the bundled SQLite.
  On Alpine: `apk add rust cargo build-base`. On Debian/Ubuntu:
  `apt install rustc cargo build-essential`.
- **dnsmasq** configured to log queries (see [Log format](#log-format)).
- **Linux/musl or glibc.** The reference deployment is Alpine + OpenRC, but the
  ingester and TUI are portable. The whitelist/reload keys assume OpenRC (see
  [Whitelist management](#whitelist-management)).
- **terminfo** on the machine where you run the TUI (on minimal Alpine:
  `apk add ncurses-terminfo`).

## Build

```bash
cargo build --release          # standard release build
./build.sh                     # static musl release build (crt-static)
TARGET=x86_64-unknown-linux-musl ./build.sh
```

`build.sh` also supports `--deploy` to copy the binary to a remote host
(`HOST`, `DEST` are overridable):

```bash
HOST=myhost ./build.sh --deploy
```

No installation step is required; the binary is self-contained. Copy it to
`/usr/local/bin/dnsmasq-cli-rs` on the target host.

## Quick start

1. **Make dnsmasq log its queries** to a dedicated file. Add to `/etc/dnsmasq.d/`:

   ```conf
   log-queries=extra
   log-facility=/var/log/dnsmasq.log
   ```

   Then restart dnsmasq.

2. **Ingest the log** (leave it running, e.g. under a service):

   ```bash
   dnsmasq-cli-rs ingest
   ```

3. **Open the TUI**:

   ```bash
   dnsmasq-cli-rs
   ```

   Point both at non-default paths if you like:

   ```bash
   dnsmasq-cli-rs ingest --log ./dnsmasq.log --db ./logs.db
   dnsmasq-cli-rs        --db ./logs.db
   ```

## Commands

| Command | Description |
| --- | --- |
| `dnsmasq-cli-rs` | TUI (default). `tui` also works explicitly. |
| `dnsmasq-cli-rs ingest` | Tail the log and ingest into SQLite. Meant to run as a service. |
| `dnsmasq-cli-rs info` | Print DB summary: row count, time range, outcome breakdown. |
| `dnsmasq-cli-rs prune` | Delete rows older than a cutoff. Dry-run by default. |
| `dnsmasq-cli-rs --help` | Usage. |
| `dnsmasq-cli-rs --version` | Version. |

## Options

| Option | Default | Description |
| --- | --- | --- |
| `--log <PATH>` | `/var/log/dnsmasq.log` | dnsmasq query log to ingest. |
| `--db <PATH>` | `/var/lib/dnsmasq-cli-rs/logs.db` | SQLite database path. |
| `--retention-days <N>` | `30` | Rows older than this are deleted by `ingest`. |
| `--poll-ms <MS>` | `1000` | `ingest` polling interval. |
| `--top <N>` | `10` | Number of entries in TUI/stats rankings. |
| `--tz-offset <OFFSET>` | `utc` | Local time offset for display (`utc` / `+9` / `-5` / `+05:30`). |
| `--older-than <DUR>` | `30d` | `prune` cutoff (e.g. `12h`, `90m`, `2w`). |
| `--apply` | off | Actually perform the `prune` (otherwise dry-run). |
| `--vacuum` | off | Run `VACUUM` after `prune`. |

`--log`, `--db`, and `--tz-offset` also accept the `--log=PATH` / `--db=PATH` /
`--tz-offset=+9` form.

Example — prune everything older than 90 days and reclaim space:

```bash
dnsmasq-cli-rs prune --older-than 90d            # dry-run, shows what would go
dnsmasq-cli-rs prune --older-than 90d --apply --vacuum
```

## TUI key bindings

| Key | Action |
| --- | --- |
| `Tab` | Cycle tabs: **Live** → **Stats** → **Daily**. |
| `↑` / `↓` (or `k` / `j`) | Move selection. |
| `PgUp` / `PgDn` | Page up / down. |
| `Home` / `End` (or `g` / `G`) | Jump to top / bottom. |
| `/` | Search domains (substring, case-insensitive). |
| `c` | Filter by client. |
| `b` | Cycle the block filter: all → blocked only → exclude blocked. |
| `p` | Pause/resume the live feed. |
| `Esc` | Reset all filters. |
| `1` / `2` / `3` | Stats window: 1 h / 24 h / 7 d. |
| `T` | Cycle the stats window. |
| `w` | Whitelist the selected domain and reload dnsmasq (asks `y/n`). |
| `R` | Reload dnsmasq (asks `y/n`). |
| `q` / `Ctrl-C` | Quit. |

## Log format

Ingestion expects the format produced by `log-queries=extra`. The `dnsmasq[PID]:`
prefix is optional (lines logged directly to a file without syslog also work):

```text
<seq> <client>/<port> query[A] <domain> from <client>
<seq> <client>/<port> forwarded <domain> to <server>
<seq> <client>/<port> reply <domain> is <ip>
<seq> <client>/<port> cached <domain> is <ip|NXDOMAIN|NODATA>
<seq> <client>/<port> config <domain> is <ip>      # 0.0.0.0 / :: ⇒ blocked
<seq> <client>/<port> /etc/hosts <domain> is <ip>
```

Details worth knowing:

- A `query[...]` line is held as *pending* until its corresponding outcome line
  (same `seq`) arrives; the pair becomes one row.
- `seq` resets to 1 when dnsmasq restarts, so a change of PID flushes pending queries.
- **The dedicated dnsmasq log has no timestamps**, so the stored `ts` is the *ingest
  time*, not the time the query was made. This is a deliberate trade-off. Lines that
  accumulated while `ingest` was stopped are all stamped with the moment it resumed, so
  the daily chart shows a spike where none really happened (`ts` is UTC epoch millis).
- **Display defaults to UTC.** Pass `--tz-offset +9` (or set
  `DNSMASQ_CLI_RS_TZ_OFFSET=+9`) to render TUI / `info` times and the daily
  buckets in local time. The stored `ts` stays UTC, so this is safe to change at any
  time.
- Insertion of events and the update of the tail offset happen in **one transaction**;
  a crash can never duplicate or lose lines.
- Log rotation (inode change) and truncation (size shrink) are detected and followed.
- A `config` or `reply` answer of `0.0.0.0` / `::` is classified as `blocked` — that is
  how ad-blocking lists normally sinkhole domains.

## Database schema

```sql
CREATE TABLE queries (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts      INTEGER NOT NULL,   -- unix millis (ingest time)
  pid     INTEGER,            -- dnsmasq PID
  seq     INTEGER,            -- log-queries=extra sequence number
  client  TEXT    NOT NULL,
  domain  TEXT    NOT NULL,
  qtype   TEXT,
  outcome TEXT    NOT NULL,   -- blocked|forwarded|cached|reply|hosts|unknown|...
  answer  TEXT,
  raw     TEXT                -- original log line
);
-- indexed on ts / domain / client / outcome

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
-- tail position, schema_version
```

Because it is ordinary SQLite, you can query the database with any tool:

```bash
sqlite3 /var/lib/dnsmasq-cli-rs/logs.db \
  "SELECT domain, COUNT(*) c FROM queries WHERE outcome='blocked'
   GROUP BY domain ORDER BY c DESC LIMIT 20;"
```

## Whitelist management

The TUI's `w` key lets you un-block a domain without editing files by hand:

1. Append the domain to `/etc/dnsmasq-cli-rs/whitelist.txt`.
2. Remove matching `address=/<domain>/...` lines from
   `/etc/dnsmasq.d/adblock.conf`.
3. Restart dnsmasq so the change takes effect immediately.

This is useful together with a periodic ad-block list generator that reads the same
whitelist file and excludes those domains on its next run. The bundled
`deploy/setup.sh` installs a whitelist-aware weekly generator as an example.

Notes:

- All management actions run through `sudo -n` (non-interactive). They fail
  immediately — and report on the status line — if a password would be required, so
  the TUI never blocks.
- Reloading uses `rc-service dnsmasq restart`, so `w` / `R` are **OpenRC-only**.
  Ingest, TUI, `info`, and `prune` work on any system.

## Running as a service

On Alpine/OpenRC, the example init script is
[`deploy/dnsmasq-cli-rs.initd`](deploy/dnsmasq-cli-rs.initd). The idempotent setup
script [`deploy/setup.sh`](deploy/setup.sh) installs it along with the dnsmasq logging
config, logrotate rules, directories, and the whitelist-aware generator:

```bash
sudo SVC_USER=myserviceuser sh deploy/setup.sh
```

The service then runs as an unprivileged user:

```bash
rc-service dnsmasq-cli-rs {start|stop|restart|status}
```

On systemd, run the ingester as a simple long-lived unit (adjust `User` and paths):

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

Permissions:

- The ingester needs **read** access to the log and **write** access to the database
  directory. Running it as its own unprivileged user is recommended.
- dnsmasq creates its dedicated log as `dnsmasq:dnsmasq 0640`, so add the ingest user
  to the `dnsmasq` group (`addgroup <user> dnsmasq`). Without it the ingester cannot
  read the log and silently stops. `deploy/setup.sh` performs this step.
- The TUI only reads the database, so it needs no privileges. `w` / `R` require
  password-less sudo for the specific commands on OpenRC systems.

## Troubleshooting

- **`/var/log/dnsmasq.log` is not created** — make sure dnsmasq was restarted after
  adding the logging config.
- **The TUI shows garbled characters** — install terminfo
  (on Alpine: `apk add ncurses-terminfo`).
- **`w` reports `whitelist failed: …`** — check that the user has password-less sudo
  and that `/etc/dnsmasq-cli-rs/whitelist.txt` exists.
- **The database cannot be opened** — check ownership of `logs.db` (and the `-wal` /
  `-shm` sidecars). The TUI and ingester must be able to access the same file.
- **No rows appear** — confirm the log actually contains `log-queries=extra` output
  in the expected format, and that `ingest` is running.
- **Data stops partway through** — check that `/var/log/dnsmasq.log` is readable.
  dnsmasq creates it `dnsmasq:dnsmasq 0640`, so the ingest user must be in the
  `dnsmasq` group. While it cannot open the log, `ingest` writes one warning to
  `ingest.log` and then keeps reporting `heartbeat: +0 rows`; it logs
  `log readable again` when access is restored.
- **Times are off by hours** — display defaults to UTC. Use `--tz-offset +9` or set
  `DNSMASQ_CLI_RS_TZ_OFFSET=+9`.

## Development

```bash
cargo build                              # debug build
cargo build --release                    # release build
./build.sh                               # static musl release build
```

Module layout:

```
src/
  main.rs       entry point / subcommand dispatch
  cli.rs        hand-written argument parsing (no clap)
  model.rs      QueryEvent / Outcome
  parser.rs     dnsmasq line parser (pure functions)
  store.rs      rusqlite: schema, atomic batch insert + tail state, aggregates
  ingest.rs     tail + follow + rotation handling + pending correlation
  util.rs       UTC time formatting / duration parsing
  tui/
    mod.rs      App state + event loop + key handling
    ui.rs       ratatui rendering (Live / Stats)
    manage.rs   whitelist append / dnsmasq reload (sudo -n)
```

There is currently no test suite.

## License

Released under the [MIT License](LICENSE).
