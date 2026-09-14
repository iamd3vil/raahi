# Raahi {{ meta.tag }}

Raahi's first public release.

## Included

- Native Linux archives for amd64 and arm64
- Multi-architecture container image at `ghcr.io/iamd3vil/raahi:{{ meta.version }}` and `ghcr.io/iamd3vil/raahi:latest`
- Reverse proxying for HTTP, HTTPS, WebSocket, and TCP traffic
- SNI certificate selection and ACME certificate management
- Route matching, load balancing, health checks, traffic splitting, and policy plugins
- Admin UI, REST API, Prometheus metrics, and JSON backup and restore

## Install

Download the archive for your architecture and `checksums.txt`, then verify and extract it:

```bash
sha256sum -c checksums.txt --ignore-missing
mkdir raahi-{{ meta.tag }}-linux-amd64

tar xzf raahi-{{ meta.tag }}-linux-amd64.tar.gz -C raahi-{{ meta.tag }}-linux-amd64
cd raahi-{{ meta.tag }}-linux-amd64
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
