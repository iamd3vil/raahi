use crate::{ApiError, ApiResult, AppState, reload};
use axum::{Json, extract::State};
use raahi_core::{ApplicationCreated, ApplicationSpec, parse_application_upstream};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub async fn create(
    State(state): State<AppState>,
    Json(spec): Json<ApplicationSpec>,
) -> ApiResult<Json<ApplicationCreated>> {
    let spec = spec.normalized().map_err(ApiError::BadRequest)?;
    if spec.https {
        let certificates = state.store.list_certificates().await?;
        // Match the runtime's first parseable SNI certificate, then check the
        // actual certificate hostname rather than trusting its configured label.
        let usable = certificates
            .iter()
            .find(|c| {
                c.sni
                    .iter()
                    .any(|h| raahi_core::host_matches(h, &spec.domain))
                    && raahi_proxy::validate_cert(&c.cert_pem, &c.key_pem).is_ok()
            })
            .and_then(|c| pingora::tls::x509::X509::from_pem(c.cert_pem.as_bytes()).ok())
            .is_some_and(|cert| cert.check_host(&spec.domain).unwrap_or(false));
        if !usable {
            return Err(ApiError::BadRequest(format!(
                "Add or issue a certificate covering {} in Certificates first",
                spec.domain
            )));
        }
    }
    let application = state.store.create_application(&spec).await?;
    reload(&state).await?;
    Ok(Json(application))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {
    pub upstream_url: String,
}

#[derive(Serialize)]
pub struct ProbeResult {
    pub reachable: bool,
    pub status: Option<u16>,
    pub latency_ms: u128,
    pub message: String,
}

/// Editor-only on the guarded admin router. Private upstreams are intentional.
/// A single GET tests the configured origin; no redirects, cookies, environment
/// proxies, or embedded credentials are followed. No configuration is written.
pub async fn test_upstream(Json(request): Json<ProbeRequest>) -> ApiResult<Json<ProbeResult>> {
    parse_application_upstream(&request.upstream_url).map_err(ApiError::BadRequest)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let start = Instant::now();
    let response = client
        .get(request.upstream_url.trim())
        .header("user-agent", "raahi-application-check")
        .send()
        .await;
    let (reachable, status, message) = match response {
        Ok(response) => (true, Some(response.status().as_u16()), format!("Upstream responded with HTTP {}", response.status().as_u16())),
        Err(e) if e.is_timeout() => (false, None, "No response within 5 seconds. Check the address and whether the application is running.".into()),
        Err(_) => (false, None, "Could not connect. Check the address, DNS, and the upstream TLS certificate if using HTTPS.".into()),
    };
    Ok(Json(ProbeResult {
        reachable,
        status,
        latency_ms: start.elapsed().as_millis(),
        message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn probe_reports_http_status_without_following_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            let read = stream.read(&mut buf).await.unwrap();
            assert!(read > 0, "client closed before sending a request");
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let Json(result) = test_upstream(Json(ProbeRequest {
            upstream_url: format!("http://{addr}"),
        }))
        .await
        .unwrap();
        assert!(result.reachable);
        assert_eq!(result.status, Some(302));
        server.await.unwrap();
        assert!(
            test_upstream(Json(ProbeRequest {
                upstream_url: "file:///etc/passwd".into()
            }))
            .await
            .is_err()
        );
    }
}
