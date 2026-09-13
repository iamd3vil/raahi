---
title: Development
description: Run the backend, admin UI, and documentation site while changing Raahi.
sidebar:
  order: 1
---

Raahi has a Rust backend, a Svelte admin UI, and an Astro documentation site.

## Run the backend and admin UI

Start the backend in one terminal:

```bash
just run-seed
```

Start the UI development server in another:

```bash
just ui-dev
```

Open [http://localhost:5173](http://localhost:5173). Vite forwards API, health, and event requests to the management API on port 9080.

For backend-only changes, skip the UI build:

```bash
just run-fast -- --seed
```

## Repository layout

| Path | Code in that directory |
| --- | --- |
| `crates/raahi-core` | Resource types, route matching, and active configuration |
| `crates/raahi-store` | SQLite migrations, database queries, exports, imports, and configuration builds |
| `crates/raahi-proxy` | HTTP and TCP proxying, plugins, health checks, metrics, and TLS |
| `crates/raahi-api` | Management API, user authentication, SSO, and admin UI serving |
| `crates/raahi-acme` | ACME accounts, challenges, certificate issuance, and renewal |
| `src/main.rs` | Process startup and listener setup |
| `ui/` | Svelte admin UI |
| `site/` | Astro documentation |

## Run checks

```bash
just fmt-check
just clippy-strict
just test
just ui-check
cd site && npm run build
```

Run the smallest relevant check while editing. Run the complete set before a release.

## Update the API contract

`site/public/openapi.yaml` points to `docs/openapi.yaml`. The interactive API reference and downloadable specification therefore use the contract checked into the repository.
