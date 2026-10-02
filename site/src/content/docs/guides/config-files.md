---
title: Config files
description: Keep services, routes, plugins, consumers, and certificates in a YAML or HUML file and apply it to a running instance.
sidebar:
  order: 6
---

You can write Raahi's configuration in a file, commit it, and apply it with `raahi apply`. The command sends the file to the management API, which writes it to the database like any other change. The UI keeps working, and you can mix the two. Raahi reads YAML, [HUML](https://huml.io), and JSON.

## Write a file

The file refers to everything by name. A route names its service, and plugins sit under the service or route they apply to.

```yaml
raahi_config: 1

services:
  - name: orders
    health_path: /healthz
    targets:
      - { host: 10.0.0.5, port: 8080 }
      - { host: 10.0.0.6, port: 8080 }
    plugins:                   # apply to every route on this service
      - { type: rate-limit, config: { limit: 100, window_secs: 1 } }

routes:
  - name: orders-api
    service: orders
    hosts: [api.example.com]
    paths: [/orders]
    plugins:                   # apply to this route only
      - { type: key-auth }

plugins:                       # global
  - { type: request-id }

consumers:
  - username: ci
    groups: [internal]
    credentials:
      - { type: key-auth, identifier: "${CI_API_KEY}" }

certificates:
  - name: api
    sni: [api.example.com]
    acme_config: { challenge: dns-01, email: ops@example.com }
```

The same service in HUML:

```
raahi_config: 1
services::
  - ::
    name: "orders"
    health_path: "/healthz"
    targets::
      - ::
        host: "10.0.0.5"
        port: 8080
```

Each entry takes the same fields as the API's request body, with names where the API wants IDs.

| Section | Entries | Notes |
| --- | --- | --- |
| `services` | Services | Nest `targets`, `discovery` sources, and service `plugins` inside each one |
| `routes` | HTTP routes | `service` and each `splits[].service` take a service name |
| `stream_routes` | TCP listeners | `service` takes a service name |
| `plugins` | Global plugins | |
| `consumers` | Consumers | Nest `credentials` inside each one |
| `certificates` | Certificates | `acme_config` for ACME, or `cert_pem` and `key_pem` for an uploaded certificate |

Raahi rejects unknown fields. A typo, or an export-style `service_id`, fails with the field name and line number instead of being ignored.

:::caution
Quote wildcard hosts in YAML. A bare `*.example.com` is YAML alias syntax and fails to parse. Write `hosts: ["*.example.com"]`.
:::

## Apply it

`raahi apply` reads the management URL from `RAAHI_URL` and the admin token from `RAAHI_TOKEN`. The URL defaults to `http://127.0.0.1:9080`. Start with a dry run:

```
$ raahi apply -f raahi.yaml --dry-run
+ service orders
+ target 10.0.0.5:8080 in orders
+ target 10.0.0.6:8080 in orders
+ certificate api
+ route orders-api
+ plugin request-id (global)
+ plugin rate-limit on service orders
+ plugin key-auth on route orders-api
+ consumer ci
+ credential key-auth for ci
Dry run: 10 changes planned, nothing applied.
```

`+` creates, `~` updates, and `-` deletes. Drop `--dry-run` to apply. Run the same file again and it prints `No changes.`

Raahi picks the format from the file extension, which can be `.yaml`, `.yml`, `.huml`, or `.json`. Pass `--format` to override it, or `-f -` to read from stdin.

Applying needs the `admin` role.

## Start from a running instance

If you set Raahi up through the UI, dump the current state and use that as your first file:

```bash
raahi dump > raahi.yaml
raahi dump --format huml > raahi.huml
```

The dump has every section and no secrets. Apply it unchanged and Raahi prints `No changes.`

:::caution
A dump still contains key-auth API keys, plugin configs, and discovery configs. Replace the keys with `${VAR}` references before you commit the file.
:::

## How apply changes things

Raahi compares the file with the database and changes only what differs, in one transaction. If any part fails, nothing changes.

