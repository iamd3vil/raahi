---
title: Routes and matching
description: Match requests by hostname, path, method, or header, then send them to an upstream service.
sidebar:
  order: 3
---

A route decides which service handles an HTTP request. Every field that contains a matcher must match.

## Match fields

| Field | What it matches |
| --- | --- |
| `hosts` | Exact hostnames, `*.example.com` wildcards, or `*` |
| `paths` | Path prefixes or regular expressions that start with `~` |
| `methods` | HTTP methods such as `GET` or `POST` |
| `headers` | An exact header value, or `"*"` when the header only needs to exist |
| `priority` | Which matching route wins. Higher values win. |

An empty list matches any value for that field.

## Hostnames

Hostname matching ignores case, an incoming port, and a trailing DNS dot.

- `api.example.com` matches only that hostname.
- `*.example.com` matches `api.example.com` and `a.b.example.com`.
- `*.example.com` does not match `example.com`.
- `*` matches every hostname.

## Path prefixes

A path prefix stops at a segment boundary:

| Pattern | `/api` | `/api/users` | `/apixyz` |
| --- | --- | --- | --- |
| `/api` | match | match | no match |

`/` matches every path.

## Regular expression paths

Start a path with `~` to use a regular expression:

```json
{
  "paths": ["~/users/[0-9]+"]
}
```

Raahi always starts the match at the beginning of the request path. Add `$` when the expression must match the complete path.

:::caution
Raahi leaves an invalid regular expression out of the active router. The route cannot match until you fix the pattern. Use the router tester after saving a regular-expression route.
:::

## How Raahi chooses between matches

When several routes match, Raahi chooses them in this order:

1. Highest `priority`
2. Longest matched path
3. Lowest route ID

The last rule makes ties deterministic.

## Rewrite the upstream request

Set `strip_path` to remove the matched path before proxying. On a route with `/api`, a request to `/api/users` reaches the upstream as `/users`.

Set `preserve_host` to forward the client's original `Host` header. Otherwise Raahi sends the target hostname.

Raahi adds `X-Forwarded-For`, `X-Forwarded-Host`, and `X-Forwarded-Proto`.

## Split traffic between services

A route normally sends requests to `service_id`. If `splits` contains entries, Raahi uses their weights instead:

```json
{
  "service_id": 12,
  "splits": [
    { "service_id": 12, "weight": 90 },
    { "service_id": 18, "weight": 10 }
  ]
}
```

Use the [router tester](/operations/observability/#test-route-selection) to select a route without contacting its upstream.
