//! Active upstream health checks. Every interval, each backend is probed:
//!
//! - **TCP check** (default): connect succeeds = pass.
//! - **HTTP check** (service has `health_path`): plaintext HTTP/1.1 GET; pass =
//!   status 200–399.
//!
//! State flips use consecutive-result thresholds (2 fails to eject, 2 passes to
//! recover) so a single blip doesn't flap targets. Passive marking also happens the
//! moment a proxied request fails to connect (see `fail_to_connect` in the proxy).
//! Runs as a Pingora background service.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_core::Id;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::runtime::ConfigHandle;

/// Consecutive failures before a target is ejected.
const UNHEALTHY_AFTER: u32 = 2;
/// Consecutive passes before an ejected target recovers.
const HEALTHY_AFTER: u32 = 2;

pub struct HealthService {
    pub config: ConfigHandle,
    pub interval: Duration,
    pub connect_timeout: Duration,
}

struct Probe {
    target_id: Id,
    host: String,
    port: u16,
    health_path: Option<String>,
    flag: Arc<AtomicBool>,
}

/// One HTTP/1.1 GET, returning pass/fail on the status line. Plaintext only —
/// use the TCP check for TLS upstreams.
async fn http_probe(addr: &str, host: &str, path: &str, deadline: Duration) -> bool {
    let run = async {
        let mut stream = TcpStream::connect(addr).await.ok()?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: raahi-health\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.ok()?;
        // The status line fits comfortably in one small read.
        let mut buf = [0u8; 256];
        let n = stream.read(&mut buf).await.ok()?;
        let line = String::from_utf8_lossy(&buf[..n]);
        // "HTTP/1.1 200 OK"
        let status: u16 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some((200..400).contains(&status))
    };
    matches!(timeout(deadline, run).await, Ok(Some(true)))
}

async fn tcp_probe(addr: &str, deadline: Duration) -> bool {
    matches!(timeout(deadline, TcpStream::connect(addr)).await, Ok(Ok(_)))
}

impl HealthService {
    /// Snapshot the probe list (cloning the shared health flags) so we don't hold
    /// the config guard across awaits.
    fn probes(&self) -> Vec<Probe> {
        let rc = self.config.load();
        rc.services
            .values()
            .flat_map(|sr| {
                let health_path = sr.service.health_path.clone();
                sr.backends.iter().map(move |b| Probe {
                    target_id: b.target_id,
                    host: b.host.clone(),
                    port: b.port,
                    health_path: health_path.clone(),
                    flag: b.healthy.clone(),
                })
            })
            .collect()
    }

    async fn run_checks(&self, streaks: &mut HashMap<Id, (u32, u32)>) {
        let probes = self.probes();
        // Drop streak state for targets that no longer exist.
        let live: std::collections::HashSet<Id> = probes.iter().map(|p| p.target_id).collect();
        streaks.retain(|id, _| live.contains(id));

        for p in probes {
            let addr = format!("{}:{}", p.host, p.port);
            let pass = match &p.health_path {
                Some(path) if !path.is_empty() => {
                    http_probe(&addr, &p.host, path, self.connect_timeout).await
                }
                _ => tcp_probe(&addr, self.connect_timeout).await,
            };

            let (fails, passes) = streaks.entry(p.target_id).or_insert((0, 0));
            if pass {
                *passes += 1;
                *fails = 0;
            } else {
                *fails += 1;
                *passes = 0;
            }

            let was = p.flag.load(Ordering::Relaxed);
            let now = if was {
                *fails < UNHEALTHY_AFTER // stay up until enough consecutive failures
            } else {
                *passes >= HEALTHY_AFTER // stay down until enough consecutive passes
            };
            if was != now {
                p.flag.store(now, Ordering::Relaxed);
                tracing::info!(
                    "health: {addr} is now {} ({})",
                    if now { "up" } else { "down" },
                    if p.health_path.is_some() { "http" } else { "tcp" },
                );
            }
        }
    }
}

#[async_trait]
impl BackgroundService for HealthService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        // Per-target consecutive (failures, passes).
        let mut streaks: HashMap<Id, (u32, u32)> = HashMap::new();
        let mut tick = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => self.run_checks(&mut streaks).await,
            }
        }
    }
}
