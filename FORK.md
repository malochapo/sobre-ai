# Statement of changes

`sobre` is a **reduced hard fork** of [lean-ctx](https://github.com/yvgude/lean-ctx)
by Yves Gugger, licensed under the Apache License 2.0.

This document is the notice of modification required by
**Apache-2.0 section 4(b)**. It applies to the work as a whole: `sobre` is a
substantially modified derivative, and every file in this repository should be
assumed to be modified relative to upstream unless the git history shows
otherwise.

## Provenance

- Fork point: upstream `main` at `4e65c53d4`, corresponding to lean-ctx v3.9.12.
- The **complete upstream git history is preserved** in this repository. Every
  commit authored by Yves Gugger and the upstream contributors remains intact
  and attributable. `git log` is the authoritative record of what came from
  where.
- Upstream is tracked as the `upstream` remote and is *not* merged
  automatically. `sobre` is an independent project, not a downstream
  distribution of lean-ctx.

## Relationship to upstream

`sobre` is not a competitor to lean-ctx and does not present itself as a newer,
official or endorsed version of it. lean-ctx aims to be a broad context
platform; `sobre` deliberately aims at a fraction of that scope. Users who need
the full feature surface should use lean-ctx.

Per **Apache-2.0 section 6**, this fork receives no trademark licence. The names
`lean-ctx`, `LeanCTX` and the associated marks, logos and domains belong to
their owner and are used here only descriptively, to identify the origin of this
work.

Contributors to `sobre` are free to contribute to lean-ctx in parallel, and are
encouraged to send genuine bug fixes upstream first.

## What was removed and why

The goal of the fork is **subtraction**, not addition. Categories removed:

| Removed | Reason |
|---|---|
| Mascot, achievements, sprite and ASCII-art subsystem (`core/buddy`) | Gamification; no bearing on the tool's job |
| Cloud server, billing, org gateway, shape translation | `sobre` is community open source with nothing to commercialise |
| Web fetching, remote providers, multi-agent coordination | Outside the scope of a local context layer |
| The downstream packages, language SDKs and editor extensions | A distribution surface two maintainers can support |
| Marketing, blog and email material | Not source code |
| Multiple tool-visibility modes, tool profiles, per-client quirks | Replaced by a single fixed tool surface |

The complete removal log, with measured line counts and coupling for each
module, is maintained by the maintainers alongside the repository.

## Licence

`sobre` remains under the Apache License 2.0. See [`LICENSE`](LICENSE) for the
licence text and [`NOTICE`](NOTICE) for the attribution notices, which reproduce
the upstream notices as required by section 4(d).
