//! Checked-in OpenAPI contract and its interactive documentation page.

use axum::http::header::CONTENT_TYPE;
use axum::response::Html;

const SPEC: &str = include_str!("../../../docs/openapi.yaml");

/// Machine-readable OpenAPI contract. It deliberately stays outside admin auth so
/// API clients can discover the contract before they have a token.
pub async fn spec() -> impl axum::response::IntoResponse {
    ([(CONTENT_TYPE, "application/yaml; charset=utf-8")], SPEC)
}

/// Interactive API reference. Scalar is loaded from its CDN; the raw contract at
/// `/openapi.yaml` remains usable when the browser has no internet access.
pub async fn docs() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Raahi Admin API</title>
  </head>
  <body>
    <script id="api-reference" data-url="/openapi.yaml"></script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
  </body>
</html>"#,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    #[test]
    fn contract_is_valid_yaml_with_unique_operation_ids() {
        let doc: serde_yaml::Value = serde_yaml::from_str(super::SPEC).expect("valid OpenAPI YAML");
        assert_eq!(doc["openapi"].as_str(), Some("3.0.3"));

        let paths = doc["paths"].as_mapping().expect("paths map");
        let mut ids = HashSet::new();
        let mut operations = 0;
        for item in paths.values().filter_map(serde_yaml::Value::as_mapping) {
            for method in ["get", "post", "put", "delete"] {
                let key = serde_yaml::Value::String(method.into());
                let Some(operation) = item.get(&key).and_then(serde_yaml::Value::as_mapping) else {
                    continue;
                };
                let operation_id = operation
                    .get(serde_yaml::Value::String("operationId".into()))
                    .and_then(serde_yaml::Value::as_str)
                    .expect("every operation has operationId");
                assert!(
                    ids.insert(operation_id.to_owned()),
                    "duplicate operationId: {operation_id}"
                );
                operations += 1;
            }
        }
        assert_eq!(
            operations, 53,
            "contract must cover every registered operation"
        );
    }
}
