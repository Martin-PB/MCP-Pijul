#!/usr/bin/env bash
# scripts/local-check.sh - full local verification of mcp-server-pijul on Linux.
#
# Mirrors the CI workflows (unit.yml + integration.yml) and adds a live stdio
# smoke test: it spawns the freshly built server, drives a real MCP session
# (initialize, tools/list, tools/call) against a throwaway Pijul repository,
# and checks the PIJUL_MCP_READONLY guard. No network access is needed beyond
# crates.io for the initial build.
#
# Usage (from the repository root):
#     ./scripts/local-check.sh
#
# Requirements: cargo. If `pijul` is on PATH the full suite runs; otherwise
# only the no-pijul checks run (same degradation as the CI unit job).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN="$ROOT/target/debug/mcp-server-pijul"
PIJUL_SPEC='~1.0.0-beta'
PASS=0
FAIL=0

step() { printf '\n== %s ==\n' "$1"; }
ok()   { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
ko()   { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }
skip() { printf 'SKIP: %s\n' "$1"; }

# --- Preflight -------------------------------------------------------------

step "Preflight"
if command -v cargo >/dev/null 2>&1; then
    ok "cargo $(cargo --version | cut -d' ' -f2)"
else
    echo "cargo not found. Install the Rust toolchain first: https://rustup.rs"
    exit 1
fi

HAVE_PIJUL=0
if command -v pijul >/dev/null 2>&1; then
    HAVE_PIJUL=1
    ok "pijul $(pijul --version 2>&1 | grep -o '[0-9][0-9a-zA-Z.-]*' | head -1)"
else
    echo "pijul not found on PATH."
    echo "For the full run, install it with: cargo install --version '$PIJUL_SPEC' pijul"
    echo "Continuing with the no-pijul subset only (same as the CI unit job)."
fi

# --- Build -----------------------------------------------------------------
# No Cargo.lock is committed in this repository, so --locked is not used
# (unlike the CI workflows, which assume one is present).

step "Build"
cargo build --quiet
[ -x "$BIN" ] && ok "cargo build (target/debug/mcp-server-pijul)" || ko "binary not found at $BIN"

# --- Unit tests (no pijul required) ----------------------------------------

step "Protocol tests (no pijul required)"
if cargo test --quiet --test protocol; then
    ok "cargo test --test protocol"
else
    ko "cargo test --test protocol"
fi

# --- Tests that need pijul ---------------------------------------------------

if [ "$HAVE_PIJUL" -eq 1 ]; then
    step "Annotations, readonly and merge_preview tests"
    if cargo test --quiet --test annotations --test readonly --test merge_preview; then
        ok "annotations + readonly + merge_preview"
    else
        ko "annotations + readonly + merge_preview"
    fi

    step "Repository integration tests (generated pijul repos)"
    if cargo test --quiet --test repositories; then
        ok "cargo test --test repositories"
    else
        ko "cargo test --test repositories"
    fi
else
    step "Annotations, readonly, merge_preview, repositories"
    skip "need pijul on PATH"
fi

# --- Smoke test: live MCP stdio session (needs pijul) ------------------------

if [ "$HAVE_PIJUL" -eq 1 ]; then
    step "Smoke test: live MCP stdio session"

    TMP="$(mktemp -d)"
    cleanup() { rm -rf "$TMP"; }
    trap cleanup EXIT

    REPO="$TMP/repo"
    mkdir -p "$REPO"

    # A throwaway repository with one recorded change on the main channel,
    # and one more on a feature channel, so pijul_merge_preview has
    # something real to compare.
    if ( cd "$REPO" &&
         pijul init >/dev/null &&
         printf 'hello\n' > a.txt &&
         pijul add . >/dev/null &&
         pijul record --all --message "base" >/dev/null &&
         pijul channel new feature >/dev/null &&
         pijul channel switch feature >/dev/null &&
         printf 'world\n' > b.txt &&
         pijul add . >/dev/null &&
         pijul record --all --message "feature" >/dev/null &&
         pijul channel switch main >/dev/null ); then
        ok "throwaway pijul repo prepared (2 channels, 2 changes)"
    else
        ko "throwaway pijul repo preparation"
    fi

    # Run 1: initialize + tools/list -> serverInfo present, 13 tools.
    printf '%s\n%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' > "$TMP/req1.jsonl"
    ( cd "$REPO" && "$BIN" < "$TMP/req1.jsonl" ) > "$TMP/resp1.jsonl" 2>/dev/null
    if grep -q '"serverInfo"' "$TMP/resp1.jsonl"; then
        ok "initialize handshake"
    else
        ko "initialize handshake"
    fi
    TOOLS=$(grep -o '"name":"pijul_' "$TMP/resp1.jsonl" | wc -l)
    if [ "$TOOLS" -eq 13 ]; then
        ok "tools/list exposes 13 tools"
    else
        ko "tools/list exposes $TOOLS tools (expected 13)"
    fi

    # Run 2: pijul_status on the throwaway repo (clean -> no error).
    printf '%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"pijul_status","arguments":{}}}' > "$TMP/req2.jsonl"
    ( cd "$REPO" && "$BIN" < "$TMP/req2.jsonl" ) > "$TMP/resp2.jsonl" 2>/dev/null
    if grep -q '"type":"text"' "$TMP/resp2.jsonl" && ! grep -q '"isError":true' "$TMP/resp2.jsonl"; then
        ok "tools/call pijul_status"
    else
        ko "tools/call pijul_status"
    fi

    # Run 3: pijul_merge_preview main <- feature, preview only.
    printf '%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"pijul_merge_preview","arguments":{"source":"feature"}}}' > "$TMP/req3.jsonl"
    ( cd "$REPO" && "$BIN" < "$TMP/req3.jsonl" ) > "$TMP/resp3.jsonl" 2>/dev/null
    if grep -q '"to merge": 1' "$TMP/resp3.jsonl" && ! grep -q '"isError":true' "$TMP/resp3.jsonl"; then
        ok "tools/call pijul_merge_preview (preview, 1 change to merge)"
    else
        ko "tools/call pijul_merge_preview"
        sed 's/^/    /' "$TMP/resp3.jsonl" || true
    fi

    # Run 4: PIJUL_MCP_READONLY guard rejects a mutating tool up front.
    printf '%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"pijul_channel_create","arguments":{"name":"nope"}}}' > "$TMP/req4.jsonl"
    ( cd "$REPO" && PIJUL_MCP_READONLY=1 "$BIN" < "$TMP/req4.jsonl" ) > "$TMP/resp4.jsonl" 2>/dev/null
    if grep -q 'read-only mode' "$TMP/resp4.jsonl" && grep -q '"isError":true' "$TMP/resp4.jsonl"; then
        ok "PIJUL_MCP_READONLY guard (write tool rejected)"
    else
        ko "PIJUL_MCP_READONLY guard"
    fi
else
    step "Smoke test: live MCP stdio session"
    skip "need pijul on PATH"
fi

# --- Summary ----------------------------------------------------------------

step "Summary"
printf 'PASS: %d  FAIL: %d  (skips are not failures)\n' "$PASS" "$FAIL"
if [ "$FAIL" -eq 0 ]; then
    echo "All checks passed."
else
    echo "Some checks failed - see above."
    exit 1
fi
