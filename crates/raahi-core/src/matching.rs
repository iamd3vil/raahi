//! Host and path matching primitives, kept separate so they can be unit-tested in
//! isolation from the full config snapshot.

/// Match a request `host` against a route host pattern.
///
/// - `*` matches any host.
/// - `*.example.com` matches any single- or multi-label subdomain of `example.com`
///   (but not the bare apex `example.com`).
/// - otherwise an exact, case-insensitive match (port stripped from `host`).
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.split(':').next().unwrap_or(host).trim();
    let host = host.trim_end_matches('.'); // tolerate FQDN trailing dot
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // "*.example.com" -> host must end with ".example.com"
        return host.len() > suffix.len()
            && host
                .get(host.len() - suffix.len() - 1..)
                .map(|tail| tail.eq_ignore_ascii_case(&format!(".{suffix}")))
                .unwrap_or(false);
    }
    pattern.eq_ignore_ascii_case(host)
}

/// Whether `path` is matched by the prefix `prefix`.
///
/// A prefix matches when the path equals it, or continues with a `/` boundary, so
/// `/api` matches `/api` and `/api/x` but not `/apixyz`. `/` matches everything.
pub fn path_matches(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() || prefix == "/" {
        return true;
    }
    let prefix = prefix.trim_end_matches('/');
    if !path.starts_with(prefix) {
        return false;
    }
    match path.as_bytes().get(prefix.len()) {
        None => true,        // exact match
        Some(b'/') => true,  // boundary
        Some(_) => false,    // e.g. "/apixyz" vs "/api"
    }
}

/// Strip a matched `prefix` from `path`, always returning a path starting with `/`.
pub fn strip_prefix(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return path.to_string();
    }
    let rest = path.strip_prefix(prefix).unwrap_or(path);
    if rest.is_empty() {
        "/".to_string()
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_wildcard_and_exact() {
        assert!(host_matches("*", "anything.com"));
        assert!(host_matches("api.example.com", "api.example.com"));
        assert!(host_matches("API.example.com", "api.example.com"));
        assert!(host_matches("api.example.com", "api.example.com:8080"));
        assert!(host_matches("*.example.com", "api.example.com"));
        assert!(host_matches("*.example.com", "a.b.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "example.org"));
        assert!(!host_matches("api.example.com", "other.example.com"));
    }

    #[test]
    fn path_prefix_boundaries() {
        assert!(path_matches("/", "/anything"));
        assert!(path_matches("/api", "/api"));
        assert!(path_matches("/api", "/api/users"));
        assert!(path_matches("/api/", "/api/users"));
        assert!(!path_matches("/api", "/apixyz"));
        assert!(!path_matches("/api", "/other"));
    }

    #[test]
    fn strip_prefix_cases() {
        assert_eq!(strip_prefix("/api", "/api/users"), "/users");
        assert_eq!(strip_prefix("/api", "/api"), "/");
        assert_eq!(strip_prefix("/api/", "/api/users"), "/users");
        assert_eq!(strip_prefix("", "/api/users"), "/api/users");
        assert_eq!(strip_prefix("/", "/api/users"), "/api/users");
    }
}
