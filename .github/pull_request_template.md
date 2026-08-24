## Summary

What does this PR change and why?

## Test plan

- [ ] `cd rust && cargo check --all-targets`
- [ ] `cd rust && cargo test` (the whole suite, not `--lib`; compare the failure
      set against a baseline run on the commit you started from)
- [ ] `cd rust && cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cd rust && cargo fmt --check`

## Notes for reviewers

- Risk areas / edge cases:
- Behaviour that changed by default:
- Docs updated (links/files):
