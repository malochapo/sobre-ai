# sobre — Rust engine

The project's rules are the five invariants in `../INVARIANTS.md`. The
hard-won working practices for editing this inherited codebase are in
`../docs/working-rules.md` (gitignored): the compiler is the only reliable
oracle (`cargo check --all-targets`, and `--features dense` too), names lie so
verify what a module does before acting on it, and a tool leaving `tools/list`
does not mean its code is dead.

Build: `cargo check` (local-only default) or `cargo check --features dense`
(local embeddings + GPU). Never `--no-default-features` for a warning audit —
`dead_code = "deny"` produces false positives when features are off.
