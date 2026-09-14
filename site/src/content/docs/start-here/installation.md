---
title: Installation
description: Build Raahi from source or package a static Linux release.
sidebar:
  order: 1
---

Raahi ships as one executable plus the built admin UI.

## Prerequisites

- The latest stable Rust toolchain to compile Raahi
- Node.js 22.12 or newer
- `cmake`, Go, Perl, and a C or C++ compiler for native build dependencies
- `just` for the repository recipes
- pnpm or npm for the admin UI

On Debian or Ubuntu:

```bash
sudo apt install build-essential cmake golang perl
```

Run the repository check before building:

```bash
just doctor
```

:::note[Build dependencies]
Go and Perl run code generators while compiling the TLS library. The Raahi executable does not require either runtime.
:::

## Install a release

GitHub Releases publishes Linux archives for amd64 and arm64. The archive contains the executable, admin UI, README, and license.

```bash
# Replace amd64 with arm64 when needed.
curl -LO https://github.com/iamd3vil/raahi/releases/download/v0.1.0/raahi-v0.1.0-linux-amd64.tar.gz
curl -LO https://github.com/iamd3vil/raahi/releases/download/v0.1.0/checksums.txt
sha256sum -c checksums.txt --ignore-missing
mkdir raahi-v0.1.0-linux-amd64

tar xzf raahi-v0.1.0-linux-amd64.tar.gz -C raahi-v0.1.0-linux-amd64
cd raahi-v0.1.0-linux-amd64
./raahi --help
```

Raahi also publishes a multi-architecture image for amd64 and arm64:

```bash
docker run --rm \
  -p 8080:8080 \
  -p 8443:8443 \
  -p 127.0.0.1:9080:9080 \
  -v raahi-data:/data \
  ghcr.io/iamd3vil/raahi:0.1.0
```

The image stores the SQLite database in `/data`. It serves the bundled UI and binds the admin listener inside the container.

## Build from source

```bash
git clone https://github.com/iamd3vil/raahi.git
cd raahi
just setup
just release
```

The release binary is written to `target/release/raahi`. The UI build is written to `ui/build`.

Run it from the repository root:

```bash
./target/release/raahi
```

## Build a portable Linux archive

`just dist` builds an x86-64 static musl executable and packages it with the admin UI for local testing:

```bash
uv tool install cargo-zigbuild
just dist
```

The archive is written under `dist/`. You need `zig` on `PATH` to build it. GitHub Releases uses native amd64 and arm64 runners instead of this local packaging recipe.

```bash
tar xzf dist/raahi-v*-x86_64-unknown-linux-musl.tar.gz
cd raahi-v*
./raahi
```

## Fix `stddef.h` build failures

Some Linux distributions install libclang without its built-in C headers. The `just` recipes set `BINDGEN_EXTRA_CLANG_ARGS` when needed. Set it yourself if you run Cargo directly:

```bash
export BINDGEN_EXTRA_CLANG_ARGS="-I$(cc -print-file-name=include)"
cargo build --release
```

Continue with the [quickstart](/start-here/quickstart/) to run a local upstream and send traffic through Raahi.
