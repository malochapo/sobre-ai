Install with `curl -fsSL https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.sh | sh` on Linux and macOS, or `irm https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.ps1 | iex` in Windows PowerShell; `--dense` (`-Dense`) picks the build with local embeddings and `--version <tag>` (`-Version <tag>`) a given release. Then run `sobre init`.

Each platform has up to two builds:

- `sobre-<target>`: everything except local embeddings. `sobre_search` ranks with BM25.
- `sobre-dense-<target>`: adds local embeddings (dense and hybrid search, `embedding_model`). It loads ONNX Runtime at run time: run `sobre embeddings provision` once to download Microsoft's CPU runtime and the model, or set `ORT_DYLIB_PATH`. The Linux builds need glibc 2.28 or later. Intel macOS has no dense build: Microsoft ships no ONNX Runtime 1.24 for it.
