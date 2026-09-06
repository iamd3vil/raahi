use crate::{Store, StoreError};
use raahi_core::AcmeEabCredentials;
use sqlx::Row;

impl Store {
    pub async fn get_acme_eab(
        &self,
        directory: &str,
    ) -> Result<Option<AcmeEabCredentials>, StoreError> {
        let row = sqlx::query(
            "SELECT directory_url, key_id, hmac_key FROM acme_eab WHERE directory_url=?",
        )
        .bind(directory)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| AcmeEabCredentials {
            directory_url: r.get("directory_url"),
            key_id: r.get("key_id"),
            hmac_key: r.get("hmac_key"),
        }))
    }
    pub async fn set_acme_eab(&self, credentials: &AcmeEabCredentials) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO acme_eab (directory_url, key_id, hmac_key) VALUES (?,?,?) ON CONFLICT(directory_url) DO UPDATE SET key_id=excluded.key_id, hmac_key=excluded.hmac_key")
            .bind(&credentials.directory_url).bind(&credentials.key_id).bind(&credentials.hmac_key)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_acme_eab(&self, directory: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM acme_eab WHERE directory_url=?")
            .bind(directory)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Only for explicitly requested secret backups, never ordinary API responses.
    pub async fn list_acme_eab(&self) -> Result<Vec<AcmeEabCredentials>, StoreError> {
        let rows = sqlx::query(
            "SELECT directory_url, key_id, hmac_key FROM acme_eab ORDER BY directory_url",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| AcmeEabCredentials {
                directory_url: r.get("directory_url"),
                key_id: r.get("key_id"),
                hmac_key: r.get("hmac_key"),
            })
            .collect())
    }
}
