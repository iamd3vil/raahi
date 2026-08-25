//! Sliding-window rate limiting, keyed by client IP, consumer, or route.
//!
//! Uses the two-window approximation: each key keeps the previous and current
//! fixed-window counts, and the effective rate is `prev * overlap + cur`, which
//! smooths the burst-at-window-edge problem of a plain fixed window.
//!
//! Emits `RateLimit-Limit` / `RateLimit-Remaining` / `RateLimit-Reset` response
//! headers on allowed requests, and `429` + `Retry-After` when over the limit.
//! State survives config reloads (instances are carried over when the plugin's
//! config is unchanged — see `PluginSet::build`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{Action, Effects, ReqInput, ShortResp};

/// Prune the bucket map when it grows past this many keys.
const EVICT_THRESHOLD: usize = 10_000;

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RateKey {
    #[default]
    Ip,
    Consumer,
    Route,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct RateLimitCfg {
    pub limit: u32,
    pub window_secs: u64,
    pub key: RateKey,
    /// Emit RateLimit-* headers on allowed responses.
    pub headers: bool,
}

impl Default for RateLimitCfg {
    fn default() -> Self {
        RateLimitCfg {
            limit: 60,
            window_secs: 60,
            key: RateKey::Ip,
            headers: true,
        }
    }
}

struct Bucket {
    window_start: Instant,
    prev_count: u32,
    cur_count: u32,
}

pub struct RateLimitState {
    cfg: RateLimitCfg,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimitState {
    pub fn new(cfg: RateLimitCfg) -> Self {
        RateLimitState {
            cfg,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, input: &ReqInput, effects: &mut Effects) -> Action {
        let key = match self.cfg.key {
            RateKey::Ip => input.client_ip.unwrap_or("unknown").to_string(),
            RateKey::Consumer => effects
                .consumer
                .as_ref()
                .map(|(_, u)| format!("consumer:{u}"))
                .unwrap_or_else(|| format!("ip:{}", input.client_ip.unwrap_or("anon"))),
            RateKey::Route => format!("route:{}", input.route_id),
        };

        let window = Duration::from_secs(self.cfg.window_secs.max(1));
        let now = Instant::now();

        let (effective, reset_secs) = {
            let mut buckets = self.buckets.lock().unwrap();

            // Bounded memory: drop long-idle keys once the map gets large.
            if buckets.len() >= EVICT_THRESHOLD {
                buckets.retain(|_, b| now.duration_since(b.window_start) < window * 2);
            }

            let b = buckets.entry(key).or_insert(Bucket {
                window_start: now,
                prev_count: 0,
                cur_count: 0,
            });

            // Roll windows forward as needed.
            let elapsed = now.duration_since(b.window_start);
            if elapsed >= window * 2 {
                b.window_start = now;
                b.prev_count = 0;
                b.cur_count = 0;
            } else if elapsed >= window {
                b.window_start += window;
                b.prev_count = b.cur_count;
                b.cur_count = 0;
            }

            b.cur_count += 1;
            let in_window = now.duration_since(b.window_start);
            let prev_weight = 1.0 - (in_window.as_secs_f64() / window.as_secs_f64());
            let effective = (b.prev_count as f64 * prev_weight).floor() as u32 + b.cur_count;
            let reset = window.saturating_sub(in_window).as_secs().max(1);
            (effective, reset)
        };

        if effective > self.cfg.limit {
            Action::Respond(ShortResp {
                status: 429,
                headers: vec![
                    ("retry-after".into(), reset_secs.to_string()),
                    ("ratelimit-limit".into(), self.cfg.limit.to_string()),
                    ("ratelimit-remaining".into(), "0".into()),
                    ("ratelimit-reset".into(), reset_secs.to_string()),
                    ("content-type".into(), "text/plain; charset=utf-8".into()),
                ],
                body: b"Raahi: rate limit exceeded\n".to_vec(),
            })
        } else {
            if self.cfg.headers {
                let remaining = self.cfg.limit.saturating_sub(effective);
                effects
                    .resp_add
                    .push(("ratelimit-limit".into(), self.cfg.limit.to_string()));
                effects
                    .resp_add
                    .push(("ratelimit-remaining".into(), remaining.to_string()));
                effects
                    .resp_add
                    .push(("ratelimit-reset".into(), reset_secs.to_string()));
            }
            Action::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn input(headers: &HeaderMap) -> ReqInput<'_> {
        ReqInput {
            method: "GET",
            path: "/",
            query: None,
            host: "h",
            is_tls: false,
            client_ip: Some("1.2.3.4"),
            headers,
            route_id: 1,
        }
    }

    #[test]
    fn blocks_after_limit_and_sets_headers() {
        let st = RateLimitState::new(RateLimitCfg {
            limit: 3,
            window_secs: 60,
            key: RateKey::Ip,
            headers: true,
        });
        let headers = HeaderMap::new();
        let inp = input(&headers);
        for _ in 0..3 {
            let mut fx = Effects::default();
            assert!(matches!(st.check(&inp, &mut fx), Action::Continue));
            assert!(fx.resp_add.iter().any(|(k, _)| k == "ratelimit-remaining"));
        }
        let mut fx = Effects::default();
        match st.check(&inp, &mut fx) {
            Action::Respond(r) => assert_eq!(r.status, 429),
            _ => panic!("expected 429"),
        }
    }
}
