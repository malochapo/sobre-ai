#!/usr/bin/env bash
# nextest setup script: make tests hermetic against the host's lean-ctx dirs.
# Tests like core::pathjail and core::workspace_config assert that paths
# outside the jail root are rejected, but jail_path() calls Config::load(),
# which reads the real user config — if that lists "/tmp" in allow_paths, the
# /tmp-based fixtures are wrongly accepted and the tests fail.
#
# Rather than redirecting TMPDIR (which would move fixtures into the repo and
# break core::protocol::detect_project_root's .git upward-walk), point every
# LEAN_CTX_<CATEGORY>_DIR at an empty dir: Config::load() finds no config.toml
# -> defaults -> allow_paths = [] -> jail assertions hold, while fixtures stay
# in /tmp (outside the repo). The dirs live under target/ so `cargo clean`
# removes them, and are wiped each run so no stray config.toml leaks.
set -euo pipefail

base="${CARGO_TARGET_DIR:-$PWD/target}/lean-ctx-test-dirs"
rm -rf "$base"
for cat in config data state cache; do
  mkdir -p "$base/$cat"
  echo "LEAN_CTX_${cat^^}_DIR=$base/$cat" >> "$NEXTEST_ENV"
done
