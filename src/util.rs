//! 時刻整形などの小道具。chrono を使わず UTC で整形する。
//!
//! 表示は `--tz-offset` (または環境変数 `DNSMASQ_CLI_RS_TZ_OFFSET`) で指定した
//! オフセットを加えた「ローカル時刻」で行う。既定は 0 = UTC。
//! DB に保存する `ts` は常に UTC (unix millis) のまま。

use std::sync::atomic::{AtomicI64, Ordering};

/// 表示・日別集計に加えるローカル時刻オフセット (ミリ秒)。
static TZ_OFFSET_MS: AtomicI64 = AtomicI64::new(0);

pub fn set_tz_offset_ms(ms: i64) {
    TZ_OFFSET_MS.store(ms, Ordering::Relaxed);
}

pub fn tz_offset_ms() -> i64 {
    TZ_OFFSET_MS.load(Ordering::Relaxed)
}

/// unix millis → "YYYY-MM-DD HH:MM:SS" (ローカル)
pub fn fmt_datetime(ts_ms: i64) -> String {
    let (y, m, d, hh, mm, ss) = breakdown(ts_ms);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

/// unix millis → "HH:MM:SS" (ローカル)
pub fn fmt_time(ts_ms: i64) -> String {
    let (_, _, _, hh, mm, ss) = breakdown(ts_ms);
    format!("{hh:02}:{mm:02}:{ss:02}")
}

/// unix millis → "YYYY-MM-DD" (ローカル)
pub fn fmt_date(ts_ms: i64) -> String {
    let (y, m, d, _, _, _) = breakdown(ts_ms);
    format!("{y:04}-{m:02}-{d:02}")
}

fn breakdown(ts_ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    let ts_ms = ts_ms + tz_offset_ms();
    let days = ts_ms.div_euclid(86_400_000);
    let rem = ts_ms.rem_euclid(86_400_000);
    let secs = (rem / 1000) as u32;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// Howard Hinnant の civil_from_days。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// タイムゾーンオフセット表記をミリ秒に変換する。
/// 受け付ける形式: `utc` / `Z` / `+9` / `-5` / `+05:30` / `-03:30`
pub fn parse_tz_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("utc") || s.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1i64, s.strip_prefix('+').unwrap_or(s)),
    };
    let hours = if let Some((h, m)) = rest.split_once(':') {
        let h: i64 = h.trim().parse().ok()?;
        let m: i64 = m.trim().parse().ok()?;
        if m >= 60 {
            return None;
        }
        h * 3_600_000 + m * 60_000
    } else {
        rest.trim().parse::<i64>().ok()? * 3_600_000
    };
    if hours.abs() > 14 * 3_600_000 {
        return None;
    }
    Some(sign * hours)
}

/// "30d" / "12h" / "45m" / "90s" をミリ秒に変換する。
pub fn parse_duration_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let unit_ms: i64 = match unit {
        "s" | "S" => 1_000,
        "m" | "M" => 60_000,
        "h" | "H" => 3_600_000,
        "d" | "D" => 86_400_000,
        "w" | "W" => 604_800_000,
        _ => return s.parse::<i64>().ok().map(|n| n * 1000),
    };
    num.parse::<i64>().ok().map(|n| n * unit_ms)
}
