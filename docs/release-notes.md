Each platform has up to two builds:

- `sobre-<target>`: everything except local embeddings. `sobre_search` ranks with BM25.
- `sobre-dense-<target>`: adds local embeddings (dense and hybrid search, `embedding_model`). It loads ONNX Runtime at run time: run `sobre embeddings provision` once to download Microsoft's CPU runtime and the model, or set `ORT_DYLIB_PATH`. The Linux builds need glibc 2.28 or later. Intel macOS has no dense build: Microsoft ships no ONNX Runtime for it.
