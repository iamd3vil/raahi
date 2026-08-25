//! Dynamic per-SNI TLS certificate selection for the HTTPS listener (boringssl), with
//! live reload. All configured certificates are parsed into an in-memory [`CertStore`]
//! held behind an [`ArcSwap`] ([`CertHandle`]); the SNI callback reads the current store
//! on every handshake, so the admin API can swap certificates with no restart. Private
//! keys stay in memory (never written to disk).

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use pingora::listeners::tls::TlsSettings;
use pingora::listeners::{TlsAccept, TlsAcceptCallbacks};
use pingora::protocols::tls::TlsRef;
use pingora::tls::ext::{ssl_add_chain_cert, ssl_use_certificate, ssl_use_private_key};
use pingora::tls::pkey::{PKey, Private};
use pingora::tls::ssl::{AlpnError, NameType, select_next_proto};
use pingora::tls::x509::X509;
use raahi_core::{AlpnChallengeRegistry, Certificate, host_matches};

const ACME_ALPN: &[u8] = b"acme-tls/1";
const SERVER_ALPN_PREFERENCE: &[u8] = b"\x0bacme-tls/1\x02h2\x08http/1.1";
const HTTP_ALPN_PREFERENCE: &[u8] = b"\x02h2\x08http/1.1";

/// A parsed certificate + key with the SNI patterns it serves.
pub struct CertEntry {
    pub id: i64,
    pub sni: Vec<String>,
    cert: X509,
    chain: Vec<X509>,
    key: PKey<Private>,
}

/// All loaded certificates, with a default fallback for non-SNI / unmatched requests.
pub struct CertStore {
    entries: Vec<CertEntry>,
    default_idx: usize,
}

/// Validate that a cert + key parse as PEM (used by the API for create-time feedback).
pub fn validate_cert(cert_pem: &str, key_pem: &str) -> Result<(), String> {
    let certs = X509::stack_from_pem(cert_pem.as_bytes())
        .map_err(|e| format!("invalid certificate PEM: {e}"))?;
    if certs.is_empty() {
        return Err("certificate PEM contains no certificates".into());
    }
    PKey::private_key_from_pem(key_pem.as_bytes())
        .map_err(|e| format!("invalid private key PEM: {e}"))?;
    Ok(())
}

