//! The Pingora `ProxyHttp` implementation: per-request route matching, plugin
//! execution (auth / rate-limit / CORS / transforms), upstream selection, path/host
//! rewriting, and access logging. Reads the hot-swappable [`ConfigHandle`] each request.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::modules::http::HttpModules;
use pingora::modules::http::compression::{ResponseCompression, ResponseCompressionBuilder};
use pingora::prelude::*;
use pingora::{Error, ErrorType};
use raahi_core::{Id, strip_prefix};

use crate::httplog::{LogEvent, LogSender};
use crate::metrics::{Metrics, RequestRecord};
use crate::plugins::{
    Action, BodyTransformCfg, CacheIntent, Effects, ReqInput, RespInput, ShortResp,
};
use crate::runtime::ConfigHandle;

/// The data-plane service.
pub struct RaahiProxy {
    pub config: ConfigHandle,
    pub metrics: Arc<Metrics>,
    /// Channel to the http-log delivery service.
    pub log_tx: LogSender,
    /// Round-robin counter for routes with weighted traffic splits.
    pub split_counter: AtomicUsize,
}

/// Per-request state threaded across filter phases.
#[derive(Default)]
pub struct Ctx {
    pub route_id: Option<Id>,
    pub service_id: Option<Id>,
    matched_prefix: String,
    strip_path: bool,
    preserve_host: bool,
    upstream_host: Option<String>,
    upstream_addr: Option<String>,
    client_ip: Option<String>,
    is_tls: bool,
    pub consumer: Option<String>,
    req_add: Vec<(String, String)>,
    req_remove: Vec<String>,
    resp_add: Vec<(String, String)>,
    resp_remove: Vec<String>,
    /// proxy-cache miss: armed intent to store the response (dropped on non-200 or
    /// oversized bodies).
    cache_store: Option<CacheIntent>,
    /// Response headers captured for the cache entry (hop-by-hop excluded).
    cache_headers: Vec<(String, String)>,
    /// Response body accumulated for the cache entry (a copy; passthrough untouched).
    cache_buf: Vec<u8>,
    /// response-body-transform: armed config (dropped on content-type mismatch or
    /// oversized bodies).
    body_transform: Option<BodyTransformCfg>,
    /// Body chunks withheld from downstream until end-of-stream for transformation.
    transform_buf: Vec<u8>,
    method: String,
    host: String,
    path: String,
    start: Option<Instant>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Client IP as a string, if the connection is over IP (not a unix socket).
fn client_ip(session: &Session) -> Option<String> {
    session
        .client_addr()
        .and_then(|a| a.as_inet())
        .map(|s| s.ip().to_string())
}

/// Extract the request host (without port), preferring the `Host` header.
fn req_host(req: &RequestHeader) -> String {
    if let Some(h) = req.headers.get("host") {
        if let Ok(s) = h.to_str() {
            return s.split(':').next().unwrap_or(s).to_string();
        }
    }
    req.uri.host().map(|s| s.to_string()).unwrap_or_default()
}

/// Write a plugin-produced short-circuit response.
async fn send_response(session: &mut Session, r: &ShortResp) -> Result<()> {
    let mut resp = ResponseHeader::build(r.status, None)?;
    for (k, v) in &r.headers {
        let _ = resp.insert_header(k.clone(), v.as_str());
    }
    if !r
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
    {
        let _ = resp.insert_header("content-length", r.body.len().to_string());
    }
    session.set_keepalive(None);
    session.write_response_header(Box::new(resp), false).await?;
    session
        .write_response_body(Some(Bytes::copy_from_slice(&r.body)), true)
        .await?;
    Ok(())
}

#[async_trait]
impl ProxyHttp for RaahiProxy {
    type CTX = Ctx;

    fn new_ctx(&self) -> Self::CTX {
        Ctx::default()
    }

    /// Register Pingora's downstream response-compression module at level 0, i.e.
    /// present but disabled: the `response-compression` plugin turns it on per
    /// request in [`Self::request_filter`]. The module's body filter runs after
    /// `ProxyHttp::response_body_filter` (see `Session::write_response_body`), so the
    /// proxy-cache capture and the response-body transform always see — and store —
    /// uncompressed bytes; compression is applied last, on the way downstream.
    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        modules.add_module(ResponseCompressionBuilder::enable(0));
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        ctx.start = Some(Instant::now());

