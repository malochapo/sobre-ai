---
name: sobre
description: How to work through sobre's sobre_* MCP tools — orienting in a codebase, reading files at the right granularity, searching, editing by anchors, running shell commands with compressed output, getting exact content back, and keeping memory across sessions. Use when the sobre_* tools are available and you are exploring, reading, searching, editing or running commands in a repository.
---

# sobre

sobre is a local MCP server and CLI that answers file, search and
shell requests with the smallest output that still does the job. Everything
runs on this machine. Native tools keep working; the sobre_* tools are cheaper
when they fit.

## Orient before reading

- `sobre_compose` with no `task` returns the project overview: structure, markers,
  the last task, recent decisions and facts from earlier sessions. Call it at
  the start of a session.
- `sobre_compose(task="how does X work")` returns ranked files with the relevant
  symbol source inline. It replaces a search → read → symbol chain; do not
  re-read files whose source it already returned.
- `sobre_tree(path, depth)` for directory shape, `sobre_outline(path)` for the
  symbols of a file or folder before reading it.

## Read at the right granularity

`sobre_read(path, mode=...)` — `auto` when unsure.

| mode | gives |
|---|---|
| `signatures` | the API: declarations only |
| `map` | structure with line ranges |
| `full` | the whole file, ready for an edit (secret-like values show as `[REDACTED:…]`) |
| `anchored` | full text with `N:hash` anchors for `sobre_patch` |
| `lines:N-M` | a window (`lines:5,10-20` for several) |
| `diff` | what changed since this session last read the file |
| `task` | the parts relevant to a task |
| `raw=true` | the bytes on disk: no cache, framing or redaction |

Re-reading an unchanged file returns a short stub (~13 tokens); pass
`fresh=true` to force the content again.

## Search

- `sobre_search(pattern=...)` — regex across the project, compact results.
- `sobre_search(action="symbol", name=...)` — a definition by name.
- `sobre_search(action="semantic", query=...)` — by meaning ("where are retries handled").
- `sobre_glob(pattern)` — files by name.
- `sobre_graph(action="callers"|"callees", symbol=...)` — call sites;
  `sobre_graph(action="trace", symbol=..., to=...)` — the call chain between two
  functions.
- `sobre_graph(action="impact", path=...)` — the files that depend on a file
  or a type; `sobre_graph(action="changes")` — the same for the uncommitted
  changes; `sobre_graph(action="neighbors"|"path", ...)` — file dependencies.

A question about meaning goes to search, not to whole-file reads: reading more
is not understanding more.

## Edit

1. `sobre_read(path, mode="anchored")`.
2. `sobre_patch` with the `line` + `hash` anchors — you never re-type the old
   text. `op="create"` for a new file, `replace_unique(old_text, new_text)` for
   an exact unique replacement without a read, `ops=[...]` for several edits,
   across files if needed.
3. `CONFLICT` means the file changed under you: read it again.

Native Read → Edit also works. `sobre_refactor` finds references and
definitions through the language server, and previews a rename (it writes
nothing; apply the edits with `sobre_patch`). `sobre_checkpoint` snapshots your changes in a
history separate from the user's git, and can restore them.

## Shell

- `sobre_shell(command)` compresses build, test and log output; errors carry
  `[exit:N]` and are never dropped.
- Long jobs: `run_in_background=true`, then poll the returned `job_id`.
- sobre runs every command as given; what may run is your host's decision.
- Multi-line scripts: write them to a file, then run the file.
- Where the host supports hooks, the output of your native shell commands may
  come back compressed, ending with a `[sobre: …]` line that names where the
  full output is kept.

## Getting exact content back

Compressed output is not exact evidence. Before quoting a line, counting, or
editing from it:

- an output footer with an id → `sobre_expand(id=...)`, optionally `search=` or a
  line range;
- a file → `sobre_read(mode="full")` or `raw=true`, or read the path with any tool;
- a command → `sobre_shell(raw=true)`, or on the CLI `sobre raw "<command>"`.

Never rebuild exact content from a compressed view in line-by-line chunks.

## Memory

- `sobre_session` — this session: `task`, `finding`, `decision` (value=text),
  `save` at the end, `load` at the start, `status`.
- `sobre_knowledge` — facts that outlive the session: `remember value=...`,
  `recall query=...`, `gotcha trigger=... resolution=...` for pitfalls.

## Output

Never echo tool output back, no narration comments, show only changed code.
