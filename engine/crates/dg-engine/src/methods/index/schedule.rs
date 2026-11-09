//! The re-index schedule (was `Schedule` in Reindexer.swift) and the little date handling it needs: ISO 8601
//! stamps for `lastReindexRun` / `lastReindexSuccess` (what EngineClient.swift mirrors to UserDefaults as Dates),
//! and local-time "today at H:00" for the daily run.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    Off,
    Hourly,
    SixHours,
    Daily,
}

impl Schedule {
    pub const ALL: [Schedule; 4] = [Schedule::Off, Schedule::Hourly, Schedule::SixHours, Schedule::Daily];

    /// The stored value (Swift raw values).
    pub fn raw(self) -> &'static str {
        match self {
            Schedule::Off => "off",
            Schedule::Hourly => "hourly",
            Schedule::SixHours => "every6h",
            Schedule::Daily => "daily",
        }
    }

    pub fn from_raw(raw: &str) -> Option<Schedule> {
        Schedule::ALL.into_iter().find(|s| s.raw() == raw)
    }

    /// A stored value, including those saved by earlier versions ("Off", "Every 6 hours"); anything else is daily.
    pub fn from_stored(raw: Option<&str>) -> Schedule {
        let raw = raw.unwrap_or("");
        Schedule::from_raw(raw).unwrap_or(match raw {
            "Off" => Schedule::Off,
            "Every 6 hours" => Schedule::SixHours,
            _ => Schedule::Daily,
        })
    }

    /// What `schedule.set` accepts (AppControl's aliases, case-insensitive, then the raw values).
    pub fn parse_alias(raw: &str) -> Option<Schedule> {
        match raw.to_lowercase().as_str() {
            "off" | "never" => Some(Schedule::Off),
            "hourly" | "every_hour" => Some(Schedule::Hourly),
            "every_6_hours" | "6h" | "every6h" => Some(Schedule::SixHours),
            "daily" => Some(Schedule::Daily),
            _ => Schedule::from_raw(raw),
        }
    }

    pub fn label(self, hour: u32) -> String {
        match self {
            Schedule::Off => "Off".into(),
            Schedule::Hourly => "Every hour".into(),
            Schedule::SixHours => "Every 6 hours".into(),
            Schedule::Daily => format!("Daily at {}", hour_label(hour)),
        }
    }

    /// Whether a scheduled run is due. Never before the first run: that one starts from the Re-index button, after
    /// a backend is chosen. Times are Unix seconds.
    pub fn is_due(self, last_run: Option<f64>, now: f64, hour: u32) -> bool {
        let Some(last) = last_run else { return false };
        match self {
            Schedule::Off => false,
            Schedule::Hourly => now - last >= 3600.0,
            Schedule::SixHours => now - last >= 6.0 * 3600.0,
            Schedule::Daily => {
                let Some(todays) = local_at_hour(now, hour, 0) else { return false };
                let most_recent = if now >= todays {
                    todays
                } else {
                    match local_at_hour(now, hour, -1) {
                        Some(t) => t,
                        None => return false,
                    }
                };
                last < most_recent
            }
        }
    }
}

/// "3 AM", "12 PM".
pub fn hour_label(h: u32) -> String {
    let suffix = if h < 12 { "AM" } else { "PM" };
    let twelve = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{twelve} {suffix}")
}

/// `scheduleHour` clamped to 0–23 (3 when missing or not a number).
pub fn hour_setting(v: Option<Value>) -> u32 {
    let h = match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(3),
        _ => 3,
    };
    h.clamp(0, 23) as u32
}

