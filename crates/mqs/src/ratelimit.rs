//! Active-message rate limiting, following QQ's published policy.
//!
//! passive   C2C   valid 60min, 4 replies per msg_id
//! passive   group valid 5min,  5 replies per msg_id
//!
//! active    C2C   certified    bot 10 qps,  relation 20 qpm, daily 1000/user
//! active    C2C   unverified   bot 5 qps & 30 qpm, relation 20 qpm, daily 1000/user
//! active    group certified    bot 60 qpm,  relation 20 qpm, daily 1000/group
//! active    group unverified   bot 30 qpm,  relation 20 qpm, daily 1000/group
//!
//! 个人认证与企业认证的限额完全一致，因此合并为 `certified`。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde_json::json;

use crate::clock::Clock;
use crate::error::rate_limited;

pub const DAILY_LIMIT: i64 = 1000;
pub const RELATION_QPM: i64 = 20;

#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub limit: i64,
    pub period_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct RateInfo {
    pub limit: i64,
    pub remaining: i64,
    pub reset_after: f64,
    pub bucket: String,
}

impl RateInfo {
    pub fn as_dict(&self) -> serde_json::Value {
        json!({
            "limit": self.limit,
            "remaining": self.remaining,
            "reset_after": (self.reset_after * 1000.0).round() / 1000.0,
            "bucket": self.bucket,
        })
    }
}

pub struct RateLimiter {
    pub clock: Arc<Clock>,
    pub mode: String,
    windows: HashMap<String, VecDeque<f64>>,
    pub rejected: u64,
    pub allowed: u64,
}

fn normalize(mode: &str) -> Result<String, String> {
    match mode.to_lowercase().as_str() {
        "strict" | "unverified" => Ok("unverified".into()),
        "loose"
        | "certified"
        | "personal"
        | "enterprise"
        | "personal_certified"
        | "enterprise_certified"
        | "on" => Ok("certified".into()),
        "off" => Ok("off".into()),
        other => Err(format!("unknown rate mode: {other}")),
    }
}

impl RateLimiter {
    pub fn new(clock: Arc<Clock>, mode: &str) -> Self {
        Self {
            clock,
            mode: normalize(mode).unwrap_or_else(|_| "certified".into()),
            windows: HashMap::new(),
            rejected: 0,
            allowed: 0,
        }
    }

    pub fn set_mode(&mut self, mode: &str) -> Result<(), String> {
        self.mode = normalize(mode)?;
        self.windows.clear();
        Ok(())
    }

    pub fn reset(&mut self) {
        self.windows.clear();
        self.rejected = 0;
        self.allowed = 0;
    }

    /// Sliding-window quota: at most `limit` events in any `period`.
    fn take(&mut self, key: &str, window: Window, now: f64) -> (bool, f64, i64) {
        let period = window.period_ms as f64 / 1000.0;
        let stamps = self.windows.entry(key.to_string()).or_default();
        let cutoff = now - period;
        while stamps.front().map(|s| *s <= cutoff).unwrap_or(false) {
            stamps.pop_front();
        }
        if stamps.len() as i64 >= window.limit {
            let wait = (stamps.front().copied().unwrap_or(now) + period - now).max(0.0);
            return (false, wait, 0);
        }
        stamps.push_back(now);
        let remaining = window.limit - stamps.len() as i64;
        let reset_after = stamps
            .front()
            .map(|first| (first + period - now).max(0.0))
            .unwrap_or(period);
        (true, reset_after, remaining)
    }

    /// Fixed QPM/QPS limits on read endpoints (e.g. group members 60 QPM).
    pub fn check_read(
        &mut self,
        bot_id: &str,
        bucket: &str,
        limit: i64,
        period_ms: i64,
    ) -> Result<RateInfo, crate::error::ApiError> {
        if self.mode == "off" {
            return Ok(RateInfo::default());
        }
        let now = self.clock.now_ms() as f64 / 1000.0;
        let (ok, wait, remaining) = self.take(
            &format!("{bot_id}:read:{bucket}"),
            Window { limit, period_ms },
            now,
        );
        if !ok {
            self.rejected += 1;
            return Err(rate_limited(wait, limit, 0, bucket));
        }
        self.allowed += 1;
        Ok(RateInfo {
            limit,
            remaining,
            reset_after: wait,
            bucket: bucket.to_string(),
        })
    }

