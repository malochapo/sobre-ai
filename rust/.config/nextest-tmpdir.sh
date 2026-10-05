#!/usr/bin/env bash
# nextest setup script: make tests hermetic against the host's lean-ctx dirs.
# Point each LEAN_CTX_<CATEGORY>_DIR at a fresh, empty directory so Config::load()
# finds no config.toml, defaults apply, and jail path assertions hold true.
# The dirs live under target/ so `cargo clean` removes them.
set -euo pipefail

base="${CARGO_TARGET_DIR:-$PWD/target}/lean-ctx-test-dirs"
rm -rf "$base"
for cat in config data state cache; do
  mkdir -p "$base/$cat"
  echo "LEAN_CTX_${cat^^}_DIR=$base/$cat" >> "$NEXTEST_ENV"
done
