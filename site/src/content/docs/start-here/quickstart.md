---
title: Quickstart
description: Start Raahi with a demo route and proxy two local HTTP servers.
sidebar:
  order: 2
---

This walkthrough starts the management API on `127.0.0.1:9080`, the HTTP proxy on `0.0.0.0:8080`, and one route that matches every request.

## Start two upstreams

Open two terminals:

```bash
python3 -m http.server 9001
```

```bash
python3 -m http.server 9002
```

## Start Raahi

From the repository root:

```bash
just run-seed
```

Or build the admin UI and run Raahi through Cargo:

```bash
cd ui && npm install && npm run build && cd ..
cargo run --release -- --seed
```

Open the admin UI at [http://localhost:9080](http://localhost:9080).

The seeded setup contains:

- The `demo-service` upstream, with round-robin load balancing
- The targets `127.0.0.1:9001` and `127.0.0.1:9002`
- The `demo-route` route, which matches every host and path

## Send traffic

```bash
curl -i http://localhost:8080/
curl -i http://localhost:8080/
```

Requests alternate between the two targets. Stop one server and Raahi removes it from rotation after a failed connection or health check.

## Protect the admin API

Raahi starts with management authentication disabled. Create the first admin before exposing the management listener to another machine:

```bash
curl -X POST http://127.0.0.1:9080/api/v1/users \
  -H 'content-type: application/json' \
  -d '{
    "email": "you@example.com",
    "name": "You",
    "role": "admin",
    "password": "replace-with-a-long-password"
  }'
```

Creating the first user enables authentication immediately. Sign in through the web UI for normal work.

## Next steps

- [Add an application](/guides/add-an-application/) with its domain, upstream, and HTTPS policy.
- Read how [route selection](/concepts/routes-and-matching/) works.
- Configure [TLS and certificate renewal](/guides/tls-and-acme/).
- Protect automation with [API authentication](/api/authentication/).
