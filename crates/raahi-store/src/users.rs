//! Admin users, cookie sessions, and SSO settings. Password hashing happens in the
//! API layer; this module only stores hashes.

use chrono::{DateTime, Duration, Utc};
use raahi_core::{Id, Role, SsoConfig, User};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::rows::parse_dt;
use crate::{Store, StoreError};

fn map_user(r: &SqliteRow) -> User {
    User {
        id: r.get("id"),
        email: r.get("email"),
        name: r.get("name"),
        role: r.get::<String, _>("role").parse().unwrap_or(Role::Viewer),
        has_password: r.get::<Option<String>, _>("password_hash").is_some(),
        created_at: parse_dt(&r.get::<String, _>("created_at")),
        last_login_at: r
            .get::<Option<String>, _>("last_login_at")
            .map(|s| parse_dt(&s)),
    }
}

fn map_conflict(e: sqlx::Error) -> StoreError {
    match e.as_database_error() {
        Some(db) if db.is_unique_violation() => {
            StoreError::Conflict("a user with that email already exists".into())
        }
        _ => StoreError::Db(e),
    }
}

/// Emails are compared case-insensitively; store them lowercase.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

impl Store {
    // ---- users ----------------------------------------------------------------
    pub async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_user).collect())
    }

    pub async fn count_users(&self) -> Result<i64, StoreError> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("n"))
    }

    pub async fn count_admins(&self) -> Result<i64, StoreError> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM users WHERE role = 'admin'")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("n"))
    }

    pub async fn get_user(&self, id: Id) -> Result<Option<User>, StoreError> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_user))
    }

    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        let row = sqlx::query("SELECT * FROM users WHERE email = ?")
            .bind(normalize_email(email))
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_user))
    }

    /// The stored bcrypt hash for a user (None when SSO-only).
    pub async fn get_user_password_hash(&self, id: Id) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT password_hash FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get("password_hash")))
    }

    pub async fn create_user(
        &self,
        email: &str,
        name: &str,
        role: Role,
        password_hash: Option<&str>,
    ) -> Result<User, StoreError> {
        let res =
            sqlx::query("INSERT INTO users (email, name, role, password_hash) VALUES (?, ?, ?, ?)")
                .bind(normalize_email(email))
                .bind(name)
                .bind(role.as_str())
                .bind(password_hash)
                .execute(&self.pool)
                .await
                .map_err(map_conflict)?;
        Ok(self.get_user(res.last_insert_rowid()).await?.unwrap())
    }

    /// Full update of profile fields; `password_hash = None` keeps the current one.
    pub async fn update_user(
        &self,
        id: Id,
        email: &str,
        name: &str,
        role: Role,
        password_hash: Option<&str>,
    ) -> Result<Option<User>, StoreError> {
        let res = sqlx::query(
            "UPDATE users SET email=?, name=?, role=?, \
             password_hash = COALESCE(?, password_hash) WHERE id=?",
        )
        .bind(normalize_email(email))
        .bind(name)
        .bind(role.as_str())
        .bind(password_hash)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_conflict)?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_user(id).await
    }

    pub async fn set_user_password_hash(&self, id: Id, hash: &str) -> Result<bool, StoreError> {
        let res = sqlx::query("UPDATE users SET password_hash=? WHERE id=?")
            .bind(hash)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn touch_user_login(&self, id: Id) -> Result<(), StoreError> {
        sqlx::query("UPDATE users SET last_login_at=? WHERE id=?")
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a user; their sessions cascade.
    pub async fn delete_user(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- sessions -------------------------------------------------------------
    /// Record a session by the hash of its cookie token. Expired rows are pruned
    /// opportunistically here so the table stays small without a sweeper.
    pub async fn create_session(
        &self,
        token_hash: &str,
        user_id: Id,
        ttl: Duration,
    ) -> Result<DateTime<Utc>, StoreError> {
        let now = Utc::now();
        let expires = now + ttl;
        sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?, ?, ?)")
            .bind(token_hash)
            .bind(user_id)
            .bind(expires.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(expires)
    }

    /// Resolve a live session to its user (None if unknown or expired).
    pub async fn session_user(&self, token_hash: &str) -> Result<Option<User>, StoreError> {
        let row = sqlx::query(
            "SELECT u.* FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = ? AND s.expires_at > ?",
        )
        .bind(token_hash)
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(map_user))
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Sign a user out everywhere (role change / password reset).
    pub async fn delete_user_sessions(&self, user_id: Id) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- sso ------------------------------------------------------------------
    pub async fn get_sso_config(&self) -> Result<Option<SsoConfig>, StoreError> {
        let row = sqlx::query("SELECT sso_config FROM settings WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .and_then(|r| r.get::<Option<String>, _>("sso_config"))
            .and_then(|s| serde_json::from_str(&s).ok()))
    }

    pub async fn set_sso_config(&self, cfg: Option<&SsoConfig>) -> Result<(), StoreError> {
        let json = cfg.map(|c| serde_json::to_string(c).unwrap_or_default());
        sqlx::query("UPDATE settings SET sso_config = ? WHERE id = 1")
            .bind(json)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
