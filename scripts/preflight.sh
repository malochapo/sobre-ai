#!/usr/bin/env bash
# The checks CI runs, runnable locally. CI's lint job calls this same script;
# `full` adds the tests CI runs on Linux. CI also runs cargo-deny
# (rust/deny.toml), which this script does not.
#
#   scripts/preflight.sh        lint only (fmt, clippy, doc, generated docs, file size)
#   scripts/preflight.sh full   lint + the whole test suite, in both builds
set -euo pipefail

cd "$(dirname "$0")/../rust"

step() { printf '\n▶ %s\n' "$*"; "$@"; }

step cargo fmt --check
step cargo clippy --all-targets --locked -- -D warnings
step cargo clippy --all-targets --locked --features dense -- -D warnings
RUSTDOCFLAGS="-D warnings" step cargo doc --no-deps --locked --features dense
step cargo run --quiet --locked --example gen_docs --features dev-tools -- --check
step ../scripts/loc-gate.sh

if [[ "${1:-}" == "full" ]]; then
  step cargo test --locked
  step cargo test --locked --features dense
fi

printf '\npreflight: all checks passed\n'