- Services, routes, stream routes, consumers, and certificates match by name. Renaming one deletes it and creates a new one.
- Targets match by host and port within their service.
- Plugins match by type, in order, among the plugins of the same service, route, or global list.
- Anything that matches keeps its database ID. Its discovered targets, health state, and issued certificate stay too.

Changes take effect without a restart. The exception is adding, removing, or rebinding a stream listener, and apply prints a note when that happens.

## Leave sections out

Raahi leaves a section alone if the file doesn't have it. If the file has the section, even as `[]`, Raahi deletes every entry in it that the file doesn't list.

So you can put part of the configuration in a file and leave the rest to the UI. Start with services and routes. A bad edit there breaks traffic, so they gain the most from review. Consumers and certificates can move later, or never.

Raahi refuses to delete a service that a route outside the file still uses:

```
service 'billing' is not in the file but route 'billing-api' still uses it
```

:::danger
Inside a section the file has, the file wins. If the file has a `routes` section, the next apply deletes any route someone added through the UI. Run `--dry-run` before applying.
:::

## Run it from CI

Store the admin token as a CI secret and expose it as `RAAHI_TOKEN`. On pull requests, run `raahi apply --dry-run` so reviewers see the planned changes next to the diff. After merge, run `raahi apply`. A dry run runs the same checks and the same transaction as a real apply, then rolls back. Invalid plugin configs, unknown services, and name conflicts all fail at the dry run.

## Keep secrets out of the file

`raahi apply` replaces `${NAME}` in any string with the value of the environment variable `NAME`, then sends the file. An unset variable stops the apply. Write `$${` for a literal `${`.

```yaml
consumers:
  - username: billing
    credentials:
      - { type: basic-auth, identifier: billing, secret: "${BILLING_PASSWORD}" }
      - { type: jwt, identifier: billing-issuer, secret: "${BILLING_JWT_SECRET}", algorithm: HS256 }
```

The values come from the machine that runs `raahi apply`. The server never expands variables.

If you leave out the `secret` of a basic-auth or jwt credential that already exists, Raahi keeps the stored one. A new credential needs its secret. Raahi stores basic-auth passwords as bcrypt hashes. If the password in the file still matches, it keeps the old hash, so the credential doesn't show up as changed.

## Certificates

An ACME certificate needs `name`, `sni`, and `acme_config`. Raahi requests it after the apply and renews it like any other ACME certificate. If you change its `sni` or `acme_config`, Raahi requests a new certificate and keeps serving the old one until the new one arrives. Set up the ACME provider first, as described in [TLS and ACME](/guides/tls-and-acme/).

An uploaded certificate needs `cert_pem` and `key_pem`, usually from variables:

```yaml
certificates:
  - name: internal
    sni: [internal.example.com]
    cert_pem: "${INTERNAL_CERT_PEM}"
    key_pem: "${INTERNAL_KEY_PEM}"
```

If an uploaded certificate already exists and you leave out both PEMs, Raahi keeps the stored ones.

## What the file doesn't cover

These stay in the UI and API:

- Settings and listener addresses
- WASM modules, though a `wasm` plugin in the file can reference one by name
- ACME accounts, EAB credentials, and the Cloudflare token
- Users, SSO, and the admin token

For a complete copy of an instance, secrets included, use [backup and restore](/operations/backup-and-restore/).

## Use the API directly

`raahi apply` and `raahi dump` call two endpoints. You can call them with curl, but the server won't fill in `${VAR}` references. It stores them as literal text.

```bash
curl -X POST 'http://127.0.0.1:9080/api/v1/config/apply?dry_run=true' \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/yaml' \
  --data-binary @raahi.yaml

curl 'http://127.0.0.1:9080/api/v1/config/current?format=huml' \
  -H "Authorization: Bearer $RAAHI_TOKEN"
```

`POST /api/v1/config/apply` accepts `application/yaml`, `application/huml`, or `application/json` and returns the list of changes. `GET /api/v1/config/current` takes `format=yaml`, `huml`, or `json`. Both need the `admin` role. The [API reference](/api-reference) has the full schema.
