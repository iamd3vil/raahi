---
title: Backup and restore
description: Export a Raahi configuration or replace an instance from a saved export.
sidebar:
  order: 2
---

Raahi can export its complete configuration as JSON and import that file into another instance. Use this for backups, migrations, or configuration stored in version control.

## Export without secrets

```bash
curl -fsS http://127.0.0.1:9080/api/v1/export \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -o raahi-export.json
```

The default export omits private keys and other secret values. It includes discovery source configuration, but not the targets currently materialized from those sources. Providers repopulate those targets after restore.

## Export a restorable copy

An admin can include secrets:

```bash
curl -fsS 'http://127.0.0.1:9080/api/v1/export?include_secrets=true' \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -o raahi-export-secrets.json
chmod 600 raahi-export-secrets.json
```

This file may contain TLS private keys, ACME account credentials, Cloudflare and EAB credentials, consumer secrets, HTTP discovery headers, and other sensitive values.

## Import an export

```bash
curl -X POST http://127.0.0.1:9080/api/v1/import \
  -H "Authorization: Bearer $RAAHI_TOKEN" \
  -H 'content-type: application/json' \
  --data-binary @raahi-export-secrets.json
```

Import replaces the current routing configuration and assigns new database IDs where needed. Raahi commits the complete import in one transaction. If any part fails, it keeps the previous configuration.

:::danger
Import deletes the current routing configuration. Export the instance before replacing it.
:::

## Back up the database file

You can also use SQLite's backup command or stop Raahi and copy the database. Do not copy only the main database file while Raahi is writing to it. Data may still be present in SQLite write-ahead log files.
