//! Built-in plugin layer. Plugins are compiled once per config snapshot into
//! [`PluginSet`]; stateful ones (rate-limit) are carried over across reloads when
//! their config is unchanged, so counters survive unrelated config edits. The WASM
//! user-function phase will add a `WasmPlugin` variant implementing the same contract.

mod access;
mod auth;
mod cors;
mod ratelimit;
mod traffic;
mod transform;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use raahi_core::{Id, Plugin, PluginType, ProxyConfig};
use serde::Deserialize;

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
/// preflight, termination, redirect).
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

/// Config for the `http-log` plugin. Consumed by the log dispatcher (see
/// `crate::httplog`), not during the request phase.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct HttpLogCfg {
    /// Collector endpoint; request records are POSTed there as a JSON array.
    pub endpoint: String,
    /// Extra headers to send (e.g. an auth token).
    pub headers: BTreeMap<String, String>,
    /// Flush when this many records are buffered…
    pub batch_max: usize,
    /// …or after this long, whichever comes first.
    pub flush_interval_ms: u64,
}

impl Default for HttpLogCfg {
    fn default() -> Self {
        HttpLogCfg {
            endpoint: String::new(),
            headers: BTreeMap::new(),
            batch_max: 50,
            flush_interval_ms: 2000,
        }
    }
}

enum PluginInstance {
    KeyAuth(auth::KeyAuthCfg),
    BasicAuth(auth::BasicAuthCfg),
    Jwt(auth::JwtCfg),
    Acl(access::AclCfg),
    IpRestriction(access::IpRestrictionCfg),
    RateLimit(Arc<ratelimit::RateLimitState>),
    RequestSizeLimit(traffic::SizeLimitCfg),
    RequestTermination(traffic::TerminationCfg),
    Redirect(traffic::RedirectCfg),
    Cors(cors::CorsCfg),
    RequestTransform(transform::TransformCfg),
    ResponseTransform(transform::TransformCfg),
    HttpLog(HttpLogCfg),
}

/// Compiled plugin instances keyed by plugin id.
#[derive(Default)]
pub struct PluginSet {
    instances: HashMap<Id, PluginInstance>,
    /// Raw config per plugin id, used to decide whether stateful instances can be
    /// carried over on reload.
    configs: HashMap<Id, serde_json::Value>,
}

impl PluginSet {
    pub fn build(plugins: &[Plugin], prev: Option<&PluginSet>) -> Self {
        let mut instances = HashMap::new();
        let mut configs = HashMap::new();
        for p in plugins {
            let cfg = || p.config.clone();
            let inst = match p.plugin_type {
                PluginType::KeyAuth => {
                    PluginInstance::KeyAuth(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::BasicAuth => {
                    PluginInstance::BasicAuth(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::Jwt => {
                    PluginInstance::Jwt(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::Acl => {
                    PluginInstance::Acl(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::IpRestriction => PluginInstance::IpRestriction(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::RateLimit => {
                    // Carry over the counters when this plugin's config is unchanged.
                    let carried = prev.and_then(|ps| {
                        match (ps.instances.get(&p.id), ps.configs.get(&p.id)) {
                            (Some(PluginInstance::RateLimit(state)), Some(old))
                                if *old == p.config =>
                            {
                                Some(state.clone())
                            }
                            _ => None,
                        }
                    });
                    PluginInstance::RateLimit(carried.unwrap_or_else(|| {
                        Arc::new(ratelimit::RateLimitState::new(
                            serde_json::from_value(cfg()).unwrap_or_default(),
                        ))
                    }))
                }
                PluginType::RequestSizeLimit => PluginInstance::RequestSizeLimit(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::RequestTermination => PluginInstance::RequestTermination(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::Redirect => {
                    PluginInstance::Redirect(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::Cors => {
                    PluginInstance::Cors(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::RequestTransform => PluginInstance::RequestTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::ResponseTransform => PluginInstance::ResponseTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::HttpLog => {
                    PluginInstance::HttpLog(serde_json::from_value(cfg()).unwrap_or_default())
                }
            };
            instances.insert(p.id, inst);
            configs.insert(p.id, p.config.clone());
        }
        PluginSet { instances, configs }
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
            PluginInstance::Jwt(c) => auth::jwt_auth(c, input, cfg, effects),
            PluginInstance::Acl(c) => access::acl(c, input, cfg, effects),
            PluginInstance::IpRestriction(c) => access::ip_restriction(c, input),
            PluginInstance::RateLimit(s) => s.check(input, effects),
            PluginInstance::RequestSizeLimit(c) => traffic::size_limit(c, input),
            PluginInstance::RequestTermination(c) => traffic::terminate(c),
            PluginInstance::Redirect(c) => traffic::redirect(c, input),
            PluginInstance::Cors(c) => cors::cors(c, input, effects),
            PluginInstance::RequestTransform(c) => {
                c.apply_request(effects);
                Action::Continue
            }
            PluginInstance::ResponseTransform(c) => {
                c.apply_response(effects);
                Action::Continue
            }
            PluginInstance::HttpLog(_) => Action::Continue, // consumed in the log phase
        }
    }

    /// The compiled http-log config for a plugin id, if that plugin is an http-log.
    pub fn httplog_cfg(&self, id: Id) -> Option<&HttpLogCfg> {
        match self.instances.get(&id) {
            Some(PluginInstance::HttpLog(c)) => Some(c),
            _ => None,
        }
    }
}

/// Type-based execution precedence: CORS preflights answer first, network-level
/// checks precede auth, auth precedes ACL (which needs the consumer) and rate-limit
/// (which may key on the consumer), then shaping, then transforms.
pub fn type_priority(t: PluginType) -> u8 {
    match t {
        PluginType::Cors => 0,
        PluginType::IpRestriction => 1,
        PluginType::Jwt | PluginType::KeyAuth | PluginType::BasicAuth => 2,
        PluginType::Acl => 3,
        PluginType::RequestSizeLimit => 4,
        PluginType::RateLimit => 5,
        PluginType::RequestTermination => 6,
        PluginType::Redirect => 7,
        PluginType::RequestTransform => 8,
        PluginType::ResponseTransform => 9,
        PluginType::HttpLog => 10,
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
