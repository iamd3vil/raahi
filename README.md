# Raahi

**Raahi** (Hindi/Urdu: *traveler, wayfarer*) is a configurable reverse proxy in the
spirit of Kong — a fast Rust data plane built on [Pingora](https://github.com/cloudflare/pingora),
with a REST admin API and a polished web UI on top of a single SQLite config store.

Configuration is **hot-reloaded**: changes made through the UI/API are persisted to
SQLite and atomically swapped into the running proxy with no restart.

```
            ┌──────────────────────── raahi (one process) ───────────────────────┐
 client ───▶│  Pingora data plane  ── reads ──┐                                   │
 :8080/:8443│   route → plugins → LB → upstream │   ArcSwap<Config>  (hot reload)  │
 admin  ───▶│  axum admin API + UI  ── writes ─┴──▶ SQLite  ──── rebuild snapshot  │
 :9080      │  active health checks                                               │
            └─────────────────────────────────────────────────────────────────────┘
```

## Features

- **Routing** by host (exact + `*.wildcard`), path prefix (longest-match) or
  **`~`-prefixed regex paths** (anchored at the path start; `strip_path` strips the
  matched portion), method, and **header conditions** (exact value or presence),
  with priorities.
- **Traffic splitting / canary**: routes can split across services by weight
  (weighted round-robin per request).
- **Load balancing** across weighted targets: round-robin, weighted, random, consistent-hash (by client IP).
- **L4 stream routes**: raw TCP listeners spliced to services (databases, Redis, any
  TCP protocol) reusing the same load balancing and health flags, with per-listener
  connection/byte metrics. Retargeting applies live; listener add/remove needs a restart.
- **Path & host rewriting** (`strip_path`, `preserve_host`) and `X-Forwarded-*` injection.
- **Built-in plugins** (native Rust; scoped global / per-service / per-route):
  - Auth: `key-auth` (header or query), `basic-auth` (bcrypt), and `jwt`
    (HS256/384/512 + RS256, per-consumer credentials looked up by key claim)
  - Access: `acl` (consumer-group allow/deny) and `ip-restriction` (CIDR allow/deny)
  - `rate-limit` (sliding window, keyed by IP / consumer / route, `RateLimit-*`
    headers, counters survive config reloads)
  - Traffic: `request-termination` (maintenance mode), `request-size-limit` (413),
    and `redirect` (301/302/307/308 with path preservation)
  - `cors` (preflight + response headers)
  - `request-id` (correlation id: UUID v4 injected upstream and echoed downstream,
    preserved from the client when present; runs first so even short-circuited
    responses carry it)
  - `response-compression` (gzip / brotli / zstd for downstream clients, negotiated
    from `Accept-Encoding` by Pingora's built-in compression module)
  - `hsts` (`Strict-Transport-Security` on direct HTTPS responses only, with optional
    `includeSubDomains` and `preload` directives)
  - `proxy-cache` (in-memory TTL response cache with `x-cache` headers and a purge API)
  - `request-transform` / `response-transform` (add / remove headers) and
    `response-body-transform` (find/replace on text bodies)
  - `http-log` (batched JSON delivery of request records to an external collector,
    off the hot path)
- **WASM user plugins**: upload `.wasm` binaries or WAT source and run them per
  route/service/globally, sandboxed and fuel-metered (wasmi). Modules implement a
  JSON-over-memory ABI (`raahi_alloc`, `on_request`, `on_response`) and can
  short-circuit requests, mutate headers, and react to upstream responses. Multiple
  wasm plugins stack on one route. See `examples/wasm/`.
- **Admin users, roles, and SSO**: password sign-in (bcrypt) and OpenID Connect
  single sign-on (authorization code + PKCE; Google, Keycloak, Authentik, Pocket ID,
  Auth0, ...) with cookie sessions, plus `viewer` / `editor` / `admin` roles enforced by
  the API. Optional auto-provisioning on first SSO login, restricted by email domain.
  The legacy admin bearer token (SHA-256, generated/rotated from the UI or
  `POST /api/v1/admin/token`; SSE uses `?access_token=`) remains for automation and
  acts as admin. Auth is open until a user or token exists. Lockout recovery: empty
  the `users` table / clear `settings.admin_token_hash` in SQLite and restart.
- **TLS termination** via boringssl with per-SNI certificate selection and live (no-restart)
  certificate reload, **automatic ACME issuance and renewal** via TLS-ALPN-01 or Cloudflare
  DNS-01 (including wildcards), and **upstream TLS**.
- **Health**: active checks per target (TCP connect, or HTTP GET on a per-service
  `health_path` with 2xx/3xx = pass) with consecutive-failure thresholds, plus
  passive circuit breaking — a failed connect ejects the backend immediately.
  Multi-address hosts (e.g. `localhost` → ::1 + 127.0.0.1) are resolved once per
  config snapshot; probes try every address and elect the working one, which the
  proxy, L4 splicer, and health checks all share (`GET /api/v1/health` shows it).
- **Declarative config**: `GET /api/v1/export` (optionally with secrets for a restorable
  backup) and `POST /api/v1/import` — a transactional full-replace with id remapping,
  usable for GitOps and disaster recovery.
- **Prometheus**: `GET /metrics` exposition endpoint (requests, status classes,
  latency percentiles, per-route/consumer counters, target health gauges).
- **JWKS / identity-provider auth**: the `jwt` plugin can verify RS256 tokens
  against a `jwks_url` (refreshed every 30s, kid-matched) instead of per-consumer
  credentials — validates Auth0/Keycloak/Google-style tokens out of the box.
- **Live observability**: request metrics with latency percentiles (p50/p95/p99, µs precision),
  status breakdown, an SSE-driven dashboard with a "transit map" visualizing traffic flowing
  routes → services → targets (health-aware), a filterable live request log, and per-target
  health via `GET /api/v1/health`.
- **Operator tooling**: a route tester (`GET /api/v1/router/test`) that dry-runs the router
  for any method/host/path, and one-click config export (`GET /api/v1/export`, secrets excluded).

## Workspace layout

| Crate | Responsibility |
|-------|----------------|
| `raahi-core`  | Domain types, the compiled `ProxyConfig` snapshot, and router matching (pure, unit-tested). |
| `raahi-store` | SQLite persistence (sqlx), migrations, CRUD, and snapshot building. |
| `raahi-proxy` | The Pingora `ProxyHttp` data plane: LB, plugins, health checks, metrics, hot reload. |
| `raahi-api`   | axum admin REST API + UI serving (users, roles, sessions, OIDC SSO), run as a Pingora background service. |
| `raahi-acme`  | ACME accounts, Cloudflare DNS-01, TLS-ALPN-01, issuance, and renewal. |
| `raahi` (bin) | Wires the store, data plane, and control plane into one Pingora server. |
| `ui/`         | Svelte 5 + Vite SPA (the "Aurora Transit" admin UI). |

## Prerequisites

- **Rust** ≥ 1.84 (uses edition 2024).
- **cmake**, a **Go** toolchain, and a C/C++ compiler — required to build Pingora's
  native dependencies (BoringSSL via `boring-sys`, and `libz-ng-sys` for compression).
  On most distros: `apt install cmake golang` / `pacman -S cmake go`. Without root,
  `uv tool install cmake` (or `pip install cmake`) provides cmake on `PATH`.
- **Node ≥ 20 + pnpm** to build the UI.

## Quick start

```bash
# 1. Build the UI (served by the admin API in production)
cd ui && pnpm install && pnpm build && cd ..

# 2. Build and run the proxy (creates raahi.db, seeds a demo service + route)
cargo run --release -- --seed

# Proxy:     http://localhost:8080
# Admin UI:  http://localhost:9080
```

Try it against two local upstreams:

```bash
python3 -m http.server 9001 &
python3 -m http.server 9002 &
curl localhost:8080/        # proxied + round-robined to :9001 / :9002
```

## Development

Run the backend, then the UI dev server (it proxies `/api` and SSE to the admin port):

```bash
cargo run -- --seed          # backend on :8080 (proxy) and :9080 (admin)
cd ui && pnpm dev            # UI with hot reload on http://localhost:5173
```

## CLI / configuration

```
raahi [OPTIONS]
  --db <URL>           SQLite URL            [env RAAHI_DB]   (default sqlite://raahi.db)
  --http-addr <ADDR>   proxy HTTP listener   (default from settings, 0.0.0.0:8080)
  --admin-addr <ADDR>  admin API listener    (default from settings, 0.0.0.0:9080)
  --ui-dir <DIR>       built UI to serve     [env RAAHI_UI_DIR] (default ui/build)
  --run-dir <DIR>      runtime artifacts dir [env RAAHI_RUN_DIR] (default .raahi)
  --seed               seed a demo service + route if the DB is empty
```

Listener addresses, the default LB algorithm, and the active TLS certificate live in the
`settings` table (editable in the UI). Routes, services, plugins, consumers, and
certificates are all managed via `/api/v1/*` and take effect immediately.

## Admin API (`/api/v1`)

The complete OpenAPI 3.0 contract is served at [`/openapi.yaml`](http://localhost:9080/openapi.yaml),
with an interactive reference at [`/docs`](http://localhost:9080/docs). See
[`docs/API.md`](docs/API.md) for authentication, common workflows, and operational caveats.

`services`, `services/{id}/targets`, `targets/{id}`, `routes`, `plugins`, `consumers`,
`consumers/{id}/credentials`, `credentials/{id}`, `certificates`, and `settings` provide
the control-plane CRUD surface. PUT is a full replacement, not a partial update.

```bash
curl -X POST localhost:9080/api/v1/services \
  -H 'content-type: application/json' \
  -d '{"name":"api","protocol":"http"}'
```

## TLS notes

Raahi terminates TLS with the **boringssl** backend, which supports **dynamic per-SNI
certificate selection**: all configured certificates are loaded into memory and a single
HTTPS listener serves the right one based on the SNI server name (exact or `*.wildcard`),
falling back to the active/default certificate for non-SNI or unmatched requests. Upstream
(proxy→backend) TLS is supported too.

Manage certificates in the UI — upload PEM material or create an ACME-managed certificate,
replace/remove it, or change the default. The running HTTPS listener picks changes up **live,
with no restart**, including the first certificate issued after startup.

ACME supports Let's Encrypt production/staging or a custom HTTPS directory. TLS-ALPN-01
requires the requested domains to resolve to Raahi with public port 443 reachable. DNS-01
uses a Cloudflare API token and is required for wildcard names. The background service issues
missing certificates immediately, retries failures, scans every six hours, and renews within
30 days of expiry. ACME account credentials, Cloudflare tokens, and certificate private keys
stay in SQLite and memory, are never logged or returned by ordinary APIs, and are included only
in exports made with `include_secrets=true`. Protect the database and secret exports accordingly.

## Security notes

- **Admin API authentication is off until configured.** It defaults to loopback and is
  open until a user or admin token exists. Then callers need a session cookie (password
  or SSO sign-in; `HttpOnly`, `SameSite=Lax`, `Secure` behind HTTPS) or the admin token
  (`Authorization: Bearer ...` / `X-Admin-Token`). Roles gate what a session may do;
  failed password logins are rate limited per client IP. Keep network controls in place
  if the listener is exposed beyond localhost; `/healthz`, `/metrics`, `/openapi.yaml`,
  `/docs`, `/api/v1/admin/status`, and the login/SSO endpoints intentionally remain public.
- **User passwords are bcrypt-hashed**; session tokens are stored as SHA-256 hashes; the
  SSO client secret lives in SQLite and is never returned by the API.
- Basic-auth passwords are hashed with **bcrypt**; Basic/JWT secrets and TLS private keys
  are not returned by ordinary resource APIs or written to logs. Key-auth API keys are
  identifiers and are returned by credential listings, so protect those responses.
- All database access uses **parameterized queries** (sqlx bind parameters).
- Defaults follow least privilege (no permissive CORS on the admin API; loopback admin bind).

## Roadmap / out of scope

- gRPC and multi-node config sync.
