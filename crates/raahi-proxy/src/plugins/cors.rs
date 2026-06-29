//! CORS plugin. Answers preflight `OPTIONS` requests directly (204) and adds the
//! appropriate `Access-Control-*` headers to actual responses via [`Effects`].

use serde::Deserialize;

use super::{Action, Effects, ReqInput, ShortResp};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CorsCfg {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub expose_headers: Vec<String>,
    pub allow_credentials: bool,
    pub max_age: u32,
}

impl Default for CorsCfg {
    fn default() -> Self {
        CorsCfg {
            allow_origins: vec!["*".into()],
            allow_methods: vec![
                "GET".into(),
                "POST".into(),
                "PUT".into(),
                "PATCH".into(),
                "DELETE".into(),
                "OPTIONS".into(),
            ],
            allow_headers: vec!["*".into()],
            expose_headers: vec![],
            allow_credentials: false,
            max_age: 3600,
        }
    }
}

impl CorsCfg {
    fn allows(&self, origin: &str) -> bool {
        self.allow_origins.iter().any(|o| o == "*" || o == origin)
    }

    fn base_headers(&self, allow_origin: &str) -> Vec<(String, String)> {
        let mut v = vec![(
            "access-control-allow-origin".into(),
            allow_origin.to_string(),
        )];
        if self.allow_credentials {
            v.push(("access-control-allow-credentials".into(), "true".into()));
        }
        if allow_origin != "*" {
            v.push(("vary".into(), "Origin".into()));
        }
        v
    }
}

pub fn cors(c: &CorsCfg, input: &ReqInput, effects: &mut Effects) -> Action {
    let origin = input
        .headers
        .get("origin")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);

    // Resolve the Allow-Origin value. With credentials we must echo the concrete
    // origin (never "*").
    let allow_origin = match &origin {
        Some(o) if c.allows(o) => {
            if c.allow_credentials || !c.allow_origins.iter().any(|x| x == "*") {
                o.clone()
            } else {
                "*".to_string()
            }
        }
        None if c.allow_origins.iter().any(|x| x == "*") && !c.allow_credentials => "*".to_string(),
        _ => return Action::Continue, // origin not allowed: emit no CORS headers
    };

    // Preflight: answer immediately.
    if input.method.eq_ignore_ascii_case("OPTIONS") {
        let mut headers = c.base_headers(&allow_origin);
        headers.push((
            "access-control-allow-methods".into(),
            c.allow_methods.join(", "),
        ));
        if !c.allow_headers.is_empty() {
            headers.push((
                "access-control-allow-headers".into(),
                c.allow_headers.join(", "),
            ));
        }
        if c.max_age > 0 {
            headers.push(("access-control-max-age".into(), c.max_age.to_string()));
        }
        headers.push(("content-length".into(), "0".into()));
        return Action::Respond(ShortResp {
            status: 204,
            headers,
            body: vec![],
        });
    }

    // Actual request: stage response headers.
    for h in c.base_headers(&allow_origin) {
        effects.resp_add.push(h);
    }
    if !c.expose_headers.is_empty() {
        effects.resp_add.push((
            "access-control-expose-headers".into(),
            c.expose_headers.join(", "),
        ));
    }
    Action::Continue
}
