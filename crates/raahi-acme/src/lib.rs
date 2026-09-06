mod cloudflare;

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use cloudflare::{CloudflareProvider, DnsProvider, DnsRecord};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, ExternalAccountKey,
    Identifier, NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_core::{
    AcmeChallenge, AcmeConfig, AcmeEabCredentials, AcmeState, AcmeStatus, AlpnChallengeCertificate,
    AlpnChallengeRegistry, Certificate, LETS_ENCRYPT_PRODUCTION, LETS_ENCRYPT_STAGING,
    ZEROSSL_DIRECTORY,
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
    load_account_with_client(store, directory_url, email, Account::builder()?).await
}

async fn load_account_with_client(
    store: &Store,
    directory_url: &str,
    email: Option<&str>,
    builder: instant_acme::AccountBuilder,
) -> Result<Account, AcmeError> {
    if let Some((credentials, _)) = store.get_acme_account(directory_url).await? {
        let credentials = serde_json::from_str::<AccountCredentials>(&credentials)?;
        return Ok(builder.from_credentials(credentials).await?);
    }

    let eab = store.get_acme_eab(directory_url).await?;
    if directory_url == ZEROSSL_DIRECTORY && eab.is_none() {
        return Err(AcmeError::Config("ZeroSSL requires EAB credentials for account registration. Configure them in Certificates.".into()));
    }
    let external_key = eab.as_ref().map(external_account_key).transpose()?;
    let contact = email.map(|email| format!("mailto:{email}"));
    let contacts = contact.iter().map(String::as_str).collect::<Vec<_>>();
    let (account, credentials) = builder
        .create(
            &NewAccount {
                contact: &contacts,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory_url.to_string(),
            external_key.as_ref(),
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
    validate_directory_url(directory)?;
    if directory == ZEROSSL_DIRECTORY && config.challenge != AcmeChallenge::Dns01 {
        return Err(AcmeError::Config(
            "Choose DNS-01 for ZeroSSL. Raahi does not yet implement HTTP-01.".into(),
        ));
    }
    Ok(())
}

pub fn validate_directory_url(value: &str) -> Result<(), AcmeError> {
    let url = reqwest::Url::parse(normalize_directory_url(value))
        .map_err(|_| AcmeError::Config("Invalid ACME directory URL".into()))?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    );
    if url.host_str().is_none()
        || (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
    {
        return Err(AcmeError::Config(
            "ACME directory must use HTTPS (HTTP is allowed only for loopback testing)".into(),
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AcmeError::Config(
            "ACME directory URL must not contain credentials, a query, or a fragment".into(),
        ));
    }
    Ok(())
}

pub fn normalize_directory_url(value: &str) -> &str {
    match value.trim() {
        "" | "production" | "letsencrypt" => LETS_ENCRYPT_PRODUCTION,
        "staging" | "letsencrypt-staging" => LETS_ENCRYPT_STAGING,
        "zerossl" => ZEROSSL_DIRECTORY,
        value if value.trim_end_matches('/') == ZEROSSL_DIRECTORY => ZEROSSL_DIRECTORY,
        value => value,
    }
}

pub fn normalize_eab_credentials(
    credentials: &AcmeEabCredentials,
) -> Result<AcmeEabCredentials, AcmeError> {
    validate_directory_url(&credentials.directory_url)?;
    let normalized = AcmeEabCredentials {
        directory_url: normalize_directory_url(&credentials.directory_url).to_string(),
        key_id: credentials.key_id.trim().to_string(),
        hmac_key: credentials.hmac_key.trim().to_string(),
    };
    external_account_key(&normalized)?;
    Ok(normalized)
}

fn external_account_key(credentials: &AcmeEabCredentials) -> Result<ExternalAccountKey, AcmeError> {
    if credentials.key_id.is_empty()
        || credentials.key_id.len() > 1024
        || credentials.key_id.chars().any(char::is_control)
    {
        return Err(AcmeError::Config(
            "EAB key ID must be non-empty and at most 1024 bytes".into(),
        ));
    }
    if credentials.hmac_key.len() > 4096 {
        return Err(AcmeError::Config("EAB HMAC key is too long".into()));
    }
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&credentials.hmac_key)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&credentials.hmac_key))
        .map_err(|_| AcmeError::Config("EAB HMAC key must be base64url encoded".into()))?;
    if key.is_empty() {
        return Err(AcmeError::Config("EAB HMAC key must not be empty".into()));
    }
    Ok(ExternalAccountKey::new(credentials.key_id.clone(), &key))
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

#[cfg(test)]
mod provider_tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn provider_defaults_aliases_and_zero_challenges() {
        let default: AcmeConfig =
            serde_json::from_value(json!({"challenge":"tls-alpn-01"})).unwrap();
        assert_eq!(default.directory_url, LETS_ENCRYPT_PRODUCTION);
        assert_eq!(
            normalize_directory_url("production"),
            LETS_ENCRYPT_PRODUCTION
        );
        assert_eq!(normalize_directory_url("staging"), LETS_ENCRYPT_STAGING);
        assert_eq!(normalize_directory_url("zerossl"), ZEROSSL_DIRECTORY);
        assert_eq!(
            normalize_directory_url("https://acme.zerossl.com/v2/DV90/"),
            ZEROSSL_DIRECTORY
        );
        let mut config = default;
        config.directory_url = "zerossl".into();
        assert!(validate_config(&["example.com".into()], &config).is_err());
        config.challenge = AcmeChallenge::Dns01;
        assert!(validate_config(&["*.example.com".into()], &config).is_ok());
        for directory in [
            "http://ca.example.com/directory",
            "https://key:secret@ca.example.com",
            "https://ca.example.com/?token=secret",
        ] {
            assert!(validate_directory_url(directory).is_err());
        }
    }

    #[test]
    fn eab_validation_accepts_base64url_and_redacts_debug() {
        for key in ["c2VjcmV0", "YQ==", "YQ"] {
            let eab = AcmeEabCredentials {
                directory_url: "zerossl".into(),
                key_id: " secret-id ".into(),
                hmac_key: key.into(),
            };
            let normalized = normalize_eab_credentials(&eab).unwrap();
            assert_eq!(normalized.directory_url, ZEROSSL_DIRECTORY);
            assert_eq!(normalized.key_id, "secret-id");
            assert!(!format!("{eab:?}").contains("secret-id"));
            assert!(!format!("{eab:?}").contains(key));
        }
        for key in ["", "not base64!", "YQ===="] {
            assert!(
                normalize_eab_credentials(&AcmeEabCredentials {
                    directory_url: "zerossl".into(),
                    key_id: "id".into(),
                    hmac_key: key.into()
                })
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn registers_with_signed_eab_and_reuses_account_without_registration_secrets() {
        use axum::{
            Json, Router,
            http::StatusCode,
            routing::{get, head, post},
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        for with_eab in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let directory_url = format!("{origin}/directory");
            let directory = json!({"newNonce":format!("{origin}/nonce"),"newAccount":format!("{origin}/account"),"newOrder":format!("{origin}/order"),"revokeCert":format!("{origin}/revoke"),"keyChange":format!("{origin}/key-change")});
            let registrations = Arc::new(AtomicUsize::new(0));
            let count = registrations.clone();
            let expected_url = format!("{origin}/account");
            let app = Router::new()
                .route(
                    "/directory",
                    get(move || {
                        let directory = directory.clone();
                        async move { Json(directory) }
                    }),
                )
                .route(
                    "/nonce",
                    head(|| async { ([("replay-nonce", "dGVzdC1ub25jZQ")], "") }),
                )
                .route(
                    "/account",
                    post(move |Json(jws): Json<Value>| {
                        let count = count.clone();
                        let expected_url = expected_url.clone();
                        async move {
                            let decode = |s: &Value| {
                                base64::engine::general_purpose::URL_SAFE_NO_PAD
                                    .decode(s.as_str().unwrap())
                                    .unwrap()
                            };
                            let payload: Value =
                                serde_json::from_slice(&decode(&jws["payload"])).unwrap();
                            if with_eab {
                                let eab = &payload["externalAccountBinding"];
                                let protected: Value =
                                    serde_json::from_slice(&decode(&eab["protected"])).unwrap();
                                assert_eq!(protected["kid"], "test-id");
                                assert_eq!(protected["url"], expected_url);
                                assert_eq!(protected["alg"], "HS256");
                                let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"secret");
                                ring::hmac::verify(
                                    &key,
                                    format!(
                                        "{}.{}",
                                        eab["protected"].as_str().unwrap(),
                                        eab["payload"].as_str().unwrap()
                                    )
                                    .as_bytes(),
                                    &decode(&eab["signature"]),
                                )
                                .unwrap();
                                let outer: Value =
                                    serde_json::from_slice(&decode(&jws["protected"])).unwrap();
                                let bound_key: Value =
                                    serde_json::from_slice(&decode(&eab["payload"])).unwrap();
                                assert_eq!(bound_key, outer["jwk"]);
                            } else {
                                assert!(payload.get("externalAccountBinding").is_none());
                            }
                            count.fetch_add(1, Ordering::Relaxed);
                            (
                                StatusCode::CREATED,
                                [
                                    ("location", format!("{expected_url}/1")),
                                    ("replay-nonce", "bmV4dC1ub25jZQ".into()),
                                ],
                                Json(json!({"status":"valid"})),
                            )
                        }
                    }),
                );
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let dir = tempfile::tempdir().unwrap();
            let store = Store::connect(&format!(
                "sqlite://{}",
                dir.path().join("test.db").display()
            ))
            .await
            .unwrap();
            if with_eab {
                store
                    .set_acme_eab(&AcmeEabCredentials {
                        directory_url: directory_url.clone(),
                        key_id: "test-id".into(),
                        hmac_key: "c2VjcmV0".into(),
                    })
                    .await
                    .unwrap();
            }
            let client = || {
                Account::builder_with_http(Box::new(
                    hyper_util::client::legacy::Client::builder(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .build_http(),
                ))
            };
            tokio::time::timeout(
                Duration::from_secs(5),
                load_account_with_client(
                    &store,
                    &directory_url,
                    Some("test@example.com"),
                    client(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(registrations.load(Ordering::Relaxed), 1);
            store.delete_acme_eab(&directory_url).await.unwrap();
            tokio::time::timeout(
                Duration::from_secs(5),
                load_account_with_client(&store, &directory_url, None, client()),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(registrations.load(Ordering::Relaxed), 1);
            server.abort();
            store.pool().close().await;
        }
    }
}
