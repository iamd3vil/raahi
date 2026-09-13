---
title: Configuration
description: Set listener addresses, worker threads, logging, and the database location.
sidebar:
  order: 1
---

Routes, upstreams, plugins, users, and certificates live in the Raahi database. Startup options select the database, admin UI files, listener overrides, and worker count.

## Startup options

| Option | Environment variable | Purpose |
| --- | --- | --- |
| `--db <URL>` | `RAAHI_DB` | SQLite URL. Default: `sqlite://raahi.db` |
| `--seed` | | Create the demo service and route when no service exists |
| `--http-addr <ADDR>` | | Override the stored HTTP listener |
| `--https-addr <ADDR>` | | Override the stored HTTPS listener |
| `--admin-addr <ADDR>` | | Override the stored management listener |
| `--ui-dir <DIR>` | `RAAHI_UI_DIR` | Built admin UI directory. Default: `ui/build` |
| `--threads <N>` | `RAAHI_THREADS` | Proxy worker threads. Default: available CPU cores |

Use `RUST_LOG` to select log levels. Raahi logs at `info` by default.

```bash
RUST_LOG=raahi_proxy=debug,raahi_api=info raahi
```

## Default listeners

A new database contains these values:

| Setting | Default |
| --- | --- |
| HTTP proxy | `0.0.0.0:8080` |
| HTTPS proxy | `0.0.0.0:8443` |
| Management API | `127.0.0.1:9080` |
| Default load balancing | `round_robin` |
| Active certificate | none |

Startup flags override stored listener addresses for that process.

## Changes that apply without a restart

Changes to services, targets, HTTP routes, plugins, consumers, credentials, and certificates apply while Raahi continues serving traffic. Changing a certificate also updates the active HTTPS listener.

## Changes that require a restart

- Add or remove a TCP stream listener.
- Change a stream route's `listen_addr`.
- Change the proxy worker count.
- Change a listener address when the process must bind a different socket.

Pointing an existing stream route at another service does not require a restart.

:::caution
PUT endpoints replace the complete resource. Include every field that you want to keep.
:::