        // Everything up to the plugin verdict borrows the request header in place
        // (no HeaderMap clone on the hot path); session writes (error responses,
        // compression module) happen after the borrow ends, driven by `verdict`.
        enum Verdict {
            NoRoute,
            NoBackend,
            Short(ShortResp),
            Proceed,
        }

        let (verdict, compression_level): (Verdict, Option<u32>) = 'matched: {
            let is_tls = session
                .as_downstream()
                .digest()
                .is_some_and(|digest| digest.ssl_digest.is_some());
            ctx.is_tls = is_tls;
            let req = session.req_header();
            ctx.client_ip = client_ip(session);
            ctx.method = req.method.as_str().to_string();
            ctx.path = req.uri.path().to_string();
            ctx.host = req_host(req);
            let query = req.uri.query();

            // Match, validate backend, and run request-phase plugins — all under one
            // read of the live config.
            let rc = self.config.load();
            let header = |name: &str| {
                req.headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            };
            let Some(m) = rc
                .data
                .match_route(&ctx.host, &ctx.path, &ctx.method, &header)
            else {
                break 'matched (Verdict::NoRoute, None);
            };
            let route_id = m.route.id;
            // Weighted traffic split: pick a service by round-robin over the split
            // weights, ignoring entries whose service vanished from the snapshot.
            let service_id = if m.route.splits.is_empty() {
                m.service.id
            } else {
                let valid: Vec<_> = m
                    .route
                    .splits
                    .iter()
                    .filter(|sp| rc.services.contains_key(&sp.service_id))
                    .collect();
                let total: u32 = valid.iter().map(|sp| sp.weight).sum();
                if total == 0 {
                    m.service.id // no valid split entry — fall back to the route's service
                } else {
                    let mut n = (self.split_counter.fetch_add(1, Ordering::Relaxed)
                        % total as usize) as u32;
                    let mut picked = m.service.id;
                    for sp in &valid {
                        if n < sp.weight {
                            picked = sp.service_id;
                            break;
                        }
                        n -= sp.weight;
                    }
                    picked
                }
            };
            ctx.route_id = Some(route_id);
            ctx.service_id = Some(service_id);
            ctx.matched_prefix = m.matched_prefix.clone();
            ctx.strip_path = m.route.strip_path;
            ctx.preserve_host = m.route.preserve_host;

            let has_backend = rc
                .services
                .get(&service_id)
                .map(|sr| !sr.backends.is_empty())
                .unwrap_or(false);
            if !has_backend {
                break 'matched (Verdict::NoBackend, None);
            }

            let ordered = rc.plugins_ordered(route_id, service_id);

            let input = ReqInput {
                method: &ctx.method,
                path: &ctx.path,
                query,
                host: &ctx.host,
                is_tls,
                client_ip: ctx.client_ip.as_deref(),
                headers: &req.headers,
                route_id,
                service_id,
                config_generation: rc.generation,
            };

            let mut effects = Effects::default();
            let mut short = None;
            for p in ordered.iter() {
                match rc
                    .plugins
                    .run_request(p, &input, rc.data.as_ref(), &mut effects)
                {
                    Action::Continue => {}
                    Action::Respond(r) => {
                        short = Some(r);
                        break;
                    }
                }
            }

            ctx.consumer = effects.consumer.map(|(_, u)| u);
            ctx.req_add = effects.req_add;
            ctx.req_remove = effects.req_remove;
            ctx.resp_add = effects.resp_add;
            ctx.resp_remove = effects.resp_remove;
            ctx.cache_store = effects.cache_store;
            ctx.body_transform = effects.body_transform;
            match short {
                Some(r) => (Verdict::Short(r), effects.compression_level),
                None => (Verdict::Proceed, effects.compression_level),
            }
        };

        match verdict {
            Verdict::NoRoute => {
                session
                    .respond_error_with_body(404, Bytes::from_static(b"Raahi: no matching route\n"))
                    .await?;
                return Ok(true);
            }
            Verdict::NoBackend => {
                session
                    .respond_error_with_body(
                        502,
                        Bytes::from_static(b"Raahi: no upstream backend\n"),
                    )
                    .await?;
                return Ok(true);
            }
            Verdict::Short(_) | Verdict::Proceed => {}
        }

