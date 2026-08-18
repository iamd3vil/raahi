//! In-memory traffic metrics and a bounded recent-request log. Lives on the proxy
//! (stable across config reloads), read by the admin API for the dashboard and live
//! tail. A broadcast channel streams each request to SSE subscribers.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Number of independently locked shards. Sixteen worker threads round-robining
/// over 16 shards makes lock collisions rare; the shard count is a fixed power of
/// two so the modulo is a mask.
const SHARDS: usize = 16;

/// The mutable per-request state, sharded to avoid a global lock on the hot path.
/// Each shard holds a slice of the recent-request ring plus partial hit counters;
/// readers merge all shards.
#[derive(Default)]
struct Shard {
    recent: VecDeque<RequestRecord>,
    route_hits: HashMap<Id, RouteStats>,
    consumer_hits: HashMap<String, u64>,
}

pub struct Metrics {
    total: AtomicU64,
    class_2xx: AtomicU64,
    class_3xx: AtomicU64,
    class_4xx: AtomicU64,
    class_5xx: AtomicU64,
    no_route: AtomicU64,
    latency_sum_us: AtomicU64,
    shards: Vec<Mutex<Shard>>,
    /// Round-robin cursor spreading writers across shards.
    cursor: AtomicU64,
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
            shards: (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect(),
            cursor: AtomicU64::new(0),
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

    /// Record a completed request: bump counters, update one shard, broadcast.
    /// One shard lock and zero clones on the hot path (the record is cloned only
    /// when an SSE subscriber is actually listening).
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

        if self.tx.receiver_count() > 0 {
            // Lagging/closed subscribers are fine to drop.
            let _ = self.tx.send(rec.clone());
        }

        let i = self.cursor.fetch_add(1, Ordering::Relaxed) as usize % SHARDS;
        if let Ok(mut shard) = self.shards[i].lock() {
            if let Some(rid) = rec.route_id {
                let s = shard.route_hits.entry(rid).or_default();
                s.count += 1;
                if rec.status >= 400 {
                    s.errors += 1;
                }
                s.latency_sum_us += (rec.latency_ms * 1000.0) as u64;
            }
            if let Some(consumer) = &rec.consumer {
                if let Some(n) = shard.consumer_hits.get_mut(consumer.as_str()) {
                    *n += 1;
                } else {
                    shard.consumer_hits.insert(consumer.clone(), 1);
                }
            }
            if shard.recent.len() >= RECENT_CAP.div_ceil(SHARDS) {
                shard.recent.pop_back();
            }
            shard.recent.push_front(rec);
        }
    }

    /// Most recent requests across all shards, newest first.
    pub fn recent(&self, limit: usize) -> Vec<RequestRecord> {
        let mut all: Vec<RequestRecord> = Vec::with_capacity(limit.min(RECENT_CAP) * 2);
        for shard in &self.shards {
            if let Ok(s) = shard.lock() {
                all.extend(s.recent.iter().cloned());
            }
        }
        all.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
        all.truncate(limit);
        all
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total.load(Ordering::Relaxed);
        let sum_us = self.latency_sum_us.load(Ordering::Relaxed);
        let avg = if total > 0 {
            sum_us as f64 / 1000.0 / total as f64
        } else {
            0.0
        };

        // Merge every shard's partial state: latencies for percentiles over the
        // bounded recent window, plus per-route / per-consumer counters.
        let mut lat: Vec<f64> = Vec::with_capacity(RECENT_CAP);
        let mut routes: HashMap<Id, RouteStats> = HashMap::new();
        let mut consumers: HashMap<String, u64> = HashMap::new();
        for shard in &self.shards {
            let Ok(s) = shard.lock() else { continue };
            lat.extend(s.recent.iter().map(|x| x.latency_ms));
            for (&rid, st) in &s.route_hits {
                let agg = routes.entry(rid).or_default();
                agg.count += st.count;
                agg.errors += st.errors;
                agg.latency_sum_us += st.latency_sum_us;
            }
            for (c, &n) in &s.consumer_hits {
                *consumers.entry(c.clone()).or_insert(0) += n;
            }
        }

        let (p50, p95, p99) = if lat.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let at = |p: f64| lat[((lat.len() - 1) as f64 * p) as usize];
            (at(0.50), at(0.95), at(0.99))
        };

        let mut top_routes: Vec<RouteHit> = routes
            .iter()
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
            .collect();
        top_routes.sort_by(|a, b| b.count.cmp(&a.count));
        top_routes.truncate(5);

        let mut top_consumers: Vec<ConsumerHit> = consumers
            .into_iter()
            .map(|(consumer, count)| ConsumerHit { consumer, count })
            .collect();
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
