---
title: Service discovery
description: Populate a service's targets from DNS, SRV records, or an HTTP registry, and control how stale endpoints leave.
sidebar:
  order: 6
---

A **discovery source** belongs to one service and fills that service's target list from somewhere else. Raahi polls the source, compares the result with the targets it already owns, and adds or retires targets to match. Everything else works as before: load balancing, health checks, plugins, and stream routes treat a discovered target like one you typed in.

Sources may contain registry credentials, so reading or mutating source resources requires the admin role. The provider descriptor endpoint remains available to viewers.

A service can hold several sources. Operator-created targets and discovered targets can coexist in the same service.

## Providers

| Provider | Reads | Supplies its own port | Supplies weight and priority |
| --- | --- | --- | --- |
| `dns` | A and AAAA records for a hostname | no | no |
| `dns-srv` | SRV records for a service name | yes | yes |
| `http` | A JSON document from a registry | yes, per endpoint | yes, per endpoint |

Ask a running instance what it supports instead of hard-coding the list:

```bash
curl http://127.0.0.1:9080/api/v1/discovery/providers \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

Each provider entry carries its JSON Schema in `config_schema` and its defaults in `defaults`.

### Shared config keys

Every provider accepts these inside `config`:

| Key | Purpose | Default |
| --- | --- | --- |
| `interval_ms` | Maximum time between scheduled refreshes; Raahi subtracts up to 10% jitter | `30000` |
| `timeout_ms` | Deadline for one refresh attempt | `5000` |
| `port` | Port for endpoints that arrive without one | required for `dns` |
| `weight` | Weight for endpoints that arrive without one | `100` |
| `priority` | Priority for endpoints that arrive without one | `0` |

Raahi subtracts up to 10% from each scheduled interval. This keeps many sources or Raahi instances from hitting DNS and HTTP registries at the same instant. DNS sources also use the record TTL when it is shorter than `interval_ms`.

### dns

Resolves one hostname and turns every returned address into a target. Set `port`, because A and AAAA records carry no port. `record_types` defaults to both `a` and `aaaa`. Set `resolver` to query a specific resolver instead of the system one.

```json
{"hostname": "orders.service.internal", "port": 8080, "interval_ms": 15000}
```

### dns-srv

Reads SRV records, so each record already carries a host, port, weight, and priority. Raahi uses the record's weight and priority by default. Set `use_record_weight` or `use_record_priority` to `false` to override them with the `weight` and `priority` in `config`. SRV priority is lowest-first, the same direction as target priority, so it maps across unchanged.

```json
{"service_name": "_http._tcp.orders.internal", "use_record_priority": true}
```

### http

Sends a GET to `url` every interval and expects JSON. Each endpoint object needs `host` and may also set `port`, `weight`, `priority`, `enabled`, `key`, and `metadata`. When the array is not at the document root, point `endpoints_path` at it, such as `data.endpoints`.

```json
[{"host": "10.4.1.7", "port": 8080, "weight": 100, "key": "orders-a"}]
```

Use `headers` for an authorization header. Those headers are stored in the Raahi database and returned to anyone who can read the source, so scope the credential to reading the registry. Leave `tls_verify` on.

## Endpoint identity

Raahi matches endpoints across refreshes by `provider_key`: the `key` field for `http`, and the host and port pair for the DNS providers. A target whose key comes back keeps its ID, health state, and counters. Without a stable key, an endpoint that flaps looks like a delete followed by a create, and its health check starts over from unknown.

## Source lifecycle

Refresh results change the source's `state`:

| State | Meaning |
| --- | --- |
| `pending` | Created, first refresh has not finished |
| `healthy` | The last refresh succeeded |
| `failing` | Refreshes are failing and the last known good endpoints are still trusted |
| `stale` | Refreshes have failed for longer than `stale_after_ms` |
| `disabled` | `enabled` is `false`, so the source no longer refreshes |

Read the current state, the endpoint counts, and the last error:

```bash
curl http://127.0.0.1:9080/api/v1/discovery-sources/1/status \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

