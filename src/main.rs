//! Raahi entrypoint: boots the SQLite store, compiles the initial config snapshot,
//! and runs the Pingora data plane (HTTP + optional boringssl HTTPS with per-SNI
//! certificate selection) plus the admin API and health checks as background services.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use pingora::prelude::*;
use pingora::services::background::background_service;
use raahi_api::ApiService;
use raahi_core::{RouteSpec, ServiceSpec, TargetSpec};
use raahi_proxy::{
    config_handle, log_channel, sni_tls_settings, CertHandle, CertStore, HttpLogService, Metrics,
    RaahiProxy,
};
use raahi_store::Store;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "raahi", version, about = "Raahi — a configurable reverse proxy")]
struct Cli {
    /// SQLite database URL (created if missing).
    #[arg(long, env = "RAAHI_DB", default_value = "sqlite://raahi.db")]
    db: String,

    /// Seed a demo service + route on startup (idempotent-ish; skips if a service exists).
    #[arg(long)]
    seed: bool,

    /// Override the proxy HTTP listen address (else taken from settings).
    #[arg(long)]
    http_addr: Option<String>,

    /// Override the proxy HTTPS listen address (else taken from settings).
    #[arg(long)]
    https_addr: Option<String>,

    /// Override the admin API listen address (else taken from settings).
    #[arg(long)]
    admin_addr: Option<String>,

    /// Directory of the built UI to serve from the admin API.
    #[arg(long, env = "RAAHI_UI_DIR", default_value = "ui/build")]
    ui_dir: PathBuf,
}

fn map_pingora<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("pingora: {e}")
}

/// Insert a demo service (two local targets) + catch-all route if the DB is empty.
async fn seed(store: &Store) -> anyhow::Result<()> {
    if !store.list_services().await?.is_empty() {
        info!("seed: services already present, skipping");
        return Ok(());
    }
    let svc = store
        .create_service(&ServiceSpec {
            name: "demo-service".into(),
            protocol: raahi_core::Protocol::Http,
            connect_timeout_ms: 2000,
            read_timeout_ms: 10000,
            write_timeout_ms: 10000,
            retries: 1,
            lb_algorithm: raahi_core::LbAlgorithm::RoundRobin,
            tls_sni: None,
            health_path: None,
        })
        .await?;
    for port in [9001u16, 9002] {
        store
            .create_target(
                svc.id,
                &TargetSpec { host: "127.0.0.1".into(), port, weight: 100, enabled: true },
            )
            .await?;
    }
    store
        .create_route(&RouteSpec {
            name: "demo-route".into(),
            service_id: svc.id,
            priority: 0,
            hosts: vec![],
            paths: vec!["/".into()],
            methods: vec![],
            strip_path: false,
            preserve_host: false,
            enabled: true,
        })
        .await?;
    info!("seed: created demo-service + demo-route");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    // One-shot async setup on a current-thread runtime, dropped before the server runs.
    let setup_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let (snapshot, settings, cert_store) = setup_rt.block_on(async {
        let store = Store::connect(&cli.db)
            .await
            .with_context(|| format!("open db {}", cli.db))?;
        if cli.seed {
            seed(&store).await?;
        }
        let snapshot = store.build_snapshot().await?;
        let settings = snapshot.settings.clone();
        // Load all certificates into an in-memory SNI store (boringssl serves them all).
        let certs = store.list_certificates().await?;
        let cert_store = CertStore::from_certs(certs, settings.active_certificate_id);
        Ok::<_, anyhow::Error>((snapshot, settings, cert_store))
    })?;
    drop(setup_rt);

    let config = config_handle(snapshot);
    let metrics = Arc::new(Metrics::new());
    // Live cert handle shared by the TLS listener (reads it per handshake) and the admin
    // API (swaps it on certificate/default changes — no restart).
    let cert_count = cert_store.len();
    let cert_handle = CertHandle::new(cert_store);

    let http_addr = cli.http_addr.unwrap_or_else(|| settings.proxy_http_addr.clone());

    let mut server = Server::new(None).map_err(map_pingora)?;
    server.bootstrap();

    // http-log delivery channel: the proxy's log phase produces, a background
    // service batches + POSTs to collectors.
    let (log_tx, log_rx) = log_channel();

    let proxy = RaahiProxy {
        config: config.clone(),
        metrics: metrics.clone(),
        log_tx,
    };
    let mut proxy_svc = http_proxy_service(&server.configuration, proxy);
    proxy_svc.add_tcp(&http_addr);
    info!("proxy HTTP listener on {http_addr}");

    let https_addr = cli.https_addr.or_else(|| settings.proxy_https_addr.clone());
    if let Some(https_addr) = &https_addr {
        if cert_count > 0 {
            match sni_tls_settings(cert_handle.clone()) {
                Ok(settings_tls) => {
                    proxy_svc.add_tls_with_settings(https_addr, None, settings_tls);
                    info!("proxy HTTPS (boringssl) listener on {https_addr}; {cert_count} cert(s), live per-SNI selection");
                }
                Err(e) => warn!("TLS listener disabled ({https_addr}): {e}"),
            }
        } else {
            info!("HTTPS address set but no certificates configured; HTTPS disabled (add a certificate and restart to bind the listener)");
        }
    }

    server.add_service(proxy_svc);

    // Admin API (REST + UI) as a background service sharing the live config + metrics.
    let admin_addr = cli.admin_addr.unwrap_or_else(|| settings.admin_addr.clone());
    let api = ApiService {
        addr: admin_addr.clone(),
        db_url: cli.db.clone(),
        config: config.clone(),
        metrics: metrics.clone(),
        cert_handle: cert_handle.clone(),
        ui_dir: Some(cli.ui_dir.clone()),
    };
    server.add_service(background_service("admin-api", api));
    info!("admin API on {admin_addr}");

    server.add_service(background_service("http-log", HttpLogService::new(log_rx)));

    // JWKS refresher for jwt plugins in identity-provider mode.
    server.add_service(background_service(
        "jwks",
        raahi_proxy::JwksService { config: config.clone() },
    ));

    // Active health checks for upstream targets.
    let health = raahi_proxy::HealthService {
        config: config.clone(),
        interval: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(2),
    };
    server.add_service(background_service("health", health));

    info!("Raahi started");
    server.run_forever();
}
