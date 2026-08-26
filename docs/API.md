# Raahi Admin API

Raahi's control plane is a JSON REST API under `/api/v1`. The checked-in
[OpenAPI contract](openapi.yaml) is the authoritative machine-readable reference. A running
instance serves it at `/openapi.yaml` and renders an interactive reference at `/docs`.

## Authentication

The API is open until either an admin token is generated or a user is created; it normally
binds to `127.0.0.1:9080`. After that every request must carry one of:

- **Admin token** (automation; acts as `admin`): `Authorization: Bearer $RAAHI_TOKEN` or
  `X-Admin-Token: $RAAHI_TOKEN`. Generate or rotate it with `POST /api/v1/admin/token`
  (the plaintext is returned once). Server-sent events also accept `?access_token=...`
  because browser `EventSource` cannot set headers; prefer a header everywhere else.
- **Session cookie** (`raahi_session`, HttpOnly, SameSite=Lax, 7 days) issued by
  `POST /api/v1/auth/login` (`{"email","password"}`) or by OpenID Connect sign-in
  (`GET /api/v1/auth/sso/start`, a browser navigation). `GET /api/v1/auth/me` reports the
  caller's identity and role; `POST /api/v1/auth/logout` ends the session.

### Roles

| Role | May |
|------|-----|
| `viewer` | `GET` anything (except the user list, SSO settings, and secret exports). |
| `editor` | Everything a viewer may, plus create/update/delete gateway configuration. |
| `admin`  | Everything, plus `/users`, `/sso/config`, `/admin/token`, `PUT /settings`, the Cloudflare token, `/import`, and `GET /export?include_secrets=true`. |

Insufficient role returns `403 {"error":"forbidden: requires the editor role"}`.

### Bootstrapping

While the API is open, create the first admin (this immediately requires sign-in):

```bash
curl -X POST http://127.0.0.1:9080/api/v1/users -H 'content-type: application/json' \
  -d '{"email":"you@example.com","name":"You","role":"admin","password":"<at least 8 chars>"}'
```

The last admin cannot be deleted or demoted. Lockout recovery: delete all rows from
`users` (or clear `settings.admin_token_hash`) in the SQLite database and restart.

### Single sign-on

Admins configure OIDC with `PUT /api/v1/sso/config` (`issuer`, `client_id`,
`client_secret`, optional `label`, `auto_provision_role`, `allowed_domains`). Register
`https://<your-admin-host>/api/v1/auth/sso/callback` as the redirect URI at the provider.
With `auto_provision_role` unset, only pre-created users can sign in through SSO; with it
set, unknown users whose email domain is in `allowed_domains` are created on first login.
The redirect URI is derived from `X-Forwarded-Proto`/`X-Forwarded-Host` (or `Host`), so it
matches whatever hostname the UI is served from.

`/healthz`, `/metrics`, `/openapi.yaml`, `/docs`, `/api/v1/admin/status`, and the
login/SSO endpoints are always open.

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
- Application errors use `{"error":"message"}` with `400`, `401`, `403`, `404`, `429`, or `500`.
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
