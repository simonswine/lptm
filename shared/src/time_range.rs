pub const PRESETS: &[&str] = &[
    "5m", "15m", "30m", "1h", "3h", "6h", "12h", "24h", "2d", "7d", "30d",
];

/// Parses "5m", "1h", "2d", "30s" → seconds. Returns `None` if unrecognised.
pub fn parse_time_range(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('s') {
        n.parse::<u64>().ok()
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<u64>().ok().map(|v| v * 60)
    } else if let Some(n) = s.strip_suffix('h') {
        n.parse::<u64>().ok().map(|v| v * 3600)
    } else if let Some(n) = s.strip_suffix('d') {
        n.parse::<u64>().ok().map(|v| v * 86400)
    } else {
        None
    }
}

// ── Absolute datetime parsing ─────────────────────────────────────────────────

/// Compute the Julian Day Number (integer) for a proleptic Gregorian date.
/// Algorithm: https://en.wikipedia.org/wiki/Julian_day#Converting_Gregorian_calendar_date_to_Julian_Day_Number
fn julian_day(y: i64, m: i64, d: i64) -> i64 {
    let a = (14 - m) / 12;
    let y2 = y + 4800 - a;
    let m2 = m + 12 * a - 3;
    d + (153 * m2 + 2) / 5 + 365 * y2 + y2 / 4 - y2 / 100 + y2 / 400 - 32045
}

const EPOCH_JD: i64 = 2440588; // Julian Day of 1970-01-01

/// Parses "YYYY-MM-DD HH:MM:SS" as UTC and returns Unix nanoseconds.
/// Returns `None` if the format is invalid.
pub fn parse_datetime_utc_ns(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date_str, time_str) = s.split_once(' ')?;
    let mut dp = date_str.splitn(3, '-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: i64 = dp.next()?.parse().ok()?;
    let day: i64 = dp.next()?.parse().ok()?;
    let mut tp = time_str.splitn(3, ':');
    let hour: i64 = tp.next()?.parse().ok()?;
    let min: i64 = tp.next()?.parse().ok()?;
    let sec: i64 = tp.next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&min)
        || !(0..=59).contains(&sec)
    {
        return None;
    }
    let days = julian_day(year, month, day) - EPOCH_JD;
    let total_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    if total_secs < 0 {
        return None;
    }
    Some(total_secs as u64 * 1_000_000_000)
}

/// Parses "YYYY-MM-DD HH:MM:SS" as UTC and returns Unix seconds.
pub fn parse_datetime_utc_s(s: &str) -> Option<u64> {
    parse_datetime_utc_ns(s).map(|ns| ns / 1_000_000_000)
}

/// Formats a Unix-seconds timestamp as "YYYY-MM-DD HH:MM:SS" (UTC).
pub fn format_unix_s_utc(secs: u64) -> String {
    let s = secs as i64;
    let days = s / 86400;
    let time_of_day = s % 86400;
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;

    // Reverse Julian Day algorithm (from JD back to Gregorian)
    let jd = days + EPOCH_JD;
    let p = jd + 68569;
    let q = 4 * p / 146097;
    let r = p - (146097 * q + 3) / 4;
    let y = 4000 * (r + 1) / 1461001;
    let r = r - 1461 * y / 4 + 31;
    let month = 80 * r / 2447;
    let day = r - 2447 * month / 80;
    let month2 = month + 2 - 12 * (month / 11);
    let year = 100 * (q - 49) + y + month / 11;

    format!("{year:04}-{month2:02}-{day:02} {hh:02}:{mm:02}:{ss:02}")
}

// ── Absolute range: "YYYY-MM-DD HH:MM:SS/YYYY-MM-DD HH:MM:SS" ──────────────

pub const ABS_SEP: char = '/';

/// Returns true if `time_range` encodes an absolute start/end pair.
pub fn is_absolute(time_range: &str) -> bool {
    time_range.contains(ABS_SEP)
}

/// Encodes a start/end pair into the storage format.
pub fn encode_absolute(from: &str, to: &str) -> String {
    format!("{from}{ABS_SEP}{to}")
}

