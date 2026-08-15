//! Access-control plugins: `acl` (consumer-group allow/deny) and `ip-restriction`
//! (CIDR allow/deny on the client address).

use std::net::IpAddr;

use raahi_core::ProxyConfig;
use serde::Deserialize;

use super::{Action, Effects, ReqInput, ShortResp};

// ---- acl -------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AclCfg {
    /// If non-empty, the consumer must belong to at least one of these groups.
    pub allow: Vec<String>,
    /// Consumers in any of these groups are rejected (checked before `allow`).
    pub deny: Vec<String>,
}

pub fn acl(c: &AclCfg, _input: &ReqInput, cfg: &ProxyConfig, effects: &mut Effects) -> Action {
    let Some((cid, _)) = &effects.consumer else {
        // ACL needs an identity; an auth plugin must run before it.
        return Action::Respond(ShortResp::text(
            401,
            "Raahi: ACL requires an authenticated consumer\n",
        ));
    };
    let groups: &[String] = cfg
        .consumers
        .get(cid)
        .map(|c| c.groups.as_slice())
        .unwrap_or(&[]);

    if !c.deny.is_empty() && groups.iter().any(|g| c.deny.iter().any(|d| d == g)) {
        return Action::Respond(ShortResp::text(403, "Raahi: consumer group is denied\n"));
    }
    if !c.allow.is_empty() && !groups.iter().any(|g| c.allow.iter().any(|a| a == g)) {
        return Action::Respond(ShortResp::text(403, "Raahi: consumer group not allowed\n"));
    }
    Action::Continue
}

// ---- ip-restriction ----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IpRestrictionCfg {
    /// CIDRs or plain IPs. If non-empty, the client must match one.
    pub allow: Vec<String>,
    /// CIDRs or plain IPs rejected outright (checked before `allow`).
    pub deny: Vec<String>,
    pub status: u16,
    pub message: String,
}

impl Default for IpRestrictionCfg {
    fn default() -> Self {
        IpRestrictionCfg {
            allow: vec![],
            deny: vec![],
            status: 403,
            message: "Your IP address is not allowed".into(),
        }
    }
}

/// `pattern` is an IP (`10.0.0.1`) or CIDR (`10.0.0.0/8`, `2001:db8::/32`).
fn cidr_contains(pattern: &str, ip: IpAddr) -> bool {
    let (addr_s, prefix) = match pattern.split_once('/') {
        Some((a, p)) => match p.parse::<u32>() {
            Ok(n) => (a, Some(n)),
            Err(_) => return false,
        },
        None => (pattern, None),
    };
    let Ok(net_addr) = addr_s.parse::<IpAddr>() else {
        return false;
    };

    // Compare in a common 128-bit space; v4 and v6 never cross-match.
    let (ip_bits, net_bits, width) = match (ip, net_addr) {
        (IpAddr::V4(a), IpAddr::V4(b)) => (u32::from(a) as u128, u32::from(b) as u128, 32u32),
        (IpAddr::V6(a), IpAddr::V6(b)) => (u128::from(a), u128::from(b), 128u32),
        _ => return false,
    };
    let prefix = prefix.unwrap_or(width).min(width);
    if prefix == 0 {
        return true;
    }
    let shift = width - prefix;
    (ip_bits >> shift) == (net_bits >> shift)
}

pub fn ip_restriction(c: &IpRestrictionCfg, input: &ReqInput) -> Action {
    let reject = || {
        Action::Respond(ShortResp::text(
            if c.status >= 100 { c.status } else { 403 },
            &format!("Raahi: {}\n", c.message),
        ))
    };

    let Some(ip) = input.client_ip.and_then(|s| s.parse::<IpAddr>().ok()) else {
        // No usable client address (e.g. unix socket): fail closed only if an
        // allowlist is configured.
        return if c.allow.is_empty() { Action::Continue } else { reject() };
    };

    if c.deny.iter().any(|p| cidr_contains(p, ip)) {
        return reject();
    }
    if !c.allow.is_empty() && !c.allow.iter().any(|p| cidr_contains(p, ip)) {
        return reject();
    }
    Action::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_v4() {
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(cidr_contains("10.0.0.0/8", ip));
        assert!(cidr_contains("10.1.2.3", ip));
        assert!(!cidr_contains("10.2.0.0/16", ip));
        assert!(!cidr_contains("192.168.0.0/16", ip));
        assert!(cidr_contains("0.0.0.0/0", ip));
    }

    #[test]
    fn cidr_v6() {
        let ip: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(cidr_contains("2001:db8::/32", ip));
        assert!(!cidr_contains("2001:db9::/32", ip));
        assert!(!cidr_contains("10.0.0.0/8", ip)); // no cross-family match
    }

    #[test]
    fn cidr_invalid_patterns() {
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(!cidr_contains("not-an-ip", ip));
        assert!(!cidr_contains("10.0.0.0/abc", ip));
    }
}
