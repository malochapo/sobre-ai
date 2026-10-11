Install with `curl -fsSL https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.sh | sh` on Linux and macOS, or `irm https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.ps1 | iex` in Windows PowerShell; `--dense` (`-Dense`) picks the build with local embeddings and `--version <tag>` (`-Version <tag>`) a given release. Then run `sobre init`.

Each platform has up to two builds:

- `sobre-<target>`: everything except local embeddings. `sobre_search` ranks with BM25.
- `sobre-dense-<target>`: adds local embeddings (dense and hybrid search, `embedding_model`). It loads ONNX Runtime at run time: run `sobre embeddings provision` once to download Microsoft's CPU runtime and the model, or set `ORT_DYLIB_PATH`. The Linux builds need glibc 2.28 or later. Intel macOS has no dense build: Microsoft ships no ONNX Runtime 1.24 for it.

## Changes since v0.1.0-rc.2

- **Semantic search uses the local embeddings by default.** In the build with embeddings, `sobre_search` with `action="semantic"` and no `mode` now ranks by hybrid (BM25 and embeddings) once the model is installed, and by BM25 otherwise. rc.2 ranked by BM25 unless the agent asked for `mode`, so installing the model changed nothing.
- **The dense index stays current for a whole session.** On large projects, after the first dense or hybrid search, the next one reported "dense index not built" and fell back to BM25. A memory trim of the cached BM25 index made the embeddings look stale. The trim is gone.
- **A failed model download says why.** `sobre embeddings provision` named the model as lacking files when a download failed; it now says which file could not be downloaded and prints the error, a network error for example.

What sobre costs and saves on real agent runs is measured in [sobre-bench](https://github.com/malochapo/sobre-bench).
