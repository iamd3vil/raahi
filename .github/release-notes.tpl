# Raahi {{ meta.tag }}

Raahi now terminates TLS with rustls instead of BoringSSL, and it runs on Pingora 0.9. Building from source needs only Rust, Node.js, and a C compiler. Go, cmake, Perl, and libclang are no longer required.

## What's new

- The HTTPS listener uses rustls with the ring crypto provider. Per-SNI certificate selection, live certificate reload, and TLS-ALPN-01 challenges work as before.
- TLS connections to upstream targets also use rustls. They still verify certificates against the operating system's trust store.
- Pingora 0.9 parses requests more strictly. It rejects ambiguous authorities, CR or LF bytes in an HTTP/2 `:path`, and delimiter bytes in request lines. It also bounds the default HTTP/2 server limits.
- Uploading a certificate now fails when its private key doesn't match it.

## Behaviour changes

- Raahi strips hop-by-hop headers from requests it sends upstream. These are `Connection` and the headers it names, `Keep-Alive`, `Proxy-Connection`, `Proxy-Authorization`, `TE`, `Trailer`, and `Transfer-Encoding`. WebSocket upgrades still pass through. Other `Upgrade` protocols, such as h2c, no longer reach the upstream.
- rustls can't sign with P-521 keys. Raahi rejects P-521 certificates on upload and skips stored ones at startup with a warning, so hosts that rely on one lose HTTPS. RSA, P-256, P-384, and Ed25519 keys work.
- After Raahi answers a TLS-ALPN-01 validation, Pingora logs `Downstream handshake error ... ACME TLS-ALPN-01 validation answered` at ERROR level. Expect one line per validation attempt. Nothing failed.

## Upgrading

This release has no database migrations. Certificates Raahi issues through ACME use P-256 keys and are unaffected. Before upgrading, check any certificate you uploaded yourself. If `openssl x509 -noout -text -in cert.pem` shows `NIST CURVE: P-521`, reissue it with another key type.

## Included

- Native Linux archives for amd64 and arm64
- Multi-architecture container image at `ghcr.io/iamd3vil/raahi:{{ meta.version }}` and `ghcr.io/iamd3vil/raahi:latest`
- Reverse proxying for HTTP, HTTPS, WebSocket, and TCP traffic
- SNI certificate selection and ACME certificate management
- Route matching, load balancing, health checks, traffic splitting, and policy plugins
- Service discovery from DNS, SRV, and HTTP registries
- Config files in YAML, HUML, or JSON with `raahi apply` and `raahi dump`
- Static hosting from a local directory
- Admin UI, REST API, Prometheus metrics, and JSON backup and restore

## Install

Download the archive for your architecture and `checksums.txt`, then verify and extract it:

```bash
archive=raahi-{{ meta.tag }}-linux-amd64.tar.gz
checksum_line="$(grep -F "$archive" checksums.txt)"
expected="${checksum_line#*$'\t'}"
echo "$expected  $archive" | sha256sum -c -
tar xzf "$archive"
cd "${archive%.tar.gz}"
./raahi --help
```

Or run the container:

```bash
docker run --rm \
  -p 8080:8080 \
  -p 8443:8443 \
  -p 127.0.0.1:9080:9080 \
  -v raahi-data:/data \
  ghcr.io/iamd3vil/raahi:{{ meta.version }}
```

Raahi is pre-1.0. The management API and configuration model may change between releases.
