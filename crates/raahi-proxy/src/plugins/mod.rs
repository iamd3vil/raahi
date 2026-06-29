//! Built-in plugin layer: key-auth, basic-auth, rate-limit, CORS, and request/response
//! header transforms. Plugins are compiled once per config snapshot into [`PluginSet`]
//! (so stateful ones like rate-limit keep their counters). The WASM user-function phase
//! will add a `WasmPlugin` variant implementing the same contract.

mod auth;
mod cors;
mod ratelimit;
mod transform;

use std::collections::HashMap;

use raahi_core::{Id, Plugin, PluginType, ProxyConfig};

/// Read-only request inputs handed to each plugin during the request phase.
pub struct ReqInput<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub host: &'a str,
    pub client_ip: Option<&'a str>,
    pub headers: &'a http::HeaderMap,
    pub route_id: Id,
}

/// Side effects a plugin accumulates: an authenticated consumer, request-header
/// mutations (applied upstream), and response-header mutations (applied downstream).
#[derive(Default)]
pub struct Effects {
    pub consumer: Option<(Id, String)>,
    pub req_add: Vec<(String, String)>,
    pub req_remove: Vec<String>,
    pub resp_add: Vec<(String, String)>,
    pub resp_remove: Vec<String>,
}

/// A short-circuit response produced by a plugin (auth failure, rate limit, CORS
/// preflight).
pub struct ShortResp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ShortResp {
    pub fn text(status: u16, body: &str) -> ShortResp {
        ShortResp {
            status,
            headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
            body: body.as_bytes().to_vec(),
        }
    }
}

pub enum Action {
    Continue,
    Respond(ShortResp),
}

enum PluginInstance {
    KeyAuth(auth::KeyAuthCfg),
    BasicAuth(auth::BasicAuthCfg),
    RateLimit(ratelimit::RateLimitState),
    Cors(cors::CorsCfg),
    RequestTransform(transform::TransformCfg),
    ResponseTransform(transform::TransformCfg),
}

/// Compiled plugin instances keyed by plugin id.
#[derive(Default)]
pub struct PluginSet {
    instances: HashMap<Id, PluginInstance>,
}

impl PluginSet {
    pub fn build(plugins: &[Plugin]) -> Self {
        let mut instances = HashMap::new();
        for p in plugins {
            let cfg = || p.config.clone();
            let inst = match p.plugin_type {
                PluginType::KeyAuth => {
                    PluginInstance::KeyAuth(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::BasicAuth => {
                    PluginInstance::BasicAuth(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::RateLimit => PluginInstance::RateLimit(
                    ratelimit::RateLimitState::new(serde_json::from_value(cfg()).unwrap_or_default()),
                ),
                PluginType::Cors => {
                    PluginInstance::Cors(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::RequestTransform => PluginInstance::RequestTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::ResponseTransform => PluginInstance::ResponseTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
            };
            instances.insert(p.id, inst);
        }
        PluginSet { instances }
    }

    /// Execute one plugin's request-phase logic.
    pub fn run_request(
        &self,
        plugin: &Plugin,
        input: &ReqInput,
        cfg: &ProxyConfig,
        effects: &mut Effects,
    ) -> Action {
        let Some(inst) = self.instances.get(&plugin.id) else {
            return Action::Continue;
        };
        match inst {
            PluginInstance::KeyAuth(c) => auth::key_auth(c, input, cfg, effects),
            PluginInstance::BasicAuth(c) => auth::basic_auth(c, input, cfg, effects),
            PluginInstance::RateLimit(s) => s.check(input, effects),
            PluginInstance::Cors(c) => cors::cors(c, input, effects),
            PluginInstance::RequestTransform(c) => {
                c.apply_request(effects);
                Action::Continue
            }
            PluginInstance::ResponseTransform(c) => {
                c.apply_response(effects);
                Action::Continue
            }
        }
    }
}

/// Type-based execution precedence so auth runs before rate-limit (which may key on
/// the authenticated consumer), CORS preflight is answered first, and transforms last.
pub fn type_priority(t: PluginType) -> u8 {
    match t {
        PluginType::Cors => 0,
        PluginType::KeyAuth | PluginType::BasicAuth => 1,
        PluginType::RateLimit => 2,
        PluginType::RequestTransform => 3,
        PluginType::ResponseTransform => 4,
    }
}

/// Parse a URL query string into key/value pairs (percent-decoding minimally).
pub(crate) fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (kv.to_string(), String::new()),
        })
        .collect()
}
