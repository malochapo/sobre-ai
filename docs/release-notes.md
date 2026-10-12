Install with `curl -fsSL https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.sh | sh` on Linux and macOS, or `irm https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.ps1 | iex` in Windows PowerShell; `--dense` (`-Dense`) picks the build with local embeddings and `--version <tag>` (`-Version <tag>`) a given release. Then run `sobre init`.

Each platform has up to two builds:

- `sobre-<target>`: everything except local embeddings. `sobre_search` ranks with BM25.
- `sobre-dense-<target>`: adds local embeddings (dense and hybrid search, `embedding_model`). It loads ONNX Runtime at run time: run `sobre embeddings provision` once to download Microsoft's CPU runtime and the model, or set `ORT_DYLIB_PATH`. The Linux builds need glibc 2.28 or later. Intel macOS has no dense build: Microsoft ships no ONNX Runtime 1.24 for it.

## Changes since v0.1.0-rc.3

- **`sobre_session` and `sobre_knowledge` are gone: 13 MCP tools.** Agents never called them on their own, and when asked to save findings for the next session they used Claude Code's own memory, which loads by itself. `sobre_compose` no longer adds a session briefing, and `sobre_read` has no `task` mode.
- **The hook folds only log lines repeated but for their timestamp**, six or more in a row or eight anywhere in an output, and never a warning or an error. rc.3 folded the lines of one log statement whatever their numbers and could fold a run of warnings: it hid lines the agent had asked for.
- **`sobre gain` prices no saving.** It shows the tokens sobre removed and, with the proxy, the measured cost of the calls, but no longer a dollar "gain" or a "cost without sobre": both were estimates against an output the agent may never have read in full. `--model` is gone with them, and the ledger contract is now `usage-ledger-v2.md` (same record format).
- **Messages say what the MCP server costs where it applies.** `init`, `gain` and `doctor overhead` give the fixed cost of the MCP server as what it adds when connected: about 400 tokens per request in Claude Code, which loads tool definitions on demand, and about 3,800 in agents that load them all.

## Changes in v0.1.0-rc.3 (since rc.2)

- **Semantic search uses the local embeddings by default.** In the build with embeddings, `sobre_search` with `action="semantic"` and no `mode` now ranks by hybrid (BM25 and embeddings) once the model is installed, and by BM25 otherwise. rc.2 ranked by BM25 unless the agent asked for `mode`, so installing the model changed nothing.
- **The dense index stays current for a whole session.** On large projects, after the first dense or hybrid search, the next one reported "dense index not built" and fell back to BM25. A memory trim of the cached BM25 index made the embeddings look stale. The trim is gone.
- **A failed model download says why.** `sobre embeddings provision` named the model as lacking files when a download failed; it now says which file could not be downloaded and prints the error, a network error for example.

[sobre-bench](https://github.com/malochapo/sobre-bench) measures what sobre costs on real agent runs. So far, with Claude Code on Sonnet 5.5 and Haiku 5.5, it costs what Claude Code alone costs, with no measurable saving.
