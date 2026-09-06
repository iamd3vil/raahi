//! Admin API authentication and role-based authorization.
//!
//! Three ways in, one [`Principal`] out:
//! - **Open**: no admin token and no users exist → everyone is an implicit admin
//!   (the historical default; loopback binding is the only protection).
//! - **Token**: the legacy admin bearer token (automation) → admin.
//! - **Session**: a `raahi_session` cookie issued by password login or SSO → the
//!   user's role.
//!
//! Authorization is by HTTP method plus a short admin-only path list: GET needs
//! `viewer`, mutations need `editor`, and user/SSO/token/listener/import management
//! needs `admin`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use axum::extract::{Path, Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use raahi_core::{Id, Role, SsoConfig, User, UserSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::{AppState, digest_eq, sha256_hex};

pub const SESSION_COOKIE: &str = "raahi_session";
const SESSION_TTL_SECS: i64 = 7 * 24 * 3600;
const MIN_PASSWORD_LEN: usize = 8;
/// Failed password logins allowed per client IP within [`LOGIN_WINDOW`].
const LOGIN_MAX_FAILURES: u32 = 10;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// bcrypt hash of an unguessable string; verified against when the email is
/// unknown so a login attempt costs the same either way.
const DUMMY_HASH: &str = "$2b$12$C6UzMDM.H6dfI/f/IKxGhuRl0Zdk1o0yZ9Yq0N3v5pC1eWq3PQjJa";

/// Live auth configuration shared by the guard middleware and the handlers that
/// change it. Nothing here touches the DB on the request path except session lookup.
pub struct AuthState {
    /// SHA-256 hex of the admin token; `None` = token auth off.
    pub admin_hash: ArcSwap<Option<String>>,
    /// Whether any user rows exist (auth is enforced iff token or users).
    pub has_users: AtomicBool,
    pub sso: ArcSwap<Option<SsoConfig>>,
    /// Per-client failed password logins: ip -> (count, window start).
    login_failures: Mutex<HashMap<String, (u32, Instant)>>,
    /// In-flight OIDC logins keyed by `state`.
    pub(crate) sso_flows: Mutex<HashMap<String, crate::sso::PendingLogin>>,
    /// Cached OIDC discovery document for the configured issuer.
    pub(crate) discovery: Mutex<Option<crate::sso::Discovery>>,
    pub(crate) http: reqwest::Client,
}

impl AuthState {
    pub fn new(admin_hash: Option<String>, has_users: bool, sso: Option<SsoConfig>) -> Self {
        AuthState {
            admin_hash: ArcSwap::from_pointee(admin_hash),
            has_users: AtomicBool::new(has_users),
            sso: ArcSwap::from_pointee(sso),
            login_failures: Mutex::new(HashMap::new()),
            sso_flows: Mutex::new(HashMap::new()),
            discovery: Mutex::new(None),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.admin_hash.load().is_some() || self.has_users.load(Ordering::Relaxed)
    }

    pub fn set_sso(&self, cfg: Option<SsoConfig>) {
        self.sso.store(Arc::new(cfg));
        *self.discovery.lock().unwrap() = None;
    }

    /// Whether `ip` has exceeded the failed-login budget.
    fn login_blocked(&self, ip: &str) -> bool {
        let mut map = self.login_failures.lock().unwrap();
        map.retain(|_, (_, start)| start.elapsed() < LOGIN_WINDOW);
        map.get(ip).is_some_and(|(n, _)| *n >= LOGIN_MAX_FAILURES)
    }

    fn record_login_failure(&self, ip: &str) {
        let mut map = self.login_failures.lock().unwrap();
        let e = map.entry(ip.to_string()).or_insert((0, Instant::now()));
        e.0 += 1;
    }

    fn clear_login_failures(&self, ip: &str) {
        self.login_failures.lock().unwrap().remove(ip);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Open,
    Token,
    Session,
}

/// Who is making the request, attached to request extensions by [`guard`].
#[derive(Debug, Clone, Serialize)]
pub struct Principal {
    pub role: Role,
    pub method: AuthMethod,
    pub user: Option<User>,
}

fn unauthorized(msg: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response()
}

fn forbidden(required: Role) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "error": format!("forbidden: requires the {} role", required.as_str()) })),
    )
        .into_response()
}