/// Splits an absolute range string into (from_str, to_str).
pub fn split_absolute(time_range: &str) -> Option<(&str, &str)> {
    time_range.split_once(ABS_SEP)
}

/// Parses an absolute range → (start_ns, end_ns). Returns `None` for relative strings.
pub fn parse_absolute_range_ns(time_range: &str) -> Option<(u64, u64)> {
    let (from, to) = split_absolute(time_range)?;
    let start = parse_datetime_utc_ns(from)?;
    let end = parse_datetime_utc_ns(to)?;
    Some((start, end))
}

// ── Unified range resolution ──────────────────────────────────────────────────

/// Returns `(start_ns, end_ns)` from either an absolute or relative range.
pub fn resolve_range_ns(time_range: &str, now_ns: u64) -> (u64, u64) {
    if let Some(pair) = parse_absolute_range_ns(time_range) {
        pair
    } else {
        let secs = parse_time_range(time_range).unwrap_or(3600);
        (now_ns.saturating_sub(secs * 1_000_000_000), now_ns)
    }
}

/// Returns `(start_s, end_s)` from either an absolute or relative range.
pub fn resolve_range_s(time_range: &str, now_s: u64) -> (u64, u64) {
    let (s_ns, e_ns) = resolve_range_ns(time_range, now_s * 1_000_000_000);
    (s_ns / 1_000_000_000, e_ns / 1_000_000_000)
}

// ── Legacy helpers (kept for callers that pass only end) ─────────────────────

/// Returns `end_ns - range_seconds * 1_000_000_000`, saturating at 0.
/// For absolute ranges, returns the start from the encoded pair.
pub fn start_unix_ns(end_ns: u64, range: &str) -> u64 {
    resolve_range_ns(range, end_ns).0
}

/// Returns `end_s - range_seconds`, saturating at 0.
pub fn start_unix_s(end_s: u64, range: &str) -> u64 {
    resolve_range_s(range, end_s).0
}

/// Returns `end_ms - range_seconds * 1_000`, saturating at 0.
pub fn start_unix_ms(end_ms: u64, range: &str) -> u64 {
    let end_ns = end_ms * 1_000_000;
    resolve_range_ns(range, end_ns).0 / 1_000_000
}

// ── Display formatting ────────────────────────────────────────────────────────

/// Human-readable label for a time range value shown in title bars.
pub fn display_label(time_range: &str) -> String {
    if let Some((from, to)) = split_absolute(time_range) {
        // Trim seconds when they're :00 for compactness.
        let fmt = |s: &str| {
            if let Some(stripped) = s.strip_suffix(":00") {
                stripped.to_string()
            } else {
                s.to_string()
            }
        };
        format!("{} → {}", fmt(from.trim()), fmt(to.trim()))
    } else {
        time_range.to_string()
    }
}

