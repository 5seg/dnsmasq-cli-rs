//! dnsmasq-cli-rs エントリポイント。

mod cli;
mod ingest;
mod model;
mod parser;
mod store;
mod tui;
mod util;

use anyhow::Result;

use cli::Cmd;
use store::Store;
use util::fmt_datetime;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = dispatch(&args) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn dispatch(args: &[String]) -> Result<()> {
    let cmd = cli::parse(args)?;
    // 表示・日別集計はこのオフセットでローカル時刻になる (DB の ts は UTC のまま)。
    util::set_tz_offset_ms(cmd.tz_offset_ms());
    match cmd {
        Cmd::Help => {
            print!("{}", cli::usage());
            Ok(())
        }
        Cmd::Version => {
            println!("dnsmasq-cli-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Tui(cfg) => tui::run(cfg),
        Cmd::Ingest(cfg) => ingest::run(&cfg),
        Cmd::Info(cfg) => info(&cfg),
        Cmd::Prune {
            config,
            older_ms,
            apply,
            vacuum,
        } => prune(&config, older_ms, apply, vacuum),
    }
}

fn info(cfg: &cli::Config) -> Result<()> {
    let store = Store::open_ro(&cfg.db)?;
    println!("db:      {}", cfg.db);
    println!("rows:    {}", store.count()?);
    if let Some((a, b)) = store.time_range()? {
        println!("range:   {} .. {}", fmt_datetime(a), fmt_datetime(b));
    } else {
        println!("range:   (empty)");
    }
    let breakdown = store.outcome_breakdown()?;
    if breakdown.is_empty() {
        println!("outcomes: (empty)");
    } else {
        println!("outcomes:");
        for (o, c) in breakdown {
            println!("  {o:<12} {c}");
        }
    }
    Ok(())
}

fn prune(cfg: &cli::Config, older_ms: i64, apply: bool, vacuum: bool) -> Result<()> {
    let cutoff = store::now_millis() - older_ms;
    if !apply {
        let store = Store::open_ro(&cfg.db)?;
        let n = store.count_older(cutoff)?;
        println!(
            "[dry-run] {} rows with ts < {} would be deleted (use --apply)",
            n,
            fmt_datetime(cutoff)
        );
        return Ok(());
    }
    let mut store = Store::open_rw(&cfg.db)?;
    let n = store.prune(older_ms, vacuum)?;
    println!("deleted {n} rows with ts < {}", fmt_datetime(cutoff));
    Ok(())
}