/// Admin token from `Authorization: Bearer`, `X-Admin-Token`, or `?access_token=`
/// (the latter for EventSource/SSE, which cannot set headers).
fn presented_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-admin-token")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.trim().to_string())
        })
        .or_else(|| {
            query.and_then(|q| {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("access_token=").map(|v| v.to_string()))
            })
        })
}

/// Value of a cookie by name from the `Cookie` header(s).
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(COOKIE).iter().find_map(|h| {
        h.to_str().ok()?.split(';').find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == name).then(|| v.trim().to_string())
        })
    })
}

/// Requests arriving via HTTPS (through Raahi or another TLS-terminating proxy)
/// get `Secure` cookies; direct plain-HTTP access to the admin port does not, so
/// local/loopback use keeps working.
fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

pub fn session_cookie(headers: &HeaderMap, token: &str, max_age: i64) -> String {
    let secure = if is_https(headers) { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

/// Which role a request needs. Pure so it can be unit-tested.
pub fn required_role(method: &Method, path: &str, query: Option<&str>) -> Role {
    let p = path.strip_prefix("/api/v1").unwrap_or(path);
    if p.starts_with("/auth/") {
        return Role::Viewer; // logout, me, own password: any signed-in user
    }
    let is_read = matches!(*method, Method::GET | Method::HEAD);
    let admin_only = p.starts_with("/users")
        || p.starts_with("/admin/")
        || p.starts_with("/sso/")
        || p == "/settings"
        || p == "/acme/cloudflare-token"
        || p == "/acme/eab"
        || p == "/import";
    if admin_only && (!is_read || p.starts_with("/users") || p.starts_with("/sso/")) {
        return Role::Admin;
    }
    if p == "/export" && query.is_some_and(|q| q.contains("include_secrets=true")) {
        return Role::Admin;
    }
    if is_read { Role::Viewer } else { Role::Editor }
}

/// Takes headers and query (not the request) so the future stays `Send`: the body
/// type is not `Sync`, and a `&Request` would be held across the session lookup.
async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<Principal, Response> {
    if !state.auth.enabled() {
        return Ok(Principal {
            role: Role::Admin,
            method: AuthMethod::Open,
            user: None,
        });
    }

    if let Some(token) = presented_token(headers, query) {
        let expected = state.auth.admin_hash.load();
        return match expected.as_ref() {
            Some(h) if digest_eq(&sha256_hex(&token), h) => Ok(Principal {
                role: Role::Admin,
                method: AuthMethod::Token,
                user: None,
            }),
            _ => Err(unauthorized("invalid admin token")),
        };
    }

    if let Some(cookie) = cookie_value(headers, SESSION_COOKIE) {
        return match state.store.session_user(&sha256_hex(&cookie)).await {
            Ok(Some(user)) => Ok(Principal {
                role: user.role,
                method: AuthMethod::Session,
                user: Some(user),
            }),
            Ok(None) => Err(unauthorized("session expired or invalid; sign in again")),
            Err(e) => {
                tracing::error!("session lookup failed: {e}");
                Err(ApiError::Store(e).into_response())
            }
        };
    }

    Err(unauthorized("authentication required"))
}

/// Authenticate, then authorize against [`required_role`]. Attaches the
/// [`Principal`] to request extensions for handlers that need it.
pub async fn guard(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let principal = match authenticate(&state, req.headers(), req.uri().query()).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let required = required_role(req.method(), req.uri().path(), req.uri().query());
    if principal.role < required {
        return forbidden(required);
    }
    req.extensions_mut().insert(principal);
    next.run(req).await
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "direct".to_string())
}

pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || bcrypt::hash(password, bcrypt::DEFAULT_COST))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e| ApiError::Internal(e.to_string()))
}

async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || bcrypt::verify(password, &hash).unwrap_or(false))
        .await
        .unwrap_or(false)
}