        // response-compression: hand the plugin's decision to Pingora's downstream
        // module. Its request filter has already run (before `request_filter`) and
        // bailed out early because the module was disabled, so the Accept-Encoding
        // list still has to be fed in here, after enabling it.
        if let Some(level) = compression_level {
            let req = session.downstream_session.req_header().clone();
            if let Some(c) = session
                .downstream_modules_ctx
                .get_mut::<ResponseCompression>()
            {
                c.adjust_level(level);
                c.request_filter(&req);
            }
        }

        if let Verdict::Short(mut r) = verdict {
            // Response-header effects staged by plugins that ran before the
            // short-circuit still apply (correlation ids, CORS headers on errors).
            for (k, v) in ctx.resp_add.drain(..) {
                if !r.headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(&k)) {
                    r.headers.push((k, v));
                }
            }
            send_response(session, &r).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let service_id = ctx
            .service_id
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "no matched service"))?;

        let rc = self.config.load();
        let sr = rc
            .services
            .get(&service_id)
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "service vanished"))?;

        let key = client_ip(session).unwrap_or_default();
        let backend = sr
            .select(key.as_bytes())
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "no healthy upstream"))?;

        let svc = &sr.service;
        let tls = svc.protocol.is_tls();
        let sni = svc.tls_sni.clone().unwrap_or_else(|| backend.host.clone());

        ctx.upstream_host = Some(backend.host.clone());
        ctx.upstream_addr = Some(backend.addr_string());

        // Connect to the health checker's elected address so both sides share one
        // view of a multi-address host (e.g. localhost as ::1 vs 127.0.0.1). Fall
        // back to the raw host:port only when resolution failed at snapshot build.
        let mut peer = match backend.active_addr() {
            Some(addr) => HttpPeer::new(addr, tls, sni),
            None => HttpPeer::new((backend.host.as_str(), backend.port), tls, sni),
        };
        peer.options.connection_timeout = Some(Duration::from_millis(svc.connect_timeout_ms));
        peer.options.read_timeout = Some(Duration::from_millis(svc.read_timeout_ms));
        peer.options.write_timeout = Some(Duration::from_millis(svc.write_timeout_ms));
        Ok(Box::new(peer))
    }

    /// Passive circuit breaking: a failed connect ejects the backend immediately;
    /// the active health checker recovers it once probes pass again.
    fn fail_to_connect(
        &self,
        _session: &mut Session,
        peer: &HttpPeer,
        ctx: &mut Self::CTX,
        e: Box<Error>,
    ) -> Box<Error> {
        if let (Some(sid), Some(addr)) = (ctx.service_id, ctx.upstream_addr.as_deref()) {
            let rc = self.config.load();
            if let Some(sr) = rc.services.get(&sid) {
                for b in &sr.backends {
                    if b.addr_string() == addr
                        && b.healthy.swap(false, std::sync::atomic::Ordering::Relaxed)
                    {
                        tracing::warn!("health: {addr} marked down (connect failed: {peer})");
                    }
                }
            }
        }
        e
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Strip the matched path prefix if requested.
        if ctx.strip_path && !ctx.matched_prefix.is_empty() {
            let new_path = strip_prefix(&ctx.matched_prefix, upstream_request.uri.path());
            let new_pq = match upstream_request.uri.query() {
                Some(q) => format!("{new_path}?{q}"),
                None => new_path,
            };
            if let Ok(uri) = new_pq.parse::<http::Uri>() {
                let _ = upstream_request.set_uri(uri);
            }
        }

        // Host handling: present the upstream's host unless the route preserves the original.
        if !ctx.preserve_host {
            if let Some(h) = &ctx.upstream_host {
                let _ = upstream_request.insert_header("host", h.as_str());
            }
        }

        // Standard forwarding headers.
        if let Some(ip) = &ctx.client_ip {
            let _ = upstream_request.insert_header("x-forwarded-for", ip.as_str());
        }
        if !ctx.host.is_empty() {
            let _ = upstream_request.insert_header("x-forwarded-host", ctx.host.as_str());
        }
        let _ = upstream_request.insert_header(
            "x-forwarded-proto",
            if ctx.is_tls { "https" } else { "http" },
        );

        coalesce_cookie_headers(upstream_request)?;

        // Plugin request-header transforms.
        for k in &ctx.req_remove {
            upstream_request.remove_header(k.as_str());
        }
        for (k, v) in &ctx.req_add {
            let _ = upstream_request.insert_header(k.clone(), v.as_str());
        }
        Ok(())
    }

    async fn response_filter(
        &self,
        _session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Response phase: plugins that react to the upstream response (wasm modules).
        if let (Some(rid), Some(sid)) = (ctx.route_id, ctx.service_id) {
            let rc = self.config.load();
            if rc.plugins.has_response_phase() {
                let input_headers = upstream_response.headers.clone();
                let input = RespInput {
                    status: upstream_response.status.as_u16(),
                    headers: &input_headers,
                };
                let ordered = rc.plugins_ordered(rid, sid);
                let mut effects = Effects::default();
                for p in ordered.iter() {
                    rc.plugins.run_response(p, &input, &mut effects);
                }
                ctx.resp_remove.extend(effects.resp_remove);
                ctx.resp_add.extend(effects.resp_add);
            }
        }

        // response-body-transform: only text-ish content-types are eligible; the body
        // length will change, so drop Content-Length and re-chunk.
        if let Some(cfg) = &ctx.body_transform {
            let ct = upstream_response
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if cfg.matches_content_type(ct) {
                upstream_response.remove_header("content-length");
                let _ = upstream_response.insert_header("transfer-encoding", "chunked");
            } else {
                ctx.body_transform = None;
            }
        }

        // proxy-cache: only 200s are stored. Capture the upstream headers for the
        // cache entry (hop-by-hop and content-length excluded — the body may be
        // re-chunked) and mark this response as a miss.
        if ctx.cache_store.is_some() {
            if ctx.cache_store.as_ref().unwrap().accepts_response(
                upstream_response.status.as_u16(),
                &upstream_response.headers,
            ) {
                ctx.cache_headers = upstream_response
                    .headers
                    .iter()
                    .filter_map(|(k, v)| {
                        let name = k.as_str();
                        if [
                            "connection",
                            "keep-alive",
                            "transfer-encoding",
                            "content-length",
                        ]
                        .contains(&name)
                        {
                            return None;
                        }
                        v.to_str().ok().map(|v| (name.to_string(), v.to_string()))
                    })
                    .collect();
                ctx.resp_add.push(("x-cache".into(), "MISS".into()));
            } else {
                ctx.cache_store = None;
            }
        }

        for k in &ctx.resp_remove {
            upstream_response.remove_header(k.as_str());
        }
        for (k, v) in &ctx.resp_add {
            let _ = upstream_response.insert_header(k.clone(), v.as_str());
        }
        Ok(())
    }

    /// Response body phase: the transform runs first (rewriting the passthrough
    /// bytes), then the cache capture — so a cache entry always stores the final,
    /// transformed body when both plugins are active.
    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>> {
        if let Some(cfg) = &ctx.body_transform {
            // Withhold chunks until end-of-stream, then rewrite the whole body.
            if let Some(b) = body.take() {
                ctx.transform_buf.extend_from_slice(&b);
            }
            if ctx.transform_buf.len() as u64 > cfg.max_body_bytes {
                // Too large to transform: flush what was withheld unmodified and
                // pass the rest of the stream through.
                *body = Some(Bytes::from(std::mem::take(&mut ctx.transform_buf)));
                ctx.body_transform = None;
            } else if end_of_stream {
                let buf = std::mem::take(&mut ctx.transform_buf);
                // Only valid UTF-8 is transformed; binary passes through unchanged.
                let out = match String::from_utf8(buf) {
                    Ok(s) => Bytes::from(cfg.apply(s)),
                    Err(e) => Bytes::from(e.into_bytes()),
                };
                *body = Some(out);
                ctx.body_transform = None;
            }
        }

        if let Some(intent) = &ctx.cache_store {
            let incoming = body.as_ref().map_or(0, |b| b.len());
            if ctx.cache_buf.len().saturating_add(incoming) as u64 > intent.max_body_bytes {
                // Too large to cache: disarm and drop the copy; passthrough unaffected.
                ctx.cache_store = None;
                ctx.cache_buf = Vec::new();
            } else {
                if let Some(b) = body.as_ref() {
                    ctx.cache_buf.extend_from_slice(b);
                }
                if end_of_stream {
                    let intent = ctx.cache_store.take().unwrap();
                    // Status is always 200 here (non-200s are disarmed in response_filter).
                    intent.store(
                        200,
                        std::mem::take(&mut ctx.cache_headers),
                        std::mem::take(&mut ctx.cache_buf),
                    );
                }
            }
        }
        Ok(None)
    }

    async fn logging(&self, session: &mut Session, _e: Option<&Error>, ctx: &mut Self::CTX) {
        let status = session
            .response_written()
            .map(|r| r.status.as_u16())
            .unwrap_or(0);
        // Microsecond precision, rounded to 3 decimals for clean JSON.
        let latency_ms = ctx
            .start
            .map(|s| (s.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0)
            .unwrap_or(0.0);

        let record = RequestRecord {
            ts_ms: now_ms(),
            method: std::mem::take(&mut ctx.method),
            host: std::mem::take(&mut ctx.host),
            path: std::mem::take(&mut ctx.path),
            status,
            latency_ms,
            route_id: ctx.route_id,
            service_id: ctx.service_id,
            upstream: ctx.upstream_addr.take(),
            consumer: ctx.consumer.take(),
        };

        // http-log: forward the record to every applicable log plugin's collector.
        {
            let rc = self.config.load();
            let chain; // keeps the matched chain alive for the borrow below
            let logs: Vec<&raahi_core::Plugin> = match (ctx.route_id, ctx.service_id) {
                (Some(rid), Some(sid)) => {
                    chain = rc.plugins_ordered(rid, sid);
                    chain
                        .iter()
                        .filter(|p| p.plugin_type == raahi_core::PluginType::HttpLog)
                        .collect()
                }
                // Unmatched requests are still visible to global log plugins.
                _ => rc
                    .data
                    .plugins
                    .iter()
                    .filter(|p| {
                        p.enabled
                            && p.plugin_type == raahi_core::PluginType::HttpLog
                            && p.scope == raahi_core::PluginScope::Global
                    })
                    .collect(),
            };
            for p in logs {
                if let Some(cfg) = rc.plugins.httplog_cfg(p.id) {
                    let _ = self.log_tx.send(LogEvent {
                        cfg: cfg.clone(),
                        record: record.clone(),
                    });
                }
            }
        }

        self.metrics.record(record);
    }
}

