//! Dynamic per-SNI TLS certificate selection for the HTTPS listener (rustls), with
//! live reload. All configured certificates are parsed into an in-memory [`CertStore`]
//! held behind an [`ArcSwap`] ([`CertHandle`]); the certificate resolver reads the
//! current store on every handshake, so the admin API can swap certificates with no
//! restart. Private keys stay in memory (never written to disk).
//!
//! ACME TLS-ALPN-01 challenges are answered before Pingora's handshake by a pre-TLS hook
//! ([`AlpnChallengeResponder`]), because Pingora's rustls listener only negotiates a fixed
//! ALPN list (`h2`, `http/1.1`) and `acme-tls/1` must be negotiated for the validator.

use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use pingora::listeners::PreTlsProcess;
use pingora::listeners::tls::TlsSettings;
use pingora::protocols::ALPN;
use pingora::protocols::l4::stream::Stream as L4Stream;
use pingora::tls::sign::CertifiedKey;
use pingora::tls::{
    CertificateDer, ClientHello, CryptoProvider, PrivateKeyDer, ResolvesServerCert, ServerConfig,
    ServerName, TlsAcceptor,
};
use pingora::{Error, ErrorType, OrErr};
use raahi_core::{AlpnChallengeRegistry, Certificate, host_matches};
use rustls_pki_types::pem::PemObject;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ACME_ALPN: &[u8] = b"acme-tls/1";
/// Upper bound on bytes buffered while looking for a complete ClientHello.
const MAX_CLIENT_HELLO: usize = 64 * 1024;

/// A parsed certificate chain + key with the SNI patterns it serves.
pub struct CertEntry {
    pub id: i64,
    pub sni: Vec<String>,
    key: Arc<CertifiedKey>,
}

/// All loaded certificates, with a default fallback for non-SNI / unmatched requests.
pub struct CertStore {
    entries: Vec<CertEntry>,
    default_idx: usize,
}

fn crypto_provider() -> Arc<CryptoProvider> {
    pingora::tls::install_default_crypto_provider();
    CryptoProvider::get_default()
        .expect("rustls crypto provider installed")
        .clone()
}

fn parse_pem(
    cert_pem: &str,
    key_pem: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), String> {
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid certificate PEM: {e}"))?;
    if certs.is_empty() {
        return Err("certificate PEM contains no certificates".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|e| format!("invalid private key PEM: {e}"))?;
    Ok((certs, key))
}

fn certified_key(
    cert_pem: &str,
    key_pem: &str,
    provider: &CryptoProvider,
) -> Result<CertifiedKey, String> {
    let (certs, key) = parse_pem(cert_pem, key_pem)?;
    CertifiedKey::from_der(certs, key, provider).map_err(|e| format!("unusable key: {e}"))
}

/// Validate that a cert + key parse as PEM and form a usable pair (used by the API for
/// create-time feedback).
pub fn validate_cert(cert_pem: &str, key_pem: &str) -> Result<(), String> {
    certified_key(cert_pem, key_pem, &crypto_provider()).map(|_| ())
}

/// Whether the leaf certificate in `cert_pem` is valid for `host` (SAN match).
pub fn cert_covers_host(cert_pem: &str, host: &str) -> bool {
    let Some(Ok(der)) = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).next() else {
        return false;
    };
    let Ok(name) = ServerName::try_from(host) else {
        return false;
    };
    webpki::EndEntityCert::try_from(&der)
        .is_ok_and(|cert| cert.verify_is_valid_for_subject_name(&name).is_ok())
}

