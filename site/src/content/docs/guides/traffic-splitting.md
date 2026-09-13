---
title: Traffic splitting
description: Send a controlled share of requests to a canary or replacement service.
sidebar:
  order: 4
---

A route can divide requests between services. Use this to test a new version with a small share of live traffic or move traffic between upstreams in steps.

```json
{
  "name": "api-canary",
  "service_id": 10,
  "hosts": ["api.example.com"],
  "paths": ["/"],
  "splits": [
    { "service_id": 10, "weight": 95 },
    { "service_id": 14, "weight": 5 }
  ]
}
```

When `splits` contains entries, Raahi ignores the route's `service_id` and uses smooth weighted round-robin across the listed services.

## Roll out a new service

1. Create a separate service for the new version.
2. Add its targets and health check.
3. Give it a small weight, such as 5.
4. Watch route errors, target health, and upstream logs.
5. Raise the weight in steps.
6. Remove the old service from the split when the move is complete.

Weights are relative. `95:5` and `19:1` produce the same distribution.

:::note
Traffic splitting does not keep a client on one service. If an application needs session affinity, handle it in the application or use `consistent` target selection inside one service.
:::