// HTTP/2 may split cookies across fields (RFC 9113 section 8.2.3).
// HTTP/1 backends need one field joined with "; ", never commas.
fn coalesce_cookie_headers(request: &mut RequestHeader) -> Result<()> {
    let mut values = request.headers.get_all(http::header::COOKIE).iter();
    let Some(first) = values.next() else {
        return Ok(());
    };
    let Some(second) = values.next() else {
        return Ok(());
    };
    let mut joined = first.as_bytes().to_vec();
    for value in std::iter::once(second).chain(values) {
        joined.extend_from_slice(b"; ");
        joined.extend_from_slice(value.as_bytes());
    }
    request.insert_header("cookie", joined)?;
    Ok(())
}

#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn split_cookies_preserve_csrf_and_session_in_order() {
        let mut request = RequestHeader::build("POST", b"/accounts/login/", None).unwrap();
        for value in [
            "messages=notice",
            "paperless_csrftoken=csrf; preference=dark",
            "paperless_sessionid=session",
        ] {
            request.append_header("cookie", value).unwrap();
        }
        request.append_header("x-test", "untouched").unwrap();
        coalesce_cookie_headers(&mut request).unwrap();
        assert_eq!(request.headers.get_all("cookie").iter().count(), 1);
        assert_eq!(
            request.headers["cookie"],
            "messages=notice; paperless_csrftoken=csrf; preference=dark; paperless_sessionid=session"
        );
        assert_eq!(request.headers["x-test"], "untouched");
    }

    #[test]
    fn single_cookie_is_unchanged() {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.append_header("cookie", "a=1; b=2").unwrap();
        coalesce_cookie_headers(&mut request).unwrap();
        assert_eq!(request.headers["cookie"], "a=1; b=2");
    }

    #[test]
    fn absent_cookie_stays_absent() {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        coalesce_cookie_headers(&mut request).unwrap();
        assert!(!request.headers.contains_key("cookie"));
    }
}
