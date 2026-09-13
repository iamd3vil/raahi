---
title: Plugins
description: Apply authentication, access rules, traffic controls, caching, transforms, and custom WASM code.
sidebar:
  order: 4
---

Plugins change how Raahi handles a matched request. A plugin can authenticate a client, reject traffic, change headers or bodies, cache a response, or send request logs elsewhere.

## Choose where a plugin applies

Each plugin uses one scope:

- `global` applies to every matched request.
- `service` applies to requests sent to one service.
- `route` applies to requests matched by one route.

For the same plugin type, route scope replaces service scope. Service scope replaces global scope. WASM plugins behave differently because every applicable WASM plugin runs.

The `ordering` field sets the order within the selected plugins. Raahi also enforces an order between plugin types. Authentication runs before ACLs and consumer rate limits.

Request IDs run early enough to appear on responses returned by another plugin without contacting an upstream.

## Built-in plugins

| Job | Plugins |
| --- | --- |
| Authenticate clients | `key-auth`, `basic-auth`, `jwt` |
| Restrict access | `acl`, `ip-restriction` |
| Control traffic | `rate-limit`, `request-size-limit`, `request-termination`, `redirect`, `cors` |
| Change requests or responses | `request-transform`, `response-transform`, `response-body-transform` |
| Change response handling | `hsts`, `response-compression`, `proxy-cache` |
| Record requests | `request-id`, `http-log` |
| Run custom code | `wasm` |

The [API reference](/api-reference) lists the JSON fields for each plugin.

## Authenticate consumers

Authentication plugins map a credential to a consumer. ACLs and consumer-based rate limits then use that identity.

- `key-auth` reads API keys from configured headers or query parameters.
- `basic-auth` checks passwords stored as bcrypt hashes.
- `jwt` accepts HS256, HS384, HS512, and RS256 consumer credentials.
- `jwt` can verify RS256 tokens against a `jwks_url`. Raahi refreshes keys in the background and selects a key by `kid`.

## Cache responses

`proxy-cache` stores anonymous GET responses in memory. Requests with authorization, cookies, a known consumer, ranges, conditions, or cache directives bypass the cache.

Responses with `Set-Cookie`, `Vary`, `Expires`, or private and no-cache directives do not enter the cache.

The cache holds at most 64 MiB and 10,000 entries. Route, service, and HTTP or HTTPS are part of the cache key. Any configuration update clears the cache.

## Run custom WASM code

Upload a WebAssembly binary or WAT source, then attach it with a `wasm` plugin. A module can inspect request and response JSON, change headers, or return a response before Raahi contacts an upstream.

Raahi limits execution with a fuel budget. Read [WASM plugins](/guides/wasm-plugins/) for the ABI and examples.

:::caution[Check saved plugin settings]
Missing or malformed plugin fields currently fall back to default values. Test the affected route after changing a plugin.
:::
