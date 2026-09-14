# Raahi — justfile
# Run `just` to see all recipes, or `just --list` for grouped output.
# Requires: just, cargo/rustc ≥1.85, pnpm ≥9 (or npm), cmake + Go + Perl + C/C++ toolchain
# For `just dist` (static musl binary): cargo-zigbuild + zig  →  `uv tool install cargo-zigbuild`
# Docs: https://just.systems  |  Project: https://github.com/iamd3vil/raahi

set shell := ["bash", "-cu"]
set dotenv-load := false

# ── bindgen / libclang headers ──────────────────────────────────────────────
# boring-sys generates its bindings with libclang. On systems that have libclang
# but not its builtin headers (e.g. Debian/Ubuntu `libclang1-N` without
# `libclang-common-N-dev`), every cargo build dies with
# "fatal error: 'stddef.h' file not found". Detect that once and point libclang at
# the C compiler's include dir. An explicit BINDGEN_EXTRA_CLANG_ARGS in the
# environment always wins; when libclang's own headers exist we add nothing.
bindgen_detected := `if compgen -G '/usr/lib/llvm-*/lib/clang/*/include/stddef.h' >/dev/null || compgen -G '/usr/lib/clang/*/include/stddef.h' >/dev/null || compgen -G '/usr/lib64/clang/*/include/stddef.h' >/dev/null; then echo ""; else inc="$(cc -print-file-name=include 2>/dev/null || true)"; [[ -f "$inc/stddef.h" ]] && echo "-I$inc" || echo ""; fi`
export BINDGEN_EXTRA_CLANG_ARGS := env("BINDGEN_EXTRA_CLANG_ARGS", bindgen_detected)

# ── variables ────────────────────────────────────────────────────────────────
ui_dir      := "ui"
ui_build    := ui_dir + "/build"
bin_debug   := "target/debug/raahi"
bin_release := "target/release/raahi"
version     := `grep -m1 '^version' Cargo.toml | cut -d'"' -f2`
# Static distribution target (BoringSSL is cross-compiled with zig via cargo-zigbuild)
musl_target := "x86_64-unknown-linux-musl"
bin_musl    := "target/" + musl_target + "/release/raahi"
dist_dir    := "dist"
# pass extra flags via `just build-release -- --features foo`
cargo_extra := ""

# ── default ────────────────────────────────────────────────────────────────
[private]
default:
    @just --list --unsorted

# ── setup / doctor ─────────────────────────────────────────────────────────

# Install all dependencies (UI deps + fetch Rust crates)
[group('setup')]
setup: ui-install
    @echo "→ fetching Rust crates…"
    cargo fetch
    @echo "✓ setup complete. Run 'just build' or 'just dev'."

# Check that required tools are present and print versions
[group('setup')]
doctor:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "== toolchain =="
    printf "%-12s " "rustc:"; rustc --version 2>&1 || echo "MISSING (need ≥1.85)"
    printf "%-12s " "cargo:"; cargo --version 2>&1 || echo "MISSING"
    printf "%-12s " "just:";  just --version 2>&1  || echo "MISSING"
    printf "%-12s " "node:";  node --version 2>&1  || echo "MISSING (need ≥20)"
    printf "%-12s " "pnpm:";  pnpm --version 2>&1  || echo "MISSING — will fall back to npm"
    printf "%-12s " "cmake:"; cmake --version 2>&1 | head -1 || echo "MISSING (needed for boringssl)"
    printf "%-12s " "go:";    go version 2>&1     || echo "MISSING (needed for boringssl)"
    printf "%-12s " "perl:";  perl --version 2>&1 | sed -n 2p || echo "MISSING (needed for boringssl asm)"
    printf "%-12s " "zig:";   zig version 2>&1    || echo "MISSING (needed for 'just dist')"
    printf "%-12s " "zigbuild:"; cargo-zigbuild --version 2>&1 || echo "MISSING (needed for 'just dist': uv tool install cargo-zigbuild)"
    printf "%-12s %s\n" "bindgen:" "${BINDGEN_EXTRA_CLANG_ARGS:-(libclang headers found; no extra args needed)}"
    printf "%-12s " "cc:";    cc --version 2>&1 | head -1 || echo "MISSING"
    echo ""
    echo "== project =="
    test -f Cargo.toml && echo "✓ Cargo.toml" || echo "✗ Cargo.toml missing"
    test -f ui/package.json && echo "✓ ui/package.json" || echo "✗ ui/package.json missing"
    test -d ui/node_modules && echo "✓ ui/node_modules" || echo "○ ui/node_modules — run 'just ui-install'"
    test -d ui/build && echo "✓ ui/build" || echo "○ ui/build — run 'just ui-build'"

# ── UI ─────────────────────────────────────────────────────────────────────

