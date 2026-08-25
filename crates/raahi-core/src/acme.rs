use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub struct AlpnChallengeCertificate {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Clone, Default)]
pub struct AlpnChallengeRegistry {
    inner: Arc<RwLock<HashMap<String, AlpnChallengeCertificate>>>,
}

impl AlpnChallengeRegistry {
    pub fn insert(&self, domain: &str, certificate: AlpnChallengeCertificate) {
        self.inner
            .write()
            .expect("ACME challenge registry poisoned")
            .insert(domain.to_ascii_lowercase(), certificate);
    }

    pub fn get(&self, domain: &str) -> Option<AlpnChallengeCertificate> {
        self.inner
            .read()
            .expect("ACME challenge registry poisoned")
            .get(&domain.to_ascii_lowercase())
            .cloned()
    }

    pub fn remove(&self, domain: &str) {
        self.inner
            .write()
            .expect("ACME challenge registry poisoned")
            .remove(&domain.to_ascii_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AcmeChallenge;

    #[test]
    fn challenge_domains_are_case_insensitive() {
        let registry = AlpnChallengeRegistry::default();
        registry.insert(
            "Example.COM",
            AlpnChallengeCertificate {
                cert_pem: "cert".into(),
                key_pem: "key".into(),
            },
        );

        assert_eq!(registry.get("example.com").unwrap().cert_pem, "cert");
        registry.remove("EXAMPLE.com");
        assert!(registry.get("example.com").is_none());
    }

    #[test]
    fn challenge_names_follow_acme_conventions() {
        assert_eq!(
            serde_json::to_string(&AcmeChallenge::Dns01).unwrap(),
            "\"dns-01\""
        );
        assert_eq!(
            serde_json::to_string(&AcmeChallenge::TlsAlpn01).unwrap(),
            "\"tls-alpn-01\""
        );
    }
}
