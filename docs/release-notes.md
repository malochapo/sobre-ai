Install with `curl -fsSL https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.sh | sh` on Linux and macOS, or `irm https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.ps1 | iex` in Windows PowerShell; `--dense` (`-Dense`) picks the build with local embeddings and `--version <tag>` (`-Version <tag>`) a given release. Then run `sobre init`.

Each platform has up to two builds:

- `sobre-<target>`: everything except local embeddings. `sobre_search` ranks with BM25.
- `sobre-dense-<target>`: adds local embeddings (dense and hybrid search, `embedding_model`). It loads ONNX Runtime at run time: run `sobre embeddings provision` once to download Microsoft's CPU runtime and the model, or set `ORT_DYLIB_PATH`. The Linux builds need glibc 2.28 or later. Intel macOS has no dense build: Microsoft ships no ONNX Runtime 1.24 for it.

## Changes since v0.1.0-rc.1

- **`sobre init` connects the hooks only.** The MCP server and the `sobre` skill are now opt-in: `init` asks in a terminal, and `sobre init --mcp` connects them in scripts. An agent that already has sobre's MCP entry keeps it. On real agent runs the MCP tools were never called, while they and the skill added about 400 tokens to every request. Zed has no hooks, so it needs `--mcp`. `sobre doctor` no longer reports a missing MCP entry as a problem.
- **`compression_level` drives the post-run hook.** Below 0.2 it compresses nothing. From 0.2, four or more consecutive lines of one log statement keep their first and last line, with a count of the rest. From 0.4, consecutive `grep -n` results of one file print its path once. From 0.8, output over 150 lines keeps its first 60 and last 30 lines, unless the command may print source, a diff, blame, search results or a listing. This works on lines that chain several commands too. A compressed output names the file that holds it in full.
- **The hook compresses only what it should.** It runs a command's own compressor only when sobre has one for that command (git, test runners, builds), and never compresses output a command read from a file.
- **A smaller prefix.** The MCP server's instructions, the skill's description and four tool descriptions are shorter: connected, they add 396 tokens to a session's first request instead of 922.
- `sobre --version`, `doctor` and the MCP server report the release's own version.

What sobre costs and saves on real agent runs is measured in [sobre-bench](https://github.com/malochapo/sobre-bench).

