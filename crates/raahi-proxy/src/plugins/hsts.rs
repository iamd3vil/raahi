use serde::Deserialize;

use super::{Action, Effects, ReqInput};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HstsCfg {
    pub max_age_secs: u64,
    pub include_subdomains: bool,
    pub preload: bool,
}

impl Default for HstsCfg {
    fn default() -> Self {
        HstsCfg {
            max_age_secs: 63_072_000,
            include_subdomains: false,
            preload: false,
        }
    }
}

pub fn hsts(c: &HstsCfg, input: &ReqInput, effects: &mut Effects) -> Action {
    if !input.is_tls {
        return Action::Continue;
    }

    let mut value = format!("max-age={}", c.max_age_secs);
    if c.include_subdomains {
        value.push_str("; includeSubDomains");
    }
    if c.preload {
        value.push_str("; preload");
    }
    effects
        .resp_add
        .push(("Strict-Transport-Security".into(), value));
    Action::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn input(headers: &HeaderMap, is_tls: bool) -> ReqInput<'_> {
        ReqInput {
            method: "GET",
            path: "/",
            query: None,
            host: "example.com",
            is_tls,
            client_ip: None,
            headers,
            route_id: 1,
            service_id: 1,
            config_generation: 0,
        }
    }

    #[test]
    fn only_adds_hsts_to_tls_responses() {
        let headers = HeaderMap::new();
        let mut effects = Effects::default();
        hsts(&HstsCfg::default(), &input(&headers, false), &mut effects);
        assert!(effects.resp_add.is_empty());

        hsts(&HstsCfg::default(), &input(&headers, true), &mut effects);
        assert_eq!(
            effects.resp_add,
            vec![(
                "Strict-Transport-Security".into(),
                "max-age=63072000".into()
            )]
        );
    }

    #[test]
    fn formats_optional_directives() {
        let headers = HeaderMap::new();
        let mut effects = Effects::default();
        let cfg = HstsCfg {
            max_age_secs: 300,
            include_subdomains: true,
            preload: true,
        };

        hsts(&cfg, &input(&headers, true), &mut effects);
        assert_eq!(
            effects.resp_add[0].1,
            "max-age=300; includeSubDomains; preload"
        );
    }
}
