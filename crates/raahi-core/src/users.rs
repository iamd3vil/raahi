//! Admin-UI users, roles, and SSO settings. These are control-plane identities (who
//! may configure the proxy), distinct from `Consumer`s (who the proxy authenticates).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Id, ParseError};

/// Access level for the admin API. Ordered: `Viewer < Editor < Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read-only access to every resource.
    Viewer,
    /// Create/update/delete gateway configuration (services, routes, plugins, ...).
    Editor,
    /// Editor plus user management, SSO/admin-token settings, listener settings, import.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Role::Viewer),
            "editor" => Ok(Role::Editor),
            "admin" => Ok(Role::Admin),
            _ => Err(ParseError),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Id,
    pub email: String,
    pub name: String,
    pub role: Role,
    /// Whether a password is set (SSO-only users have none).
    pub has_password: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

/// Create/update body for a user. `password` is optional on update (omit to keep).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSpec {
    pub email: String,
    #[serde(default)]
    pub name: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// OpenID Connect single sign-on settings for the admin UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SsoConfig {
    /// Issuer URL; `/.well-known/openid-configuration` is appended for discovery.
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// Button label in the UI, e.g. "Google".
    #[serde(default)]
    pub label: String,
    /// Create unknown users on first SSO login with this role. `None` = only
    /// pre-created users may sign in.
    #[serde(default)]
    pub auto_provision_role: Option<Role>,
    /// Email domains eligible for auto-provisioning (lowercase, no `@`). Empty = any.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

impl SsoConfig {
    /// Whether `email`'s domain is allowed for auto-provisioning.
    pub fn domain_allowed(&self, email: &str) -> bool {
        if self.allowed_domains.is_empty() {
            return true;
        }
        let Some((_, domain)) = email.rsplit_once('@') else {
            return false;
        };
        self.allowed_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(domain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Viewer < Role::Editor);
        assert!(Role::Editor < Role::Admin);
        assert_eq!("editor".parse::<Role>(), Ok(Role::Editor));
        assert!("root".parse::<Role>().is_err());
    }

    #[test]
    fn domain_allowlist() {
        let mut c = SsoConfig {
            issuer: "https://idp.example".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            label: String::new(),
            auto_provision_role: Some(Role::Viewer),
            allowed_domains: vec![],
        };
        assert!(c.domain_allowed("a@anything.example"));
        c.allowed_domains = vec!["example.com".into()];
        assert!(c.domain_allowed("a@Example.COM"));
        assert!(!c.domain_allowed("a@evil.example.com"));
        assert!(!c.domain_allowed("no-at-sign"));
    }
}
