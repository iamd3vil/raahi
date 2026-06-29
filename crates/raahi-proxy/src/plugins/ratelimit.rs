//! Fixed-window rate limiting, keyed by client IP, consumer, or route. Returns 429
//! with a `Retry-After` header when the window's limit is exceeded.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{Action, Effects, ReqInput, ShortResp};

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RateKey {
    #[default]
    Ip,
    Consumer,
    Route,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RateLimitCfg {
    pub limit: u32,
    pub window_secs: u64,
    pub key: RateKey,
}

impl Default for RateLimitCfg {
    fn default() -> Self {
        RateLimitCfg {
            limit: 60,
            window_secs: 60,
            key: RateKey::Ip,
        }
    }
}

pub struct RateLimitState {
    cfg: RateLimitCfg,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimitState {
    pub fn new(cfg: RateLimitCfg) -> Self {
        RateLimitState {
            cfg,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, input: &ReqInput, effects: &Effects) -> Action {
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
        let count = {
            let mut buckets = self.buckets.lock().unwrap();
            let entry = buckets.entry(key).or_insert((now, 0));
            if now.duration_since(entry.0) >= window {
                *entry = (now, 0);
            }
            entry.1 += 1;
            entry.1
        };

        if count > self.cfg.limit {
            Action::Respond(ShortResp {
                status: 429,
                headers: vec![
                    ("retry-after".into(), self.cfg.window_secs.to_string()),
                    ("content-type".into(), "text/plain; charset=utf-8".into()),
                ],
                body: b"Raahi: rate limit exceeded\n".to_vec(),
            })
        } else {
            Action::Continue
        }
    }
}
