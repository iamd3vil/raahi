//! key-auth and basic-auth plugins. On success they set the authenticated consumer
//! in [`Effects`]; on failure they short-circuit with 401.

use base64::Engine;
use raahi_core::ProxyConfig;
use serde::Deserialize;

use super::{parse_query, Action, Effects, ReqInput, ShortResp};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeyAuthCfg {
    /// Header and query parameter names to look for the API key in.
    pub key_names: Vec<String>,
    /// Strip the key header before forwarding upstream.
    pub hide_credentials: bool,
}

impl Default for KeyAuthCfg {
    fn default() -> Self {
        KeyAuthCfg {
            key_names: vec!["apikey".into(), "x-api-key".into()],
            hide_credentials: false,
        }
    }
}

pub fn key_auth(c: &KeyAuthCfg, input: &ReqInput, cfg: &ProxyConfig, effects: &mut Effects) -> Action {
    // Look in headers first, then query parameters.
    let mut key: Option<String> = None;
    for name in &c.key_names {
        if let Some(v) = input.headers.get(name).and_then(|h| h.to_str().ok()) {
            key = Some(v.to_string());
            break;
        }
    }
    if key.is_none() {
        if let Some(q) = input.query {
            for (k, v) in parse_query(q) {
                if c.key_names.iter().any(|n| n.eq_ignore_ascii_case(&k)) {
                    key = Some(v);
                    break;
                }
            }
        }
    }

    match key.as_deref().and_then(|k| cfg.key_index.get(k).copied()) {
        Some(cid) => {
            let username = cfg
                .consumers
                .get(&cid)
                .map(|c| c.username.clone())
                .unwrap_or_default();
            effects.consumer = Some((cid, username));
            if c.hide_credentials {
                for n in &c.key_names {
                    effects.req_remove.push(n.clone());
                }
            }
            Action::Continue
        }
        None => Action::Respond(ShortResp::text(401, "Raahi: invalid or missing API key\n")),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BasicAuthCfg {
    pub realm: String,
}

pub fn basic_auth(
    c: &BasicAuthCfg,
    input: &ReqInput,
    cfg: &ProxyConfig,
    effects: &mut Effects,
) -> Action {
    let realm = if c.realm.is_empty() { "Raahi" } else { &c.realm };
    let unauthorized = || {
        ShortResp {
            status: 401,
            headers: vec![(
                "www-authenticate".into(),
                format!("Basic realm=\"{realm}\""),
            )],
            body: b"Raahi: unauthorized\n".to_vec(),
        }
    };

    let Some(auth) = input.headers.get("authorization").and_then(|h| h.to_str().ok()) else {
        return Action::Respond(unauthorized());
    };
    let Some(b64) = auth
        .strip_prefix("Basic ")
        .or_else(|| auth.strip_prefix("basic "))
    else {
        return Action::Respond(unauthorized());
    };
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return Action::Respond(unauthorized());
    };
    let Ok(creds) = String::from_utf8(raw) else {
        return Action::Respond(unauthorized());
    };
    let Some((user, pass)) = creds.split_once(':') else {
        return Action::Respond(unauthorized());
    };

    match cfg.basic_index.get(user) {
        Some((cid, hash)) if bcrypt::verify(pass, hash).unwrap_or(false) => {
            let username = cfg
                .consumers
                .get(cid)
                .map(|c| c.username.clone())
                .unwrap_or_else(|| user.to_string());
            effects.consumer = Some((*cid, username));
            Action::Continue
        }
        _ => Action::Respond(unauthorized()),
    }
}
