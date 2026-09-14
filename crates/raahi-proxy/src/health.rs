//! Active upstream health checks. Every interval, each backend is probed:
//!
//! - **TCP check** (default): connect succeeds = pass.
//! - **HTTP check** (service has `health_path`): HTTP or HTTPS GET; pass =
//!   status 200–399.
//!
//! State flips use consecutive-result thresholds (2 fails to eject, 2 passes to
//! recover) so a single blip doesn't flap targets. Passive marking also happens the
//! moment a proxied request fails to connect (see `fail_to_connect` in the proxy).
//! Runs as a Pingora background service.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream};
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_core::Id;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::runtime::{Addresses, ConfigHandle};

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
    addresses: Arc<RwLock<Addresses>>,
    tls: bool,
    tls_sni: Option<String>,
}

/// Probe the elected IP while preserving the configured TLS server name. Do not
/// follow redirects or environment proxies: health describes this target itself.
async fn http_probe(
    addr: SocketAddr,
    host: &str,
    path: &str,
    tls: bool,
    deadline: Duration,
) -> bool {
    let authority = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let Ok(url) = reqwest::Url::parse(&format!(
        "{}://{authority}:{}{path}",
        if tls { "https" } else { "http" },
        addr.port()
    )) else {
        return false;
    };
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(deadline)
        .resolve(host, addr)
        .build()
    else {
        return false;
    };
    match client
        .get(url)
        .header("user-agent", "raahi-health")
        .send()
        .await
    {
        Ok(response) => (200..400).contains(&response.status().as_u16()),
        Err(_) => false,
    }
}

async fn tcp_probe(addr: SocketAddr, deadline: Duration) -> bool {
    matches!(timeout(deadline, TcpStream::connect(addr)).await, Ok(Ok(_)))
}

/// Probe one concrete address with the check kind configured for the service.
async fn probe_addr(p: &Probe, addr: SocketAddr, deadline: Duration) -> bool {
    match &p.health_path {
        Some(path) if !path.is_empty() => {
            http_probe(
                addr,
                if p.tls {
                    p.tls_sni.as_deref().unwrap_or(&p.host)
                } else {
                    &p.host
                },
                path,
                p.tls,
                deadline,
            )
            .await
        }
        _ => tcp_probe(addr, deadline).await,
    }
}

