#!/usr/bin/env bash
# Pre-release quality gate for lean-ctx.
# Run this BEFORE tagging a new version.
# Usage: cd rust && bash tests/pre_release_check.sh
set -euo pipefail

# Honour CARGO_TARGET_DIR / config.toml target-dir overrides (#671); fall back
# to ./target when `cargo metadata` is unavailable. `|| true` keeps the
# pipeline from tripping `set -euo pipefail` before the fallback applies.
TARGET_DIR=$(cargo metadata --no-deps --format-version=1 2>/dev/null \
    | grep -o '"target_directory":"[^"]*"' \
    | head -1 \
    | sed -E 's/^"target_directory":"(.*)"$/\1/' \
    | sed 's/\\\\/\//g' || true)
BIN="${TARGET_DIR:-./target}/release/lean-ctx"
PASS=0
FAIL=0

step() { printf "\n\033[1;34m=== %s ===\033[0m\n" "$1"; }
ok()   { printf "  \033[32m✓\033[0m %s\n" "$1"; PASS=$((PASS+1)); }
fail() { printf "  \033[31m✗\033[0m %s\n" "$1"; FAIL=$((FAIL+1)); }

# -----------------------------------------------------------------------
step "1/4  cargo test --release"
cargo test --release 2>&1 | tail -1
ok "unit + integration tests"

# -----------------------------------------------------------------------
step "2/4  cargo clippy"
cargo clippy --release -- -D warnings 2>&1 | tail -1
ok "clippy clean"

# -----------------------------------------------------------------------
step "3/4  cargo fmt --check"
cargo fmt --check 2>&1
ok "formatting clean"

# -----------------------------------------------------------------------
step "4/4  Release binary"
cargo build --release 2>&1 | tail -1
VERSION=$("$BIN" --version 2>&1)
ok "binary built: $VERSION"

# -----------------------------------------------------------------------
printf "\n\033[1;97m=== RESULT: %d passed, %d failed ===\033[0m\n" "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
    printf "\033[31mPre-release check FAILED. Do NOT tag this version.\033[0m\n"
    exit 1
fi

printf "\033[32mPre-release check PASSED. Safe to tag and release.\033[0m\n"