`revision` is an opaque token from the provider or the normalized endpoint set. Comparing it tells "nothing changed" apart from "nothing ran". It is null until the first refresh succeeds, and `next_refresh_at` is null while a source is disabled.

To queue an immediate refresh instead of waiting for `interval_ms`:

```bash
curl -X POST http://127.0.0.1:9080/api/v1/discovery-sources/1/refresh \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

The refresh call returns the current status as soon as the work is queued. Poll `/status` to see the completed result and inspect `last_error` when it fails.

## Last known good, stale, and removal grace

A discovery source that cannot reach its registry must not empty a live service immediately. Raahi keeps the last set of endpoints it resolved successfully during the configured stale window.

Two timers control what happens next, both on the source:

- `stale_after_ms` (default 300000) is how long that last known good set stays trusted. Past it, the source turns `stale` and its targets turn `stale`. They remain visible but stop taking new traffic. A later successful refresh reactivates endpoints that return.
- `removal_grace_ms` (default 60000) applies to an endpoint that disappeared from a refresh that *succeeded*. That target turns `draining`, stops taking new requests at once, and is deleted when the timer expires. If it comes back before then, it returns to `active` with its health state intact.

Target state follows from those rules:

| Target state | Takes new traffic | Why |
| --- | --- | --- |
| `active` | yes | Present in the last successful refresh |
| `draining` | no | Gone from a successful refresh, finishing in-flight requests |
| `stale` | no | Kept for diagnosis after `stale_after_ms`; a successful refresh can reactivate it |

Deleting a source skips the grace period and removes its targets straight away.

Pick the timers from how the upstream behaves. A short `removal_grace_ms` frees an address quickly but can cut long-running requests. A long `stale_after_ms` rides out a resolver outage and may delay fail-closed behavior, which is why health checks still matter for discovered targets.

## Target priority

Every target has a `priority`, and lower wins. Raahi balances across the healthy targets holding the lowest priority value in the service and moves to the next value up only when every target below it is unavailable. Weight applies within a priority, not across priorities.

Local endpoints at `0` and a remote site at `10` give you failover without a second service or route. With `dns-srv`, the SRV record's own priority field lands here directly.

## Upstream authority

Discovery usually returns IP addresses, and an application that routes on the `Host` header will not recognize `10.4.1.7`. Set `upstream_authority` on the service to the name it expects:

```bash
curl -X PUT http://127.0.0.1:9080/api/v1/services/1 \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"name":"orders","protocol":"http","upstream_authority":"orders.internal"}'
```

When the route does not preserve the incoming `Host` header, Raahi sends `Host: orders.internal` (and `:authority` on HTTP/2) to every target in the service, whatever address it dialed. The value may include a port, as in `orders.internal:8443`. Left null, Raahi falls back to the target host, which is the address discovery returned.

For an HTTPS upstream, `tls_sni` selects the name in the handshake. Unset, it falls back to `upstream_authority` and then to the target host, so setting the authority alone usually gets the certificate name right too. Set `tls_sni` when the two differ.

## Create a source

```bash
curl -X POST http://127.0.0.1:9080/api/v1/services/1/discovery-sources \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "name": "orders-dns",
    "provider": "dns",
    "config": {"hostname": "orders.service.internal", "port": 8080, "interval_ms": 15000},
    "stale_after_ms": 300000,
    "removal_grace_ms": 60000
  }'
```

The first refresh runs in the background, so the response can arrive before any target exists. Check `/status` or list the service's targets a moment later.

Targets owned by a source carry its `source_id` and are not editable through `/api/v1/targets/{id}`. Change the source, or the registry behind it, instead. Setting `enabled` to `false` stops refreshes and marks the source's targets as draining. They receive no new traffic and are removed after `removal_grace_ms`.

See [services and targets](/concepts/services-and-targets/) for the rest of the target fields and how health checks work.
