//! Response body find/replace (response-body-transform plugin).
//!
//! The request phase only arms the intent (the parsed config); the proxy buffers
//! the response body and applies the replacements at end-of-stream (see
//! `proxy.rs`). Only valid-UTF-8 bodies whose content-type matches one of the
//! configured prefixes are transformed; anything else passes through unchanged.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ReplaceRule {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BodyTransformCfg {
    pub replace: Vec<ReplaceRule>,
    /// Bodies larger than this are passed through untransformed.
    pub max_body_bytes: u64,
    /// Content-type prefixes eligible for transformation.
    pub content_types: Vec<String>,
}

impl Default for BodyTransformCfg {
    fn default() -> Self {
        BodyTransformCfg {
            replace: Vec::new(),
            max_body_bytes: 1_048_576,
            content_types: vec!["text/".to_string(), "application/json".to_string()],
        }
    }
}

impl BodyTransformCfg {
    /// Whether a response content-type matches one of the configured prefixes.
    pub fn matches_content_type(&self, content_type: &str) -> bool {
        self.content_types.iter().any(|p| content_type.starts_with(p.as_str()))
    }

    /// Apply every replacement rule in order.
    pub fn apply(&self, mut s: String) -> String {
        for r in &self.replace {
            s = s.replace(&r.from, &r.to);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_rules_in_order() {
        let cfg = BodyTransformCfg {
            replace: vec![
                ReplaceRule { from: "backend".into(), to: "served_by".into() },
                ReplaceRule { from: "served_by_x".into(), to: "y".into() },
            ],
            ..Default::default()
        };
        assert_eq!(cfg.apply("backend_x backend".into()), "y served_by");
    }

    #[test]
    fn content_type_prefix_match() {
        let cfg = BodyTransformCfg::default();
        assert!(cfg.matches_content_type("application/json; charset=utf-8"));
        assert!(cfg.matches_content_type("text/html"));
        assert!(!cfg.matches_content_type("image/png"));
    }
}
