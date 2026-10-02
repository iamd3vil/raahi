# Raahi {{ meta.tag }}

Adds config files. You can keep Raahi's configuration in a YAML, HUML, or JSON file, review it like code, and apply it to a running instance.

## What's new

- `raahi apply -f raahi.yaml` applies a config file through the management API. The file refers to services, routes, consumers, and certificates by name. Plugins sit under the service or route they apply to.
- Apply changes only what differs, in one transaction. Anything that matches by name keeps its ID, discovered targets, health state, and issued certificate.
- `--dry-run` prints the planned creates, updates, and deletes without changing anything.
- Raahi leaves a top-level section alone if the file doesn't have it, so a file can manage just services and routes while consumers stay in the UI.
- `raahi apply` fills `${VAR}` references from the local environment before sending the file. Secrets stay out of the file, and the server never expands variables.
- `raahi dump` prints the running configuration as a file. It leaves out secrets, and applying it unchanged changes nothing.
- Two admin-only endpoints back the commands: `POST /api/v1/config/apply` and `GET /api/v1/config/current`.

The [config files guide](https://raahi.sarat.dev/guides/config-files/) covers the format.

## Upgrading

This release has no database migrations. `raahi` without a subcommand still starts the server, so existing service files and container commands keep working. `/api/v1/export` and `/api/v1/import` are unchanged.

## Included

- Native Linux archives for amd64 and arm64
- Multi-architecture container image at `ghcr.io/iamd3vil/raahi:{{ meta.version }}` and `ghcr.io/iamd3vil/raahi:latest`
- Reverse proxying for HTTP, HTTPS, WebSocket, and TCP traffic
- SNI certificate selection and ACME certificate management
- Route matching, load balancing, health checks, traffic splitting, and policy plugins
- Service discovery from DNS, SRV, and HTTP registries
- Config files in YAML, HUML, or JSON with `raahi apply` and `raahi dump`
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