fn validate_password(p: &str) -> ApiResult<()> {
    if p.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

fn validate_email(e: &str) -> ApiResult<()> {
    let e = e.trim();
    if e.len() < 3 || !e.contains('@') || e.starts_with('@') || e.ends_with('@') {
        return Err(ApiError::BadRequest("invalid email address".into()));
    }
    Ok(())
}

/// Create a session for `user` and produce the matching `Set-Cookie` value.
pub async fn start_session(
    state: &AppState,
    headers: &HeaderMap,
    user: &User,
) -> ApiResult<String> {
    let token = random_hex(32);
    state
        .store
        .create_session(
            &sha256_hex(&token),
            user.id,
            chrono::Duration::seconds(SESSION_TTL_SECS),
        )
        .await?;
    state.store.touch_user_login(user.id).await?;
    Ok(session_cookie(headers, &token, SESSION_TTL_SECS))
}

// ---- public handlers -----------------------------------------------------------

/// Auth status for the UI's login screen (unauthenticated).
pub async fn status(State(s): State<AppState>) -> Json<Value> {
    let sso = s.auth.sso.load();
    Json(json!({
        "auth_enabled": s.auth.enabled(),
        "token_enabled": s.auth.admin_hash.load().is_some(),
        "users_exist": s.auth.has_users.load(Ordering::Relaxed),
        "sso": {
            "enabled": sso.is_some(),
            "label": sso.as_ref().as_ref().map(|c| c.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "SSO".into()),
        },
    }))
}

#[derive(Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

/// Password login: sets the session cookie.
pub async fn login(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    let ip = client_ip(&headers);
    if s.auth.login_blocked(&ip) {
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many failed sign-in attempts; try again later" })),
        )
            .into_response());
    }

    let user = s.store.get_user_by_email(&body.email).await?;
    let hash = match &user {
        Some(u) => s.store.get_user_password_hash(u.id).await?,
        None => None,
    };
    let ok = verify_password(
        body.password,
        hash.clone().unwrap_or_else(|| DUMMY_HASH.into()),
    )
    .await
        && hash.is_some();

    let Some(user) = user.filter(|_| ok) else {
        s.auth.record_login_failure(&ip);
        return Ok(unauthorized("invalid email or password"));
    };
    s.auth.clear_login_failures(&ip);

    let cookie = start_session(&s, &headers, &user).await?;
    Ok((
        [(SET_COOKIE, cookie)],
        Json(json!({ "user": user, "role": user.role, "method": AuthMethod::Session })),
    )
        .into_response())
}

/// End the current session (idempotent) and clear the cookie.
pub async fn logout(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(cookie) = cookie_value(&headers, SESSION_COOKIE) {
        s.store.delete_session(&sha256_hex(&cookie)).await?;
    }
    Ok((
        [(SET_COOKIE, session_cookie(&headers, "", 0))],
        Json(json!({ "signed_out": true })),
    )
        .into_response())
}

/// The caller's identity and effective role.
pub async fn me(Extension(p): Extension<Principal>) -> Json<Principal> {
    Json(p)
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    #[serde(default)]
    pub current_password: String,
    pub new_password: String,
}

/// Self-service password change for session users. The current password is
/// required whenever one is set (SSO-only accounts may set their first password).
pub async fn change_password(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(body): Json<ChangePasswordBody>,
) -> ApiResult<Json<Value>> {
    let user = p
        .user
        .ok_or_else(|| ApiError::BadRequest("password change requires a signed-in user".into()))?;
    validate_password(&body.new_password)?;
    if let Some(hash) = s.store.get_user_password_hash(user.id).await?
        && !verify_password(body.current_password, hash).await
    {
        return Err(ApiError::BadRequest("current password is incorrect".into()));
    }
    let hash = hash_password(body.new_password).await?;
    s.store.set_user_password_hash(user.id, &hash).await?;
    Ok(Json(json!({ "updated": true })))
}

// ---- users (admin) -------------------------------------------------------------

pub async fn list_users(State(s): State<AppState>) -> ApiResult<Json<Vec<User>>> {
    Ok(Json(s.store.list_users().await?))
}

pub async fn create_user(
    State(s): State<AppState>,
    Json(spec): Json<UserSpec>,
) -> ApiResult<Json<User>> {
    validate_email(&spec.email)?;
    let hash = match &spec.password {
        Some(p) => {
            validate_password(p)?;
            Some(hash_password(p.clone()).await?)
        }
        None => None,
    };
    if hash.is_none() && s.auth.sso.load().is_none() {
        return Err(ApiError::BadRequest(
            "a password is required until SSO is configured".into(),
        ));
    }
    let user = s
        .store
        .create_user(&spec.email, spec.name.trim(), spec.role, hash.as_deref())
        .await?;
    s.auth.has_users.store(true, Ordering::Relaxed);
    Ok(Json(user))
}

