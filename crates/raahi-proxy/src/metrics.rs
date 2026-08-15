//! In-memory traffic metrics and a bounded recent-request log. Lives on the proxy
//! (stable across config reloads), read by the admin API for the dashboard and live
//! tail. A broadcast channel streams each request to SSE subscribers.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use raahi_core::Id;
use serde::Serialize;
use tokio::sync::broadcast;

const RECENT_CAP: usize = 500;

/// One completed request, as surfaced to the dashboard / live tail.
#[derive(Clone, Debug, Serialize)]
pub struct RequestRecord {
    pub ts_ms: u64,
    pub method: String,
    pub host: String,
    pub path: String,
    pub status: u16,
    /// Fractional milliseconds (microsecond precision).
    pub latency_ms: f64,
    pub route_id: Option<Id>,
    pub service_id: Option<Id>,
    pub upstream: Option<String>,
    pub consumer: Option<String>,
}

/// Aggregate counters snapshot returned by `/api/v1/metrics`.
#[derive(Clone, Debug, Serialize, Default)]
pub struct MetricsSnapshot {
    pub total: u64,
    pub class_2xx: u64,
    pub class_3xx: u64,
    pub class_4xx: u64,
    pub class_5xx: u64,
    pub no_route: u64,
    pub avg_latency_ms: f64,
    /// Percentiles over the recent-request window (up to 500 requests).
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub top_routes: Vec<RouteHit>,
    pub top_consumers: Vec<ConsumerHit>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RouteHit {
    pub route_id: Id,
    pub count: u64,
    /// 4xx + 5xx responses on this route.
    pub errors: u64,
    pub avg_latency_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsumerHit {
    pub consumer: String,
    pub count: u64,
}

#[derive(Default)]
struct RouteStats {
    count: u64,
    errors: u64,
    latency_sum_us: u64,
}

pub struct Metrics {
    total: AtomicU64,
    class_2xx: AtomicU64,
    class_3xx: AtomicU64,
    class_4xx: AtomicU64,
    class_5xx: AtomicU64,
    no_route: AtomicU64,
    latency_sum_us: AtomicU64,
    recent: Mutex<VecDeque<RequestRecord>>,
    route_hits: Mutex<HashMap<Id, RouteStats>>,
    consumer_hits: Mutex<HashMap<String, u64>>,
    tx: broadcast::Sender<RequestRecord>,
}

impl Default for Metrics {
    fn default() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Metrics {
            total: AtomicU64::new(0),
            class_2xx: AtomicU64::new(0),
            class_3xx: AtomicU64::new(0),
            class_4xx: AtomicU64::new(0),
            class_5xx: AtomicU64::new(0),
            no_route: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            recent: Mutex::new(VecDeque::with_capacity(RECENT_CAP)),
            route_hits: Mutex::new(HashMap::new()),
            consumer_hits: Mutex::new(HashMap::new()),
            tx,
        }
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to the live request stream (for SSE).
    pub fn subscribe(&self) -> broadcast::Receiver<RequestRecord> {
        self.tx.subscribe()
    }

    /// Record a completed request: bump counters, push to the ring buffer, broadcast.
    pub fn record(&self, rec: RequestRecord) {
        self.total.fetch_add(1, Ordering::Relaxed);
        match rec.status / 100 {
            2 => &self.class_2xx,
            3 => &self.class_3xx,
            4 => &self.class_4xx,
            _ => &self.class_5xx,
        }
        .fetch_add(1, Ordering::Relaxed);
        if rec.route_id.is_none() {
            self.no_route.fetch_add(1, Ordering::Relaxed);
        }
        self.latency_sum_us
            .fetch_add((rec.latency_ms * 1000.0) as u64, Ordering::Relaxed);

        if let Some(rid) = rec.route_id {
            if let Ok(mut hits) = self.route_hits.lock() {
                let s = hits.entry(rid).or_default();
                s.count += 1;
                if rec.status >= 400 {
                    s.errors += 1;
                }
                s.latency_sum_us += (rec.latency_ms * 1000.0) as u64;
            }
        }
        if let Some(consumer) = &rec.consumer {
            if let Ok(mut hits) = self.consumer_hits.lock() {
                *hits.entry(consumer.clone()).or_insert(0) += 1;
            }
        }
        if let Ok(mut recent) = self.recent.lock() {
            if recent.len() == RECENT_CAP {
                recent.pop_back();
            }
            recent.push_front(rec.clone());
        }
        // Lagging/closed subscribers are fine to drop.
        let _ = self.tx.send(rec);
    }

    pub fn recent(&self, limit: usize) -> Vec<RequestRecord> {
        self.recent
            .lock()
            .map(|r| r.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total.load(Ordering::Relaxed);
        let sum_us = self.latency_sum_us.load(Ordering::Relaxed);
        let avg = if total > 0 { sum_us as f64 / 1000.0 / total as f64 } else { 0.0 };

        // Percentiles over the bounded recent window.
        let (p50, p95, p99) = self
            .recent
            .lock()
            .map(|r| {
                let mut lat: Vec<f64> = r.iter().map(|x| x.latency_ms).collect();
                if lat.is_empty() {
                    return (0.0, 0.0, 0.0);
                }
                lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let at = |p: f64| lat[((lat.len() - 1) as f64 * p) as usize];
                (at(0.50), at(0.95), at(0.99))
            })
            .unwrap_or((0.0, 0.0, 0.0));

        let mut top_routes: Vec<RouteHit> = self
            .route_hits
            .lock()
            .map(|h| {
                h.iter()
                    .map(|(&route_id, s)| RouteHit {
                        route_id,
                        count: s.count,
                        errors: s.errors,
                        avg_latency_ms: if s.count > 0 {
                            s.latency_sum_us as f64 / 1000.0 / s.count as f64
                        } else {
                            0.0
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        top_routes.sort_by(|a, b| b.count.cmp(&a.count));
        top_routes.truncate(5);

        let mut top_consumers: Vec<ConsumerHit> = self
            .consumer_hits
            .lock()
            .map(|h| {
                h.iter()
                    .map(|(c, &count)| ConsumerHit { consumer: c.clone(), count })
                    .collect()
            })
            .unwrap_or_default();
        top_consumers.sort_by(|a, b| b.count.cmp(&a.count));
        top_consumers.truncate(5);

        MetricsSnapshot {
            total,
            class_2xx: self.class_2xx.load(Ordering::Relaxed),
            class_3xx: self.class_3xx.load(Ordering::Relaxed),
            class_4xx: self.class_4xx.load(Ordering::Relaxed),
            class_5xx: self.class_5xx.load(Ordering::Relaxed),
            no_route: self.no_route.load(Ordering::Relaxed),
            avg_latency_ms: avg,
            p50_latency_ms: p50,
            p95_latency_ms: p95,
            p99_latency_ms: p99,
            top_routes,
            top_consumers,
        }
    }
}
