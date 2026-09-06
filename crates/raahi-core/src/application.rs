//! A convenience creation flow over existing gateway resources, not a new entity.
use crate::{Id, Protocol};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationSpec {
    pub name: String,
    pub domain: String,
    pub upstream_url: String,
    #[serde(default)]
    pub https: bool,
    #[serde(default)]
    pub hsts: bool,
    #[serde(default)]
    pub allowed_cidrs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UpstreamAddress {
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
}

/// The wizard configures an upstream origin; path rewriting remains in Routes.
pub fn parse_application_upstream(value: &str) -> Result<UpstreamAddress, String> {
    let url = url::Url::parse(value.trim())
        .map_err(|_| "Enter a complete upstream URL, such as http://127.0.0.1:3000".to_string())?;
    let protocol = match url.scheme() {
        "http" => Protocol::Http,
        "https" => Protocol::Https,
        _ => return Err("Upstream must use http:// or https://".into()),
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "Use an upstream origin without credentials, a path, query, or fragment".into(),
        );
    }
    let host = match url
        .host()
        .ok_or("Upstream needs a hostname or IP address")?
    {
        url::Host::Domain(h) => h.to_string(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    let port = url.port_or_known_default().ok_or("Upstream needs a port")?;
    if port == 0 {
        return Err("Upstream port must be between 1 and 65535".into());
    }
    Ok(UpstreamAddress {
        host,
        port,
        protocol,
    })
}

impl ApplicationSpec {
    pub fn normalized(&self) -> Result<Self, String> {
        let mut spec = self.clone();
        spec.name = self.name.trim().to_string();
        if spec.name.is_empty()
            || spec.name.chars().count() > 100
            || spec.name.chars().any(char::is_control)
        {
            return Err(
                "Application name must contain 1–100 characters without control characters".into(),
            );
        }
        spec.domain = self
            .domain
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if spec.domain.len() > 253
            || !spec.domain.contains('.')
            || spec.domain.parse::<IpAddr>().is_ok()
            || spec.domain.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(
                "Enter a domain such as app.example.com, without a scheme, port, path, or wildcard"
                    .into(),
            );
        }
        spec.upstream_url = self.upstream_url.trim().to_string();
        parse_application_upstream(&spec.upstream_url)?;
        if spec.hsts && !spec.https {
            return Err("HSTS requires HTTPS".into());
        }
        if spec.allowed_cidrs.len() > 100 {
            return Err("Use at most 100 allowed IP addresses or networks".into());
        }
        for cidr in &mut spec.allowed_cidrs {
            *cidr = cidr.trim().to_string();
            let (address, prefix) = cidr
                .split_once('/')
                .map_or((cidr.as_str(), None), |(ip, p)| (ip, Some(p)));
            let ip: IpAddr = address
                .parse()
                .map_err(|_| format!("Invalid allowed IP address: {cidr}"))?;
            if let Some(prefix) = prefix {
                let bits: u8 = prefix
                    .parse()
                    .map_err(|_| format!("Invalid network prefix: {cidr}"))?;
                if bits > if ip.is_ipv4() { 32 } else { 128 } {
                    return Err(format!("Invalid network prefix: {cidr}"));
                }
            }
        }
        spec.allowed_cidrs.sort();
        spec.allowed_cidrs.dedup();
        Ok(spec)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplicationCreated {
    pub name: String,
    pub url: String,
    pub service_id: Id,
    pub target_id: Id,
    pub route_id: Id,
    pub plugin_ids: Vec<Id>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_origins_support_tls_ipv6_and_default_ports() {
        let address = parse_application_upstream("https://[::1]:8443").unwrap();
        assert_eq!(address.host, "::1");
        assert_eq!(address.port, 8443);
        assert_eq!(
            parse_application_upstream("https://example.com")
                .unwrap()
                .port,
            443
        );
        for invalid in [
            "localhost:3000",
            "ftp://example.com",
            "http://u:p@example.com",
            "http://example.com/base",
            "http://example.com?token=x",
            "http://example.com/#x",
            "http://localhost:0",
        ] {
            assert!(parse_application_upstream(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn validates_domains_and_access_rules() {
        let mut spec = ApplicationSpec {
            name: " Test ".into(),
            domain: "APP.Example.com.".into(),
            upstream_url: "http://localhost:3000".into(),
            https: false,
            hsts: false,
            allowed_cidrs: vec!["100.64.0.0/10".into(), "::1/128".into()],
        };
        assert_eq!(spec.normalized().unwrap().domain, "app.example.com");
        for domain in [
            "*.example.com",
            "https://example.com",
            "example.com/path",
            "-bad.example.com",
            "127.0.0.1",
            "",
        ] {
            spec.domain = domain.into();
            assert!(spec.normalized().is_err());
        }
        spec.domain = "app.example.com".into();
        spec.allowed_cidrs = vec!["10.0.0.0/33".into()];
        assert!(spec.normalized().is_err());
        spec.allowed_cidrs.clear();
        spec.hsts = true;
        assert!(spec.normalized().is_err());
    }
}
