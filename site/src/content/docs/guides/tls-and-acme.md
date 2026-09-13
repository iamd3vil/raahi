---
title: TLS and ACME
description: Serve HTTPS for many domains and renew their certificates automatically.
sidebar:
  order: 2
---

Raahi can terminate HTTPS for many domains on one listener. It selects a certificate from the requested hostname and can replace that certificate while traffic continues.

## Enable HTTPS

Set `proxy_https_addr` in Settings or pass a startup override:

```bash
raahi --https-addr 0.0.0.0:8443
```

On Linux, port 443 requires root or `CAP_NET_BIND_SERVICE`.

## Select a certificate by hostname

Raahi loads every issued certificate and matches the client's Server Name Indication value against exact or wildcard names. `*.example.com` matches subdomains but not `example.com` itself.

If nothing matches, Raahi uses the active default certificate.

Adding, replacing, removing, or changing the default certificate does not require a restart. Raahi skips a certificate with invalid key material instead of rejecting the rest of the certificate set.

## Upload an existing certificate

Provide:

- A name
- One or more hostnames
- A PEM certificate chain
- The matching PEM private key

Raahi checks the certificate and key before saving them.

## Issue a certificate automatically

| Challenge | Use it for | Requirements |
| --- | --- | --- |
| TLS-ALPN-01 | A non-wildcard certificate issued directly through Raahi | Public DNS points to Raahi and port 443 reaches it |
| Cloudflare DNS-01 | Wildcard or non-wildcard certificates for a Cloudflare DNS zone | A Cloudflare API token with DNS edit access |

Wildcard certificates require DNS-01. Raahi rejects a wildcard configured with TLS-ALPN-01.

## Choose an ACME provider

`directory_url` accepts:

- `production` or `letsencrypt`
- `staging` or `letsencrypt-staging`
- `zerossl`
- A custom HTTPS ACME directory URL

Let's Encrypt production is the default. Use staging while testing certificate setup to avoid production rate limits.

ZeroSSL requires External Account Binding credentials and uses DNS-01 in Raahi. Save the EAB key ID and base64url HMAC key before registering the account.

## Renewal schedule

Raahi tries to issue missing certificates when it starts. It then checks every six hours and renews certificates with fewer than 30 days left. The HTTPS listener uses the new certificate without restarting.

## Protect certificate secrets

Raahi stores private keys, ACME account credentials, Cloudflare tokens, and EAB credentials in its database. Ordinary API responses omit them. An export with `include_secrets=true` includes them.

:::caution
Give Raahi a Cloudflare API token that can edit DNS only for the required zone. Do not use a global API key.
:::