impl CertStore {
    /// Parse PEM certificates into a store. `default_id` chooses the fallback certificate
    /// (else the first). Certificates that fail to parse are skipped with a warning, so a
    /// single bad cert never breaks a live reload.
    pub fn from_certs(certs: Vec<Certificate>, default_id: Option<i64>) -> CertStore {
        let provider = crypto_provider();
        let mut entries = Vec::new();
        for c in certs {
            if c.cert_pem.is_empty() || c.key_pem.is_empty() {
                continue;
            }
            match certified_key(&c.cert_pem, &c.key_pem, &provider) {
                Ok(key) => entries.push(CertEntry {
                    id: c.id,
                    sni: c.sni,
                    key: Arc::new(key),
                }),
                Err(error) => tracing::warn!(
                    "certificate '{}' (#{}) failed to parse ({error}); skipping",
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
        if let Some(name) = sni
            && let Some(e) = self
                .entries
                .iter()
                .find(|e| e.sni.iter().any(|p| host_matches(p, name)))
        {
            return Some(e);
        }
        self.entries.get(self.default_idx)
    }
}

/// A cheaply-cloneable handle to the live certificate store. The certificate resolver
/// reads it per handshake; the admin side swaps it via [`store`](CertHandle::store).
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

impl fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SniResolver")
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name();
        let store = self.handle.current();
        let Some(entry) = store.pick(name) else {
            tracing::warn!("tls: no certificate available for SNI {:?}", name);
            return None; // handshake fails without a certificate
        };
        Some(entry.key.clone())
    }
}

/// Answers ACME TLS-ALPN-01 validation connections before Pingora's TLS handshake.
///
/// While a challenge is pending, it buffers the ClientHello, then rewinds it onto the
/// stream. A ClientHello offering `acme-tls/1` for a domain with a pending challenge is
/// completed here with the challenge certificate and the connection is dropped; every
/// other connection continues to the normal handshake untouched.
pub struct AlpnChallengeResponder {
    challenges: AlpnChallengeRegistry,
}

impl AlpnChallengeResponder {
    pub fn new(challenges: AlpnChallengeRegistry) -> Self {
        AlpnChallengeResponder { challenges }
    }
}

/// Read until a complete ClientHello parses. Returns the bytes read and, if it parsed,
/// the SNI and whether `acme-tls/1` was offered.
async fn read_client_hello(
    stream: &mut L4Stream,
) -> pingora::Result<(Vec<u8>, Option<(Option<String>, bool)>)> {
    let mut buf = Vec::new();
    let mut acceptor = rustls::server::Acceptor::default();
    let mut chunk = [0u8; 4096];
    while buf.len() < MAX_CLIENT_HELLO {
        let n = stream
            .read(&mut chunk)
            .await
            .or_err(ErrorType::ReadError, "reading TLS ClientHello")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut rd = &chunk[..n];
        while !rd.is_empty() {
            if acceptor.read_tls(&mut rd).is_err() {
                return Ok((buf, None));
            }
        }
        match acceptor.accept() {
            Ok(None) => continue,
            Ok(Some(accepted)) => {
                let hello = accepted.client_hello();
                let sni = hello.server_name().map(str::to_string);
                let wants_acme = hello.alpn().is_some_and(|mut p| p.any(|p| p == ACME_ALPN));
                return Ok((buf, Some((sni, wants_acme))));
            }
            Err(_) => break,
        }
    }
    Ok((buf, None))
}

#[async_trait]
impl PreTlsProcess for AlpnChallengeResponder {
    async fn process(&self, stream: &mut L4Stream) -> pingora::Result<()> {
        if self.challenges.is_empty() {
            return Ok(());
        }
        let (hello, parsed) = read_client_hello(stream).await?;
        stream.rewind(&hello);
        let Some((Some(domain), true)) = parsed else {
            return Ok(());
        };
        let Some(challenge) = self.challenges.get(&domain) else {
            return Ok(());
        };
        let (certs, key) = parse_pem(&challenge.cert_pem, &challenge.key_pem).or_err(
            ErrorType::InternalError,
            "ACME TLS-ALPN challenge certificate",
        )?;
        let mut config = ServerConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .and_then(|b| b.with_no_client_auth().with_single_cert(certs, key))
            .or_err(ErrorType::InternalError, "ACME TLS-ALPN challenge config")?;
        config.alpn_protocols = vec![ACME_ALPN.to_vec()];
        let mut tls = TlsAcceptor::from(Arc::new(config))
            .accept(&mut *stream)
            .await
            .or_err(ErrorType::TLSHandshakeFailure, "ACME TLS-ALPN handshake")?;
        let _ = tls.shutdown().await;
        tracing::info!("ACME TLS-ALPN: answered validation for {domain}");
        // The validator only needs the handshake; dropping the connection here keeps it
        // away from the HTTP proxy.
        Error::e_explain(
            ErrorType::ConnectionClosed,
            "ACME TLS-ALPN-01 validation answered",
        )
    }
}

/// Build TLS settings that select a certificate per SNI from the live `handle`. Pair
/// with [`AlpnChallengeResponder`] on the listener to answer TLS-ALPN-01 challenges.
pub fn sni_tls_settings(handle: CertHandle) -> TlsSettings {
    // Cert/key paths are ignored once a resolver is set.
    let mut settings = TlsSettings::intermediate("", "").expect("rustls TlsSettings");
    settings.set_cert_resolver(Arc::new(SniResolver { handle }));
    settings.set_alpn(ALPN::H2H1);
    settings
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pingora::listeners::PreTlsProcess;
    use pingora::protocols::l4::stream::Stream as L4Stream;
    use pingora::tls::{
        CertificateDer, ClientConfig, RootCertStore, ServerConfig, ServerName, TlsAcceptor,
        TlsConnector,
    };
    use raahi_core::{AlpnChallengeCertificate, AlpnChallengeRegistry, Certificate};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    use rustls_pki_types::pem::PemObject;
    use tokio::net::{TcpListener, TcpStream};

    use super::{AlpnChallengeResponder, CertStore, crypto_provider, parse_pem};

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
        assert_eq!(store.entries[0].key.cert.len(), 2);
    }

