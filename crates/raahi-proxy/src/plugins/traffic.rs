//! Traffic-shaping plugins: `request-termination` (fixed response), `request-size-limit`
//! (Content-Length ceiling), and `redirect` (Location response).

use serde::Deserialize;

use super::{Action, ReqInput, ShortResp};

// ---- request-termination -----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TerminationCfg {
    pub status: u16,
    pub message: String,
    pub content_type: String,
}

impl Default for TerminationCfg {
    fn default() -> Self {
        TerminationCfg {
            status: 503,
            message: "Service temporarily unavailable".into(),
            content_type: "text/plain; charset=utf-8".into(),
        }
    }
}

pub fn terminate(c: &TerminationCfg) -> Action {
    Action::Respond(ShortResp {
        status: if c.status >= 100 { c.status } else { 503 },
        headers: vec![("content-type".into(), c.content_type.clone())],
        body: format!("{}\n", c.message).into_bytes(),
    })
}

// ---- request-size-limit --------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SizeLimitCfg {
    /// Maximum request body size in bytes (checked against Content-Length).
    pub max_bytes: u64,
    /// Reject requests that carry a body without a Content-Length (chunked uploads).
    pub require_content_length: bool,
}

impl Default for SizeLimitCfg {
    fn default() -> Self {
        SizeLimitCfg {
            max_bytes: 10 * 1024 * 1024,
            require_content_length: false,
        }
    }
}

pub fn size_limit(c: &SizeLimitCfg, input: &ReqInput) -> Action {
    match input
        .headers
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(len) if len > c.max_bytes => Action::Respond(ShortResp::text(
            413,
            &format!("Raahi: request body exceeds {} bytes\n", c.max_bytes),
        )),
        Some(_) => Action::Continue,
        None => {
            let chunked = input
                .headers
                .get("transfer-encoding")
                .and_then(|h| h.to_str().ok())
                .map(|v| v.to_ascii_lowercase().contains("chunked"))
                .unwrap_or(false);
            if chunked && c.require_content_length {
                Action::Respond(ShortResp::text(411, "Raahi: Content-Length required\n"))
            } else {
                Action::Continue
            }
        }
    }
}

// ---- redirect ----------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedirectCfg {
    /// 301, 302, 307, or 308.
    pub status: u16,
    /// Redirect target, e.g. `https://new.example.com`.
    pub location: String,
    /// Append the incoming path + query to `location`.
    pub preserve_path: bool,
}

impl Default for RedirectCfg {
    fn default() -> Self {
        RedirectCfg {
            status: 302,
            location: String::new(),
            preserve_path: true,
        }
    }
}

pub fn redirect(c: &RedirectCfg, input: &ReqInput) -> Action {
    if c.location.is_empty() {
        return Action::Continue; // misconfigured; do not black-hole traffic
    }
    let mut location = c.location.trim_end_matches('/').to_string();
    if c.preserve_path {
        location.push_str(input.path);
        if let Some(q) = input.query {
            location.push('?');
            location.push_str(q);
        }
    }
    let status = match c.status {
        301 | 302 | 307 | 308 => c.status,
        _ => 302,
    };
    Action::Respond(ShortResp {
        status,
        headers: vec![
            ("location".into(), location),
            ("content-type".into(), "text/plain; charset=utf-8".into()),
        ],
        body: b"Redirecting\n".to_vec(),
    })
}
