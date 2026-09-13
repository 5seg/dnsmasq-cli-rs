//! 手書きの引数パース (clap 不使用で依存を最小化する)。

use anyhow::{bail, Result};

pub const DEFAULT_LOG: &str = "/var/log/dnsmasq.log";
pub const DEFAULT_DB: &str = "/var/lib/dnsmasq-cli-rs/logs.db";
pub const DEFAULT_RETENTION_DAYS: i64 = 30;
pub const DEFAULT_POLL_MS: u64 = 1000;

#[derive(Clone, Debug)]
pub struct Config {
    pub log: String,
    pub db: String,
    pub retention_days: i64,
    pub poll_ms: u64,
    pub top: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            log: DEFAULT_LOG.to_string(),
            db: DEFAULT_DB.to_string(),
            retention_days: DEFAULT_RETENTION_DAYS,
            poll_ms: DEFAULT_POLL_MS,
            top: 10,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Tui(Config),
    Ingest(Config),
    Info(Config),
    Prune {
        config: Config,
        older_ms: i64,
        apply: bool,
        vacuum: bool,
    },
    Help,
    Version,
}

pub fn parse(args: &[String]) -> Result<Cmd> {
    let mut cfg = Config::default();
    let mut sub: Option<String> = None;
    let mut older_ms: Option<i64> = None;
    let mut apply = false;
    let mut vacuum = false;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => return Ok(Cmd::Help),
            "-V" | "--version" => return Ok(Cmd::Version),
            "--log" => {
                i += 1;
                cfg.log = take(args, i, "--log")?;
            }
            "--db" => {
                i += 1;
                cfg.db = take(args, i, "--db")?;
            }
            "--retention-days" => {
                i += 1;
                cfg.retention_days = take(args, i, "--retention-days")?.parse()?;
            }
            "--poll-ms" => {
                i += 1;
                cfg.poll_ms = take(args, i, "--poll-ms")?.parse()?;
            }
            "--top" => {
                i += 1;
                cfg.top = take(args, i, "--top")?.parse()?;
            }
            "--older-than" => {
                i += 1;
                let v = take(args, i, "--older-than")?;
                older_ms = Some(
                    crate::util::parse_duration_ms(&v)
                        .ok_or_else(|| anyhow::anyhow!("invalid duration: {v}"))?,
                );
            }
            "--apply" => apply = true,
            "--vacuum" => vacuum = true,
            other => {
                if let Some(rest) = other.strip_prefix("--log=") {
                    cfg.log = rest.to_string();
                } else if let Some(rest) = other.strip_prefix("--db=") {
                    cfg.db = rest.to_string();
                } else if other.starts_with('-') {
                    bail!("unknown option: {other}");
                } else if sub.is_none() {
                    sub = Some(other.to_string());
                } else {
                    bail!("unexpected argument: {other}");
                }
            }
        }
        i += 1;
    }

    match sub.as_deref() {
        None | Some("tui") => Ok(Cmd::Tui(cfg)),
        Some("ingest") => Ok(Cmd::Ingest(cfg)),
        Some("info") => Ok(Cmd::Info(cfg)),
        Some("prune") => Ok(Cmd::Prune {
            config: cfg,
            older_ms: older_ms
                .unwrap_or_else(|| DEFAULT_RETENTION_DAYS * 86_400_000),
            apply,
            vacuum,
        }),
        Some(other) => bail!("unknown subcommand: {other}"),
    }
}

fn take(args: &[String], i: usize, flag: &str) -> Result<String> {
    args.get(i)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))
}

pub fn usage() -> String {
    format!(
        "\
dnsmasq-cli-rs — dnsmasq クエリログの取り込み + TUI モニタ

USAGE:
    dnsmasq-cli-rs [SUBCOMMAND] [OPTIONS]

SUBCOMMANDS:
    tui       ライブ表示・統計・検索・管理の TUI (既定)
    ingest    dnsmasq ログを tail して SQLite に取り込む (常駐サービス用)
    info      DB の概要 (件数/期間/outcome 内訳) を表示
    prune     保持期間を超えた行を削除 (既定は dry-run)

OPTIONS:
    --log <PATH>             dnsmasq ログ (既定: {DEFAULT_LOG})
    --db <PATH>              SQLite DB   (既定: {DEFAULT_DB})
    --retention-days <N>     保持日数 (既定: {DEFAULT_RETENTION_DAYS})
    --poll-ms <MS>           ingest のポーリング間隔 (既定: {DEFAULT_POLL_MS})
    --top <N>                統計/TUI の上位件数 (既定: 10)
    --older-than <DUR>       prune 対象 (例 30d / 12h / 90m)
    --apply                  prune を実行 (無指定は dry-run)
    --vacuum                 prune 後に VACUUM
    -h, --help               ヘルプ
    -V, --version            バージョン
"
    )
}