/// A target passes when ANY of its resolved addresses answers, starting with the
/// currently elected one. A pass on a different address elects it for proxy
/// connects too — so a host resolving to ::1 + 127.0.0.1 with only one of them
/// listening converges on the working address instead of proxying to the dead one
/// while probes pass on the other.
async fn elect_and_probe(p: &Probe, deadline: Duration) -> bool {
    let (addrs, active) = {
        let state = p.addresses.read().unwrap();
        (state.addrs.clone(), state.active)
    };
    let n = addrs.len();
    if n == 0 {
        return false; // unresolvable host: always failing
    }
    let start = active.min(n - 1);
    for i in 0..n {
        let idx = (start + i) % n;
        if probe_addr(p, addrs[idx], deadline).await {
            if idx != start {
                let mut state = p.addresses.write().unwrap();
                if state.addrs == addrs {
                    state.active = idx;
                }
                tracing::info!(
                    "health: {}:{} switched to address {}",
                    p.host,
                    p.port,
                    addrs[idx]
                );
            }
            return true;
        }
    }
    false
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
                    addresses: b.addresses.clone(),
                    tls: sr.service.protocol.is_tls(),
                    tls_sni: sr
                        .service
                        .tls_sni
                        .clone()
                        .or_else(|| sr.service.upstream_authority.clone()),
                })
            })
            .collect()
    }

    async fn refresh_dns(&self) {
        stream::iter(self.probes())
            .map(|p| async move {
                if p.host.parse::<std::net::IpAddr>().is_ok() {
                    return;
                }
                match timeout(
                    self.connect_timeout,
                    tokio::net::lookup_host((p.host.as_str(), p.port)),
                )
                .await
                {
                    Ok(Ok(addresses)) => {
                        let addrs: Vec<_> = addresses.collect();
                        if !addrs.is_empty() {
                            p.addresses.write().unwrap().refresh(addrs);
                        }
                    }
                    _ => tracing::warn!(
                        "DNS refresh failed for {}:{}; retaining last known addresses",
                        p.host,
                        p.port
                    ),
                }
            })
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
    }

    async fn run_checks(&self, streaks: &mut HashMap<Id, (u32, u32)>) {
        let probes = self.probes();
        // Drop streak state for targets that no longer exist.
        let live: std::collections::HashSet<Id> = probes.iter().map(|p| p.target_id).collect();
        streaks.retain(|id, _| live.contains(id));

        let mut checks = stream::iter(probes)
            .map(|p| async move {
                let pass = elect_and_probe(&p, self.connect_timeout).await;
                (p, pass)
            })
            .buffer_unordered(16);
        while let Some((p, pass)) = checks.next().await {
            let addr = format!("{}:{}", p.host, p.port);

            let (fails, passes) = streaks.entry(p.target_id).or_insert((0, 0));
            if pass {
                *passes = passes.saturating_add(1);
                *fails = 0;
            } else {
                *fails = fails.saturating_add(1);
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
                    if p.health_path.is_some() {
                        "http"
                    } else {
                        "tcp"
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(addrs: Vec<SocketAddr>, active: usize) -> Probe {
        Probe {
            target_id: 1,
            host: "test".into(),
            port: 0,
            health_path: None,
            flag: Arc::new(AtomicBool::new(true)),
            addresses: Arc::new(RwLock::new(Addresses { addrs, active })),
            tls: false,
            tls_sni: None,
        }
    }

    #[tokio::test]
    async fn http_probe_handles_fragmented_status_and_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for status in [200, 302, 503] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(request.starts_with("GET /healthz HTTP/1.1"));
                assert!(request.contains("health.test"));
                socket.write_all(b"HTTP/1.1 ").await.unwrap();
                tokio::task::yield_now().await;
                socket.write_all(format!("{status} Test\r\nContent-Length: 0\r\nLocation: http://127.0.0.1:1/\r\n\r\n").as_bytes()).await.unwrap();
            });
            assert_eq!(
                http_probe(
                    addr,
                    "health.test",
                    "/healthz",
                    false,
                    Duration::from_secs(2)
                )
                .await,
                status < 400
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn https_probe_does_not_accept_plaintext_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            assert_ne!(&buffer[..3], b"GET");
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
        });
        assert!(
            !http_probe(
                addr,
                "health.test",
                "/healthz",
                true,
                Duration::from_secs(2)
            )
            .await
        );
        server.await.unwrap();
    }

    /// A dead address on 127.0.0.1: bind a listener, note the port, drop it.
    async fn dead_addr() -> SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    }

    #[tokio::test]
    async fn elects_the_working_address_when_the_active_one_is_dead() {
        let live = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = probe(vec![dead_addr().await, live.local_addr().unwrap()], 0);
        assert!(elect_and_probe(&p, Duration::from_secs(1)).await);
        assert_eq!(p.addresses.read().unwrap().active, 1);
        // Subsequent checks start from (and keep) the elected address.
        assert!(elect_and_probe(&p, Duration::from_secs(1)).await);
        assert_eq!(p.addresses.read().unwrap().active, 1);
    }

    #[tokio::test]
    async fn fails_when_no_address_answers_or_none_resolved() {
        let p = probe(vec![dead_addr().await], 0);
        assert!(!elect_and_probe(&p, Duration::from_secs(1)).await);
        assert!(!elect_and_probe(&probe(vec![], 0), Duration::from_secs(1)).await);
    }
}

#[async_trait]
impl BackgroundService for HealthService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        // Per-target consecutive (failures, passes).
        let mut streaks: HashMap<Id, (u32, u32)> = HashMap::new();
        let mut tick = tokio::time::interval(self.interval);
        let mut dns_tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => {
                    tokio::select! {
                        _ = shutdown.changed() => break,
                        _ = self.run_checks(&mut streaks) => {}
                    }
                },
                _ = dns_tick.tick() => {
                    tokio::select! {
                        _ = shutdown.changed() => break,
                        _ = self.refresh_dns() => {}
                    }
                },
            }
        }
    }
}
