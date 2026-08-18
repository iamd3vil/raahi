//! `request-id`: tags every request with a correlation id (UUID v4), forwarded
//! upstream and optionally echoed downstream, so one request can be traced across
//! the proxy, upstream logs, and http-log records.

use serde::Deserialize;

use super::{Action, Effects, ReqInput};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RequestIdCfg {
    /// Header carrying the id, on both the upstream request and the response.
    pub header_name: String,
    /// Keep an id the client already sent (gateway chains); false always overwrites.
    pub preserve: bool,
    /// Also set the header on the downstream response.
    pub echo_downstream: bool,
}

impl Default for RequestIdCfg {
    fn default() -> Self {
        RequestIdCfg {
            header_name: "X-Request-Id".into(),
            preserve: true,
            echo_downstream: true,
        }
    }
}

pub fn request_id(cfg: &RequestIdCfg, input: &ReqInput, effects: &mut Effects) -> Action {
    let incoming = if cfg.preserve {
        input
            .headers
            .get(&cfg.header_name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
    } else {
        None
    };
    let id = incoming.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    effects.req_add.push((cfg.header_name.clone(), id.clone()));
    if cfg.echo_downstream {
        effects.resp_add.push((cfg.header_name.clone(), id));
    }
    Action::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(headers: &http::HeaderMap) -> ReqInput<'_> {
        ReqInput {
            method: "GET",
            path: "/",
            query: None,
            host: "h",
            client_ip: None,
            headers,
            route_id: 1,
        }
    }

    #[test]
    fn generates_and_echoes_an_id() {
        let headers = http::HeaderMap::new();
        let mut fx = Effects::default();
        request_id(&RequestIdCfg::default(), &input(&headers), &mut fx);
        let (name, id) = &fx.req_add[0];
        assert_eq!(name, "X-Request-Id");
        assert_eq!(id.len(), 36); // uuid v4
        assert_eq!(fx.resp_add[0].1, *id);
    }

    #[test]
    fn preserves_incoming_id_by_default_but_not_when_disabled() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-request-id", "abc-123".parse().unwrap());
        let mut fx = Effects::default();
        request_id(&RequestIdCfg::default(), &input(&headers), &mut fx);
        assert_eq!(fx.req_add[0].1, "abc-123");

        let mut fx = Effects::default();
        let cfg = RequestIdCfg {
            preserve: false,
            ..Default::default()
        };
        request_id(&cfg, &input(&headers), &mut fx);
        assert_ne!(fx.req_add[0].1, "abc-123");
    }
}
