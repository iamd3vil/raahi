---
title: Architecture
description: How Raahi accepts traffic, applies configuration changes, and stores its state.
sidebar:
  order: 1
---

Raahi runs the reverse proxy and its management API in one process.

```text
                              Raahi
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  HTTP / HTTPS -> route -> policy -> load balance -> upstream │
│                       │                                      │
│                       └──── reads active configuration        │
│                                      ^                       │
│  Web UI / API -> SQLite -> build complete configuration      │
│                                      │                       │
│                              replace active version          │
│                                                              │
│  Background jobs: health checks, certificates, keys, logs    │
└──────────────────────────────────────────────────────────────┘
```

## Request handling

The proxy accepts HTTP and HTTPS connections. For each request, Raahi:

1. Selects the matching route.
2. Runs the route's policies.
3. Chooses a healthy target.
4. Rewrites the path or host when configured.
5. Proxies the request and response.

Proxy worker threads default to the machine's available CPU cores. Set `--threads` or `RAAHI_THREADS` to choose another value.

## Configuration changes

The web UI and management API use the same update path:

1. Raahi validates the request.
2. It commits the change to SQLite.
3. It builds a complete routing configuration.
4. It replaces the active configuration in one operation.
5. It returns the API response.

Each request uses one complete configuration. A request cannot see half of an update.

## Background work

Raahi runs these jobs alongside the proxy:

- Active target health checks
- Certificate issuance and renewal
- JSON Web Key Set refresh for external JWT issuers
- Batched delivery for the `http-log` plugin

## Stored state

SQLite stores services, targets, routes, plugins, users, certificates, ACME credentials, and settings. Back up the database or use the [export endpoint](/operations/backup-and-restore/).

:::caution
The database can contain TLS private keys, ACME account credentials, a Cloudflare API token, and an OpenID Connect client secret. It also contains password and session hashes.

Protect the database and any export created with `include_secrets=true`.
:::

## Implementation details

Raahi uses Pingora for proxying and Axum for the management API. An atomic pointer replaces the active configuration when settings change. The web UI is built with Svelte.

These details are useful when changing Raahi itself. Operators use the same routes, policies, and settings regardless of the implementation.
