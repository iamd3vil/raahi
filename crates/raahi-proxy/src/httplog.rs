//! http-log delivery: the proxy's log phase pushes (config, record) events onto an
//! unbounded channel; this background service batches them per endpoint and POSTs
//! them as JSON arrays. Delivery is best-effort — failures are logged and dropped,
//! never blocking the data plane.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::metrics::RequestRecord;
use crate::plugins::HttpLogCfg;

/// One request record destined for one collector endpoint.
pub struct LogEvent {
    pub cfg: HttpLogCfg,
    pub record: RequestRecord,
}

pub type LogSender = UnboundedSender<LogEvent>;

pub fn log_channel() -> (LogSender, UnboundedReceiver<LogEvent>) {
    unbounded_channel()
}

/// Pingora background service that drains the log channel.
pub struct HttpLogService {
    pub rx: Mutex<Option<UnboundedReceiver<LogEvent>>>,
}

impl HttpLogService {
    pub fn new(rx: UnboundedReceiver<LogEvent>) -> Self {
        HttpLogService {
            rx: Mutex::new(Some(rx)),
        }
    }
}

struct Batch {
    cfg: HttpLogCfg,
    records: Vec<RequestRecord>,
    last_flush: std::time::Instant,
}

async fn flush(client: &reqwest::Client, batch: &mut Batch) {
    if batch.records.is_empty() {
        return;
    }
    batch.last_flush = std::time::Instant::now();
    let records = std::mem::take(&mut batch.records);
    let count = records.len();
    let mut req = client
        .post(&batch.cfg.endpoint)
        .timeout(Duration::from_secs(10))
        .json(&records);
    for (k, v) in &batch.cfg.headers {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(resp) if !resp.status().is_success() => {
            tracing::warn!(
                "http-log: {} returned {} ({count} records dropped)",
                batch.cfg.endpoint,
                resp.status()
            );
        }
        Err(e) => {
            tracing::warn!(
                "http-log: {} failed: {e} ({count} records dropped)",
                batch.cfg.endpoint
            );
        }
        Ok(_) => {}
    }
}

#[async_trait]
impl BackgroundService for HttpLogService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let Some(mut rx) = self.rx.lock().await.take() else {
            return;
        };
        let client = reqwest::Client::new();
        // Batches keyed by endpoint (one collector = one batch/flush cadence).
        let mut batches: HashMap<String, Batch> = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_millis(500));

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    for batch in batches.values_mut() {
                        flush(&client, batch).await;
                    }
                    break;
                }
                _ = tick.tick() => {
                    for batch in batches.values_mut() {
                        if batch.last_flush.elapsed()
                            >= Duration::from_millis(batch.cfg.flush_interval_ms.max(500))
                        {
                            flush(&client, batch).await;
                        }
                    }
                }
                ev = rx.recv() => {
                    let Some(ev) = ev else { break };
                    if ev.cfg.endpoint.is_empty() {
                        continue;
                    }
                    let batch = batches
                        .entry(ev.cfg.endpoint.clone())
                        .or_insert_with(|| Batch {
                            cfg: ev.cfg.clone(),
                            records: Vec::new(),
                            last_flush: std::time::Instant::now(),
                        });
                    batch.cfg = ev.cfg; // pick up header/batch changes live
                    batch.records.push(ev.record);
                    if batch.records.len() >= batch.cfg.batch_max.max(1) {
                        flush(&client, batch).await;
                    }
                }
            }
        }
    }
}