/// Refuse changes that would leave no admin user (the token may be disabled too).
async fn ensure_not_last_admin(
    s: &AppState,
    current: &User,
    new_role: Option<Role>,
) -> ApiResult<()> {
    if current.role == Role::Admin
        && new_role != Some(Role::Admin)
        && s.store.count_admins().await? <= 1
    {
        return Err(ApiError::BadRequest(
            "cannot remove or demote the last admin user".into(),
        ));
    }
    Ok(())
}

pub async fn update_user(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<UserSpec>,
) -> ApiResult<Json<User>> {
    validate_email(&spec.email)?;
    let current = s.store.get_user(id).await?.ok_or(ApiError::NotFound)?;
    ensure_not_last_admin(&s, &current, Some(spec.role)).await?;
    let hash = match &spec.password {
        Some(p) if !p.is_empty() => {
            validate_password(p)?;
            Some(hash_password(p.clone()).await?)
        }
        _ => None,
    };
    let user = s
        .store
        .update_user(
            id,
            &spec.email,
            spec.name.trim(),
            spec.role,
            hash.as_deref(),
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    if hash.is_some() {
        // A reset password signs that user out everywhere.
        s.store.delete_user_sessions(id).await?;
    }
    Ok(Json(user))
}

pub async fn delete_user(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    let current = s.store.get_user(id).await?.ok_or(ApiError::NotFound)?;
    ensure_not_last_admin(&s, &current, None).await?;
    if !s.store.delete_user(id).await? {
        return Err(ApiError::NotFound);
    }
    let remaining = s.store.count_users().await?;
    s.auth.has_users.store(remaining > 0, Ordering::Relaxed);
    Ok(Json(json!({ "deleted": true })))
}

// ---- admin token (admin) -------------------------------------------------------

/// Generate (or rotate) the admin token. The plaintext is returned exactly once.
pub async fn create_admin_token(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let token = random_hex(32);
    let hash = sha256_hex(&token);
    s.store.set_admin_token_hash(Some(&hash)).await?;
    s.auth.admin_hash.store(Arc::new(Some(hash)));
    Ok(Json(json!({
        "token": token,
        "note": "Store this token now — it is not retrievable later.",
    })))
}

/// Disable token auth. Users (if any) keep protecting the API.
pub async fn delete_admin_token(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.store.set_admin_token_hash(None).await?;
    s.auth.admin_hash.store(Arc::new(None));
    Ok(Json(
        json!({ "auth_enabled": s.auth.enabled(), "token_enabled": false }),
    ))
}

// ---- sso settings (admin) ------------------------------------------------------

fn redact(cfg: &SsoConfig) -> Value {
    json!({
        "issuer": cfg.issuer,
        "client_id": cfg.client_id,
        "client_secret_set": !cfg.client_secret.is_empty(),
        "label": cfg.label,
        "auto_provision_role": cfg.auto_provision_role,
        "allowed_domains": cfg.allowed_domains,
    })
}

pub async fn get_sso_config(State(s): State<AppState>) -> Json<Value> {
    match s.auth.sso.load().as_ref() {
        Some(cfg) => Json(json!({ "enabled": true, "config": redact(cfg) })),
        None => Json(json!({ "enabled": false, "config": Value::Null })),
    }
}

#[derive(Deserialize)]
pub struct SsoConfigBody {
    pub issuer: String,
    pub client_id: String,
    /// Omit or leave empty to keep the stored secret.
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub auto_provision_role: Option<Role>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

pub async fn set_sso_config(
    State(s): State<AppState>,
    Json(body): Json<SsoConfigBody>,
) -> ApiResult<Json<Value>> {
    let issuer = body.issuer.trim().trim_end_matches('/').to_string();
    let url = reqwest::Url::parse(&issuer)
        .map_err(|_| ApiError::BadRequest("issuer must be an absolute URL".into()))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(ApiError::BadRequest(
            "issuer must use https (http is allowed for localhost only)".into(),
        ));
    }
    if body.client_id.trim().is_empty() {
        return Err(ApiError::BadRequest("client_id is required".into()));
    }
    let existing = s.auth.sso.load();
    let client_secret = match body.client_secret.filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => existing
            .as_ref()
            .as_ref()
            .map(|c| c.client_secret.clone())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| ApiError::BadRequest("client_secret is required".into()))?,
    };
    let cfg = SsoConfig {
        issuer,
        client_id: body.client_id.trim().to_string(),
        client_secret,
        label: body.label.trim().to_string(),
        auto_provision_role: body.auto_provision_role,
        allowed_domains: body
            .allowed_domains
            .iter()
            .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
            .filter(|d| !d.is_empty())
            .collect(),
    };
    s.store.set_sso_config(Some(&cfg)).await?;
    s.auth.set_sso(Some(cfg.clone()));
    Ok(Json(json!({ "enabled": true, "config": redact(&cfg) })))
}

