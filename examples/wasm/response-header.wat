(module
  (memory (export "memory") 1)
  (data (i32.const 0) "{\"resp_add\":{\"x-wasm-response\":\"hello-from-wasm\"}}")
  (func (export "raahi_alloc") (param i32) (result i32) (i32.const 8192))
  (func (export "on_response") (param i32 i32) (result i64) (i64.const 50)))
