# sobre — Rust engine

The project's rules are the six invariants in `../INVARIANTS.md`; how to
work in this codebase is in `../CONTRIBUTING.md`. In short: the compiler is
the only reliable oracle (`cargo check --all-targets`, and `--features dense`
too), names lie so verify what a module does before acting on it, and a tool
leaving `tools/list` does not mean its code is dead.

Until 0.1.0, sobre owes nothing to a previous install: the release candidates
promise no compatibility. Never write migration code, legacy-format readers,
fallbacks to old paths or old config keys, or any note about "existing
installs" (re-downloads, re-indexing, lost history). If a change would only
matter to a previous install, it doesn't matter.

Build: `cargo check` (local-only default) or `cargo check --features dense`
(local embeddings). Never `--no-default-features` for a warning audit —
`dead_code = "deny"` produces false positives when features are off.
