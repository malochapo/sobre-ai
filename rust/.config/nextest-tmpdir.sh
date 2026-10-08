#!/usr/bin/env bash
# nextest setup script: make tests hermetic against the host's sobre dirs.
# Many tests call Config::load(), which would otherwise read the real user
# config and pick up its settings.
#
# Rather than redirecting TMPDIR (which would move fixtures into the repo and
# break core::protocol::detect_project_root's .git upward-walk), point every
# SOBRE_<CATEGORY>_DIR at an empty dir: Config::load() finds no config.toml
# -> defaults, while fixtures stay in /tmp (outside the repo). The dirs live under target/ so `cargo clean`
# removes them, and are wiped each run so no stray config.toml leaks.
set -euo pipefail

base="${CARGO_TARGET_DIR:-$PWD/target}/sobre-test-dirs"
rm -rf "$base"
for cat in config data state cache; do
  mkdir -p "$base/$cat"
  echo "SOBRE_${cat^^}_DIR=$base/$cat" >> "$NEXTEST_ENV"
done
