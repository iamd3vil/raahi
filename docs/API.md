# Raahi Admin API

Raahi's control plane is a JSON REST API under `/api/v1`. The checked-in
[OpenAPI contract](openapi.yaml) is the authoritative machine-readable reference. A running
instance serves it at `/openapi.yaml` and renders an interactive reference at `/docs`.

## Authentication

The API starts with authentication disabled and normally binds to `127.0.0.1:9080`. Generate
or rotate the admin token with:

```bash
curl -X POST http://127.0.0.1:9080/api/v1/admin/token
```

The plaintext token is returned once. After that, send it with either of these headers:

```bash
curl -H "Authorization: Bearer $RAAHI_TOKEN" http://127.0.0.1:9080/api/v1/services
curl -H "X-Admin-Token: $RAAHI_TOKEN" http://127.0.0.1:9080/api/v1/services
```

Server-sent events also accept `?access_token=...` because browser `EventSource` cannot set
headers. Prefer a header for every other request because URLs are commonly logged.

`/healthz`, `/metrics`, `/openapi.yaml`, `/docs`, and `/api/v1/admin/status` are always open.

## Create a route

Create a service, add at least one target, then attach a route to the service:

```bash
service_id=$(curl -fsS -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"name":"orders","protocol":"http"}' \
  http://127.0.0.1:9080/api/v1/services | jq -r .id)

curl -fsS -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"host":"127.0.0.1","port":9001}' \
  "http://127.0.0.1:9080/api/v1/services/$service_id/targets"

curl -fsS -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"orders-api\",\"service_id\":$service_id,\"hosts\":[\"api.example.com\"],\"paths\":[\"/orders\"]}" \
  http://127.0.0.1:9080/api/v1/routes
```

Mutations are persisted and hot-reloaded before the response is returned. `PUT` bodies are
full replacements: include every value you want to retain. Omitted optional values reset to
their documented defaults.

## Operational details

- Successful creates, updates, and deletes return `200`. Deletes return `{"deleted":true}`.
- Application errors use `{"error":"message"}` with `400`, `404`, `401`, or `500`.
- Collection endpoints are currently unpaginated. `/requests` alone accepts `limit`, defaulting
  to 100 and capped at 500.
- Basic/JWT credential secrets, certificate private keys, and WASM bytes are never returned by
  ordinary resource endpoints. Key-auth API keys are credential identifiers and do appear in
  credential listings.
- `GET /api/v1/export?include_secrets=true` returns sensitive material. Handle it as a secret.
- `POST /api/v1/import` transactionally replaces the current routing configuration and remaps
  IDs. Take an export first.
- Adding/removing an L4 stream listener or changing its `listen_addr` requires a restart.
  Retargeting an existing listener applies live.
- Plugin `config` shapes and defaults are described in the OpenAPI `PluginConfig` schemas.
