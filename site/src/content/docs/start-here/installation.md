---
title: Installation
description: Build Raahi from source or package a static Linux release.
sidebar:
  order: 1
---

Raahi ships as one executable plus the built admin UI.

## Prerequisites

- Rust 1.85 or newer to compile Raahi
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

`just dist` builds a static musl executable and packages it with the admin UI:

```bash
uv tool install cargo-zigbuild
just dist
```

The archive is written under `dist/`. It runs on x86-64 Linux without shared library dependencies. You need `zig` on `PATH` to build it.

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
