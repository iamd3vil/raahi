//! Raahi core: pure domain types, the compiled in-memory [`ProxyConfig`] snapshot,
//! and the router matching logic. No I/O, no Pingora — this crate is shared by the
//! store, the data plane, and the admin API.

mod config;
mod matching;
mod model;
mod spec;

pub use config::{JwtCred, ProxyConfig, RouteMatch};
pub use matching::{compile_path_regex, host_matches, is_regex_path, path_matches, strip_prefix};
pub use model::*;
pub use spec::*;

/// Database row id type used across all entities.
pub type Id = i64;
