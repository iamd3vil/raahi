//! The Pingora `ProxyHttp` implementation: per-request route matching, plugin
//! execution (auth / rate-limit / CORS / transforms), upstream selection, path/host
//! rewriting, and access logging. Reads the hot-swappable [`ConfigHandle`] each request.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::prelude::*;
use pingora::{Error, ErrorType};
use raahi_core::{strip_prefix, Id, Plugin};

use crate::metrics::{Metrics, RequestRecord};
use crate::plugins::{type_priority, Action, Effects, ReqInput, ShortResp};
use crate::runtime::ConfigHandle;

/// The data-plane service.
pub struct RaahiProxy {
    pub config: ConfigHandle,
    pub metrics: Arc<Metrics>,
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
    pub consumer: Option<String>,
    req_add: Vec<(String, String)>,
    req_remove: Vec<String>,
    resp_add: Vec<(String, String)>,
    resp_remove: Vec<String>,
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
    if !r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length")) {
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

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        ctx.start = Some(Instant::now());

        let (method, path, host, query, req_headers) = {
            let req = session.req_header();
            (
                req.method.as_str().to_string(),
                req.uri.path().to_string(),
                req_host(req),
                req.uri.query().map(|s| s.to_string()),
                req.headers.clone(),
            )
        };
        let client = client_ip(session);
        ctx.client_ip = client.clone();
        ctx.method = method.clone();
        ctx.path = path.clone();
        ctx.host = host.clone();

        // Match, validate backend, and run request-phase plugins — all under one read
        // of the live config. Produces an optional short-circuit response.
        let short: Option<ShortResp> = {
            let rc = self.config.load();
            let Some(m) = rc.data.match_route(&host, &path, &method) else {
                session
                    .respond_error_with_body(404, Bytes::from_static(b"Raahi: no matching route\n"))
                    .await?;
                return Ok(true);
            };
            let route_id = m.route.id;
            let service_id = m.service.id;
            ctx.route_id = Some(route_id);
            ctx.service_id = Some(service_id);
            ctx.matched_prefix = m.matched_prefix.to_string();
            ctx.strip_path = m.route.strip_path;
            ctx.preserve_host = m.route.preserve_host;

            let has_backend = rc
                .services
                .get(&service_id)
                .map(|sr| !sr.backends.is_empty())
                .unwrap_or(false);
            if !has_backend {
                session
                    .respond_error_with_body(502, Bytes::from_static(b"Raahi: no upstream backend\n"))
                    .await?;
                return Ok(true);
            }

            let mut ordered: Vec<&Plugin> = rc.data.plugins_for(route_id, service_id);
            ordered.sort_by_key(|p| (type_priority(p.plugin_type), p.ordering, p.id));

            let input = ReqInput {
                method: &method,
                path: &path,
                query: query.as_deref(),
                host: &host,
                client_ip: client.as_deref(),
                headers: &req_headers,
                route_id,
            };

            let mut effects = Effects::default();
            let mut short = None;
            for p in &ordered {
                match rc.plugins.run_request(p, &input, rc.data.as_ref(), &mut effects) {
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
            short
        };

        if let Some(r) = short {
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
        ctx.upstream_addr = Some(format!("{}:{}", backend.host, backend.port));

        let mut peer = HttpPeer::new((backend.host.as_str(), backend.port), tls, sni);
        peer.options.connection_timeout = Some(Duration::from_millis(svc.connect_timeout_ms));
        peer.options.read_timeout = Some(Duration::from_millis(svc.read_timeout_ms));
        peer.options.write_timeout = Some(Duration::from_millis(svc.write_timeout_ms));
        Ok(Box::new(peer))
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
        let _ = upstream_request.insert_header("x-forwarded-proto", "http");

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
        for k in &ctx.resp_remove {
            upstream_response.remove_header(k.as_str());
        }
        for (k, v) in &ctx.resp_add {
            let _ = upstream_response.insert_header(k.clone(), v.as_str());
        }
        Ok(())
    }

    async fn logging(&self, session: &mut Session, _e: Option<&Error>, ctx: &mut Self::CTX) {
        let status = session
            .response_written()
            .map(|r| r.status.as_u16())
            .unwrap_or(0);
        let latency_ms = ctx
            .start
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);

        self.metrics.record(RequestRecord {
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
        });
    }
}
