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

- **Routing** by host (exact + `*.wildcard`), path prefix (longest-match), and method, with priorities.
- **Load balancing** across weighted targets: round-robin, weighted, random, consistent-hash (by client IP).
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
  - `request-transform` / `response-transform` (add / remove headers)
  - `http-log` (batched JSON delivery of request records to an external collector,
    off the hot path)
- **WASM user plugins**: upload `.wasm` binaries or WAT source and run them per
  route/service/globally, sandboxed and fuel-metered (wasmi). Modules implement a
  JSON-over-memory ABI (`raahi_alloc`, `on_request`, `on_response`) and can
  short-circuit requests, mutate headers, and react to upstream responses. Multiple
  wasm plugins stack on one route. See `examples/wasm/`.
- **Admin API auth**: optional bearer-token protection for the admin API (SHA-256
  token, generated/rotated from the UI or `POST /api/v1/admin/token`; SSE uses
  `?access_token=`). Lockout recovery: clear `settings.admin_token_hash` in SQLite
  and restart.
- **TLS termination** via boringssl with per-SNI certificate selection and live (no-restart)
  certificate reload, and **upstream TLS**.
- **Health**: active checks per target (TCP connect, or HTTP GET on a per-service
  `health_path` with 2xx/3xx = pass) with consecutive-failure thresholds, plus
  passive circuit breaking — a failed connect ejects the backend immediately.
- **Declarative config**: `GET /export` (optionally with secrets for a restorable
  backup) and `POST /import` — a transactional full-replace with id remapping,
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
| `raahi-api`   | axum admin REST API + UI serving, run as a Pingora background service. |
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

`services`, `services/{id}/targets`, `targets/{id}`, `routes`, `plugins`, `consumers`,
`consumers/{id}/credentials`, `credentials/{id}`, `certificates`, `settings` — standard
REST CRUD. Plus `metrics`, `requests`, `events` (SSE live stream), and `config` (debug).

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

Manage certificates in the UI — add, replace, remove, or change the default, and the
running HTTPS listener picks them up **live, with no restart**. (One exception: if HTTPS
starts with zero certificates the listener isn't bound, so enabling HTTPS for the very
first time needs a restart.) Cert private keys are treated as secrets: they stay in
memory, are never written to disk, never logged, and never returned by the API.

## Security notes

- **The admin API is unauthenticated.** It defaults to binding `127.0.0.1:9080`
  (loopback only). Do **not** expose it on a public interface without putting an
  authenticating reverse proxy / network controls in front of it. The proxy data plane
  (`:8080` / `:8443`) is the public surface.
- Passwords are hashed with **bcrypt**; API keys and TLS private keys are never returned
  by the API or written to logs.
- All database access uses **parameterized queries** (sqlx bind parameters).
- Defaults follow least privilege (no permissive CORS on the admin API; loopback admin bind).

## Roadmap / out of scope

- **WASM user-function plugins** — the plugin trait/registry is the seam; native plugins
  ship today, WASM is a later phase.
- gRPC / raw TCP stream proxying; multi-node config sync.
