//! `response-compression`: compresses eligible upstream responses for downstream
//! clients. The actual work is done by Pingora's built-in `ResponseCompression`
//! downstream module (registered in `RaahiProxy::init_downstream_modules`), which
//! owns Accept-Encoding negotiation (gzip / br / zstd), the content-type and
//! content-length sanity checks, and the `Content-Encoding` / `Vary` bookkeeping.
//! This plugin only decides *whether* and *at which level* the module runs, and
//! hands that decision to the proxy through [`Effects`].

use serde::Deserialize;

use super::{Action, Effects};

/// Highest level accepted for every algorithm the module may pick. The level is
/// applied across gzip / brotli / zstd alike, so the ceiling is gzip's (9); brotli
/// (max 11) and zstd (max 22) simply run below their maximum.
pub const MAX_LEVEL: u32 = 9;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CompressionCfg {
    /// Compression level, 1..=[`MAX_LEVEL`]. 5 trades a little ratio for noticeably
    /// less CPU than gzip's own default of 6.
    pub level: u32,
}

impl Default for CompressionCfg {
    fn default() -> Self {
        CompressionCfg { level: 5 }
    }
}

pub fn compress(cfg: &CompressionCfg, effects: &mut Effects) -> Action {
    // 0 would disable the module; clamp so a bad stored config can't silently turn
    // compression off or panic the encoder (zstd rejects out-of-range levels).
    effects.compression_level = Some(cfg.level.clamp(1, MAX_LEVEL));
    Action::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_level_five() {
        let cfg: CompressionCfg = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(cfg.level, 5);
    }

    #[test]
    fn clamps_out_of_range_levels() {
        let mut fx = Effects::default();
        compress(&CompressionCfg { level: 0 }, &mut fx);
        assert_eq!(fx.compression_level, Some(1));

        let mut fx = Effects::default();
        compress(&CompressionCfg { level: 99 }, &mut fx);
        assert_eq!(fx.compression_level, Some(MAX_LEVEL));
    }
}
