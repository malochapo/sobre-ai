#!/usr/bin/env bash
# LOC gate: no Rust source file may exceed LIMIT lines. Split a file that
# grows past it into submodules.
set -euo pipefail

LIMIT=1500

cd "$(dirname "$0")/.."

fail=0
while IFS= read -r file; do
  lines=$(wc -l <"$file" | tr -d ' ')
  if ((lines > LIMIT)); then
    echo "FAIL: $file has $lines lines (> $LIMIT — split it into submodules)"
    fail=1
  fi
done < <(find rust/src -name '*.rs' -type f)

if ((fail == 0)); then
  echo "LOC gate OK: all Rust files <= $LIMIT lines"
fi
exit "$fail"
