mod cloudflare;

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cloudflare::{CloudflareProvider, DnsProvider, DnsRecord};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_core::{
    AcmeChallenge, AcmeConfig, AcmeState, AcmeStatus, AlpnChallengeCertificate,
    AlpnChallengeRegistry, Certificate, LETS_ENCRYPT_PRODUCTION, LETS_ENCRYPT_STAGING,
};
use raahi_proxy::{CertHandle, CertStore};
use raahi_store::Store;
use rcgen::{CertificateParams, CustomExtension, DistinguishedName, KeyPair};
use tokio::sync::Notify;
use tracing::{error, info, warn};
use x509_parser::pem::parse_x509_pem;

const RENEW_BEFORE_DAYS: i64 = 30;
const DNS_PROPAGATION_DELAY: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum AcmeError {
    #[error("ACME client: {0}")]
    Client(#[from] instant_acme::Error),
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("store: {0}")]
    Store(#[from] raahi_store::StoreError),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("certificate generation: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("invalid configuration: {0}")]
    Config(String),
}

#[derive(Clone, Default)]
pub struct AcmeHandle {
    notify: std::sync::Arc<Notify>,
}

impl AcmeHandle {
    pub fn trigger(&self) {
        self.notify.notify_one();
    }
}

pub fn validate_account_credentials(credentials: &str) -> Result<(), AcmeError> {
    let _ = serde_json::from_str::<AccountCredentials>(credentials)?;
    Ok(())
}

pub struct AcmeService {
    pub db_url: String,
    pub cert_handle: CertHandle,
    pub challenges: AlpnChallengeRegistry,
    pub handle: AcmeHandle,
    pub interval: Duration,
}

#[async_trait]
impl BackgroundService for AcmeService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let store = match Store::connect(&self.db_url).await {
            Ok(store) => store,
            Err(error) => {
                error!("ACME: failed to open database: {error}");
                return;
            }
        };

        loop {
            if let Err(error) =
                renew_due_certificates(&store, &self.cert_handle, &self.challenges, Utc::now())
                    .await
            {
                error!("ACME renewal scan failed: {error}");
            }

            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {},
                _ = self.handle.notify.notified() => {},
                _ = shutdown.changed() => break,
            }
        }
    }
}

pub fn needs_renewal(certificate: &Certificate, now: DateTime<Utc>) -> bool {
    if certificate.acme_config.is_none() {
        return false;
    }
    if certificate.cert_pem.is_empty() || certificate.key_pem.is_empty() {
        return true;
    }
    if certificate
        .acme_status
        .as_ref()
        .is_none_or(|status| status.state != AcmeState::Issued)
    {
        return true;
    }
    certificate
        .acme_status
        .as_ref()
        .and_then(|status| status.expires_at)
        .map(|expires| expires <= now + chrono::Duration::days(RENEW_BEFORE_DAYS))
        .unwrap_or(true)
}

async fn renew_due_certificates(
    store: &Store,
    cert_handle: &CertHandle,
    challenges: &AlpnChallengeRegistry,
    now: DateTime<Utc>,
) -> Result<(), AcmeError> {
    for certificate in store.list_certificates().await? {
        if !needs_renewal(&certificate, now) {
            continue;
        }

        let mut status = certificate
            .acme_status
            .clone()
            .unwrap_or_else(AcmeStatus::pending);
        status.state = AcmeState::Issuing;
        status.last_attempt = Some(now);
        status.last_error = None;
        if !store
            .try_claim_acme_certificate(certificate.id, &status, now - chrono::Duration::hours(1))
            .await?
        {
            continue;
        }

        match issue_certificate(store, challenges, &certificate).await {
            Ok((cert_pem, key_pem, expires_at)) => {
                status.state = AcmeState::Issued;
                status.issued_at = Some(Utc::now());
                status.expires_at = Some(expires_at);
                store
                    .update_acme_certificate(certificate.id, &cert_pem, &key_pem, &status)
                    .await?;
                reload_certificates(store, cert_handle).await?;
                info!(
                    certificate_id = certificate.id,
                    domains = ?certificate.sni,
                    expires_at = %expires_at,
                    "ACME certificate issued"
                );
            }
            Err(error) => {
                status.state = AcmeState::Failed;
                status.last_error = Some(error.to_string());
                store
                    .update_certificate_acme_status(certificate.id, &status)
                    .await?;
                warn!(
                    certificate_id = certificate.id,
                    domains = ?certificate.sni,
                    "ACME issuance failed: {error}"
                );
            }
        }
    }
    Ok(())
}

