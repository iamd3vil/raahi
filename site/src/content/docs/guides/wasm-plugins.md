---
title: WASM plugins
description: Run custom request and response code with a per-call execution limit.
sidebar:
  order: 5
---

Raahi accepts compiled `.wasm` files and WebAssembly Text source. Upload a module, then refer to it by name from a `wasm` plugin.

## Required exports

A module must export:

```text
memory
raahi_alloc(len: i32) -> i32
on_request(ptr: i32, len: i32) -> i64
on_response(ptr: i32, len: i32) -> i64
```

Raahi writes a JSON input into module memory. The handler returns a packed `i64` that contains the output pointer and length. The output is also JSON.

A module can:

- Continue the request
- Add or remove upstream request headers
- Add or remove downstream response headers
- Return a status, headers, and body without contacting the upstream
- Read configuration supplied by the plugin

## Attach a module

```json
{
  "type": "wasm",
  "scope": "route",
  "route_id": 12,
  "config": {
    "module": "teapot",
    "config": {"message": "maintenance"},
    "fuel": 100000
  }
}
```

Every applicable WASM plugin runs. A route-level module does not replace a global or service-level module.

## Failure behavior

Raahi creates a new module instance for each call and limits execution with fuel. If the module traps, exhausts its fuel, omits an export, or returns invalid output, Raahi logs a warning and continues the proxy request.

Because failure allows the request to continue, do not rely on WASM alone for mandatory authentication or access control. Use the built-in authentication and access plugins for those checks.

## Examples

The repository includes:

- `examples/wasm/teapot.wat`, which returns a response without calling an upstream
- `examples/wasm/response-header.wat`, which changes a response header

Use these files as ABI examples for modules written in Rust, TinyGo, AssemblyScript, or WAT.
