//! Virtual clock: everything time-sensitive goes through here.

use std::sync::Mutex;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, TimeZone};

/// Asia/Shanghai has been a fixed +08:00 offset since 1991 — no tz database needed.
pub fn tz() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).unwrap()
}

#[derive(Debug, Clone)]
struct Inner {
    mode: String,
    virtual_now_ms: i64,
}

#[derive(Debug)]
pub struct Clock {
    inner: Mutex<Inner>,
}

fn wall_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Default for Clock {
    fn default() -> Self {
        Self::new("real")
    }
}

impl Clock {
    pub fn new(mode: &str) -> Self {
        Self {
            inner: Mutex::new(Inner {
                mode: if mode == "virtual" {
                    "virtual".into()
                } else {
                    "real".into()
                },
                virtual_now_ms: wall_ms(),
            }),
        }
    }

    pub fn mode(&self) -> String {
        self.inner.lock().unwrap().mode.clone()
    }

    pub fn now_ms(&self) -> i64 {
        let inner = self.inner.lock().unwrap();
        if inner.mode == "virtual" {
            inner.virtual_now_ms
        } else {
            wall_ms()
        }
    }

    pub fn set_mode(&self, mode: &str) {
        let mut inner = self.inner.lock().unwrap();
        if mode == "virtual" && inner.mode != "virtual" {
            inner.virtual_now_ms = wall_ms();
        }
        inner.mode = if mode == "virtual" {
            "virtual".into()
        } else {
            "real".into()
        };
    }

    /// QQ-style RFC3339 timestamp, e.g. `2026-01-01T12:00:00+08:00`.
    pub fn iso(&self, ms: Option<i64>) -> String {
        let value = ms.unwrap_or_else(|| self.now_ms());
        let dt: DateTime<FixedOffset> = tz()
            .timestamp_millis_opt(value)
            .single()
            .unwrap_or_else(|| tz().timestamp_millis_opt(0).unwrap());
        dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
    }

    pub fn now(&self) -> DateTime<FixedOffset> {
        tz().timestamp_millis_opt(self.now_ms()).single().unwrap()
    }

    /// `YYYY-MM-DD` in the sandbox timezone (used for daily quotas).
    pub fn day(&self) -> String {
        let now = self.now();
        format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
    }

    pub fn advance(&self, ms: i64) -> i64 {
        let mut inner = self.inner.lock().unwrap();
        if inner.mode == "virtual" {
            inner.virtual_now_ms += ms;
        } else {
            inner.virtual_now_ms = wall_ms() + ms;
            inner.mode = "virtual".into();
        }
        inner.virtual_now_ms
    }

    pub fn set_ms(&self, ms: i64) -> i64 {
        let mut inner = self.inner.lock().unwrap();
        inner.virtual_now_ms = ms;
        inner.mode = "virtual".into();
        ms
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "mode": self.mode(),
            "now_ms": self.now_ms(),
            "now": self.iso(None),
        })
    }

    pub fn restore(&self, mode: &str, now_ms: i64) {
        let mut inner = self.inner.lock().unwrap();
        inner.mode = if mode == "virtual" {
            "virtual".into()
        } else {
            "real".into()
        };
        inner.virtual_now_ms = now_ms;
    }
}

pub fn human_duration(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.abs();
    for (size, suffix) in [
        (86_400_000_i64, "d"),
        (3_600_000, "h"),
        (60_000, "m"),
        (1_000, "s"),
    ] {
        if ms >= size {
            let value = ms as f64 / size as f64;
            return format!("{sign}{value}{suffix}");
        }
    }
    format!("{sign}{ms}ms")
}

/// Parse `500ms` / `6m` / `2h` / `1d` / bare seconds into milliseconds.
pub fn parse_duration(text: &str) -> Result<i64, String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Err("empty duration".into());
    }
    if text.chars().all(|c| c.is_ascii_digit()) {
        return Ok(text.parse::<i64>().map_err(|e| e.to_string())? * 1000);
    }
    let (number, factor) = if let Some(rest) = text.strip_suffix("ms") {
        (rest, 1.0_f64)
    } else {
        let suffix = text.chars().last().unwrap();
        let factor = match suffix {
            's' => 1000.0,
            'm' => 60_000.0,
            'h' => 3_600_000.0,
            'd' => 86_400_000.0,
            _ => {
                return Err(format!(
                    "cannot parse duration: {text:?} (use e.g. 500ms, 3s, 6m, 2h)"
                ))
            }
        };
        (&text[..text.len() - suffix.len_utf8()], factor)
    };
    let value = number
        .parse::<f64>()
        .map_err(|_| format!("cannot parse duration: {text:?}"))?;
    Ok((value * factor) as i64)
}

/// Parse an RFC3339 timestamp (or a bare millisecond timestamp) into ms.
pub fn parse_ts(text: &str) -> Result<i64, String> {
    let text = text.trim();
    if text.chars().all(|c| c.is_ascii_digit()) && text.len() > 10 {
        return text.parse::<i64>().map_err(|e| e.to_string());
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Ok(dt.timestamp_millis());
    }
    // Python's fromisoformat accepts a space instead of "T" and no offset.
    let normalized = text.replace(' ', "T");
    if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S") {
        return Ok(tz()
            .from_local_datetime(&naive)
            .single()
            .unwrap()
            .timestamp_millis());
    }
    Err(format!("invalid RFC3339 timestamp: {text}"))
}

/// Local wall-clock `HH:MM:SS`, used by `mqs logs`.
pub fn hm(ms: i64) -> String {
    tz().timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("500ms").unwrap(), 500);
        assert_eq!(parse_duration("3s").unwrap(), 3000);
        assert_eq!(parse_duration("6m").unwrap(), 360_000);
        assert_eq!(parse_duration("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration("1d").unwrap(), 86_400_000);
        assert_eq!(parse_duration("42").unwrap(), 42_000);
        assert!(parse_duration("nope").is_err());
        assert_eq!(human_duration(360_000), "6m");
    }

    #[test]
    fn virtual_clock() {
        let clock = Clock::new("real");
        let before = clock.now_ms();
        assert!((before - wall_ms()).abs() < 5000);
        clock.set_mode("virtual");
        let frozen = clock.now_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(clock.now_ms(), frozen);
        clock.advance(360_000);
        assert_eq!(clock.now_ms(), frozen + 360_000);
    }

    #[test]
    fn iso_format() {
        let clock = Clock::new("real");
        let iso = clock.iso(Some(0));
        assert_eq!(iso, "1970-01-01T08:00:00+08:00");
    }
}
