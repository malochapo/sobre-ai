#!/usr/bin/env bash
# The checks CI runs, runnable locally. CI calls this same script, so a green
# run here means the same gates pass there.
#
#   scripts/preflight.sh        lint only (fmt, clippy, doc, generated docs, file size,
#                               line ceiling)
#   scripts/preflight.sh full   lint + the whole test suite
set -euo pipefail

cd "$(dirname "$0")/../rust"

step() { printf '\n▶ %s\n' "$*"; "$@"; }

step cargo fmt --check
step cargo clippy --all-targets --locked -- -D warnings
step cargo clippy --all-targets --locked --features dense -- -D warnings
RUSTDOCFLAGS="-D warnings" step cargo doc --no-deps --locked --features dense
step cargo run --quiet --locked --example gen_docs --features dev-tools -- --check
step ../scripts/loc-gate.sh
step python3 ../scripts/loc-ceiling.py

if [[ "${1:-}" == "full" ]]; then
  step cargo test --locked
fi

printf '\npreflight: all checks passed\n'
