//! Built-in plugin layer. Plugins are compiled once per config snapshot into
//! [`PluginSet`]; stateful ones (rate-limit) are carried over across reloads when
//! their config is unchanged, so counters survive unrelated config edits. The WASM
//! user-function phase will add a `WasmPlugin` variant implementing the same contract.

mod access;
mod auth;
mod bodytransform;
mod cache;
mod compression;
mod cors;
mod ratelimit;
mod requestid;
mod traffic;
mod transform;
mod wasm;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use raahi_core::{Id, Plugin, PluginType, ProxyConfig};
use serde::Deserialize;

pub use bodytransform::BodyTransformCfg;
pub use cache::{CacheIntent, purge_cache};
pub use compression::MAX_LEVEL as MAX_COMPRESSION_LEVEL;
pub use wasm::{validate_wasm, wat_to_wasm};

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

/// Read-only response inputs handed to plugins during the response phase.
pub struct RespInput<'a> {
    pub status: u16,
    pub headers: &'a http::HeaderMap,
}

/// Side effects a plugin accumulates: an authenticated consumer, request-header
/// mutations (applied upstream), response-header mutations (applied downstream),
/// and response-body intents (cache store / transform) consumed by the proxy's
/// body phase.
#[derive(Default)]
pub struct Effects {
    pub consumer: Option<(Id, String)>,
    pub req_add: Vec<(String, String)>,
    pub req_remove: Vec<String>,
    pub resp_add: Vec<(String, String)>,
    pub resp_remove: Vec<String>,
    /// proxy-cache miss: store the upstream response under this intent.
    pub cache_store: Option<CacheIntent>,
    /// response-body-transform: rewrite the response body with this config.
    pub body_transform: Option<BodyTransformCfg>,
    /// response-compression: enable Pingora's downstream compression module at this
    /// level (applied to the session by the proxy; plugins can't reach the session).
    pub compression_level: Option<u32>,
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
    ProxyCache(cache::CacheCfg),
    RequestSizeLimit(traffic::SizeLimitCfg),
    RequestTermination(traffic::TerminationCfg),
    Redirect(traffic::RedirectCfg),
    Cors(cors::CorsCfg),
    Wasm(wasm::WasmPlugin),
    RequestTransform(transform::TransformCfg),
    ResponseTransform(transform::TransformCfg),
    ResponseBodyTransform(bodytransform::BodyTransformCfg),
    HttpLog(HttpLogCfg),
    RequestId(requestid::RequestIdCfg),
    ResponseCompression(compression::CompressionCfg),
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
    pub fn build(data: &ProxyConfig, prev: Option<&PluginSet>) -> Self {
        let plugins = &data.plugins;
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
                PluginType::IpRestriction => {
                    PluginInstance::IpRestriction(serde_json::from_value(cfg()).unwrap_or_default())
                }
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
                PluginType::ProxyCache => {
                    PluginInstance::ProxyCache(serde_json::from_value(cfg()).unwrap_or_default())
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
                PluginType::Wasm => {
                    let wcfg: wasm::WasmCfg = serde_json::from_value(cfg()).unwrap_or_default();
                    let bytes = data.wasm_modules.get(&wcfg.module);
                    PluginInstance::Wasm(wasm::WasmPlugin::new(wcfg, bytes))
                }
                PluginType::RequestTransform => PluginInstance::RequestTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::ResponseTransform => PluginInstance::ResponseTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::ResponseBodyTransform => PluginInstance::ResponseBodyTransform(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
                PluginType::HttpLog => {
                    PluginInstance::HttpLog(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::RequestId => {
                    PluginInstance::RequestId(serde_json::from_value(cfg()).unwrap_or_default())
                }
                PluginType::ResponseCompression => PluginInstance::ResponseCompression(
                    serde_json::from_value(cfg()).unwrap_or_default(),
                ),
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
            PluginInstance::ProxyCache(c) => cache::check(c, input, effects),
            PluginInstance::RequestSizeLimit(c) => traffic::size_limit(c, input),
            PluginInstance::RequestTermination(c) => traffic::terminate(c),
            PluginInstance::Redirect(c) => traffic::redirect(c, input),
            PluginInstance::Cors(c) => cors::cors(c, input, effects),
            PluginInstance::Wasm(w) => w.on_request(input, effects),
            PluginInstance::RequestTransform(c) => {
                c.apply_request(effects);
                Action::Continue
            }
            PluginInstance::ResponseTransform(c) => {
                c.apply_response(effects);
                Action::Continue
            }
            PluginInstance::ResponseBodyTransform(c) => {
                effects.body_transform = Some(c.clone());
                Action::Continue
            }
            PluginInstance::HttpLog(_) => Action::Continue, // consumed in the log phase
            PluginInstance::RequestId(c) => requestid::request_id(c, input, effects),
            PluginInstance::ResponseCompression(c) => compression::compress(c, effects),
        }
    }

    /// Execute one plugin's response-phase logic (currently only wasm modules).
    pub fn run_response(&self, plugin: &Plugin, input: &RespInput, effects: &mut Effects) {
        if let Some(PluginInstance::Wasm(w)) = self.instances.get(&plugin.id) {
            w.on_response(input, effects);
        }
    }

    /// Whether any compiled plugin participates in the response phase.
    pub fn has_response_phase(&self) -> bool {
        self.instances
            .values()
            .any(|i| matches!(i, PluginInstance::Wasm(_)))
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
/// (which may key on the consumer), then the cache (so auth and limits still apply
/// to cached hits), then shaping, then transforms.
pub fn type_priority(t: PluginType) -> u8 {
    match t {
        // request-id runs before everything so short-circuited responses (auth
        // failures, rate limits) still carry the correlation id.
        PluginType::RequestId => 0,
        // response-compression only arms Pingora's downstream compression module, so
        // it runs early — before anything that can short-circuit (cache hits, auth
        // errors, redirects), which then get compressed too. Where the compression
        // itself happens in the body pipeline is fixed by the module, not by this.
        PluginType::ResponseCompression => 1,
        PluginType::Cors => 2,
        PluginType::IpRestriction => 3,
        PluginType::Jwt | PluginType::KeyAuth | PluginType::BasicAuth => 4,
        PluginType::Acl => 5,
        PluginType::RequestSizeLimit => 6,
        PluginType::RateLimit => 7,
        PluginType::ProxyCache => 8,
        PluginType::Wasm => 9,
        PluginType::RequestTermination => 10,
        PluginType::Redirect => 11,
        PluginType::RequestTransform => 12,
        PluginType::ResponseTransform => 13,
        PluginType::ResponseBodyTransform => 14,
        PluginType::HttpLog => 15,
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