# Install UI dependencies (pnpm install, falls back to npm)
[group('ui')]
ui-install:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ui_dir}}
    if command -v pnpm &>/dev/null; then
        pnpm install
    else
        echo "pnpm not found, falling back to npm install…" >&2
        npm install
    fi

# Build the Svelte 5 SPA → ui/build (served by the admin API in production)
[group('ui')]
ui-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ui_dir}}
    if command -v pnpm &>/dev/null; then
        pnpm run build
    else
        npm run build
    fi
    echo "✓ UI built → {{ui_build}}/"

# Run Vite dev server with HMR (proxies /api + /healthz → :9080)
[group('ui')]
ui-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ui_dir}}
    if command -v pnpm &>/dev/null; then exec pnpm run dev; else exec npm run dev; fi

# Type-check the UI (svelte-check)
[group('ui')]
ui-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ui_dir}}
    if command -v pnpm &>/dev/null; then pnpm run check; else npm run check; fi

# Preview the production UI build locally
[group('ui')]
ui-preview:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ui_dir}}
    if command -v pnpm &>/dev/null; then exec pnpm run preview; else exec npm run preview; fi

# Remove UI build artifacts and node_modules
[group('ui')]
ui-clean:
    rm -rf {{ui_build}} {{ui_dir}}/node_modules

# ── Rust / backend ─────────────────────────────────────────────────────────

# Check the workspace without building (fast)
[group('rust')]
check *args="":
    cargo check --workspace {{args}}

# Build debug binary (also builds the UI first so the binary can serve it)
[group('rust')]
build *args="": ui-build
    cargo build {{cargo_extra}} {{args}}
    @echo "✓ debug binary → {{bin_debug}} ({{bin_debug}} --help)"

# Build release binary (LTO + opt-level 3, see Cargo.toml [profile.release])
[group('rust')]
build-release *args="": ui-build
    cargo build --release {{cargo_extra}} {{args}}
    @ls -lh {{bin_release}}
    @echo "✓ release binary → {{bin_release}}"

# Release build without rebuilding the UI (when UI hasn't changed)
[group('rust')]
build-release-fast *args="":
    cargo build --release {{cargo_extra}} {{args}}
    @ls -lh {{bin_release}}

# Debug build without rebuilding the UI
[group('rust')]
build-fast *args="":
    cargo build {{cargo_extra}} {{args}}

# Format all Rust code
[group('rust')]
fmt:
    cargo fmt --all

# Check formatting without modifying files (CI)
[group('rust')]
fmt-check:
    cargo fmt --all -- --check

# Lint with clippy (all targets, pedantic warnings)
[group('rust')]
clippy *args="":
    cargo clippy --workspace --all-targets -- {{args}}

# Strict clippy — deny warnings (for CI)
[group('rust')]
clippy-strict:
    cargo clippy --workspace --all-targets -- -D warnings

# Run tests (all crates)
[group('rust')]
test *args="":
    cargo test --workspace {{args}}

# Run a single test by filter, e.g. `just test-one router`
[group('rust')]
test-one filter *args="":
    cargo test --workspace -- {{filter}} {{args}}

# ── run / dev ──────────────────────────────────────────────────────────────

# Run the proxy in debug mode (rebuilds UI + binary). Pass extra args after `--`, e.g. `just run -- --seed`
[group('dev')]
run *args="": ui-build
    cargo run -- {{args}}

# Run with --seed (create demo service + route if DB is empty)
[group('dev')]
run-seed *args="": ui-build
    cargo run -- --seed {{args}}

# Run the release binary (builds release first)
[group('dev')]
run-release *args="": build-release
    ./{{bin_release}} {{args}}

# Quick dev run without rebuilding the UI (faster iteration on Rust only)
[group('dev')]
run-fast *args="":
    cargo run -- {{args}}

# Development workflow helper — prints the two-terminal dev setup
[group('dev')]
dev:
    @echo "Raahi dev workflow (two terminals):"
    @echo ""
    @echo "  Terminal 1 — backend (proxy :8080, admin :9080):"
    @echo "    just run -- --seed"
    @echo "    # or without UI rebuild: just run-fast -- --seed"
    @echo ""
    @echo "  Terminal 2 — UI dev server with HMR (http://localhost:5173):"
    @echo "    just ui-dev"
    @echo ""
    @echo "Vite proxies /api and /healthz to the admin API on :9080."

# ── release ────────────────────────────────────────────────────────────────

# Full glibc release (dynamically linked): UI build + release binary, then verify. Portable static build: `just dist`
[group('release')]
release: ui-build build-release
    @just release-verify

# Verify the release binary exists and print size + linked libs
[group('release')]
release-verify:
    #!/usr/bin/env bash
    set -euo pipefail
    test -f {{bin_release}} || { echo "✗ {{bin_release}} not found — run 'just build-release'"; exit 1; }
    echo "binary : {{bin_release}}"
    ls -lh {{bin_release}}
    file {{bin_release}} | sed 's/^/file   : /'
    if command -v ldd &>/dev/null; then ldd {{bin_release}} | sed 's/^/ldd    : /' | head -20; fi
    {{bin_release}} --help | head -20 | sed 's/^/help   : /'
    echo "✓ release verified"