async fn issue_certificate(
    store: &Store,
    challenges: &AlpnChallengeRegistry,
    certificate: &Certificate,
) -> Result<(String, String, DateTime<Utc>), AcmeError> {
    let config = certificate
        .acme_config
        .as_ref()
        .ok_or_else(|| AcmeError::Config("missing ACME configuration".into()))?;
    validate_config(&certificate.sni, config)?;

    let directory_url = normalize_directory_url(&config.directory_url);
    let account = load_or_create_account(store, directory_url, config.email.as_deref()).await?;
    let identifiers = certificate
        .sni
        .iter()
        .map(|domain| Identifier::Dns(domain.clone()))
        .collect::<Vec<_>>();
    let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;

    let cloudflare = match config.challenge {
        AcmeChallenge::Dns01 => Some(CloudflareProvider::new(
            store
                .get_cloudflare_api_token()
                .await?
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| {
                    AcmeError::Config("Cloudflare API token is not configured".into())
                })?,
        )),
        AcmeChallenge::TlsAlpn01 => None,
    };
    let mut dns_records: Vec<DnsRecord> = Vec::new();
    let mut alpn_domains: Vec<String> = Vec::new();

    let challenge_result = async {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authorization = result?;
            match authorization.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => {
                    return Err(AcmeError::Config(format!(
                        "authorization is in unexpected state {status:?}"
                    )));
                }
            }

            let challenge_type = match config.challenge {
                AcmeChallenge::Dns01 => ChallengeType::Dns01,
                AcmeChallenge::TlsAlpn01 => ChallengeType::TlsAlpn01,
            };
            let mut challenge = authorization.challenge(challenge_type).ok_or_else(|| {
                AcmeError::Config("ACME server did not offer the requested challenge".into())
            })?;
            let domain = challenge.identifier().to_string();
            let key_authorization = challenge.key_authorization();

            match config.challenge {
                AcmeChallenge::Dns01 => {
                    let record = cloudflare
                        .as_ref()
                        .expect("Cloudflare provider initialized")
                        .create_txt(&domain, &key_authorization.dns_value())
                        .await?;
                    dns_records.push(record);
                    tokio::time::sleep(DNS_PROPAGATION_DELAY).await;
                }
                AcmeChallenge::TlsAlpn01 => {
                    let challenge_certificate =
                        tls_alpn_certificate(&domain, key_authorization.digest().as_ref())?;
                    challenges.insert(&domain, challenge_certificate);
                    alpn_domains.push(domain);
                }
            }
            challenge.set_ready().await?;
        }

        let status = order.poll_ready(&RetryPolicy::default()).await?;
        if status != OrderStatus::Ready {
            return Err(AcmeError::Config(format!(
                "order reached unexpected state {status:?}"
            )));
        }
        Ok::<(), AcmeError>(())
    }
    .await;

    for domain in &alpn_domains {
        challenges.remove(domain);
    }
    if let Some(provider) = &cloudflare {
        for record in &dns_records {
            if let Err(error) = provider.delete_txt(record).await {
                warn!("ACME: failed to remove Cloudflare challenge record: {error}");
            }
        }
    }
    challenge_result?;

    let key_pem = order.finalize().await?;
    let cert_pem = order.poll_certificate(&RetryPolicy::default()).await?;
    let expires_at = certificate_expiry(&cert_pem)?;
    Ok((cert_pem, key_pem, expires_at))
}

