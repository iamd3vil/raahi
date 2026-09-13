---
title: Services and targets
description: Define upstream applications, backend servers, load balancing, and health checks.
sidebar:
  order: 2
---

A **service** describes one upstream application. A **target** is a server that can handle requests for that service.

## Service fields

| Field | Purpose | Default |
| --- | --- | --- |
| `name` | Unique operator-facing name | required |
| `protocol` | Upstream `http` or `https` | `http` |
| `connect_timeout_ms` | TCP and TLS connection timeout | `5000` |
| `read_timeout_ms` | Upstream response timeout | `60000` |
| `write_timeout_ms` | Upstream request timeout | `60000` |
| `retries` | Retry count | `1` |
| `lb_algorithm` | Target selection strategy | `round_robin` |
| `tls_sni` | SNI sent to HTTPS upstreams | target host |
| `health_path` | HTTP path for active checks | TCP connection check |

## Target fields

| Field | Purpose | Default |
| --- | --- | --- |
| `host` | IP address or DNS name | required |
| `port` | Upstream port | required |
| `weight` | Share used by weighted balancing | `100` |
| `enabled` | Whether the target may receive traffic | `true` |

## Load balancing

- `round_robin` rotates through healthy targets.
- `weighted` gives targets traffic in proportion to their `weight` values.
- `random` chooses a healthy target at random.
- `consistent` hashes the client IP so the same client usually reaches the same target.

Raahi sends traffic only to targets that are enabled and healthy.

## Health checks

Without `health_path`, Raahi opens a TCP connection to test the target. With `health_path`, it sends an HTTP GET and accepts a 2xx or 3xx response. Checks for HTTPS services verify the upstream certificate.

A hostname may resolve to several addresses. Raahi tests each address and uses one that works. It refreshes DNS every 30 seconds and keeps the previous addresses if a lookup fails.

HTTP proxying, TCP proxying, and health checks use the same selected address. A failed proxy connection removes the target before the next scheduled health check.

Check the current state of every target:

```bash
curl http://127.0.0.1:9080/api/v1/health
```