/// `day_offset` days from the local date of `now`, at `hour`:00:00 local time (DST-aware via mktime).
fn local_at_hour(now: f64, hour: u32, day_offset: i32) -> Option<f64> {
    unsafe {
        let t: libc::time_t = now.floor() as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        tm.tm_hour = hour as i32;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_mday += day_offset;
        tm.tm_isdst = -1;
        let r = libc::mktime(&mut tm);
        (r != -1).then_some(r as f64)
    }
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ---- ISO 8601 ----

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// `2026-11-06T16:00:00Z` (what Swift's ISO8601DateFormatter writes).
pub fn iso(secs: f64) -> String {
    let s = secs.floor() as i64;
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Parses ISO 8601 (`…T…Z`, `±HH:MM`, `±HHMM`, fractional seconds) and the `defaults read` form
/// `2026-11-06 16:00:00 +0000`. Returns Unix seconds.
pub fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut rest = &s[19..];
    let mut frac = 0.0;
    if let Some(r) = rest.strip_prefix('.') {
        let digits: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        frac = format!("0.{digits}").parse().unwrap_or(0.0);
        rest = &r[digits.len()..];
    }
    let rest = rest.trim();
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = rest[1..].chars().filter(|c| *c != ':').collect();
            if digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            sign * (digits[..2].parse::<i64>().ok()? * 3600 + digits[2..].parse::<i64>().ok()? * 60)
        }
    };
    let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - offset;
    Some(secs as f64 + frac)
}

/// A stored date setting (ISO string, legacy string, or a number of Unix seconds).
pub fn date_setting(v: Option<Value>) -> Option<f64> {
    match v? {
        Value::String(s) => parse_date(&s),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_aliases() {
        assert_eq!(Schedule::Daily.label(3), "Daily at 3 AM");
        assert_eq!(Schedule::Daily.label(0), "Daily at 12 AM");
        assert_eq!(Schedule::Daily.label(12), "Daily at 12 PM");
        assert_eq!(Schedule::Daily.label(15), "Daily at 3 PM");
        assert_eq!(Schedule::SixHours.label(3), "Every 6 hours");
        assert_eq!(Schedule::parse_alias("Every_6_Hours"), Some(Schedule::SixHours));
        assert_eq!(Schedule::parse_alias("never"), Some(Schedule::Off));
        assert_eq!(Schedule::parse_alias("weekly"), None);
        assert_eq!(Schedule::from_stored(Some("Every 6 hours")), Schedule::SixHours);
        assert_eq!(Schedule::from_stored(Some("Off")), Schedule::Off);
        assert_eq!(Schedule::from_stored(None), Schedule::Daily);
        assert_eq!(hour_setting(Some(serde_json::json!(40))), 23);
        assert_eq!(hour_setting(Some(serde_json::json!(-2))), 0);
        assert_eq!(hour_setting(Some(serde_json::json!("x"))), 3);
    }

    #[test]
    fn due_rules() {
        let now = 1_800_000_000.0;
        assert!(!Schedule::Hourly.is_due(None, now, 3), "never before the first run");
        assert!(!Schedule::Off.is_due(Some(0.0), now, 3));
        assert!(Schedule::Hourly.is_due(Some(now - 3600.0), now, 3));
        assert!(!Schedule::Hourly.is_due(Some(now - 3599.0), now, 3));
        assert!(Schedule::SixHours.is_due(Some(now - 6.0 * 3600.0), now, 3));
        assert!(!Schedule::SixHours.is_due(Some(now - 5.0 * 3600.0), now, 3));
        // daily: due iff the last run was before the most recent H:00
        let todays = local_at_hour(now, 3, 0).unwrap();
        let most_recent = if now >= todays { todays } else { local_at_hour(now, 3, -1).unwrap() };
        assert!(Schedule::Daily.is_due(Some(most_recent - 1.0), now, 3));
        assert!(!Schedule::Daily.is_due(Some(most_recent + 1.0), now, 3));
        assert!(Schedule::Daily.is_due(Some(now - 25.0 * 3600.0), now, 3));
    }

    #[test]
    fn iso_round_trip_and_legacy() {
        assert_eq!(iso(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_date("2026-11-06T16:00:00Z"), Some(1_793_980_800.0));
        assert_eq!(iso(1_793_980_800.0), "2026-11-06T16:00:00Z");
        assert_eq!(parse_date("2026-11-06 16:00:00 +0000"), Some(1_793_980_800.0));
        assert_eq!(parse_date("2026-11-06T17:00:00+01:00"), Some(1_793_980_800.0));
        assert_eq!(parse_date("2026-11-06T16:00:00.500Z"), Some(1_793_980_800.5));
        assert_eq!(parse_date("yesterday"), None);
        for t in [1.0e9, 1_700_000_123.0, 951_782_400.0] {
            assert_eq!(parse_date(&iso(t)), Some(t));
        }
    }
}