async fn load_or_create_account(
    store: &Store,
    directory_url: &str,
    email: Option<&str>,
) -> Result<Account, AcmeError> {
    if let Some((credentials, _)) = store.get_acme_account(directory_url).await? {
        let credentials = serde_json::from_str::<AccountCredentials>(&credentials)?;
        return Ok(Account::builder()?.from_credentials(credentials).await?);
    }

    let contact = email.map(|email| format!("mailto:{email}"));
    let contacts = contact.iter().map(String::as_str).collect::<Vec<_>>();
    let (account, credentials) = Account::builder()?
        .create(
            &NewAccount {
                contact: &contacts,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory_url.to_string(),
            None,
        )
        .await?;
    store
        .upsert_acme_account(directory_url, email, &serde_json::to_string(&credentials)?)
        .await?;
    Ok(account)
}

fn tls_alpn_certificate(
    domain: &str,
    key_authorization_digest: &[u8],
) -> Result<AlpnChallengeCertificate, AcmeError> {
    let mut params = CertificateParams::new(vec![domain.trim_start_matches("*.").to_string()])?;
    params.distinguished_name = DistinguishedName::new();
    let mut extension_value = Vec::with_capacity(key_authorization_digest.len() + 2);
    extension_value.push(0x04);
    extension_value.push(key_authorization_digest.len() as u8);
    extension_value.extend_from_slice(key_authorization_digest);
    let mut extension =
        CustomExtension::from_oid_content(&[1, 3, 6, 1, 5, 5, 7, 1, 31], extension_value);
    extension.set_criticality(true);
    params.custom_extensions.push(extension);
    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok(AlpnChallengeCertificate {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

fn certificate_expiry(cert_pem: &str) -> Result<DateTime<Utc>, AcmeError> {
    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())
        .map_err(|error| AcmeError::Config(format!("parse issued certificate: {error}")))?;
    let cert = pem
        .parse_x509()
        .map_err(|error| AcmeError::Config(format!("parse issued certificate: {error}")))?;
    DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0)
        .ok_or_else(|| AcmeError::Config("certificate has invalid expiry".into()))
}

pub fn validate_config(domains: &[String], config: &AcmeConfig) -> Result<(), AcmeError> {
    if domains.is_empty() || domains.iter().any(|domain| domain.trim().is_empty()) {
        return Err(AcmeError::Config(
            "at least one non-empty SNI domain is required".into(),
        ));
    }
    CertificateParams::new(domains.to_vec())?;
    if config.challenge == AcmeChallenge::TlsAlpn01
        && domains.iter().any(|domain| domain.starts_with("*."))
    {
        return Err(AcmeError::Config(
            "wildcard certificates require the dns-01 challenge".into(),
        ));
    }
    let directory = normalize_directory_url(&config.directory_url);
    let url = reqwest::Url::parse(directory)
        .map_err(|error| AcmeError::Config(format!("invalid ACME directory URL: {error}")))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(AcmeError::Config(
            "ACME directory must use HTTPS (HTTP is allowed only for loopback testing)".into(),
        ));
    }
    Ok(())
}

fn normalize_directory_url(value: &str) -> &str {
    match value.trim() {
        "" | "production" => LETS_ENCRYPT_PRODUCTION,
        "staging" => LETS_ENCRYPT_STAGING,
        value => value,
    }
}

async fn reload_certificates(store: &Store, handle: &CertHandle) -> Result<(), AcmeError> {
    let certs = store.list_certificates().await?;
    let settings = store.get_settings().await?;
    handle.store(CertStore::from_certs(certs, settings.active_certificate_id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed(expires_at: Option<DateTime<Utc>>) -> Certificate {
        Certificate {
            id: 1,
            name: "example".into(),
            sni: vec!["example.com".into()],
            cert_pem: "cert".into(),
            key_pem: "key".into(),
            acme_config: Some(AcmeConfig {
                directory_url: "staging".into(),
                challenge: AcmeChallenge::TlsAlpn01,
                email: None,
            }),
            acme_status: Some(AcmeStatus {
                state: AcmeState::Issued,
                issued_at: None,
                expires_at,
                last_attempt: None,
                last_error: None,
            }),
        }
    }

    #[test]
    fn renews_missing_and_expiring_certificates() {
        let now = Utc::now();
        let mut missing = managed(Some(now + chrono::Duration::days(60)));
        missing.cert_pem.clear();
        assert!(needs_renewal(&missing, now));
        let mut forced = managed(Some(now + chrono::Duration::days(60)));
        forced.acme_status = Some(AcmeStatus::pending());
        assert!(needs_renewal(&forced, now));
        assert!(needs_renewal(
            &managed(Some(now + chrono::Duration::days(29))),
            now
        ));
        assert!(!needs_renewal(
            &managed(Some(now + chrono::Duration::days(31))),
            now
        ));
    }

    #[test]
    fn rejects_wildcards_for_tls_alpn() {
        let error = validate_config(
            &["*.example.com".into()],
            &AcmeConfig {
                directory_url: "staging".into(),
                challenge: AcmeChallenge::TlsAlpn01,
                email: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("wildcard"));
    }

    #[test]
    fn tls_alpn_certificate_contains_critical_acme_identifier() {
        let digest = [7u8; 32];
        let challenge = tls_alpn_certificate("example.com", &digest).unwrap();
        let (_, pem) = parse_x509_pem(challenge.cert_pem.as_bytes()).unwrap();
        let certificate = pem.parse_x509().unwrap();
        let extension = certificate
            .extensions()
            .iter()
            .find(|extension| extension.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
            .unwrap();

        assert!(extension.critical);
        assert_eq!(extension.value, [&[0x04, 0x20], digest.as_slice()].concat());
    }
}
