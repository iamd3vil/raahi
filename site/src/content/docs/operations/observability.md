---
title: Observability
description: Inspect recent requests, route selection, target health, and Prometheus metrics.
sidebar:
  order: 3
---

Raahi reports recent requests, latency, target health, route counters, and Prometheus metrics. The admin dashboard updates through server-sent events.

## Admin dashboard

The web UI shows:

- Request and status counts
- p50, p95, and p99 latency
- Traffic from routes to services and targets
- Target health
- A searchable recent request list

## JSON metrics

```bash
curl http://127.0.0.1:9080/api/v1/metrics \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

The request endpoint returns 100 records by default and accepts at most 500:

```bash
curl 'http://127.0.0.1:9080/api/v1/requests?limit=200' \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

Latency percentiles use this bounded recent-request window. They are not calculated from an unbounded historical histogram.

## Prometheus metrics

```bash
curl http://127.0.0.1:9080/metrics
```

The endpoint reports total requests, response status classes, unmatched requests, recent latency, route requests and errors, consumer requests, target health, and stream connection counts.

## Target health

```bash
curl http://127.0.0.1:9080/api/v1/health \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

Each target includes its health state and the resolved address that Raahi currently uses.

## Test route selection

The router tester selects a route without contacting an upstream:

```bash
curl --get http://127.0.0.1:9080/api/v1/router/test \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  --data-urlencode 'method=GET' \
  --data-urlencode 'host=api.example.com' \
  --data-urlencode 'path=/v1/users'
```

Use it when a request returns an unmatched-route 404.

## Send request logs to another service

The `http-log` plugin buffers request records and sends JSON arrays to an HTTP endpoint. You can set request headers, the maximum batch size, and the flush interval. Delivery runs outside the proxy request path.
