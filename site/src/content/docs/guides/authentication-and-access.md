---
title: Authentication and access control
description: Authenticate proxied clients, assign identities, and restrict who can reach an upstream.
sidebar:
  order: 3
---

Raahi uses **consumers** to identify clients that send traffic through the proxy. This is separate from user accounts for the management UI.

## Request flow

```text
credential -> authentication plugin -> consumer -> ACL or consumer rate limit
```

1. Create a consumer.
2. Assign group names if you plan to use an ACL.
3. Add one or more credentials.
4. Attach an authentication plugin to a route, service, or all traffic.
5. Add an ACL or consumer-based rate limit if needed.

## API key authentication

For a `key-auth` credential, the `identifier` is the API key. The plugin checks the configured headers and query parameters. The defaults include `apikey` and `x-api-key`.

```json
{
  "type": "key-auth",
  "scope": "route",
  "route_id": 12,
  "config": {
    "key_names": ["x-api-key"],
    "hide_credentials": true
  }
}
```

## Basic authentication

Create a `basic-auth` credential with the username in `identifier` and the plaintext password in `secret`. Raahi stores a bcrypt hash and never returns the password after creation.

## JWT authentication

Consumer JWT credentials support HS256, HS384, HS512, and RS256. Raahi uses the `iss` claim to find a consumer by default.

To accept tokens from an identity provider, set `jwks_url` on the plugin. Raahi refreshes the JSON Web Key Set in the background and selects the key by `kid`.

It verifies RS256 tokens without requiring one Raahi credential for every external user.

## Consumer groups and ACLs

Assign groups to consumers, then configure `acl`:

```json
{
  "allow": ["paid", "internal"],
  "deny": ["suspended"]
}
```

Raahi checks deny groups first. An ACL requires an authentication plugin because it needs a consumer identity.

## IP restrictions

`ip-restriction` accepts CIDRs and individual addresses in `allow` and `deny` lists. It does not require a consumer. Raahi checks deny entries first.

Raahi uses the forwarded client address for access checks. Trust forwarded headers only when the proxy directly in front of Raahi is under your control.
