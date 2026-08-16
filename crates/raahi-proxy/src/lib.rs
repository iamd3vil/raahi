//! Raahi data plane: the Pingora `ProxyHttp` implementation, load balancing, the
//! plugin layer, and in-memory metrics. Reads a hot-swappable config snapshot.

mod health;
mod httplog;
mod jwks;
mod metrics;
mod plugins;
mod proxy;
mod runtime;
mod tls;

pub use health::HealthService;
pub use httplog::{log_channel, HttpLogService, LogEvent, LogSender};
pub use jwks::JwksService;
pub use plugins::{purge_cache, validate_wasm, wat_to_wasm};
pub use metrics::{ConsumerHit, Metrics, MetricsSnapshot, RequestRecord, RouteHit};
pub use proxy::{Ctx, RaahiProxy};
pub use runtime::{BackendRt, ConfigHandle, RuntimeConfig, ServiceRuntime};
pub use tls::{sni_tls_settings, validate_cert, CertHandle, CertStore};

use raahi_core::ProxyConfig;

/// Build the initial [`ConfigHandle`] from a config snapshot.
pub fn config_handle(initial: ProxyConfig) -> ConfigHandle {
    ConfigHandle::new(RuntimeConfig::build(initial, None))
}
