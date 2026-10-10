## Summary

What does this PR change and why?

Does it add an MCP tool or a config key? Then name the one it replaces
(`INVARIANTS.md` caps both).

## Test plan

- [ ] `scripts/preflight.sh full` (fmt, clippy on both builds, rustdoc,
      generated docs, file size, then the whole test suite and the library
      tests with embeddings). CI also runs cargo-deny: `cargo deny check` from
      `rust/` when dependencies change
- [ ] Platform-specific change (paths, shells, file locking): CI run on macOS
      and Windows (Actions → CI → Run workflow)

## Notes for reviewers

- Risk areas / edge cases:
- Behaviour that changed by default:
- Docs updated (links/files):
