---
title: TCP stream routes
description: Proxy databases, Redis, and other TCP services without parsing their protocol.
sidebar:
  order: 5
---

A stream route opens a TCP listener and forwards each connection to a target in an existing service. It uses the service's load balancing and target health state.

```json
{
  "name": "postgres",
  "listen_addr": "0.0.0.0:5432",
  "service_id": 7,
  "enabled": true
}
```

Raahi does not inspect SNI or application data on a stream route. One listening address points to one service.

## Changes that require a restart

| Change | Applies live? |
| --- | --- |
| Point an existing stream route at another service | Yes |
| Change target health or weight | Yes |
| Add or remove a stream listener | No. Restart Raahi. |
| Change `listen_addr` | No. Restart Raahi. |

Raahi reports connection and byte counters for each stream listener through the metrics endpoints.

:::tip
Use HTTP routes for WebSockets. Use stream routes when Raahi should forward TCP without parsing HTTP.
:::
