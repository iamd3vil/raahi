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
    pub latency_ms: u64,
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
    pub top_routes: Vec<RouteHit>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RouteHit {
    pub route_id: Id,
    pub count: u64,
}

pub struct Metrics {
    total: AtomicU64,
    class_2xx: AtomicU64,
    class_3xx: AtomicU64,
    class_4xx: AtomicU64,
    class_5xx: AtomicU64,
    no_route: AtomicU64,
    latency_sum_ms: AtomicU64,
    recent: Mutex<VecDeque<RequestRecord>>,
    route_hits: Mutex<HashMap<Id, u64>>,
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
            latency_sum_ms: AtomicU64::new(0),
            recent: Mutex::new(VecDeque::with_capacity(RECENT_CAP)),
            route_hits: Mutex::new(HashMap::new()),
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
        self.latency_sum_ms
            .fetch_add(rec.latency_ms, Ordering::Relaxed);

        if let Some(rid) = rec.route_id {
            if let Ok(mut hits) = self.route_hits.lock() {
                *hits.entry(rid).or_insert(0) += 1;
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
        let sum = self.latency_sum_ms.load(Ordering::Relaxed);
        let avg = if total > 0 { sum as f64 / total as f64 } else { 0.0 };

        let mut top_routes: Vec<RouteHit> = self
            .route_hits
            .lock()
            .map(|h| {
                h.iter()
                    .map(|(&route_id, &count)| RouteHit { route_id, count })
                    .collect()
            })
            .unwrap_or_default();
        top_routes.sort_by(|a, b| b.count.cmp(&a.count));
        top_routes.truncate(5);

        MetricsSnapshot {
            total,
            class_2xx: self.class_2xx.load(Ordering::Relaxed),
            class_3xx: self.class_3xx.load(Ordering::Relaxed),
            class_4xx: self.class_4xx.load(Ordering::Relaxed),
            class_5xx: self.class_5xx.load(Ordering::Relaxed),
            no_route: self.no_route.load(Ordering::Relaxed),
            avg_latency_ms: avg,
            top_routes,
        }
    }
}
