//! Raahi entrypoint: boots the SQLite store, compiles the initial config snapshot,
//! and runs the Pingora data plane (HTTP + optional rustls HTTPS) plus — from
//! milestone 3 — the admin API as a background service.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use pingora::prelude::*;
use pingora::services::background::background_service;
use raahi_api::ApiService;
use raahi_core::{RouteSpec, ServiceSpec, TargetSpec};
use raahi_proxy::{config_handle, Metrics, RaahiProxy};
use raahi_store::Store;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "raahi", version, about = "Raahi — a configurable reverse proxy")]
struct Cli {
    /// SQLite database URL (created if missing).
    #[arg(long, env = "RAAHI_DB", default_value = "sqlite://raahi.db")]
    db: String,

    /// Directory for runtime artifacts (materialized TLS cert/key).
    #[arg(long, env = "RAAHI_RUN_DIR", default_value = ".raahi")]
    run_dir: PathBuf,

    /// Seed a demo service + route on startup (idempotent-ish; skips if a service exists).
    #[arg(long)]
    seed: bool,

    /// Override the proxy HTTP listen address (else taken from settings).
    #[arg(long)]
    http_addr: Option<String>,

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

/// Materialize the active certificate to disk so the (rustls) TLS listener can load
/// it. Returns the (cert_path, key_path) if a cert is configured.
async fn materialize_cert(
    store: &Store,
    cert_id: i64,
    dir: &Path,
) -> anyhow::Result<Option<(String, String)>> {
    let Some(cert) = store.get_certificate(cert_id).await? else {
        return Ok(None);
    };
    std::fs::create_dir_all(dir).with_context(|| format!("create run dir {}", dir.display()))?;
    let cert_path = dir.join("active-cert.pem");
    let key_path = dir.join("active-key.pem");
    std::fs::write(&cert_path, cert.cert_pem.as_bytes())?;
    std::fs::write(&key_path, cert.key_pem.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(Some((
        cert_path.display().to_string(),
        key_path.display().to_string(),
    )))
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

    let (snapshot, settings, tls_paths) = setup_rt.block_on(async {
        let store = Store::connect(&cli.db)
            .await
            .with_context(|| format!("open db {}", cli.db))?;
        if cli.seed {
            seed(&store).await?;
        }
        let snapshot = store.build_snapshot().await?;
        let settings = snapshot.settings.clone();
        let tls_paths = match settings.active_certificate_id {
            Some(id) => materialize_cert(&store, id, &cli.run_dir).await?,
            None => None,
        };
        Ok::<_, anyhow::Error>((snapshot, settings, tls_paths))
    })?;
    drop(setup_rt);

    let config = config_handle(snapshot);
    let metrics = Arc::new(Metrics::new());

    let http_addr = cli.http_addr.unwrap_or_else(|| settings.proxy_http_addr.clone());

    let mut server = Server::new(None).map_err(map_pingora)?;
    server.bootstrap();

    let proxy = RaahiProxy {
        config: config.clone(),
        metrics: metrics.clone(),
    };
    let mut proxy_svc = http_proxy_service(&server.configuration, proxy);
    proxy_svc.add_tcp(&http_addr);
    info!("proxy HTTP listener on {http_addr}");

    if let Some(https_addr) = &settings.proxy_https_addr {
        match &tls_paths {
            Some((cert, key)) => match proxy_svc.add_tls(https_addr, cert, key) {
                Ok(()) => info!("proxy HTTPS (rustls) listener on {https_addr}"),
                Err(e) => warn!("TLS listener disabled ({https_addr}): {e}"),
            },
            None => info!("HTTPS address set but no active certificate; HTTPS disabled"),
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
        ui_dir: Some(cli.ui_dir.clone()),
    };
    server.add_service(background_service("admin-api", api));
    info!("admin API on {admin_addr}");

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
