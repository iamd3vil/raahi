//! Raahi data plane: the Pingora `ProxyHttp` implementation, load balancing, the
//! plugin layer, and in-memory metrics. Reads a hot-swappable config snapshot.

mod health;
mod httplog;
mod jwks;
mod metrics;
mod plugins;
mod proxy;
mod runtime;
mod stream;
mod tls;

pub use health::HealthService;
pub use httplog::{HttpLogService, LogEvent, LogSender, log_channel};
pub use jwks::JwksService;
pub use metrics::{ConsumerHit, Metrics, MetricsSnapshot, RequestRecord, RouteHit};
pub use plugins::{purge_cache, validate_wasm, wat_to_wasm};
pub use proxy::{Ctx, RaahiProxy};
pub use runtime::{BackendRt, ConfigHandle, RuntimeConfig, ServiceRuntime};
pub use stream::{StreamProxyApp, stream_stats};
pub use tls::{CertHandle, CertStore, sni_tls_settings, validate_cert};

use raahi_core::ProxyConfig;

/// Build the initial [`ConfigHandle`] from a config snapshot.
pub fn config_handle(initial: ProxyConfig) -> ConfigHandle {
    ConfigHandle::new(RuntimeConfig::build(initial, None))
}
