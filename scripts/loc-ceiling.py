#!/usr/bin/env python3
"""INVARIANTS.md §5: production Rust under rust/src stays <= 200,000 lines.

Production means everything except test code: items under `#[cfg(test)]`
(brace-matched), and module files that only exist for tests (`tests.rs`,
`tests_*.rs`, `*_tests.rs`, anything under a `tests/` directory). Vendored
code lives outside rust/src and is not counted.
"""
import pathlib
import re
import sys

CEILING = 200_000
ROOT = pathlib.Path(__file__).resolve().parent.parent / "rust" / "src"
TEST_FILE = re.compile(r"(^tests(_\w+)?\.rs$)|(_tests\.rs$)")


def is_test_file(path: pathlib.Path) -> bool:
    return bool(TEST_FILE.search(path.name)) or "tests" in path.relative_to(ROOT).parts[:-1]


def strip_cfg_test(text: str) -> int:
    """Lines left once every `#[cfg(test)]` item is removed."""
    lines = text.splitlines()
    kept = 0
    i = 0
    while i < len(lines):
        if lines[i].strip() == "#[cfg(test)]":
            # Skip attributes, then the item: a `;`-terminated declaration or a
            # brace-balanced block.
            j = i + 1
            while j < len(lines) and lines[j].strip().startswith("#["):
                j += 1
            depth = 0
            opened = False
            while j < len(lines):
                code = re.sub(r'"(\\.|[^"\\])*"|//.*$', "", lines[j])
                depth += code.count("{") - code.count("}")
                opened = opened or "{" in code
                if (opened and depth <= 0) or (not opened and code.rstrip().endswith(";")):
                    break
                j += 1
            i = j + 1
            continue
        kept += 1
        i += 1
    return kept


def main() -> int:
    total = 0
    for path in sorted(ROOT.rglob("*.rs")):
        if is_test_file(path):
            continue
        total += strip_cfg_test(path.read_text(encoding="utf-8", errors="replace"))
    if total > CEILING:
        print(f"FAIL: {total:,} lines of production Rust in rust/src (ceiling {CEILING:,})")
        return 1
    print(f"Line ceiling OK: {total:,} lines of production Rust (ceiling {CEILING:,})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