pub async fn delete_sso_config(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.store.set_sso_config(None).await?;
    s.auth.set_sso(None);
    Ok(Json(json!({ "enabled": false, "config": Value::Null })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_matrix() {
        assert_eq!(
            required_role(&Method::POST, "/api/v1/applications", None),
            Role::Editor
        );
        assert_eq!(
            required_role(&Method::POST, "/api/v1/applications/test-upstream", None),
            Role::Editor
        );
        assert_eq!(
            required_role(&Method::PUT, "/api/v1/acme/eab", None),
            Role::Admin
        );
        assert_eq!(
            required_role(&Method::DELETE, "/api/v1/acme/eab", None),
            Role::Admin
        );
        assert_eq!(
            required_role(&Method::GET, "/api/v1/acme/eab", None),
            Role::Viewer
        );
        let get = Method::GET;
        let post = Method::POST;
        let put = Method::PUT;
        let del = Method::DELETE;
        assert_eq!(required_role(&get, "/api/v1/services", None), Role::Viewer);
        assert_eq!(
            required_role(&get, "/api/v1/events", Some("access_token=x")),
            Role::Viewer
        );
        assert_eq!(required_role(&post, "/api/v1/services", None), Role::Editor);
        assert_eq!(required_role(&del, "/api/v1/routes/3", None), Role::Editor);
        assert_eq!(required_role(&get, "/api/v1/settings", None), Role::Viewer);
        assert_eq!(required_role(&put, "/api/v1/settings", None), Role::Admin);
        assert_eq!(required_role(&get, "/api/v1/users", None), Role::Admin);
        assert_eq!(required_role(&post, "/api/v1/users", None), Role::Admin);
        assert_eq!(required_role(&get, "/api/v1/sso/config", None), Role::Admin);
        assert_eq!(
            required_role(&post, "/api/v1/admin/token", None),
            Role::Admin
        );
        assert_eq!(required_role(&post, "/api/v1/import", None), Role::Admin);
        assert_eq!(required_role(&get, "/api/v1/export", None), Role::Viewer);
        assert_eq!(
            required_role(&get, "/api/v1/export", Some("include_secrets=true")),
            Role::Admin
        );
        assert_eq!(
            required_role(&put, "/api/v1/acme/cloudflare-token", None),
            Role::Admin
        );
        assert_eq!(
            required_role(&get, "/api/v1/acme/cloudflare-token", None),
            Role::Viewer
        );
        // Self-service endpoints inside the guard need only a signed-in user.
        assert_eq!(
            required_role(&put, "/api/v1/auth/me/password", None),
            Role::Viewer
        );
        assert_eq!(
            required_role(&post, "/api/v1/auth/logout", None),
            Role::Viewer
        );
    }

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, "a=1; raahi_session=abc123 ; b=2".parse().unwrap());
        assert_eq!(cookie_value(&h, SESSION_COOKIE).as_deref(), Some("abc123"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }

    #[test]
    fn cookie_secure_flag_follows_forwarded_proto() {
        let mut h = HeaderMap::new();
        assert!(!session_cookie(&h, "t", 10).contains("Secure"));
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(session_cookie(&h, "t", 10).ends_with("; Secure"));
    }
}
