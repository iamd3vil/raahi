---
title: Troubleshooting
description: Diagnose unmatched routes, unavailable upstreams, redirect loops, TLS errors, and lockouts.
sidebar:
  order: 4
---

## A request returns 404

Raahi returns its own 404 when no route matches. Check:

- The incoming `Host` header
- Path segment boundaries
- Method and header conditions
- Whether the route is enabled
- Route priorities
- Invalid regular expressions that start with `~`

Use the [router tester](/operations/observability/#test-route-selection) to see which route Raahi selects.

## A request returns 502

A 502 usually means Raahi could not connect to the selected upstream.

1. Query `/api/v1/health`.
2. Connect to the target from the Raahi host.
3. Check whether the application listens only on loopback or another interface.
4. For an HTTPS upstream, check its certificate and the service's `tls_sni` value.
5. Temporarily set `RUST_LOG=raahi_proxy=debug` and inspect the logs.

## A request loops between redirects

When Raahi terminates HTTPS, it sends `X-Forwarded-Proto: https` upstream. Configure the application to trust Raahi as a proxy. An application that ignores this header may redirect every proxied request back to HTTPS.

If a redirect plugin only forces HTTPS, set `http_only` so it does not redirect requests that already use HTTPS.

## The HTTPS listener does not start

Raahi logs the error and continues without the HTTPS listener. Check:

- The listen address is valid and unused.
- The process can bind the selected port.
- The default certificate can be parsed.
- The certificate and private key match.

## ACME issuance fails

TLS-ALPN-01 requires public DNS to point at Raahi and public port 443 to reach it. Wildcard certificates require Cloudflare DNS-01. Check the DNS token's zone permissions and remove stale `_acme-challenge` records.

## The build cannot find `stddef.h`

The `just` recipes work around a common libclang packaging problem. If you run Cargo directly, set:

```bash
export BINDGEN_EXTRA_CLANG_ARGS="-I$(cc -print-file-name=include)"
```

## An admin is locked out

Stop Raahi and back up the database. Remove every user or clear `settings.admin_token_hash`, then restart.

Raahi returns to its unauthenticated bootstrap state. Keep the management listener restricted while authentication is disabled.
