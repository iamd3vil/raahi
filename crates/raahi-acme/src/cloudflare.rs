use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::AcmeError;

const API_BASE: &str = "https://api.cloudflare.com/client/v4";

#[derive(Clone)]
pub struct CloudflareProvider {
    client: Client,
    token: String,
}

pub struct DnsRecord {
    zone_id: String,
    record_id: String,
}

#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn create_txt(&self, domain: &str, value: &str) -> Result<DnsRecord, AcmeError>;
    async fn delete_txt(&self, record: &DnsRecord) -> Result<(), AcmeError>;
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    success: bool,
    result: T,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Deserialize)]
struct Zone {
    id: String,
}

#[derive(Deserialize)]
struct Record {
    id: String,
}

impl CloudflareProvider {
    pub fn new(token: String) -> Self {
        Self {
            client: Client::new(),
            token,
        }
    }

    async fn find_zone(&self, domain: &str) -> Result<String, AcmeError> {
        let labels: Vec<&str> = domain.split('.').collect();
        for index in 0..labels.len().saturating_sub(1) {
            let candidate = labels[index..].join(".");
            let response = self
                .client
                .get(format!("{API_BASE}/zones"))
                .bearer_auth(&self.token)
                .query(&[("name", candidate)])
                .send()
                .await?
                .error_for_status()?
                .json::<ApiResponse<Vec<Zone>>>()
                .await?;
            if !response.success {
                return Err(api_error("find DNS zone", &response.errors));
            }
            if let Some(zone) = response.result.into_iter().next() {
                return Ok(zone.id);
            }
        }
        Err(AcmeError::Config(format!(
            "Cloudflare zone not found for {domain}"
        )))
    }
}

#[async_trait]
impl DnsProvider for CloudflareProvider {
    async fn create_txt(&self, domain: &str, value: &str) -> Result<DnsRecord, AcmeError> {
        let domain = domain.trim_start_matches("*.").trim_end_matches('.');
        let zone_id = self.find_zone(domain).await?;
        let name = format!("_acme-challenge.{domain}");
        let response = self
            .client
            .post(format!("{API_BASE}/zones/{zone_id}/dns_records"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "type": "TXT",
                "name": name,
                "content": value,
                "ttl": 60,
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<ApiResponse<Record>>()
            .await?;
        if !response.success {
            return Err(api_error("create DNS record", &response.errors));
        }
        Ok(DnsRecord {
            zone_id,
            record_id: response.result.id,
        })
    }

    async fn delete_txt(&self, record: &DnsRecord) -> Result<(), AcmeError> {
        let response = self
            .client
            .delete(format!(
                "{API_BASE}/zones/{}/dns_records/{}",
                record.zone_id, record.record_id
            ))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<ApiResponse<serde_json::Value>>()
            .await?;
        if !response.success {
            return Err(api_error("delete DNS record", &response.errors));
        }
        Ok(())
    }
}

fn api_error(action: &str, errors: &[ApiError]) -> AcmeError {
    let detail = errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    AcmeError::Config(format!("Cloudflare {action} failed: {detail}"))
}
