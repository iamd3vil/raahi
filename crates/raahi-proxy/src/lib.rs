//! Raahi data plane: the Pingora `ProxyHttp` implementation, load balancing, the
//! plugin layer, and in-memory metrics. Reads a hot-swappable config snapshot.

mod health;
mod metrics;
mod plugins;
mod proxy;
mod runtime;
mod tls;

pub use health::HealthService;
pub use metrics::{Metrics, MetricsSnapshot, RequestRecord, RouteHit};
pub use proxy::{Ctx, RaahiProxy};
pub use runtime::{BackendRt, ConfigHandle, RuntimeConfig, ServiceRuntime};
pub use tls::{sni_tls_settings, CertStore};

use raahi_core::ProxyConfig;

/// Build the initial [`ConfigHandle`] from a config snapshot.
pub fn config_handle(initial: ProxyConfig) -> ConfigHandle {
    ConfigHandle::new(RuntimeConfig::build(initial, None))
}
