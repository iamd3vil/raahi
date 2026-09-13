---
title: Add an application
description: Publish an upstream on a domain and add HTTPS or access rules at the same time.
sidebar:
  order: 1
---

Use **Add application** when you want to publish one upstream on one domain. Raahi creates the service, target, route, and selected policies in one database transaction. If validation fails, it creates nothing.

## Use the web UI

1. Open **Applications** and choose **Add application**.
2. Enter a name, public domain, and upstream URL.
3. Test the upstream from the Raahi host.
4. Choose HTTPS, HSTS, or an IP allowlist if needed.
5. Review the resources, then save them.

You can leave the form to create a certificate or change a setting. Raahi keeps the draft until you return.

## Use the API

```bash
curl -X POST http://127.0.0.1:9080/api/v1/applications \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "name": "Photos",
    "domain": "photos.example.com",
    "upstream_url": "http://127.0.0.1:3000",
    "https": true,
    "hsts": false,
    "allowed_cidrs": ["100.64.0.0/10"]
  }'
```

Raahi creates:

- An HTTP service for the application
- One target from `upstream_url`
- An exact-host route at priority 100
- A 308 HTTP to HTTPS redirect when `https` is enabled
- HSTS and IP restriction plugins when selected

The route preserves the incoming host and forwards every path.

## Test the upstream

```bash
curl -X POST http://127.0.0.1:9080/api/v1/applications/test-upstream \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"upstream_url":"http://127.0.0.1:3000"}'
```

Raahi sends one GET with a five-second timeout and does not follow redirects. Any HTTP response, including 401 or 404, confirms that Raahi can reach the upstream.

## Prepare HTTPS first

An application with `https: true` needs an HTTPS listener and an issued certificate that matches the domain. Create the certificate before saving the application. See [TLS and ACME](/guides/tls-and-acme/).

## Fix rejected requests

Raahi rejects the operation when:

- The service name already exists.
- An exact route already uses the domain.
- A wildcard or catch-all route at priority 100 or higher covers the domain.
- HTTPS is enabled but no issued certificate matches the domain.

:::note
Raahi does not create DNS records. Point the domain at the machine or edge service that sends traffic to Raahi.
:::
