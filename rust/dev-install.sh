#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
BINARY="${INSTALL_DIR}/lean-ctx"

printf "=== lean-ctx dev-install ===\n"

# 1) Build release
printf "  Building release…\n"
cargo build --release 2>&1 | tail -3

# 2) Install
mkdir -p "$INSTALL_DIR"
# Ask cargo for the real target dir — honours CARGO_TARGET_DIR and a
# ~/.cargo/config.toml `[build] target-dir` override, not just the default
# "./target" (a hardcoded path silently installed a stale binary here).
# The `|| true` keeps `set -euo pipefail` from aborting before the fallback:
# a failing `cargo metadata` (or unmatched grep) exits the pipeline non-zero.
TARGET_DIR=$(cargo metadata --no-deps --format-version=1 2>/dev/null \
    | grep -o '"target_directory":"[^"]*"' \
    | head -1 \
    | sed -E 's/^"target_directory":"(.*)"$/\1/' \
    | sed 's/\\\\/\//g' || true)
TARGET_DIR="${TARGET_DIR:-$(pwd)/target}"
TARGET="${TARGET_DIR}/release/lean-ctx"
# Fail loudly instead of symlinking a missing/stale binary into PATH (#671).
if [ ! -x "$TARGET" ]; then
    printf "  error: built binary not found at %s\n" "$TARGET" >&2
    printf "  hint: is CARGO_TARGET_DIR or ~/.cargo/config.toml [build] target-dir pointing elsewhere?\n" >&2
    exit 1
fi
TMP_LINK="${BINARY}.tmp.$$"
ln -sf "$TARGET" "$TMP_LINK"
mv -f "$TMP_LINK" "$BINARY"

# 2b) Ad-hoc sign for macOS code-signature enforcement (Sequoia+)
if command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "$TARGET" 2>/dev/null && printf "  Signed (ad-hoc)\n" || true
fi

# 3) Verify
VERSION=$("$BINARY" --version 2>&1)
printf "  Installed: %s\n" "$VERSION"
printf "  Path: %s\n" "$BINARY"
printf "=== done ===\n"