    struct TestPki {
        ca_pem: String,
        cert_pem: String,
        key_pem: String,
    }

    /// A CA and a leaf for `example.com` signed by it.
    fn test_pki() -> TestPki {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["example.com".into()])
            .unwrap()
            .signed_by(&leaf_key, &issuer)
            .unwrap();
        TestPki {
            ca_pem: ca.pem(),
            cert_pem: leaf.pem(),
            key_pem: leaf_key.serialize_pem(),
        }
    }

    /// Connect to a listener running the responder (with one pending challenge for
    /// `example.com`) offering `client_alpn`. Connections the responder passes on get a
    /// normal `h2` handshake. Returns the ALPN the client negotiated and whether the
    /// responder passed the connection on.
    async fn connect_offering(client_alpn: &[u8]) -> (Option<Vec<u8>>, bool) {
        let pki = test_pki();
        let challenges = AlpnChallengeRegistry::default();
        challenges.insert(
            "example.com",
            AlpnChallengeCertificate {
                cert_pem: pki.cert_pem.clone(),
                key_pem: pki.key_pem.clone(),
            },
        );
        let responder = AlpnChallengeResponder::new(challenges);
        let (certs, key) = parse_pem(&pki.cert_pem, &pki.key_pem).unwrap();
        let mut normal = ServerConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();
        normal.alpn_protocols = vec![b"h2".to_vec()];

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut stream = L4Stream::from(tcp);
            if responder.process(&mut stream).await.is_err() {
                return false;
            }
            TlsAcceptor::from(Arc::new(normal))
                .accept(stream)
                .await
                .unwrap();
            true
        });

        let mut roots = RootCertStore::empty();
        for ca in CertificateDer::pem_slice_iter(pki.ca_pem.as_bytes()) {
            roots.add(ca.unwrap()).unwrap();
        }
        let mut client = ClientConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = vec![client_alpn.to_vec()];
        let tcp = TcpStream::connect(addr).await.unwrap();
        let tls = TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from("example.com").unwrap(), tcp)
            .await
            .unwrap();
        let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        (alpn, server.await.unwrap())
    }

    #[tokio::test]
    async fn acme_validation_is_answered_before_the_proxy_handshake() {
        let (alpn, passed_on) = connect_offering(b"acme-tls/1").await;
        assert_eq!(alpn.as_deref(), Some(&b"acme-tls/1"[..]));
        assert!(!passed_on);
    }

    #[tokio::test]
    async fn regular_clients_reach_the_proxy_handshake_during_a_challenge() {
        let (alpn, passed_on) = connect_offering(b"h2").await;
        assert_eq!(alpn.as_deref(), Some(&b"h2"[..]));
        assert!(passed_on);
    }
}
