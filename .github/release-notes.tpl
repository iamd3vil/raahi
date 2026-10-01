# Raahi {{ meta.tag }}

Adds service discovery.

## What's new

- Service discovery: a service's targets can now come from DNS (A/AAAA), DNS SRV records, or an HTTP registry. Raahi polls each source, adds new endpoints, and retires stale ones after a configurable grace period.
- Operator-created and discovered targets can coexist in the same service.
- Discovery sources are managed through the REST API and the admin UI. Reading or changing them requires the admin role.
- Targets now carry a priority.

## Upgrading

Raahi applies migration `0011_service_discovery.sql` on first start. It only adds tables and columns, but back up your database before upgrading, since Raahi does not roll migrations back.

## Included

- Native Linux archives for amd64 and arm64
- Multi-architecture container image at `ghcr.io/iamd3vil/raahi:{{ meta.version }}` and `ghcr.io/iamd3vil/raahi:latest`
- Reverse proxying for HTTP, HTTPS, WebSocket, and TCP traffic
- SNI certificate selection and ACME certificate management
- Route matching, load balancing, health checks, traffic splitting, and policy plugins
- Service discovery from DNS, SRV, and HTTP registries
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
