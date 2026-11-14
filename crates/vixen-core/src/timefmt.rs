//! Duration/timestamp parsing for scheduler output, and compact formatting.

use chrono::{FixedOffset, NaiveDateTime, TimeZone};

/// Slurm `[D-]HH:MM:SS`, `MM:SS`, `SS`; SGE `h_rt` (`HH:MM:SS` or seconds).
/// Returns None for `UNLIMITED`, `N/A`, `INVALID`, etc.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut secs = 0u64;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for p in &parts {
        secs = secs * 60 + p.parse::<u64>().ok()?;
    }
    // Slurm "D-HH" / "D-HH:MM" put hours first.
    if days > 0 && parts.len() < 3 {
        secs *= if parts.len() == 1 { 3600 } else { 60 };
    }
    Some(days * 86_400 + secs)
}

/// Remote clock, so naive scheduler timestamps convert correctly.
#[derive(Debug, Clone, Copy)]
pub struct RemoteClock {
    pub now: i64,
    pub offset: FixedOffset,
}

impl RemoteClock {
    pub fn local() -> Self {
        let now = chrono::Local::now();
        Self { now: now.timestamp(), offset: *now.offset() }
    }

    /// Parse the `@@TZ <epoch> <+hhmm>` header line.
    pub fn parse(arg: &str) -> Self {
        let mut it = arg.split_whitespace();
        let now = it.next().and_then(|s| s.parse().ok());
        let off = it.next().and_then(parse_offset);
        match (now, off) {
            (Some(now), Some(offset)) => Self { now, offset },
            _ => Self::local(),
        }
    }

    /// `2026-11-14T08:08:20`, `2026-11-14 08:08:20`, `11/14/2026 08:08:20`, or with `+00:00`.
    pub fn timestamp(&self, s: &str) -> Option<i64> {
        let s = s.trim();
        if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z") {
            return Some(dt.timestamp());
        }
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(dt.timestamp());
        }
        let naive = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%m/%d/%Y %H:%M:%S"]
            .iter()
            .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())?;
        self.offset.from_local_datetime(&naive).single().map(|d| d.timestamp())
    }
}

fn parse_offset(s: &str) -> Option<FixedOffset> {
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = digits[2..].parse().ok()?;
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

/// `45s`, `12m`, `3h 05m`, `2d 4h`.
pub fn human(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match () {
        _ if d > 0 => format!("{d}d {h}h"),
        _ if h > 0 => format!("{h}h {m:02}m"),
        _ if m > 0 => format!("{m}m"),
        _ => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("9:08:08"), Some(9 * 3600 + 8 * 60 + 8));
        assert_eq!(parse_duration("1-14:21:52"), Some(86_400 + 14 * 3600 + 21 * 60 + 52));
        assert_eq!(parse_duration("0:00"), Some(0));
        assert_eq!(parse_duration("2-12"), Some(2 * 86_400 + 12 * 3600));
        assert_eq!(parse_duration("3600"), Some(3600));
        assert_eq!(parse_duration("UNLIMITED"), None);
    }

    #[test]
    fn timestamps() {
        let c = RemoteClock::parse("1763130000 +0100");
        let t = c.timestamp("2026-11-14T08:00:00").unwrap();
        assert_eq!(c.timestamp("2026-11-14 07:00:00+00:00").unwrap(), t);
        assert_eq!(c.timestamp("11/14/2026 08:00:00").unwrap(), t);
        assert!(c.timestamp("N/A").is_none());
    }

    #[test]
    fn humanize() {
        assert_eq!(human(3 * 3600 + 5 * 60), "3h 05m");
        assert_eq!(human(2 * 86_400 + 3600), "2d 1h");
        assert_eq!(human(42), "42s");
    }
}
