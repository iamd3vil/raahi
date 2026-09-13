---
title: API authentication
description: Create the first admin and authenticate with a session or admin token.
sidebar:
  order: 2
---

Raahi leaves the management API unauthenticated until you create a user or admin token. The default listener is `127.0.0.1:9080`, so only the local machine can reach it unless you change the address.

## Create the first admin

While authentication is disabled:

```bash
curl -X POST http://127.0.0.1:9080/api/v1/users \
  -H 'content-type: application/json' \
  -d '{
    "email":"you@example.com",
    "name":"You",
    "role":"admin",
    "password":"replace-with-a-long-password"
  }'
```

Raahi enables authentication as soon as it creates the first user.

## Browser sessions

The web UI supports password and OpenID Connect sign-in. Raahi issues an HttpOnly session cookie with `SameSite=Lax` for seven days. When the public URL uses HTTPS, the cookie is secure.

The main endpoints are:

- `POST /api/v1/auth/login`
- `GET /api/v1/auth/me`
- `POST /api/v1/auth/logout`
- `GET /api/v1/auth/sso/start`

## Admin token

An admin can generate or rotate the automation token with `POST /api/v1/admin/token`. The response contains the plaintext token once. Raahi stores only its SHA-256 hash.

Send the token in either header:

```http
Authorization: Bearer <token>
```

```http
X-Admin-Token: <token>
```

Server-sent events also accept `?access_token=` because browser `EventSource` cannot send a custom header. Use a header for every other request.

## Roles

| Role | Access |
| --- | --- |
| `viewer` | Read gateway state, except sensitive admin resources and secret exports |
| `editor` | Viewer access plus changes to routes, upstreams, plugins, consumers, and certificates |
| `admin` | Editor access plus users, SSO, tokens, listener settings, secrets, imports, and secret exports |

Raahi prevents deletion or demotion of the last admin.

## Endpoints that stay public

Raahi does not require authentication for:

- `/healthz`
- `/metrics`
- `/openapi.yaml`
- `/docs`
- `/api/v1/admin/status`
- Login, SSO start, and SSO callback endpoints

Keep network restrictions around the management listener even after enabling application authentication.