# cargo-zigbuild lets zig cross-compile BoringSSL (C++) for musl; musl-tools
# alone ships no C++ compiler.

# Build a fully static musl release binary (LTO) → target/x86_64-unknown-linux-musl/release/raahi
[group('release')]
build-musl *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    command -v cargo-zigbuild >/dev/null || { echo "✗ cargo-zigbuild not found — install with: uv tool install cargo-zigbuild"; exit 1; }
    command -v zig >/dev/null || { echo "✗ zig not on PATH — the uv tool env ships it as 'python -m ziglang'; add a shim or install zig"; exit 1; }
    rustup target add {{musl_target}} >/dev/null 2>&1 || true
    # bindgen parses BoringSSL's headers for the musl target; prefer musl's libc
    # headers (musl-tools) over glibc's when they are installed. The compiler-header
    # fix is inherited from the justfile-level BINDGEN_EXTRA_CLANG_ARGS.
    if [[ -d /usr/include/x86_64-linux-musl ]]; then
        export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/include/x86_64-linux-musl ${BINDGEN_EXTRA_CLANG_ARGS:-}"
    fi
    # Noise from the zig toolchain, not from Raahi or BoringSSL:
    #  - zig's bundled libc++ headers are not marked as system headers, so clang emits
    #    hundreds of -Wnullability-completeness diagnostics while compiling BoringSSL's C++
    #    (cc-rs/cmake-rs append the target-specific CXXFLAGS_<target> to the CMake flags);
    #  - zig's lld warns about rustc's deprecated `-O1` linker flag and about -L dirs that
    #    boring-sys advertises but the CMake build never creates (GNU ld ignores both).
    cxx_var="CXXFLAGS_$(echo {{musl_target}} | tr - _)"
    export "${cxx_var}=${!cxx_var:-} -Wno-nullability-completeness"
    export RUSTFLAGS="${RUSTFLAGS:-} -Alinker_messages"
    cargo zigbuild --release --target {{musl_target}} {{cargo_extra}} {{args}}
    ls -lh {{bin_musl}}
    file {{bin_musl}} | sed 's/^/file : /'

# Build a distributable: static musl binary (stripped) + UI + README as dist/raahi-v<ver>-<target>.tar.gz
[group('release')]
dist: ui-build build-musl
    #!/usr/bin/env bash
    set -euo pipefail
    name="raahi-v{{version}}-{{musl_target}}"
    out="{{dist_dir}}/$name"
    rm -rf "$out" && mkdir -p "$out/ui"
    cp {{bin_musl}} "$out/raahi"
    strip "$out/raahi"
    cp -r {{ui_build}} "$out/ui/build"        # matches the binary's default --ui-dir (ui/build)
    cp README.md "$out/"
    tar czf "$out.tar.gz" -C {{dist_dir}} "$name"
    echo "binary : $(file -b "$out/raahi")"
    ls -lh "$out/raahi" "$out.tar.gz"
    echo "✓ dist → $out.tar.gz  (run: tar xzf … && cd $name && ./raahi)"

# Strip debug symbols from the release binary (further size reduction)
[group('release')]
strip:
    strip {{bin_release}} 2>/dev/null && ls -lh {{bin_release}} && echo "✓ stripped" || echo "strip not available or binary missing"

# Install the release binary to ~/.cargo/bin
[group('release')]
install:
    cargo install --path . --locked

# ── maintenance ────────────────────────────────────────────────────────────

# Remove all build artifacts (Rust + UI)
[group('maint')]
clean:
    cargo clean
    rm -rf {{ui_build}}
    @echo "✓ cleaned (cargo + ui/build). Use 'just clean-all' to also remove node_modules."

# Remove everything including node_modules and SQLite DB
[group('maint')]
clean-all: clean ui-clean
    rm -f raahi.db raahi.db-shm raahi.db-wal
    rm -rf .raahi/ {{dist_dir}}/
    @echo "✓ fully cleaned"

# Update dependencies (Rust + UI)
[group('maint')]
update:
    cargo update
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ui_dir}}
    if command -v pnpm &>/dev/null; then pnpm update; else npm update; fi

# Run all CI checks locally: fmt, clippy, tests, UI check, release build
[group('maint')]
ci: fmt-check clippy-strict test ui-check
    @echo "✓ all CI checks passed — building release to confirm…"
    @just build-release-fast
    @echo "✓ ci complete"

# Show cargo dependency tree (or `just tree raahi-proxy`)
[group('maint')]
tree *args="":
    cargo tree {{args}}

# Watch and re-run tests on change (requires cargo-watch)
[group('maint')]
watch *args="":
    cargo watch -x 'test --workspace {{args}}'
