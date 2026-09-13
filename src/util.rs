//! 時刻整形などの小道具。chrono を使わず UTC で整形する。

/// unix millis → "YYYY-MM-DD HH:MM:SS" (UTC)
pub fn fmt_datetime(ts_ms: i64) -> String {
    let (y, m, d, hh, mm, ss) = breakdown(ts_ms);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

/// unix millis → "HH:MM:SS" (UTC)
pub fn fmt_time(ts_ms: i64) -> String {
    let (_, _, _, hh, mm, ss) = breakdown(ts_ms);
    format!("{hh:02}:{mm:02}:{ss:02}")
}

fn breakdown(ts_ms: i64) -> (i64, u32, u32, u32, u32, u32) {
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
