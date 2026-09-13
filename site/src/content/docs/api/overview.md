---
title: API overview
description: Automate Raahi through its JSON management API.
sidebar:
  order: 1
---

Raahi exposes its management API under `/api/v1`. Use it to create routes and upstreams, attach policies, manage certificates, inspect traffic, and back up the configuration.

A running instance publishes:

- The OpenAPI document at `/openapi.yaml`
- An interactive reference at `/docs`
- This site's [interactive reference](/api-reference)
- A [downloadable copy of the checked-in OpenAPI document](/openapi.yaml)

## API groups

| Group | What it controls |
| --- | --- |
| Applications | Create an upstream, target, domain route, and optional HTTPS policy together |
| Services and targets | Define upstream applications and their servers |
| Routes and stream routes | Match HTTP traffic or open TCP listeners |
| Plugins | Add authentication, access rules, limits, caching, and transforms |
| Consumers and credentials | Identify clients that use proxied APIs |
| Certificates and ACME | Serve HTTPS and renew certificates |
| Users, SSO, and admin token | Protect management access |
| Metrics, health, and requests | Inspect live traffic and target state |
| Export and import | Back up or replace configuration |

## Response rules

- Create, update, and delete operations return HTTP 200 when they succeed.
- Delete responses contain `{"deleted": true}`.
- Errors contain `{"error": "message"}` and use a 4xx or 500 status.
- PUT replaces the complete resource. Missing fields return to their defaults.
- Collections are not paginated. `/requests` is the exception and accepts `limit` up to 500.
- A successful mutation updates the running proxy before the API response returns.

## Create a service

```bash
curl -X POST http://127.0.0.1:9080/api/v1/services \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"name":"orders","protocol":"http"}'
```

Read [API authentication](/api/authentication/) before allowing another machine to reach the management listener.
