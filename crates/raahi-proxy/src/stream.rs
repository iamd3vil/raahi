//! L4 TCP stream proxying: a raw [`ServerApp`] per stream-route listener that
//! splices bytes between the downstream connection and a load-balanced upstream
//! target with `copy_bidirectional`.
//!
//! Listeners bind at startup (adding/removing a stream route needs a restart), but
//! the route -> service lookup happens per connection against the live config, so
//! retargeting an existing stream route applies without a restart.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use pingora::apps::ServerApp;
use pingora::connectors::TransportConnector;
use pingora::protocols::Stream;
use pingora::server::ShutdownWatch;
use pingora::upstreams::peer::BasicPeer;
use tracing::warn;

use crate::runtime::ConfigHandle;

/// Per-listener counters, shared through the process-global registry so the
/// admin API can export them without a handle to the app.
#[derive(Default)]
struct StreamCounters {
    connections: AtomicU64,
    /// Bytes copied downstream -> upstream.
    bytes_up: AtomicU64,
    /// Bytes copied upstream -> downstream.
    bytes_down: AtomicU64,
}

fn registry() -> &'static Mutex<HashMap<String, Arc<StreamCounters>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<StreamCounters>>>> = OnceLock::new();
    REG.get_or_init(Default::default)
}

fn counters_for(listen_addr: &str) -> Arc<StreamCounters> {
    registry()
        .lock()
        .unwrap()
        .entry(listen_addr.to_string())
        .or_default()
        .clone()
}

/// Snapshot of every stream listener's counters:
/// `(listen_addr, connections_total, bytes_up, bytes_down)`.
pub fn stream_stats() -> Vec<(String, u64, u64, u64)> {
    let mut out: Vec<(String, u64, u64, u64)> = registry()
        .lock()
        .unwrap()
        .iter()
        .map(|(addr, c)| {
            (
                addr.clone(),
                c.connections.load(Ordering::Relaxed),
                c.bytes_up.load(Ordering::Relaxed),
                c.bytes_down.load(Ordering::Relaxed),
            )
        })
        .collect();
    out.sort();
    out
}

/// The L4 proxy app bound to one stream-route listener.
pub struct StreamProxyApp {
    pub config: ConfigHandle,
    /// The address this app's listener is bound to; used to find our stream route
    /// in the live config (routes are keyed by listen address).
    pub listen_addr: String,
    connector: TransportConnector,
}

impl StreamProxyApp {
    pub fn new(config: ConfigHandle, listen_addr: String) -> Self {
        StreamProxyApp {
            config,
            listen_addr,
            connector: TransportConnector::new(None),
        }
    }
}

#[async_trait]
impl ServerApp for StreamProxyApp {
    async fn process_new(
        self: &Arc<Self>,
        mut session: Stream,
        _shutdown: &ShutdownWatch,
    ) -> Option<Stream> {
        let counters = counters_for(&self.listen_addr);
        counters.connections.fetch_add(1, Ordering::Relaxed);

        // Live lookup: pick up retargeting (service changes) without a restart.
        // Clone the service runtime out so the config guard isn't held across awaits.
        let service_rt = {
            let rc = self.config.load();
            let Some(route) = rc
                .data
                .stream_routes
                .iter()
                .find(|r| r.enabled && r.listen_addr == self.listen_addr)
            else {
                warn!("stream {}: no enabled stream route for this listener; dropping connection", self.listen_addr);
                return None;
            };
            let Some(sr) = rc.services.get(&route.service_id) else {
                warn!(
                    "stream {}: stream route '{}' references unknown service {}; dropping connection",
                    self.listen_addr, route.name, route.service_id
                );
                return None;
            };
            sr.clone()
        };

        // LB client key: the downstream peer address when obtainable.
        let client_key = session
            .get_socket_digest()
            .and_then(|d| d.peer_addr().map(|a| a.to_string()))
            .unwrap_or_default();
        let Some(backend) = service_rt.select(client_key.as_bytes()) else {
            warn!("stream {}: no backend available; dropping connection", self.listen_addr);
            return None;
        };
        let addr = format!("{}:{}", backend.host, backend.port);

        let mut upstream = match self.connector.new_stream(&BasicPeer::new(&addr)).await {
            Ok(s) => s,
            Err(e) => {
                // Passive health: mark the backend down, like `fail_to_connect` in
                // the HTTP proxy. Active checks bring it back.
                if backend.healthy.swap(false, Ordering::Relaxed) {
                    warn!("health: {addr} marked down (stream connect failed)");
                }
                warn!("stream {}: connect to {addr} failed: {e}", self.listen_addr);
                return None;
            }
        };

        match tokio::io::copy_bidirectional(&mut session, &mut upstream).await {
            Ok((up, down)) => {
                counters.bytes_up.fetch_add(up, Ordering::Relaxed);
                counters.bytes_down.fetch_add(down, Ordering::Relaxed);
            }
            Err(e) => {
                warn!("stream {}: splice to {addr} ended with error: {e}", self.listen_addr);
            }
        }
        None
    }
}
