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
use pingora::tls::ext::{ssl_use_certificate, ssl_use_private_key};
use pingora::tls::pkey::{PKey, Private};
use pingora::tls::ssl::NameType;
use pingora::tls::x509::X509;
use raahi_core::{Certificate, host_matches};

/// A parsed certificate + key with the SNI patterns it serves.
pub struct CertEntry {
    pub id: i64,
    pub sni: Vec<String>,
    cert: X509,
    key: PKey<Private>,
}

/// All loaded certificates, with a default fallback for non-SNI / unmatched requests.
pub struct CertStore {
    entries: Vec<CertEntry>,
    default_idx: usize,
}

/// Validate that a cert + key parse as PEM (used by the API for create-time feedback).
pub fn validate_cert(cert_pem: &str, key_pem: &str) -> Result<(), String> {
    X509::from_pem(cert_pem.as_bytes()).map_err(|e| format!("invalid certificate PEM: {e}"))?;
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
            match (
                X509::from_pem(c.cert_pem.as_bytes()),
                PKey::private_key_from_pem(c.key_pem.as_bytes()),
            ) {
                (Ok(cert), Ok(key)) => entries.push(CertEntry {
                    id: c.id,
                    sni: c.sni,
                    cert,
                    key,
                }),
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
}

#[async_trait]
impl TlsAccept for SniResolver {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        let name = ssl.servername(NameType::HOST_NAME).map(str::to_string);
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
    }
}

/// Build TLS settings that select a certificate per SNI from the live `handle`.
pub fn sni_tls_settings(handle: CertHandle) -> Result<TlsSettings, String> {
    let cb: TlsAcceptCallbacks = Box::new(SniResolver { handle });
    let mut settings = TlsSettings::with_callbacks(cb).map_err(|e| format!("tls settings: {e}"))?;
    settings.enable_h2();
    Ok(settings)
}
