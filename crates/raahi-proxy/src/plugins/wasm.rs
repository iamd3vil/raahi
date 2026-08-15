//! User-supplied WASM plugins (wasmi interpreter, fuel-metered).
//!
//! ABI (JSON over linear memory) — a module exports:
//!   - `memory`
//!   - `raahi_alloc(size: i32) -> i32`  — returns a pointer the host writes input to
//!   - `on_request(ptr: i32, len: i32) -> i64`   (optional)
//!   - `on_response(ptr: i32, len: i32) -> i64`  (optional)
//!
//! The i64 return packs `(out_ptr << 32) | out_len`; `0` means "no output".
//!
//! `on_request` input: `{method, path, query, host, client_ip, consumer, headers,
//! config}`. Output: `{action: "respond", status, headers, body}` to short-circuit,
//! or any of `req_add`/`req_remove`/`resp_add`/`resp_remove` with `action` omitted
//! or `"continue"`.
//!
//! `on_response` input: `{status, headers, config}`. Output: `resp_add`/`resp_remove`.
//!
//! Failures (missing module, trap, out-of-fuel, malformed output) log a warning and
//! fail open (continue), so a broken user plugin cannot black-hole traffic.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{Action, Effects, ReqInput, RespInput, ShortResp};

/// Default per-call fuel budget (~millions of instructions; generous for filters).
const DEFAULT_FUEL: u64 = 100_000_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WasmCfg {
    /// Name of the uploaded module to run.
    pub module: String,
    /// Opaque config passed to the module on every call.
    pub config: Value,
    /// Per-call fuel limit (instruction budget).
    pub fuel: u64,
}

impl Default for WasmCfg {
    fn default() -> Self {
        WasmCfg {
            module: String::new(),
            config: json!({}),
            fuel: DEFAULT_FUEL,
        }
    }
}

pub struct WasmPlugin {
    engine: wasmi::Engine,
    module: Option<wasmi::Module>,
    module_name: String,
    config: Value,
    fuel: u64,
}