/// Returns the index of `range` within `PRESETS`, or `None` if it's a custom value.
pub fn preset_index(range: &str) -> Option<usize> {
    PRESETS.iter().position(|&p| p == range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seconds() {
        assert_eq!(parse_time_range("30s"), Some(30));
        assert_eq!(parse_time_range("1s"), Some(1));
    }

    #[test]
    fn parse_minutes() {
        assert_eq!(parse_time_range("5m"), Some(300));
        assert_eq!(parse_time_range("15m"), Some(900));
        assert_eq!(parse_time_range("30m"), Some(1800));
    }

    #[test]
    fn parse_hours() {
        assert_eq!(parse_time_range("1h"), Some(3600));
        assert_eq!(parse_time_range("6h"), Some(21600));
        assert_eq!(parse_time_range("24h"), Some(86400));
    }

    #[test]
    fn parse_days() {
        assert_eq!(parse_time_range("1d"), Some(86400));
        assert_eq!(parse_time_range("7d"), Some(604800));
        assert_eq!(parse_time_range("30d"), Some(2592000));
    }

    #[test]
    fn parse_invalid() {
        assert_eq!(parse_time_range(""), None);
        assert_eq!(parse_time_range("abc"), None);
        assert_eq!(parse_time_range("1"), None);
        assert_eq!(parse_time_range("1x"), None);
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(parse_time_range("  1h  "), Some(3600));
    }

    #[test]
    fn start_ns_calculation() {
        let end = 10_000_000_000_000u64;
        assert_eq!(start_unix_ns(end, "1h"), end - 3_600_000_000_000);
        assert_eq!(start_unix_ns(end, "5m"), end - 300_000_000_000);
    }

    #[test]
    fn start_s_calculation() {
        assert_eq!(start_unix_s(7200, "1h"), 3600);
        assert_eq!(start_unix_s(300, "1h"), 0); // saturates
    }

    #[test]
    fn start_ms_calculation() {
        assert_eq!(start_unix_ms(7_200_000, "1h"), 3_600_000);
    }

    #[test]
    fn preset_index_known() {
        assert_eq!(preset_index("5m"), Some(0));
        assert_eq!(preset_index("1h"), Some(3));
        assert_eq!(preset_index("30d"), Some(10));
    }

    #[test]
    fn preset_index_custom() {
        assert_eq!(preset_index("45m"), None);
        assert_eq!(preset_index("4h"), None);
    }

    #[test]
    fn all_presets_parseable() {
        for p in PRESETS {
            assert!(
                parse_time_range(p).is_some(),
                "preset {p:?} should be parseable"
            );
        }
    }

    #[test]
    fn parse_datetime_utc_epoch() {
        assert_eq!(parse_datetime_utc_ns("1970-01-01 00:00:00"), Some(0));
    }

    #[test]
    fn parse_datetime_utc_known() {
        // 2024-01-15 10:00:00 UTC = 1705312800 Unix seconds
        let expected_s: u64 = 1705312800;
        let got = parse_datetime_utc_ns("2024-01-15 10:00:00").unwrap();
        assert_eq!(got / 1_000_000_000, expected_s);
    }

    #[test]
    fn parse_datetime_invalid() {
        assert_eq!(parse_datetime_utc_ns("not-a-date"), None);
        assert_eq!(parse_datetime_utc_ns("2024-13-01 00:00:00"), None); // bad month
        assert_eq!(parse_datetime_utc_ns("2024-01-01 25:00:00"), None); // bad hour
    }

    #[test]
    fn format_unix_roundtrip() {
        let dt = "2024-01-15 10:00:00";
        let ns = parse_datetime_utc_ns(dt).unwrap();
        assert_eq!(format_unix_s_utc(ns / 1_000_000_000), dt);
    }

    #[test]
    fn absolute_range_encode_decode() {
        let from = "2024-01-15 10:00:00";
        let to = "2024-01-15 14:00:00";
        let encoded = encode_absolute(from, to);
        let (got_from, got_to) = split_absolute(&encoded).unwrap();
        assert_eq!(got_from, from);
        assert_eq!(got_to, to);
    }

    #[test]
    fn resolve_range_ns_relative() {
        let now = 7200_000_000_000u64; // 7200s in ns
        let (start, end) = resolve_range_ns("1h", now);
        assert_eq!(end, now);
        assert_eq!(start, now - 3_600_000_000_000);
    }

    #[test]
    fn resolve_range_ns_absolute() {
        let from = "1970-01-01 01:00:00";
        let to = "1970-01-01 02:00:00";
        let encoded = encode_absolute(from, to);
        let (start, end) = resolve_range_ns(&encoded, 0);
        assert_eq!(start, 3600_000_000_000);
        assert_eq!(end, 7200_000_000_000);
    }

    #[test]
    fn display_label_relative() {
        assert_eq!(display_label("1h"), "1h");
    }

    #[test]
    fn display_label_absolute_trims_zero_seconds() {
        let enc = encode_absolute("2024-01-15 10:00:00", "2024-01-15 14:00:00");
        assert_eq!(display_label(&enc), "2024-01-15 10:00 → 2024-01-15 14:00");
    }
}
