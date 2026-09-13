//! dnsmasq ログを tail して SQLite に取り込む常駐処理。
//!
//! - `log-queries=extra` の `seq` で query 行と outcome 行を相関させる。
//! - dnsmasq の PID が変わったら seq がリセットされるので pending をフラッシュする。
//! - イベント挿入と tail オフセット更新を 1 トランザクションで行い、
//!   クラッシュしても重複/欠落しないようにする。
//! - ローテーション (inode 変化) と truncate (サイズ縮小) を検知して追従する。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::cli::Config;
use crate::model::Outcome;
use crate::parser::{parse_line, Parsed};
use crate::store::{now_millis, Store};

/// outcome が来るのを待つ pending のタイムアウト。
const PENDING_TIMEOUT: Duration = Duration::from_secs(10);
/// 保持期間を超えた行を削除する間隔。
const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);
/// 稼働状況をログに出す間隔。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(300);

struct Pending {
    ev: crate::model::QueryEvent,
    /// このクエリ行のファイル先頭からの開始オフセット (安全オフセット算出用)。
    line_start: u64,
    forwarded_to: Option<String>,
    saw_forwarded: bool,
    at: Instant,
}

pub fn run(cfg: &Config) -> Result<()> {
    let mut store = Store::open_rw(&cfg.db)?;

    let mut inode: u64 = store
        .meta_get("tail_inode")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut base: u64 = store
        .meta_get("tail_offset")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut buf: Vec<u8> = Vec::new();
    let mut pending: HashMap<i64, Pending> = HashMap::new();
    let mut cur_pid: Option<i64> = None;
    let mut batch: Vec<crate::model::QueryEvent> = Vec::new();
    let mut last_prune = Instant::now() - PRUNE_INTERVAL;
    let mut last_beat = Instant::now();
    let mut committed_since = 0usize;
    let mut last_committed_offset = u64::MAX; // 起動直後は必ず 1 回書く

    log(&format!(
        "ingest start: log={} db={} retention={}d",
        cfg.log, cfg.db, cfg.retention_days
    ));

    loop {
        if let Ok(mut f) = File::open(&cfg.log) {
            let mut saw = false;
            if let Ok(md) = f.metadata() {
                let read_pos = base + buf.len() as u64;
                if inode == 0 || md.ino() != inode {
                    inode = md.ino();
                    base = 0;
                    buf.clear();
                } else if md.len() < read_pos {
                    // truncate (copytruncate 等)
                    base = 0;
                    buf.clear();
                }
                if f.seek(SeekFrom::Start(base)).is_ok() {
                    let mut chunk = Vec::new();
                    if f.read_to_end(&mut chunk).is_ok() && !chunk.is_empty() {
                        buf.extend_from_slice(&chunk);
                        saw = true;
                    }
                }
            }
            if saw {
                process_lines(
                    &mut buf,
                    &mut base,
                    &mut pending,
                    &mut cur_pid,
                    &mut batch,
                );
            }
        }

        // タイムアウトした pending を確定
        flush_stale(&mut pending, &mut batch, true);

        // 安全オフセット = pending が指す最も古い行の先頭 (無ければ base)
        let safe = pending
            .values()
            .map(|p| p.line_start)
            .min()
            .unwrap_or(base);
        // 書き込むものが無くオフセットも進んでいなければコミットしない (無駄な WAL 書込を避ける)
        if !batch.is_empty() || safe != last_committed_offset {
            store.commit_batch(&batch, Some(inode), Some(safe))?;
            last_committed_offset = safe;
            committed_since += batch.len();
        }
        batch.clear();

        if last_beat.elapsed() >= HEARTBEAT_INTERVAL {
            log(&format!(
                "heartbeat: +{committed_since} rows, offset={last_committed_offset}, pending={}",
                pending.len()
            ));
            committed_since = 0;
            last_beat = Instant::now();
        }

        if last_prune.elapsed() >= PRUNE_INTERVAL {
            match store.prune(cfg.retention_days * 86_400_000, false) {
                Ok(n) if n > 0 => log(&format!("pruned {n} rows")),
                _ => {}
            }
            last_prune = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(cfg.poll_ms.max(100)));
    }
}

/// 完結した行をすべて処理し、消費したバイトだけ `base` を進める。
fn process_lines(
    buf: &mut Vec<u8>,
    base: &mut u64,
    pending: &mut HashMap<i64, Pending>,
    cur_pid: &mut Option<i64>,
    batch: &mut Vec<crate::model::QueryEvent>,
) {
    let mut start = 0usize;
    while let Some(nl) = buf[start..].iter().position(|&b| b == b'\n') {
        let line_end = start + nl;
        let line = String::from_utf8_lossy(&buf[start..line_end]).into_owned();
        let line_start = *base + start as u64;
        handle_line(&line, line_start, pending, cur_pid, batch);
        start = line_end + 1;
    }
    if start > 0 {
        buf.drain(..start);
        *base += start as u64;
    }
}

fn handle_line(
    line: &str,
    line_start: u64,
    pending: &mut HashMap<i64, Pending>,
    cur_pid: &mut Option<i64>,
    batch: &mut Vec<crate::model::QueryEvent>,
) {
    match parse_line(line, now_millis()) {
        Parsed::Query(ev) => {
            if let Some(p) = ev.pid {
                if *cur_pid != Some(p) {
                    // dnsmasq 再起動 → seq がリセットされるので pending を確定
                    flush_stale(pending, batch, false);
                    *cur_pid = Some(p);
                }
            }
            match ev.seq {
                Some(seq) => {
                    pending.insert(
                        seq,
                        Pending {
                            ev,
                            line_start,
                            forwarded_to: None,
                            saw_forwarded: false,
                            at: Instant::now(),
                        },
                    );
                }
                None => batch.push(ev),
            }
        }
        Parsed::Outcome {
            seq: Some(seq),
            outcome,
            answer,
            ..
        } => {
            if let Some(mut p) = pending.remove(&seq) {
                match outcome {
                    Outcome::Forwarded => {
                        p.saw_forwarded = true;
                        p.ev.answer = answer.clone();
                        p.forwarded_to = answer;
                        pending.insert(seq, p);
                    }
                    o => {
                        p.ev.outcome = o;
                        if answer.is_some() {
                            p.ev.answer = answer;
                        }
                        batch.push(p.ev);
                    }
                }
            }
        }
        _ => {}
    }
}

/// pending を確定して batch に移す。
/// `only_stale` が true ならタイムアウトしたものだけ、false なら全部。
fn flush_stale(
    pending: &mut HashMap<i64, Pending>,
    batch: &mut Vec<crate::model::QueryEvent>,
    only_stale: bool,
) {
    let keys: Vec<i64> = pending
        .iter()
        .filter(|(_, p)| !only_stale || p.at.elapsed() >= PENDING_TIMEOUT)
        .map(|(k, _)| *k)
        .collect();
    for k in keys {
        if let Some(mut p) = pending.remove(&k) {
            p.ev.outcome = if p.saw_forwarded {
                Outcome::Forwarded
            } else {
                Outcome::Unknown
            };
            if p.ev.answer.is_none() {
                p.ev.answer = p.forwarded_to;
            }
            batch.push(p.ev);
        }
    }
}

fn log(msg: &str) {
    eprintln!("[dnsmasq-cli-rs] {msg}");
}
