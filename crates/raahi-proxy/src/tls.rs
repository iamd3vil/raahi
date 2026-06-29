//! Dynamic per-SNI TLS certificate selection for the HTTPS listener (boringssl).
//!
//! All configured certificates are parsed into memory and a single listener serves
//! them, choosing the right one per request from the SNI server name. Private keys
//! stay in memory (never written to disk).

use std::sync::Arc;

use async_trait::async_trait;
use pingora::listeners::tls::TlsSettings;
use pingora::listeners::{TlsAccept, TlsAcceptCallbacks};
use pingora::protocols::tls::TlsRef;
use pingora::tls::ext::{ssl_use_certificate, ssl_use_private_key};
use pingora::tls::pkey::{PKey, Private};
use pingora::tls::ssl::NameType;
use pingora::tls::x509::X509;
use raahi_core::{host_matches, Certificate};

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

impl CertStore {
    /// Parse PEM certificates. `default_id` chooses the fallback certificate (else the
    /// first). Returns `Ok(None)` when there are no certificates to serve.
    pub fn build(certs: Vec<Certificate>, default_id: Option<i64>) -> Result<Option<CertStore>, String> {
        let mut entries = Vec::new();
        for c in certs {
            let cert = X509::from_pem(c.cert_pem.as_bytes())
                .map_err(|e| format!("certificate '{}': {e}", c.name))?;
            let key = PKey::private_key_from_pem(c.key_pem.as_bytes())
                .map_err(|e| format!("private key '{}': {e}", c.name))?;
            entries.push(CertEntry { id: c.id, sni: c.sni, cert, key });
        }
        if entries.is_empty() {
            return Ok(None);
        }
        let default_idx = default_id
            .and_then(|id| entries.iter().position(|e| e.id == id))
            .unwrap_or(0);
        Ok(Some(CertStore { entries, default_idx }))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Pick the certificate matching `sni` (exact or `*.wildcard`), else the default.
    fn pick(&self, sni: Option<&str>) -> &CertEntry {
        if let Some(name) = sni {
            if let Some(e) = self
                .entries
                .iter()
                .find(|e| e.sni.iter().any(|p| host_matches(p, name)))
            {
                return e;
            }
        }
        &self.entries[self.default_idx]
    }
}

struct SniResolver {
    store: Arc<CertStore>,
}

#[async_trait]
impl TlsAccept for SniResolver {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        let name = ssl.servername(NameType::HOST_NAME).map(str::to_string);
        let entry = self.store.pick(name.as_deref());
        if let Err(e) = ssl_use_certificate(ssl, &entry.cert) {
            tracing::error!("tls: failed to set certificate: {e}");
        }
        if let Err(e) = ssl_use_private_key(ssl, &entry.key) {
            tracing::error!("tls: failed to set private key: {e}");
        }
    }
}

/// Build TLS settings that select a certificate per SNI from `store`.
pub fn sni_tls_settings(store: Arc<CertStore>) -> Result<TlsSettings, String> {
    let cb: TlsAcceptCallbacks = Box::new(SniResolver { store });
    let mut settings =
        TlsSettings::with_callbacks(cb).map_err(|e| format!("tls settings: {e}"))?;
    settings.enable_h2();
    Ok(settings)
}