    /// Check *active* (unsolicited) message quotas. Passive replies bypass this.
    pub fn check_active(
        &mut self,
        bot_id: &str,
        scene: &str,
        target: &str,
    ) -> Result<RateInfo, crate::error::ApiError> {
        let scene_key = if scene == "c2c" || scene == "user" || scene == "dm" {
            "c2c"
        } else {
            "group"
        };
        let windows: &[Window] = match (self.mode.as_str(), scene_key) {
            ("off", _) => &[],
            ("unverified", "c2c") => &[
                Window {
                    limit: 5,
                    period_ms: 1_000,
                },
                Window {
                    limit: 30,
                    period_ms: 60_000,
                },
            ],
            ("unverified", _) => &[Window {
                limit: 30,
                period_ms: 60_000,
            }],
            ("certified", "c2c") => &[Window {
                limit: 10,
                period_ms: 1_000,
            }],
            ("certified", _) => &[Window {
                limit: 60,
                period_ms: 60_000,
            }],
            _ => &[],
        };
        if windows.is_empty() {
            return Ok(RateInfo::default());
        }
        let now = self.clock.now_ms() as f64 / 1000.0;
        let mut worst: Option<RateInfo> = None;
        for window in windows {
            let key = format!(
                "{bot_id}:{scene_key}:bot:{}/{}",
                window.limit, window.period_ms
            );
            let (ok, wait, remaining) = self.take(&key, *window, now);
            if !ok {
                self.rejected += 1;
                return Err(rate_limited(
                    wait,
                    window.limit,
                    0,
                    &format!("bot {scene_key} {}/{}ms", window.limit, window.period_ms),
                ));
            }
            let info = RateInfo {
                limit: window.limit,
                remaining,
                reset_after: wait,
                bucket: format!("bot {scene_key}"),
            };
            if worst
                .as_ref()
                .map(|w| info.remaining < w.remaining)
                .unwrap_or(true)
            {
                worst = Some(info);
            }
        }
        let relation = Window {
            limit: RELATION_QPM,
            period_ms: 60_000,
        };
        let key = format!("{bot_id}:{scene_key}:relation:{target}");
        let (ok, wait, remaining) = self.take(&key, relation, now);
        if !ok {
            self.rejected += 1;
            return Err(rate_limited(
                wait,
                relation.limit,
                0,
                &format!("relation {scene_key} {}", &target[..target.len().min(8)]),
            ));
        }
        let info = RateInfo {
            limit: relation.limit,
            remaining,
            reset_after: wait,
            bucket: format!("relation {scene_key}"),
        };
        if worst
            .as_ref()
            .map(|w| info.remaining < w.remaining)
            .unwrap_or(true)
        {
            worst = Some(info);
        }
        self.allowed += 1;
        Ok(worst.unwrap_or_default())
    }

    pub fn stats(&self) -> serde_json::Value {
        json!({
            "mode": self.mode,
            "buckets": self.windows.len(),
            "allowed": self.allowed,
            "rejected": self.rejected,
            "daily_limit": DAILY_LIMIT,
            "relation_qpm": RELATION_QPM,
        })
    }

    pub fn plan_table() -> serde_json::Value {
        json!({
            "off": {},
            "unverified": {
                "c2c": {"bot": ["5/s", "30/min"], "relation": ["20/min"], "daily": 1000},
                "group": {"bot": ["30/min"], "relation": ["20/min"], "daily": 1000},
            },
            "certified": {
                "c2c": {"bot": ["10/s"], "relation": ["20/min"], "daily": 1000},
                "group": {"bot": ["60/min"], "relation": ["20/min"], "daily": 1000},
            },
            "note": "个人认证与企业认证限额一致，已合并为 certified",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_window_blocks_the_n_plus_one() {
        let clock = Arc::new(Clock::new("virtual"));
        let mut limiter = RateLimiter::new(clock, "unverified");
        for _ in 0..5 {
            assert!(limiter.check_active("10001", "c2c", "u_1").is_ok());
        }
        let err = limiter.check_active("10001", "c2c", "u_1").unwrap_err();
        assert_eq!(err.code, 42900);
        assert_eq!(err.limit, 5);
    }

    #[test]
    fn off_mode_is_unlimited() {
        let clock = Arc::new(Clock::new("virtual"));
        let mut limiter = RateLimiter::new(clock, "off");
        for _ in 0..100 {
            assert!(limiter.check_active("10001", "c2c", "u_1").is_ok());
        }
    }

    #[test]
    fn relation_limit_is_separate() {
        let clock = Arc::new(Clock::new("virtual"));
        let mut limiter = RateLimiter::new(clock, "certified");
        for _ in 0..20 {
            assert!(limiter.check_active("10001", "group", "g_1").is_ok());
        }
        let err = limiter.check_active("10001", "group", "g_1").unwrap_err();
        assert_eq!(err.limit, 20);
    }
}
