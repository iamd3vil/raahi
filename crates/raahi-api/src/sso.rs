//! OpenID Connect single sign-on for the admin UI: authorization-code flow with
//! PKCE, discovery-driven endpoints, and id_token verification against the
//! provider's JWKS. Works with any compliant provider (Google, Keycloak, Authentik,
//! Pocket ID, Auth0, Zitadel, ...).
//!
//! Login state lives in memory keyed by `state`; Raahi is a single process, so no
//! shared store is needed. Errors redirect back to the UI with `?sso_error=`.

use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use raahi_core::{SsoConfig, User};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{random_hex, start_session};
use crate::error::{ApiError, ApiResult};

const FLOW_TTL: Duration = Duration::from_secs(10 * 60);
const CALLBACK_PATH: &str = "/api/v1/auth/sso/callback";

/// One in-flight login, created by `/auth/sso/start` and consumed by the callback.
pub struct PendingLogin {
    nonce: String,
    pkce_verifier: String,
    redirect_uri: String,
    created: Instant,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

async fn discover(s: &AppState, cfg: &SsoConfig) -> ApiResult<Discovery> {
    if let Some(d) = s.auth.discovery.lock().unwrap().clone() {
        return Ok(d);
    }
    let url = format!("{}/.well-known/openid-configuration", cfg.issuer);
    let doc: Discovery = s
        .auth
        .http
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| ApiError::Internal(format!("OIDC discovery {url}: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("OIDC discovery {url}: invalid document: {e}")))?;
    *s.auth.discovery.lock().unwrap() = Some(doc.clone());
    Ok(doc)
}

/// The URL clients used to reach us, honouring the proxy's forwarded headers so
/// the redirect URI matches what is registered at the IdP.
fn external_base(headers: &HeaderMap) -> String {
    let hv = |name: &str| {
        headers
            .get(name)
            .and_then(|h| h.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let scheme = hv("x-forwarded-proto").unwrap_or_else(|| "http".into());
    let host = hv("x-forwarded-host")
        .or_else(|| hv("host"))
        .unwrap_or_else(|| "localhost".into());
    format!("{scheme}://{host}")
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn ui_error(msg: &str) -> Response {
    let q: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("sso_error", msg)
        .finish();
    Redirect::to(&format!("/?{q}")).into_response()
}

/// Begin the login: stash state/nonce/PKCE and redirect to the provider.
pub async fn start(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let cfg = s
        .auth
        .sso
        .load()
        .as_ref()
        .clone()
        .ok_or_else(|| ApiError::BadRequest("SSO is not configured".into()))?;
    let disc = discover(&s, &cfg).await?;

    let state = random_hex(32);
    let nonce = random_hex(32);
    let pkce_verifier = random_hex(32); // 64 unreserved chars, within RFC 7636 bounds
    let challenge = {
        use sha2::{Digest, Sha256};
        b64url(&Sha256::digest(pkce_verifier.as_bytes()))
    };
    let redirect_uri = format!("{}{CALLBACK_PATH}", external_base(&headers));

    {
        let mut flows = s.auth.sso_flows.lock().unwrap();
        flows.retain(|_, f| f.created.elapsed() < FLOW_TTL);
        flows.insert(
            state.clone(),
            PendingLogin {
                nonce: nonce.clone(),
                pkce_verifier,
                redirect_uri: redirect_uri.clone(),
                created: Instant::now(),
            },
        );
    }

    let mut url = url::Url::parse(&disc.authorization_endpoint)
        .map_err(|e| ApiError::Internal(format!("bad authorization_endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", "openid email profile")
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(Redirect::to(url.as_str()).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Claims we use from the id_token.
#[derive(Debug, Deserialize)]
pub struct IdClaims {
    pub nonce: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub preferred_username: Option<String>,
}

/// Verify an id_token's signature (against `jwks`), issuer, audience, expiry, and
/// nonce; return its claims.
pub fn verify_id_token(
    id_token: &str,
    jwks: &serde_json::Value,
    issuer: &str,
    client_id: &str,
    nonce: &str,
) -> Result<IdClaims, String> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

    let header = decode_header(id_token).map_err(|e| format!("malformed id_token: {e}"))?;
    let keys = jwks["keys"].as_array().ok_or("JWKS has no keys")?;
    let jwk = match &header.kid {
        Some(kid) => keys.iter().find(|k| k["kid"].as_str() == Some(kid)),
        None if keys.len() == 1 => keys.first(),
        None => None,
    }
    .ok_or("no JWKS key matches the id_token kid")?;

    let key = match jwk["kty"].as_str() {
        Some("RSA") => DecodingKey::from_rsa_components(
            jwk["n"].as_str().ok_or("RSA jwk missing n")?,
            jwk["e"].as_str().ok_or("RSA jwk missing e")?,
        )
        .map_err(|e| format!("bad RSA jwk: {e}"))?,
        Some("EC") => DecodingKey::from_ec_components(
            jwk["x"].as_str().ok_or("EC jwk missing x")?,
            jwk["y"].as_str().ok_or("EC jwk missing y")?,
        )
        .map_err(|e| format!("bad EC jwk: {e}"))?,
        other => return Err(format!("unsupported jwk kty {other:?}")),
    };
    if !matches!(
        header.alg,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::ES256
            | Algorithm::ES384
    ) {
        return Err(format!("unsupported id_token alg {:?}", header.alg));
    }

    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    let data = decode::<IdClaims>(id_token, &key, &validation)
        .map_err(|e| format!("id_token rejected: {e}"))?;

    if data.claims.nonce.as_deref() != Some(nonce) {
        return Err("id_token nonce mismatch".into());
    }
    Ok(data.claims)
}

/// Map verified claims to a local user, auto-provisioning when configured.
async fn resolve_user(s: &AppState, cfg: &SsoConfig, claims: &IdClaims) -> Result<User, String> {
    let email = claims
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .ok_or("the identity provider did not return an email address")?;
    if claims.email_verified == Some(false) {
        return Err("the identity provider reports this email as unverified".into());
    }
    if let Some(u) = s
        .store
        .get_user_by_email(email)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(u);
    }
    let Some(role) = cfg.auto_provision_role else {
        return Err(format!(
            "no Raahi account for {email}; ask an admin to add you"
        ));
    };
    if !cfg.domain_allowed(email) {
        return Err(format!("{email} is not in an allowed domain"));
    }
    let name = claims
        .name
        .clone()
        .or_else(|| claims.preferred_username.clone())
        .unwrap_or_default();
    let user = s
        .store
        .create_user(email, name.trim(), role, None)
        .await
        .map_err(|e| e.to_string())?;
    s.auth
        .has_users
        .store(true, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("sso: auto-provisioned {} as {}", user.email, role.as_str());
    Ok(user)
}

/// Provider redirect target: exchange the code, verify the id_token, sign in.
pub async fn callback(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(err) = q.error {
        let desc = q.error_description.unwrap_or_default();
        return ui_error(format!("sign-in cancelled by provider: {err} {desc}").trim());
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return ui_error("missing code or state in provider response");
    };
    let Some(flow) = s.auth.sso_flows.lock().unwrap().remove(&state) else {
        return ui_error("sign-in expired or state mismatch; try again");
    };
    if flow.created.elapsed() > FLOW_TTL {
        return ui_error("sign-in expired; try again");
    }
    let Some(cfg) = s.auth.sso.load().as_ref().clone() else {
        return ui_error("SSO is not configured");
    };

    match complete(&s, &cfg, &flow, &code).await {
        Ok(claims) => match resolve_user(&s, &cfg, &claims).await {
            Ok(user) => match start_session(&s, &headers, &user).await {
                Ok(cookie) => ([(SET_COOKIE, cookie)], Redirect::to("/")).into_response(),
                Err(e) => {
                    tracing::error!("sso: session create failed: {e:?}");
                    ui_error("could not create a session")
                }
            },
            Err(msg) => ui_error(&msg),
        },
        Err(msg) => {
            tracing::warn!("sso: {msg}");
            ui_error(&msg)
        }
    }
}

async fn complete(
    s: &AppState,
    cfg: &SsoConfig,
    flow: &PendingLogin,
    code: &str,
) -> Result<IdClaims, String> {
    let disc = discover(s, cfg)
        .await
        .map_err(|_| "OIDC discovery failed".to_string())?;
    let resp: TokenResponse = s
        .auth
        .http
        .post(&disc.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &flow.redirect_uri),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
            ("code_verifier", &flow.pkce_verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("token endpoint returned invalid JSON: {e}"))?;
    if let Some(err) = resp.error {
        return Err(format!(
            "token exchange rejected: {err} {}",
            resp.error_description.unwrap_or_default()
        ));
    }
    let id_token = resp.id_token.ok_or("token response had no id_token")?;

    let jwks: serde_json::Value = s
        .auth
        .http
        .get(&disc.jwks_uri)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("JWKS fetch failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("JWKS invalid: {e}"))?;

    verify_id_token(&id_token, &jwks, &disc.issuer, &cfg.client_id, &flow.nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde_json::json;

    /// ES256 signer plus the matching JWKS, built from a fresh rcgen key pair.
    fn signer() -> (EncodingKey, serde_json::Value) {
        use rcgen::PublicKeyData;
        let kp = rcgen::KeyPair::generate().unwrap(); // ECDSA P-256
        let point = kp.der_bytes(); // uncompressed point: 0x04 || X || Y
        assert_eq!(point.len(), 65);
        let jwks = json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256",
            "x": b64url(&point[1..33]), "y": b64url(&point[33..65]),
        }]});
        (EncodingKey::from_ec_der(&kp.serialize_der()), jwks)
    }

    fn token(key: &EncodingKey, claims: serde_json::Value) -> String {
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some("k1".into());
        encode(&h, &claims, key).unwrap()
    }

    fn exp() -> i64 {
        chrono::Utc::now().timestamp() + 300
    }

    #[test]
    fn verifies_a_good_token() {
        let (key, jwks) = signer();
        let t = token(
            &key,
            json!({
                "iss": "https://idp.example", "aud": "raahi", "exp": exp(),
                "nonce": "n1", "email": "a@example.com", "email_verified": true, "name": "A",
            }),
        );
        let c = verify_id_token(&t, &jwks, "https://idp.example", "raahi", "n1").unwrap();
        assert_eq!(c.email.as_deref(), Some("a@example.com"));
        assert_eq!(c.name.as_deref(), Some("A"));
    }

    #[test]
    fn rejects_wrong_nonce_audience_issuer_and_signature() {
        let (key, jwks) = signer();
        let good =
            json!({ "iss": "https://idp.example", "aud": "raahi", "exp": exp(), "nonce": "n1" });
        let t = token(&key, good.clone());
        assert!(verify_id_token(&t, &jwks, "https://idp.example", "raahi", "other").is_err());
        assert!(verify_id_token(&t, &jwks, "https://idp.example", "not-raahi", "n1").is_err());
        assert!(verify_id_token(&t, &jwks, "https://evil.example", "raahi", "n1").is_err());

        let (other_key, _) = signer();
        let forged = token(&other_key, good);
        assert!(verify_id_token(&forged, &jwks, "https://idp.example", "raahi", "n1").is_err());

        let expired = token(
            &key,
            json!({
                "iss": "https://idp.example", "aud": "raahi", "nonce": "n1",
                "exp": chrono::Utc::now().timestamp() - 600,
            }),
        );
        assert!(verify_id_token(&expired, &jwks, "https://idp.example", "raahi", "n1").is_err());
    }

    #[test]
    fn external_base_prefers_forwarded_headers() {
        let mut h = HeaderMap::new();
        h.insert("host", "127.0.0.1:19080".parse().unwrap());
        assert_eq!(external_base(&h), "http://127.0.0.1:19080");
        h.insert("x-forwarded-host", "raahi.example.com".parse().unwrap());
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(external_base(&h), "https://raahi.example.com");
    }
}