impl CertStore {
    /// Parse PEM certificates into a store. `default_id` chooses the fallback certificate
    /// (else the first). Certificates that fail to parse are skipped with a warning, so a
    /// single bad cert never breaks a live reload.
    pub fn from_certs(certs: Vec<Certificate>, default_id: Option<i64>) -> CertStore {
        let mut entries = Vec::new();
        for c in certs {
            if c.cert_pem.is_empty() || c.key_pem.is_empty() {
                continue;
            }
            match X509::stack_from_pem(c.cert_pem.as_bytes()) {
                Ok(mut certs) if !certs.is_empty() => {
                    let cert = certs.remove(0);
                    let key = match PKey::private_key_from_pem(c.key_pem.as_bytes()) {
                        Ok(key) => key,
                        Err(_) => {
                            tracing::warn!(
                                "certificate '{}' (#{}) failed to parse; skipping",
                                c.name,
                                c.id
                            );
                            continue;
                        }
                    };
                    entries.push(CertEntry {
                        id: c.id,
                        sni: c.sni,
                        cert,
                        chain: certs,
                        key,
                    });
                }
                _ => tracing::warn!(
                    "certificate '{}' (#{}) failed to parse; skipping",
                    c.name,
                    c.id
                ),
            }
        }
        let default_idx = default_id
            .and_then(|id| entries.iter().position(|e| e.id == id))
            .unwrap_or(0);
        CertStore {
            entries,
            default_idx,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Pick the certificate matching `sni` (exact or `*.wildcard`), else the default.
    fn pick(&self, sni: Option<&str>) -> Option<&CertEntry> {
        if self.entries.is_empty() {
            return None;
        }
        if let Some(name) = sni {
            if let Some(e) = self
                .entries
                .iter()
                .find(|e| e.sni.iter().any(|p| host_matches(p, name)))
            {
                return Some(e);
            }
        }
        self.entries.get(self.default_idx)
    }
}

/// A cheaply-cloneable handle to the live certificate store. The TLS callback reads it
/// per handshake; the admin side swaps it via [`store`](CertHandle::store).
#[derive(Clone)]
pub struct CertHandle {
    inner: Arc<ArcSwap<CertStore>>,
}

impl CertHandle {
    pub fn new(store: CertStore) -> Self {
        CertHandle {
            inner: Arc::new(ArcSwap::from_pointee(store)),
        }
    }

    /// Atomically replace the certificate store (live, no restart).
    pub fn store(&self, store: CertStore) {
        self.inner.store(Arc::new(store));
    }

    fn current(&self) -> Arc<CertStore> {
        self.inner.load_full()
    }
}

struct SniResolver {
    handle: CertHandle,
    challenges: AlpnChallengeRegistry,
}

#[async_trait]
impl TlsAccept for SniResolver {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        let name = ssl.servername(NameType::HOST_NAME).map(str::to_string);
        if ssl.selected_alpn_protocol() == Some(ACME_ALPN)
            && let Some(challenge) = name
                .as_deref()
                .and_then(|domain| self.challenges.get(domain))
        {
            match (
                X509::from_pem(challenge.cert_pem.as_bytes()),
                PKey::private_key_from_pem(challenge.key_pem.as_bytes()),
            ) {
                (Ok(cert), Ok(key)) => {
                    if let Err(error) = ssl_use_certificate(ssl, &cert) {
                        tracing::error!("ACME TLS-ALPN: failed to set certificate: {error}");
                    }
                    if let Err(error) = ssl_use_private_key(ssl, &key) {
                        tracing::error!("ACME TLS-ALPN: failed to set private key: {error}");
                    }
                    return;
                }
                _ => tracing::error!(
                    "ACME TLS-ALPN: challenge certificate for {:?} failed to parse",
                    name
                ),
            }
        }
        let store = self.handle.current();
        let Some(entry) = store.pick(name.as_deref()) else {
            tracing::warn!("tls: no certificate available for SNI {:?}", name);
            return; // handshake will fail without a certificate
        };
        if let Err(e) = ssl_use_certificate(ssl, &entry.cert) {
            tracing::error!("tls: failed to set certificate: {e}");
        }
        if let Err(e) = ssl_use_private_key(ssl, &entry.key) {
            tracing::error!("tls: failed to set private key: {e}");
        }
        for cert in &entry.chain {
            if let Err(e) = ssl_add_chain_cert(ssl, cert) {
                tracing::error!("tls: failed to add intermediate certificate: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use raahi_core::Certificate;
    use rcgen::{CertificateParams, KeyPair};

    use super::CertStore;

    #[test]
    fn certificate_store_preserves_intermediate_chain() {
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["example.com".into()])
            .unwrap()
            .self_signed(&leaf_key)
            .unwrap();
        let intermediate_key = KeyPair::generate().unwrap();
        let intermediate = CertificateParams::new(vec!["intermediate.example".into()])
            .unwrap()
            .self_signed(&intermediate_key)
            .unwrap();
        let store = CertStore::from_certs(
            vec![Certificate {
                id: 1,
                name: "chain".into(),
                sni: vec!["example.com".into()],
                cert_pem: format!("{}{}", leaf.pem(), intermediate.pem()),
                key_pem: leaf_key.serialize_pem(),
                acme_config: None,
                acme_status: None,
            }],
            None,
        );

        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].chain.len(), 1);
    }
}

/// Build TLS settings that select a certificate per SNI from the live `handle`.
pub fn sni_tls_settings(
    handle: CertHandle,
    challenges: AlpnChallengeRegistry,
) -> Result<TlsSettings, String> {
    let active_challenges = challenges.clone();
    let cb: TlsAcceptCallbacks = Box::new(SniResolver { handle, challenges });
    let mut settings = TlsSettings::with_callbacks(cb).map_err(|e| format!("tls settings: {e}"))?;
    settings.set_alpn_select_callback(move |ssl, client| {
        let has_challenge = ssl
            .servername(NameType::HOST_NAME)
            .and_then(|domain| active_challenges.get(domain))
            .is_some();
        let preference = if has_challenge {
            SERVER_ALPN_PREFERENCE
        } else {
            HTTP_ALPN_PREFERENCE
        };
        select_next_proto(preference, client).ok_or(AlpnError::NOACK)
    });
    Ok(settings)
}
