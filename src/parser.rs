//! dnsmasq のクエリログ行をパースする。
//!
//! `log-queries=extra` を想定した形式に対応する:
//! ```text
//! dnsmasq[PID]: <seq> <client>/<port> query[A] <domain> from <client>
//! dnsmasq[PID]: <seq> <client>/<port> forwarded <domain> to <server>
//! dnsmasq[PID]: <seq> <client>/<port> reply <domain> is <ip>
//! dnsmasq[PID]: <seq> <client>/<port> cached <domain> is <ip|NXDOMAIN|NODATA>
//! dnsmasq[PID]: <seq> <client>/<port> config <domain> is <ip>      (0.0.0.0 / :: はブロック)
//! dnsmasq[PID]: <seq> <client>/<port> /etc/hosts <domain> is <ip>
//! ```
//! `dnsmasq[PID]:` 接頭辞が無い場合 (log-facility 直書き時など) も扱えるようにしている。

use crate::model::{Outcome, QueryEvent};

#[derive(Clone, Debug)]
pub enum Parsed {
    /// `query[...]` 行。outcome は未確定 (Unknown) で pending に積む。
    Query(QueryEvent),
    /// 応答・転送・設定などの outcome 行。
    Outcome {
        seq: Option<i64>,
        outcome: Outcome,
        answer: Option<String>,
    },
    /// 無関係な行。
    Ignore,
}

/// 1 行をパースする。`ts` は取り込み時刻 (unix millis)。
pub fn parse_line(raw: &str, ts: i64) -> Parsed {
    let line = raw.trim();
    if line.is_empty() {
        return Parsed::Ignore;
    }

    let (pid, payload) = split_pid(line);
    let toks: Vec<&str> = payload.split_whitespace().collect();
    if toks.is_empty() {
        return Parsed::Ignore;
    }

    // extra 形式: 先頭が seq (整数)、次が client/port。
    let mut idx = 0;
    let mut seq = None;
    let mut client = String::new();
    if toks.len() >= 2 {
        if let Ok(n) = toks[0].parse::<i64>() {
            if let Some(slash) = toks[1].find('/') {
                seq = Some(n);
                client = toks[1][..slash].to_string();
                idx = 2;
            }
        }
    }

    let rest = &toks[idx..];
    if rest.is_empty() {
        return Parsed::Ignore;
    }

    // query[...] 行
    if let Some(stripped) = rest[0].strip_prefix("query[") {
        let qtype = stripped.strip_suffix(']').map(|s| s.to_string());
        let domain = rest.get(1).map(|s| s.to_string()).unwrap_or_default();
        // `from <client>` があれば優先
        if let Some(pos) = rest.iter().position(|t| *t == "from") {
            if let Some(c) = rest.get(pos + 1) {
                client = (*c).to_string();
            }
        }
        let mut ev = QueryEvent::new(ts, domain);
        ev.pid = pid;
        ev.seq = seq;
        ev.client = client;
        ev.qtype = qtype;
        ev.raw = line.to_string();
        return Parsed::Query(ev);
    }

    // それ以外の outcome 行
    let kind = rest[0];
    let answer = |from: usize| -> Option<String> {
        rest.get(from..)
            .filter(|s| !s.is_empty())
            .map(|s| s.join(" "))
    };

    let outcome = match kind {
        "forwarded" => Outcome::Forwarded,
        "reply" => {
            let a = answer(3).unwrap_or_default();
            if is_null_addr(&a) {
                Outcome::Blocked
            } else {
                Outcome::Reply
            }
        }
        "cached" => Outcome::Cached,
        "cached-stale" => Outcome::CachedStale,
        "config" => {
            let a = answer(3).unwrap_or_default();
            if is_null_addr(&a) {
                Outcome::Blocked
            } else {
                Outcome::Other("config".to_string())
            }
        }
        "/etc/hosts" => Outcome::Hosts,
        "nameserver" => Outcome::Other("nameserver".to_string()),
        _ => return Parsed::Ignore,
    };

    let ans = match kind {
        "forwarded" => rest
            .iter()
            .position(|t| *t == "to")
            .and_then(|p| rest.get(p + 1))
            .map(|s| s.to_string()),
        _ => answer(3),
    };

    Parsed::Outcome {
        seq,
        outcome,
        answer: ans,
    }
}

/// `dnsmasq[PID]:` 接頭辞を分離する。無ければ (None, 行全体)。
/// syslog 経由の行 (`Sep 12 14:12:54 host daemon.info dnsmasq[28206]: ...`) でも
/// この関数で前半部分が捨てられ、payload だけが残る。
fn split_pid(line: &str) -> (Option<i64>, &str) {
    if let Some(start) = line.find("dnsmasq[") {
        let after = &line[start + "dnsmasq[".len()..];
        if let Some(end) = after.find(']') {
            let pid = after[..end].parse::<i64>().ok();
            let mut rest = &after[end + 1..];
            rest = rest.strip_prefix(':').unwrap_or(rest);
            return (pid, rest.trim_start());
        }
    }
    (None, line)
}

fn is_null_addr(s: &str) -> bool {
    let s = s.trim().trim_end_matches('.');
    s == "0.0.0.0" || s == "::"
}
