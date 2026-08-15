//! key-auth, basic-auth, and jwt plugins. On success they set the authenticated
//! consumer in [`Effects`]; on failure they short-circuit with 401.

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

// ---- jwt -----------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct JwtCfg {
    /// Claim used to look up the consumer's jwt credential (Kong-style `iss` lookup).
    pub key_claim_name: String,
    /// Also accept the token from these query parameters (besides `Authorization: Bearer`).
    pub uri_param_names: Vec<String>,
    /// Reject tokens without an `exp` claim.
    pub require_exp: bool,
}

impl Default for JwtCfg {
    fn default() -> Self {
        JwtCfg {
            key_claim_name: "iss".into(),
            uri_param_names: vec!["jwt".into()],
            require_exp: false,
        }
    }
}

fn jwt_reject(msg: &str) -> Action {
    Action::Respond(ShortResp {
        status: 401,
        headers: vec![
            ("www-authenticate".into(), "Bearer".into()),
            ("content-type".into(), "text/plain; charset=utf-8".into()),
        ],
        body: format!("Raahi: {msg}\n").into_bytes(),
    })
}

pub fn jwt_auth(c: &JwtCfg, input: &ReqInput, cfg: &ProxyConfig, effects: &mut Effects) -> Action {
    // Token from `Authorization: Bearer ...` or a configured query parameter.
    let mut token: Option<String> = input
        .headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
                .map(|t| t.trim().to_string())
        });
    if token.is_none() {
        if let Some(q) = input.query {
            for (k, v) in parse_query(q) {
                if c.uri_param_names.iter().any(|n| n.eq_ignore_ascii_case(&k)) {
                    token = Some(v);
                    break;
                }
            }
        }
    }
    let Some(token) = token else {
        return jwt_reject("missing JWT");
    };

    // Read the key claim from the (unverified) payload to find the credential.
    let mut parts = token.split('.');
    let (Some(_), Some(payload_b64), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        return jwt_reject("malformed JWT");
    };
    let Ok(payload_raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64)
    else {
        return jwt_reject("malformed JWT");
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&payload_raw) else {
        return jwt_reject("malformed JWT");
    };
    let Some(key) = claims[&c.key_claim_name].as_str() else {
        return jwt_reject("missing key claim");
    };
    let Some(cred) = cfg.jwt_index.get(key) else {
        return jwt_reject("unknown JWT credential");
    };

    // Verify signature + registered time claims with the credential's material.
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
    let (alg, dkey) = match cred.algorithm.as_str() {
        "HS256" => (Algorithm::HS256, Ok(DecodingKey::from_secret(cred.secret.as_bytes()))),
        "HS384" => (Algorithm::HS384, Ok(DecodingKey::from_secret(cred.secret.as_bytes()))),
        "HS512" => (Algorithm::HS512, Ok(DecodingKey::from_secret(cred.secret.as_bytes()))),
        "RS256" => (Algorithm::RS256, DecodingKey::from_rsa_pem(cred.secret.as_bytes())),
        _ => return jwt_reject("unsupported JWT algorithm"),
    };
    let Ok(dkey) = dkey else {
        return jwt_reject("invalid JWT credential key");
    };
    let mut validation = Validation::new(alg);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.validate_aud = false;
    validation.required_spec_claims = if c.require_exp {
        std::iter::once("exp".to_string()).collect()
    } else {
        Default::default()
    };
    if decode::<serde_json::Value>(&token, &dkey, &validation).is_err() {
        return jwt_reject("invalid JWT");
    }

    let username = cfg
        .consumers
        .get(&cred.consumer_id)
        .map(|c| c.username.clone())
        .unwrap_or_default();
    effects.consumer = Some((cred.consumer_id, username));
    Action::Continue
}
