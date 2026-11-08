//! key-auth, basic-auth, and jwt plugins. On success they set the authenticated
//! consumer in [`Effects`]; on failure they short-circuit with 401.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use raahi_core::ProxyConfig;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Action, Effects, ReqInput, ShortResp, parse_query};

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

pub fn key_auth(
    c: &KeyAuthCfg,
    input: &ReqInput,
    cfg: &ProxyConfig,
    effects: &mut Effects,
) -> Action {
    // Look in headers first, then query parameters.
    let mut key: Option<String> = None;
    for name in &c.key_names {
        if let Some(v) = input.headers.get(name).and_then(|h| h.to_str().ok()) {
            key = Some(v.to_string());
            break;
        }
    }
    if key.is_none()
        && let Some(q) = input.query
    {
        for (k, v) in parse_query(q) {
            if c.key_names.iter().any(|n| n.eq_ignore_ascii_case(&k)) {
                key = Some(v);
                break;
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

/// How long a successful basic-auth check is remembered. bcrypt costs ~150 ms per request on purpose; a browser
/// app sends many requests, so each credential is fully re-verified at most this often.
const VERIFIED_TTL: Duration = Duration::from_secs(300);
const VERIFIED_MAX: usize = 1024;

/// Successful checks only, keyed by a keyed SHA-256 of (stored hash, user, password). The key is random per
/// process, so the cache never holds anything an attacker could test passwords against offline, and a changed
/// credential (new hash) never matches an old entry. Failed attempts are never cached: guessing still pays bcrypt.
fn verify_cached(user: &str, pass: &str, hash: &str) -> bool {
    static SALT: OnceLock<[u8; 32]> = OnceLock::new();
    static VERIFIED: OnceLock<Mutex<HashMap<[u8; 32], Instant>>> = OnceLock::new();
    let salt = SALT.get_or_init(|| {
        let mut salt = [0u8; 32];
        salt[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        salt[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        salt
    });
    let key: [u8; 32] = Sha256::new()
        .chain_update(salt)
        .chain_update(hash.as_bytes())
        .chain_update([0])
        .chain_update(user.as_bytes())
        .chain_update([0])
        .chain_update(pass.as_bytes())
        .finalize()
        .into();
    let cache = VERIFIED.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    if let Ok(seen) = cache.lock()
        && seen
            .get(&key)
            .is_some_and(|at| now.duration_since(*at) < VERIFIED_TTL)
    {
        return true;
    }
    let ok = bcrypt::verify(pass, hash).unwrap_or(false);
    if ok && let Ok(mut seen) = cache.lock() {
        if seen.len() >= VERIFIED_MAX {
            seen.retain(|_, at| now.duration_since(*at) < VERIFIED_TTL);
            if seen.len() >= VERIFIED_MAX {
                seen.clear();
            }
        }
        seen.insert(key, now);
    }
    ok
}

pub fn basic_auth(
    c: &BasicAuthCfg,
    input: &ReqInput,
    cfg: &ProxyConfig,
    effects: &mut Effects,
) -> Action {
    let realm = if c.realm.is_empty() {
        "Raahi"
    } else {
        &c.realm
    };
    let unauthorized = || ShortResp {
        status: 401,
        headers: vec![(
            "www-authenticate".into(),
            format!("Basic realm=\"{realm}\""),
        )],
        body: b"Raahi: unauthorized\n".to_vec(),
    };

    let Some(auth) = input
        .headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
    else {
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
        Some((cid, hash)) if verify_cached(user, pass, hash) => {
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
    /// Identity-provider mode: verify RS256 tokens against this JWKS document
    /// (refreshed in the background) instead of per-consumer credentials.
    pub jwks_url: Option<String>,
    /// JWKS mode: claim used as the consumer name in logs/rate-limits.
    pub consumer_claim: String,
}

impl Default for JwtCfg {
    fn default() -> Self {
        JwtCfg {
            key_claim_name: "iss".into(),
            uri_param_names: vec!["jwt".into()],
            require_exp: false,
            jwks_url: None,
            consumer_claim: "sub".into(),
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
    if token.is_none()
        && let Some(q) = input.query
    {
        for (k, v) in parse_query(q) {
            if c.uri_param_names.iter().any(|n| n.eq_ignore_ascii_case(&k)) {
                token = Some(v);
                break;
            }
        }
    }
    let Some(token) = token else {
        return jwt_reject("missing JWT");
    };

    // Identity-provider mode: verify against the JWKS cache instead of credentials.
    if let Some(jwks_url) = c.jwks_url.as_deref().filter(|u| !u.is_empty()) {
        return jwt_via_jwks(c, jwks_url, &token, effects);
    }

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
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
    let (alg, dkey) = match cred.algorithm.as_str() {
        "HS256" => (
            Algorithm::HS256,
            Ok(DecodingKey::from_secret(cred.secret.as_bytes())),
        ),
        "HS384" => (
            Algorithm::HS384,
            Ok(DecodingKey::from_secret(cred.secret.as_bytes())),
        ),
        "HS512" => (
            Algorithm::HS512,
            Ok(DecodingKey::from_secret(cred.secret.as_bytes())),
        ),
        "RS256" => (
            Algorithm::RS256,
            DecodingKey::from_rsa_pem(cred.secret.as_bytes()),
        ),
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

/// Verify an RS256 token against a JWKS document (kid-matched). The consumer is
/// "virtual": named by `consumer_claim` (id 0), usable for logs and rate limiting
/// but not for group-based ACLs.
fn jwt_via_jwks(c: &JwtCfg, jwks_url: &str, token: &str, effects: &mut Effects) -> Action {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

    let Ok(header) = decode_header(token) else {
        return jwt_reject("malformed JWT");
    };
    if header.alg != Algorithm::RS256 {
        return jwt_reject("JWKS mode requires RS256");
    }
    let Some(key) = crate::jwks::lookup(jwks_url, header.kid.as_deref()) else {
        return jwt_reject("unknown signing key");
    };
    let Ok(dkey) = DecodingKey::from_rsa_components(&key.n, &key.e) else {
        return jwt_reject("invalid JWKS key");
    };

    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.validate_aud = false;
    validation.required_spec_claims = if c.require_exp {
        std::iter::once("exp".to_string()).collect()
    } else {
        Default::default()
    };
    let Ok(data) = decode::<serde_json::Value>(token, &dkey, &validation) else {
        return jwt_reject("invalid JWT");
    };

    let name = data.claims[&c.consumer_claim]
        .as_str()
        .unwrap_or("jwks-authenticated")
        .to_string();
    effects.consumer = Some((0, name));
    Action::Continue
}

#[cfg(test)]
mod verified_cache_tests {
    use super::verify_cached;

    #[test]
    fn caches_only_successful_checks_for_the_same_credential() {
        let hash = bcrypt::hash("correct horse", 4).unwrap();
        assert!(verify_cached("sarat", "correct horse", &hash));
        // Served from the cache now, and still exact.
        assert!(verify_cached("sarat", "correct horse", &hash));
        assert!(!verify_cached("sarat", "wrong horse", &hash));
        // A changed credential (new hash) never matches the old entry.
        let changed = bcrypt::hash("new password", 4).unwrap();
        assert!(!verify_cached("sarat", "correct horse", &changed));
        assert!(verify_cached("sarat", "new password", &changed));
    }
}
