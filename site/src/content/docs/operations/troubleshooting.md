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

## A service has no targets, or the wrong ones

Discovered targets come from a [discovery source](/concepts/service-discovery/). Start with its status:

```bash
curl http://127.0.0.1:9080/api/v1/discovery-sources/1/status \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

- `pending` means the first refresh has not finished. Force one with `POST /api/v1/discovery-sources/1/refresh`.
- `failing` or `stale` means lookups are failing. `last_error` says why. A failing source still uses its last known good set; a stale source keeps targets visible but stops sending new traffic to them.
- A `stale_count` above zero means those targets outlived `stale_after_ms` and are retained for diagnosis until the source recovers or is deleted.
- A `draining_count` that never falls means targets are waiting for `removal_grace_ms` to expire. Draining targets receive no new traffic.
- An unchanged `revision` after a refresh means the registry returned the same endpoints, so the problem sits in the registry rather than in Raahi.

A discovered target that reaches the wrong application usually needs `upstream_authority` on the service, because the upstream is routing on a `Host` header that now carries an IP address.

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
