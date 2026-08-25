# Raahi — justfile
# Run `just` to see all recipes, or `just --list` for grouped output.
# Requires: just, cargo/rustc ≥1.84, pnpm ≥9 (or npm), cmake + Go + C/C++ toolchain
# Docs: https://just.systems  |  Project: https://github.com/…/raahi

set shell := ["bash", "-cu"]
set dotenv-load := false

# ── variables ────────────────────────────────────────────────────────────────
ui_dir      := "ui"
ui_build    := ui_dir + "/build"
bin_debug   := "target/debug/raahi"
bin_release := "target/release/raahi"
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
    printf "%-12s " "rustc:"; rustc --version 2>&1 || echo "MISSING (need ≥1.84)"
    printf "%-12s " "cargo:"; cargo --version 2>&1 || echo "MISSING"
    printf "%-12s " "just:";  just --version 2>&1  || echo "MISSING"
    printf "%-12s " "node:";  node --version 2>&1  || echo "MISSING (need ≥20)"
    printf "%-12s " "pnpm:";  pnpm --version 2>&1  || echo "MISSING — will fall back to npm"
    printf "%-12s " "cmake:"; cmake --version 2>&1 | head -1 || echo "MISSING (needed for boringssl)"
    printf "%-12s " "go:";    go version 2>&1     || echo "MISSING (needed for boringssl)"
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

# Full release: clean UI build + release binary, then verify
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

# Create a tarball of the release binary + UI (for distribution)
[group('release')]
package version="0.1.0": build-release
    #!/usr/bin/env bash
    set -euo pipefail
    out="raahi-v{{version}}-$(rustc -vV | sed -n 's|host: ||p') .tar.gz"
    # sanitise filename (e.g. x86_64-unknown-linux-gnu)
    out=$(echo "$out" | tr -d ' ')
    echo "→ packaging $out …"
    tar czf "$out" {{bin_release}} {{ui_build}} README.md --transform 's|target/release/raahi|raahi|' 2>/dev/null || \
    tar czf "$out" {{bin_release}} {{ui_build}} README.md
    ls -lh "$out"
    echo "✓ package → $out"

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
    rm -rf .raahi/
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