impl WasmPlugin {
    pub fn new(cfg: WasmCfg, bytes: Option<&Arc<Vec<u8>>>) -> Self {
        let mut econfig = wasmi::Config::default();
        econfig.consume_fuel(true);
        let engine = wasmi::Engine::new(&econfig);
        let module = bytes.and_then(|b| match wasmi::Module::new(&engine, b.as_slice()) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!("wasm plugin: module '{}' failed to compile: {e}", cfg.module);
                None
            }
        });
        if module.is_none() {
            tracing::warn!("wasm plugin: module '{}' unavailable; plugin is a no-op", cfg.module);
        }
        WasmPlugin {
            engine,
            module,
            module_name: cfg.module,
            config: cfg.config,
            fuel: cfg.fuel.max(1_000),
        }
    }

    /// Instantiate fresh (isolated) and call `export` with the JSON input.
    fn call(&self, export: &str, input: &Value) -> Option<Value> {
        let module = self.module.as_ref()?;
        // A missing export just means the module doesn't handle this phase.
        module.exports().find(|e| e.name() == export)?;

        let run = || -> Result<Option<Value>, String> {
            let mut store = wasmi::Store::new(&self.engine, ());
            store.set_fuel(self.fuel).map_err(|e| e.to_string())?;
            let linker = wasmi::Linker::<()>::new(&self.engine);
            let instance = linker
                .instantiate(&mut store, module)
                .map_err(|e| e.to_string())?
                .start(&mut store)
                .map_err(|e| e.to_string())?;
            let memory = instance
                .get_memory(&store, "memory")
                .ok_or("no exported memory")?;
            let alloc = instance
                .get_typed_func::<i32, i32>(&store, "raahi_alloc")
                .map_err(|e| e.to_string())?;
            let func = instance
                .get_typed_func::<(i32, i32), i64>(&store, export)
                .map_err(|e| e.to_string())?;

            let bytes = serde_json::to_vec(input).map_err(|e| e.to_string())?;
            let ptr = alloc
                .call(&mut store, bytes.len() as i32)
                .map_err(|e| e.to_string())?;
            memory
                .write(&mut store, ptr as u32 as usize, &bytes)
                .map_err(|e| e.to_string())?;
            let packed = func
                .call(&mut store, (ptr, bytes.len() as i32))
                .map_err(|e| e.to_string())?;
            if packed == 0 {
                return Ok(None);
            }
            let out_ptr = (packed >> 32) as u32 as usize;
            let out_len = (packed & 0xffff_ffff) as u32 as usize;
            if out_len == 0 || out_len > 1_048_576 {
                return Ok(None);
            }
            let mut buf = vec![0u8; out_len];
            memory
                .read(&store, out_ptr, &mut buf)
                .map_err(|e| e.to_string())?;
            serde_json::from_slice(&buf)
                .map(Some)
                .map_err(|e| format!("malformed output JSON: {e}"))
        };

        match run() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("wasm plugin '{}' {export} failed: {e}", self.module_name);
                None
            }
        }
    }

    pub fn on_request(&self, input: &ReqInput, effects: &mut Effects) -> Action {
        let headers: serde_json::Map<String, Value> = input
            .headers
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), json!(v))))
            .collect();
        let payload = json!({
            "method": input.method,
            "path": input.path,
            "query": input.query,
            "host": input.host,
            "client_ip": input.client_ip,
            "consumer": effects.consumer.as_ref().map(|(_, u)| u.clone()),
            "headers": headers,
            "config": self.config,
        });
        let Some(out) = self.call("on_request", &payload) else {
            return Action::Continue;
        };
        apply_header_effects(&out, effects);

        if out["action"].as_str() == Some("respond") {
            let status = out["status"].as_u64().unwrap_or(200) as u16;
            let mut resp_headers: Vec<(String, String)> = out["headers"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            if !resp_headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                resp_headers.push(("content-type".into(), "text/plain; charset=utf-8".into()));
            }
            let body = out["body"].as_str().unwrap_or("").as_bytes().to_vec();
            return Action::Respond(ShortResp {
                status: if status >= 100 { status } else { 200 },
                headers: resp_headers,
                body,
            });
        }
        Action::Continue
    }

    pub fn on_response(&self, input: &RespInput, effects: &mut Effects) {
        let headers: serde_json::Map<String, Value> = input
            .headers
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), json!(v))))
            .collect();
        let payload = json!({
            "status": input.status,
            "headers": headers,
            "config": self.config,
        });
        if let Some(out) = self.call("on_response", &payload) {
            apply_header_effects(&out, effects);
        }
    }
}

/// Merge a module's header-mutation output into [`Effects`].
fn apply_header_effects(out: &Value, effects: &mut Effects) {
    if let Some(o) = out["req_add"].as_object() {
        for (k, v) in o {
            if let Some(v) = v.as_str() {
                effects.req_add.push((k.clone(), v.to_string()));
            }
        }
    }
    if let Some(a) = out["req_remove"].as_array() {
        for v in a {
            if let Some(v) = v.as_str() {
                effects.req_remove.push(v.to_string());
            }
        }
    }
    if let Some(o) = out["resp_add"].as_object() {
        for (k, v) in o {
            if let Some(v) = v.as_str() {
                effects.resp_add.push((k.clone(), v.to_string()));
            }
        }
    }
    if let Some(a) = out["resp_remove"].as_array() {
        for v in a {
            if let Some(v) = v.as_str() {
                effects.resp_remove.push(v.to_string());
            }
        }
    }
}

/// Compile-check module bytes (used by the admin API before storing an upload).
pub fn validate_wasm(bytes: &[u8]) -> Result<(), String> {
    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    let engine = wasmi::Engine::new(&config);
    let module = wasmi::Module::new(&engine, bytes).map_err(|e| format!("invalid wasm: {e}"))?;
    let has = |n: &str| module.exports().any(|e| e.name() == n);
    if !has("memory") {
        return Err("module must export `memory`".into());
    }
    if !has("raahi_alloc") {
        return Err("module must export `raahi_alloc(size: i32) -> i32`".into());
    }
    if !has("on_request") && !has("on_response") {
        return Err("module must export `on_request` and/or `on_response`".into());
    }
    Ok(())
}

/// Convert WAT source text to wasm bytes (upload convenience).
pub fn wat_to_wasm(src: &str) -> Result<Vec<u8>, String> {
    wat::parse_str(src).map_err(|e| format!("invalid WAT: {e}"))
}
