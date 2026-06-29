//! Active upstream health checks. Periodically TCP-connects to every backend in the
//! live config and flips its shared health flag. Passive marking also happens on
//! connect failure (see `fail_to_connect`-style handling in the proxy). Runs as a
//! Pingora background service.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::runtime::ConfigHandle;

pub struct HealthService {
    pub config: ConfigHandle,
    pub interval: Duration,
    pub connect_timeout: Duration,
}

impl HealthService {
    async fn run_checks(&self) {
        // Snapshot the backend list (cloning the shared health flags) so we don't hold
        // the config guard across awaits.
        let targets: Vec<(String, u16, Arc<AtomicBool>)> = {
            let rc = self.config.load();
            rc.services
                .values()
                .flat_map(|sr| {
                    sr.backends
                        .iter()
                        .map(|b| (b.host.clone(), b.port, b.healthy.clone()))
                })
                .collect()
        };

        for (host, port, flag) in targets {
            let addr = format!("{host}:{port}");
            let ok = matches!(
                timeout(self.connect_timeout, TcpStream::connect(&addr)).await,
                Ok(Ok(_))
            );
            let was = flag.swap(ok, Ordering::Relaxed);
            if was != ok {
                tracing::info!("health: {addr} is now {}", if ok { "up" } else { "down" });
            }
        }
    }
}

#[async_trait]
impl BackgroundService for HealthService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let mut tick = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => self.run_checks().await,
            }
        }
    }
}
