//! Embedding engine for semantic code search.
//!
//! Provides dense vector embeddings for code chunks using a local ONNX model.
//! Any HuggingFace model with an ONNX export and a `tokenizer.json` works —
//! selected via the `embedding_model` config key (default: all-MiniLM-L6-v2).
//!
//! Feature-gated under `embeddings` — falls back gracefully to BM25-only
//! search when the feature or model is not available.
//!
//! Architecture:
//!   Tokenizer → ONNX Model (ort) → Pooling → L2 Normalize → `Vec<f32>`

pub mod download;
pub mod model_registry;
pub mod pooling;
pub mod tokenizer;

use std::path::{Path, PathBuf};

use model_registry::ModelMeta;
use tokenizer::TokenizedInput;

// `embed_batch` parallelises tokenization in every build (rayon is a core, non
// optional dependency), so this import must not be feature-gated — otherwise the
// no-embeddings build (e.g. FreeBSD) loses `par_iter`.
use rayon::prelude::*;
#[cfg(feature = "dense")]
use std::sync::Mutex;

pub struct EmbeddingEngine {
    dimensions: usize,
    name: String,
    meta: ModelMeta,
    #[cfg(feature = "dense")]
    tokenizer: tokenizer::Tokenizer,
    #[cfg(feature = "dense")]
    session: Mutex<ort::session::Session>,
    #[cfg(feature = "dense")]
    graph_inputs: GraphInputs,
    #[cfg(feature = "dense")]
    output_name: String,
    /// The output is already one vector per input (`[batch, dim]`), as in
    /// exports with a `sentence_embedding` output or model2vec graphs.
    #[cfg(feature = "dense")]
    output_pooled: bool,
}

/// The two ONNX graph topologies we can drive.
///
/// Transformers take `[1, seq]` id/mask tensors and emit per-token hidden
/// states `[1, seq, dim]` that we mean-pool. model2vec exports are
/// EmbeddingBag graphs: flat `input_ids: [n_tokens]` plus `offsets: [batch]`,
/// already pooled to `[batch, dim]` — ~500x faster, no attention pass.
#[cfg(feature = "dense")]
enum GraphInputs {
    Transformer {
        input_ids: String,
        attention_mask: String,
        token_type_ids: Option<String>,
    },
    EmbeddingBag {
        input_ids: String,
        offsets: String,
    },
}

/// Classify the graph topology from its input names (pure, unit-testable).
/// The model2vec signature is exactly two inputs whose second is `offsets`;
/// everything else is treated as a transformer.
#[cfg(feature = "dense")]
fn is_embedding_bag_signature(input_names: &[String]) -> bool {
    input_names.len() == 2 && input_names[1] == "offsets"
}

impl EmbeddingEngine {
    /// Load the selected embedding model from a directory. Fails when it is
    /// not installed: only `sobre embeddings provision` downloads it.
    #[cfg(feature = "dense")]
    pub fn load(model_dir: &Path) -> anyhow::Result<Self> {
        let selected = model_registry::resolve_model();
        Self::load_model(model_dir, &selected)
    }

    /// Load a specific embedding model from a directory.
    #[cfg(feature = "dense")]
    pub fn load_model(
        base_dir: &Path,
        model: &model_registry::EmbeddingModel,
    ) -> anyhow::Result<Self> {
        let name = model.name();
        let model_dir = base_dir.join(model.storage_dir_name());

        // Never download here: loading happens on tool paths the user did not
        // ask to touch the network. The model arrives through the explicit
        // `sobre embeddings provision`, which calls `ensure_downloaded`.
        anyhow::ensure!(
            model_files_present(&model_dir),
            "embedding model not installed — run `sobre embeddings provision`"
        );

        let meta = ModelMeta::load(&model_dir);
        let model_path = model_dir.join(download::MODEL_FILE);

        let eps = crate::core::ort_execution_providers::execution_providers();
        // Surface a loud, actionable warning when the user expected the GPU but
        // the CUDA runtime is unusable, so it doesn't silently crawl on CPU.
        if let Some(msg) = crate::core::ort_execution_providers::gpu_fallback_warning() {
            tracing::warn!("{msg}");
        }
        let num_cpus = std::thread::available_parallelism().map_or(4, |n| n.get().max(1));
        tracing::info!(
            model = %name,
            model_path = %model_path.display(),
            num_cpus,
            execution_providers = eps.len(),
            "Loading ONNX embedding model"
        );
        crate::core::ort_environment::ensure_ort_env(&eps)?;
        tracing::debug!("ONNX Runtime environment ready; creating embedding session builder");
        let mut session = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("ORT builder: {e}"))?
            .with_intra_threads(num_cpus)
            .map_err(|e| anyhow::anyhow!("ORT intra threads: {e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::All)
            .map_err(|e| anyhow::anyhow!("ORT optimization: {e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| anyhow::anyhow!("ORT load model: {e}"))?;
        tracing::debug!("ONNX embedding session loaded; inspecting graph signature");

        let input_names: Vec<String> = session
            .inputs()
            .iter()
            .map(|i| i.name().to_string())
            .collect();

        if input_names.len() < 2 {
            anyhow::bail!(
                "Expected model with at least 2 inputs (input_ids, attention_mask), got {}",
                input_names.len()
            );
        }

        // Topology detection: model2vec EmbeddingBag graphs expose
        // exactly (input_ids, offsets) — structurally incompatible with the
        // transformer path, so they get their own input adapter.
        let graph_inputs = if is_embedding_bag_signature(&input_names) {
            GraphInputs::EmbeddingBag {
                input_ids: input_names[0].clone(),
                offsets: input_names[1].clone(),
            }
        } else {
            GraphInputs::Transformer {
                input_ids: input_names[0].clone(),
                attention_mask: input_names[1].clone(),
                token_type_ids: input_names.get(2).cloned(),
            }
        };

        // Static-embedding models are trained without [CLS]/[SEP].
        let add_special_tokens = matches!(graph_inputs, GraphInputs::Transformer { .. });
        let tokenizer = tokenizer::Tokenizer::from_file(
            &model_dir.join(download::TOKENIZER_FILE),
            meta.max_seq_len,
            add_special_tokens,
        )
        .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;

        // Exports that end in the sentence-transformers pipeline expose the
        // pooled, normalized vector as `sentence_embedding`: prefer it.
        let output_names: Vec<String> = session
            .outputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        let output_name = output_names
            .iter()
            .find(|n| *n == "sentence_embedding")
            .or_else(|| output_names.first())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Model has no named outputs"))?;

        let shape = probe_output_shape(&mut session, &tokenizer, &graph_inputs, &output_name)
            .ok_or_else(|| anyhow::anyhow!("{name}: probe inference failed"))?;
        let (dimensions, output_pooled) = match shape.as_slice() {
            [_, dim] => (*dim, true),
            [_, _, dim] => (*dim, false),
            other => anyhow::bail!("{name}: unsupported output shape {other:?}"),
        };
        anyhow::ensure!(dimensions > 0, "{name}: zero-width output");
        tracing::debug!(dimensions, "ONNX embedding dimensions resolved");

        tracing::info!(
            "Embedding engine loaded: model={name}, {dimensions}d, max_seq_len={}, pooling={:?}, topology={}",
            meta.max_seq_len,
            if output_pooled {
                None
            } else {
                Some(meta.pooling)
            },
            match graph_inputs {
                GraphInputs::Transformer { .. } => "transformer",
                GraphInputs::EmbeddingBag { .. } => "embedding-bag (model2vec)",
            },
        );

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            dimensions,
            name,
            meta,
            graph_inputs,
            output_name,
            output_pooled,
        })
    }

    #[cfg(not(feature = "dense"))]
    pub fn load(_model_dir: &Path) -> anyhow::Result<Self> {
        anyhow::bail!(
            "this build has no local embeddings: use the sobre-dense release build, \
             or build with --features dense"
        )
    }

    /// Load from the default model directory ([`model_directory`](Self::model_directory)).
    pub fn load_default() -> anyhow::Result<Self> {
        Self::load(&Self::model_directory()?)
    }

    /// Generate an embedding vector for a single text (document/code).
    pub fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let input = self.encode(&with_prefix(self.meta.document_prefix.as_deref(), text))?;
        self.run_inference(&input)
    }

    /// Generate an embedding vector for a query string.
    /// Applies query-specific prefix if the model requires one.
    pub fn embed_query(&self, query: &str) -> anyhow::Result<Vec<f32>> {
        let input = self.encode(&with_prefix(self.meta.query_prefix.as_deref(), query))?;
        self.run_inference(&input)
    }

    /// Generate embedding vectors for multiple texts using true batched ONNX
    /// inference. Sends a single `[batch, max_seq_len]` tensor through the model
    /// instead of `batch` separate calls — up to 50× faster on CPU for typical
    /// batch sizes (64–128) by leveraging matrix-matrix instead of matrix-vector
    /// operations inside the transformer.
    pub fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Process in mini-batches to cap peak memory. Prefixing and tokenization
        // deliberately happen inside the loop: retaining those buffers for the
        // entire corpus would defeat the mini-batch cap under large indexes.
        let configured_batch_size: usize = {
            #[cfg(feature = "dense")]
            if crate::core::ort_execution_providers::gpu_active() {
                256
            } else {
                64
            }
            #[cfg(not(feature = "dense"))]
            64
        };
        // Approximate tokenizer tensors plus transformer activations per input.
        // Saturating arithmetic keeps hostile custom-model dimensions harmless.
        let estimated_bytes_per_text = self
            .meta
            .max_seq_len
            .saturating_mul(self.dimensions.max(1))
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_mul(8) as u64;
        let batch_size = crate::core::memory_guard::adaptive_batch_size(
            1,
            configured_batch_size,
            estimated_bytes_per_text.max(1),
        );

        let total = texts.len();
        let total_batches = total.div_ceil(batch_size);
        let mut results = Vec::with_capacity(total);
        let start = std::time::Instant::now();

        for (batch_idx, text_chunk) in texts.chunks(batch_size).enumerate() {
            // Cooperative cancellation checkpoint before any per-batch allocation
            // and between FFI calls. A hard pressure sample therefore prevents the
            // next prefix/tokenizer spike and cleanly drops prior batch buffers.
            if crate::core::interrupt::is_cancelled()
                || crate::core::memory_guard::abort_requested()
            {
                anyhow::bail!(
                    "embedding cancelled after {done}/{total_batches} batches ({embedded}/{total} chunks)",
                    done = batch_idx,
                    embedded = results.len(),
                );
            }

            let tokenized: Vec<TokenizedInput> = text_chunk
                .par_iter()
                .map(|text| self.encode(&with_prefix(self.meta.document_prefix.as_deref(), text)))
                .collect::<anyhow::Result<_>>()?;

            let batch_start = std::time::Instant::now();
            let batch_out = self.run_inference_batch(&tokenized)?;
            results.extend(batch_out);
            tracing::info!(
                batch = batch_idx + 1,
                total_batches,
                embedded = results.len(),
                total,
                batch_ms = batch_start.elapsed().as_millis(),
                elapsed_ms = start.elapsed().as_millis(),
                "embedding progress"
            );
        }
        Ok(results)
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub fn model_name(&self) -> &str {
        &self.name
    }

    #[cfg(feature = "dense")]
    fn encode(&self, text: &str) -> anyhow::Result<TokenizedInput> {
        self.tokenizer.encode(text)
    }

    #[cfg(not(feature = "dense"))]
    #[allow(clippy::unused_self)] // same signature as the `embeddings` build
    fn encode(&self, _text: &str) -> anyhow::Result<TokenizedInput> {
        anyhow::bail!("Embeddings feature not enabled")
    }

    /// The models directory, under the cache directory. An error when the
    /// cache directory cannot be resolved: a model is never looked for, or
    /// downloaded, relative to the working directory.
    pub fn model_directory() -> anyhow::Result<PathBuf> {
        crate::core::paths::cache_dir()
            .map(|d| d.join("models"))
            .map_err(|e| anyhow::anyhow!(e))
    }

    /// Check if the model files are present and loadable.
    pub fn is_available() -> bool {
        let selected = model_registry::resolve_model();
        Self::model_directory()
            .is_ok_and(|dir| model_files_present(&dir.join(selected.storage_dir_name())))
    }

    /// Ensure the selected model's files exist on disk, downloading them from
    /// HuggingFace if absent. Pure network/file IO — this never initializes the
    /// ONNX Runtime, so a failed bootstrap costs nothing in ORT teardown (the
    /// caller only loads ORT via [`shared_engine`] once the files are present).
    /// After `Ok`, [`is_available`](Self::is_available) is guaranteed true; on a
    /// warm cache it short-circuits without touching the network.
    ///
    /// `sobre embeddings provision` (and the model question in `init`) is the
    /// one caller: nothing else downloads the model.
    pub fn ensure_downloaded() -> anyhow::Result<()> {
        let selected = model_registry::resolve_model();
        let model_dir = Self::model_directory()?.join(selected.storage_dir_name());
        download::ensure_model(&model_dir, &selected)?;
        Ok(())
    }

    #[cfg(feature = "dense")]
    fn run_inference(&self, input: &TokenizedInput) -> anyhow::Result<Vec<f32>> {
        let seq_len = input.input_ids.len();

        let mut embedding = match &self.graph_inputs {
            GraphInputs::Transformer {
                input_ids,
                attention_mask,
                token_type_ids,
            } => {
                let ids_vec: Vec<i64> = input.input_ids.iter().map(|&x| x as i64).collect();
                let mask_vec: Vec<i64> = input.attention_mask.iter().map(|&x| x as i64).collect();
                let ids_array = ndarray::Array2::from_shape_vec((1, seq_len), ids_vec)?;
                let mask_array = ndarray::Array2::from_shape_vec((1, seq_len), mask_vec)?;
                let ids_tensor = ort::value::Tensor::from_array(ids_array)?;
                let mask_tensor = ort::value::Tensor::from_array(mask_array)?;

                let hidden = if let Some(type_id) = token_type_ids {
                    let type_vec: Vec<i64> =
                        input.token_type_ids.iter().map(|&x| x as i64).collect();
                    let type_array = ndarray::Array2::from_shape_vec((1, seq_len), type_vec)?;
                    let type_tensor = ort::value::Tensor::from_array(type_array)?;
                    let mut _guard = self
                        .session
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let outputs = _guard.run(ort::inputs![
                        input_ids.as_str() => ids_tensor,
                        attention_mask.as_str() => mask_tensor,
                        type_id.as_str() => type_tensor,
                    ])?;
                    let (_, data) =
                        outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
                    data.to_vec()
                } else {
                    let mut _guard = self
                        .session
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let outputs = _guard.run(ort::inputs![
                        input_ids.as_str() => ids_tensor,
                        attention_mask.as_str() => mask_tensor,
                    ])?;
                    let (_, data) =
                        outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
                    data.to_vec()
                };
                if self.output_pooled {
                    hidden
                } else {
                    pooling::pool(
                        &hidden,
                        &input.attention_mask,
                        seq_len,
                        self.dimensions,
                        self.meta.pooling,
                    )
                }
            }
            GraphInputs::EmbeddingBag { input_ids, offsets } => {
                if seq_len == 0 {
                    return Ok(vec![0.0; self.dimensions]);
                }
                let ids_vec: Vec<i64> = input.input_ids.iter().map(|&x| x as i64).collect();
                let ids_array = ndarray::Array1::from_shape_vec(seq_len, ids_vec)?;
                let offsets_array = ndarray::Array1::from_shape_vec(1, vec![0i64])?;
                let ids_tensor = ort::value::Tensor::from_array(ids_array)?;
                let offsets_tensor = ort::value::Tensor::from_array(offsets_array)?;
                let mut _guard = self
                    .session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let outputs = _guard.run(ort::inputs![
                    input_ids.as_str() => ids_tensor,
                    offsets.as_str() => offsets_tensor,
                ])?;
                let (_, data) = outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
                data.to_vec()
            }
        };

        pooling::normalize_l2(&mut embedding);
        Ok(embedding)
    }

    /// Run batched inference over multiple tokenized inputs.
    ///
    /// For the Transformer topology: pads all inputs to `max_seq_len` of the
    /// batch, creates a `[batch, max_seq_len]` tensor, runs ONNX once, then
    /// mean-pools and L2-normalizes each sequence individually.
    ///
    /// For the EmbeddingBag topology: concatenates tokens with per-row offsets,
    /// runs ONNX once, and L2-normalizes each output row.
    #[cfg(feature = "dense")]
    fn run_inference_batch(&self, inputs: &[TokenizedInput]) -> anyhow::Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        match &self.graph_inputs {
            GraphInputs::Transformer {
                input_ids: input_id,
                attention_mask: mask_id,
                token_type_ids,
            } => {
                let batch = inputs.len();
                let max_len = inputs.iter().map(|i| i.input_ids.len()).max().unwrap_or(0);

                let mut ids_data: Vec<i64> = Vec::with_capacity(batch * max_len);
                let mut mask_data: Vec<i64> = Vec::with_capacity(batch * max_len);
                let mut type_data: Vec<i64> = Vec::with_capacity(batch * max_len);
                let mut per_seq_masks: Vec<&[i32]> = Vec::with_capacity(batch);

                for inp in inputs {
                    let seq_len = inp.input_ids.len();
                    ids_data.extend(inp.input_ids.iter().map(|&x| x as i64));
                    ids_data.resize(ids_data.len() + (max_len - seq_len), 0);

                    mask_data.extend(inp.attention_mask.iter().map(|&x| x as i64));
                    mask_data.resize(mask_data.len() + (max_len - seq_len), 0);

                    type_data.extend(inp.token_type_ids.iter().map(|&x| x as i64));
                    type_data.resize(type_data.len() + (max_len - seq_len), 0);

                    per_seq_masks.push(inp.attention_mask.as_slice());
                }

                let ids_array = ndarray::Array2::from_shape_vec((batch, max_len), ids_data)?;
                let mask_array = ndarray::Array2::from_shape_vec((batch, max_len), mask_data)?;
                let ids_tensor = ort::value::Tensor::from_array(ids_array)?;
                let mask_tensor = ort::value::Tensor::from_array(mask_array)?;

                let hidden = if let Some(type_id) = token_type_ids {
                    let type_array = ndarray::Array2::from_shape_vec((batch, max_len), type_data)?;
                    let type_tensor = ort::value::Tensor::from_array(type_array)?;
                    let mut _guard = self
                        .session
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let outputs = _guard.run(ort::inputs![
                        input_id.as_str() => ids_tensor,
                        mask_id.as_str() => mask_tensor,
                        type_id.as_str() => type_tensor,
                    ])?;
                    let (_, data) =
                        outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
                    data.to_vec()
                } else {
                    let mut _guard = self
                        .session
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let outputs = _guard.run(ort::inputs![
                        input_id.as_str() => ids_tensor,
                        mask_id.as_str() => mask_tensor,
                    ])?;
                    let (_, data) =
                        outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
                    data.to_vec()
                };

                let mut results = if self.output_pooled {
                    hidden
                        .chunks_exact(self.dimensions)
                        .map(<[f32]>::to_vec)
                        .collect()
                } else {
                    pooling::pool_batch(
                        &hidden,
                        &per_seq_masks,
                        max_len,
                        self.dimensions,
                        self.meta.pooling,
                    )
                };
                for emb in &mut results {
                    pooling::normalize_l2(emb);
                }
                Ok(results)
            }
            GraphInputs::EmbeddingBag { input_ids, offsets } => {
                let batch = inputs.len();
                let mut flat_ids: Vec<i64> = Vec::new();
                let mut adjusted_offsets: Vec<i64> = Vec::with_capacity(batch);
                let mut last_offset = 0i64;

                for inp in inputs {
                    adjusted_offsets.push(last_offset);
                    if !inp.input_ids.is_empty() {
                        flat_ids.extend(inp.input_ids.iter().map(|&x| x as i64));
                        last_offset = flat_ids.len() as i64;
                    }
                }

                if flat_ids.is_empty() {
                    return Ok(vec![vec![0.0; self.dimensions]; batch]);
                }

                let ids_array = ndarray::Array1::from_shape_vec(flat_ids.len(), flat_ids)?;
                let offsets_array = ndarray::Array1::from_shape_vec(batch, adjusted_offsets)?;
                let ids_tensor = ort::value::Tensor::from_array(ids_array)?;
                let offsets_tensor = ort::value::Tensor::from_array(offsets_array)?;

                let mut _guard = self
                    .session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let outputs = _guard.run(ort::inputs![
                    input_ids.as_str() => ids_tensor,
                    offsets.as_str() => offsets_tensor,
                ])?;
                let (_, out_data) =
                    outputs[self.output_name.as_str()].try_extract_tensor::<f32>()?;
                let out = out_data.to_vec();

                let mut results: Vec<Vec<f32>> = out
                    .chunks_exact(self.dimensions)
                    .map(<[f32]>::to_vec)
                    .collect();
                while results.len() < batch {
                    results.push(vec![0.0; self.dimensions]);
                }
                for emb in &mut results {
                    pooling::normalize_l2(emb);
                }
                Ok(results)
            }
        }
    }

    #[cfg(not(feature = "dense"))]
    #[allow(clippy::unused_self)] // same signature as the `embeddings` build
    fn run_inference(&self, _input: &TokenizedInput) -> anyhow::Result<Vec<f32>> {
        anyhow::bail!("Embeddings feature not enabled")
    }

    #[cfg(not(feature = "dense"))]
    #[allow(clippy::unused_self)] // same signature as the `embeddings` build
    fn run_inference_batch(&self, _inputs: &[TokenizedInput]) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::bail!("Embeddings feature not enabled")
    }
}

/// The files a model needs before it can load.
fn model_files_present(model_dir: &Path) -> bool {
    model_dir.join(download::MODEL_FILE).exists()
        && model_dir.join(download::TOKENIZER_FILE).exists()
}

fn with_prefix(prefix: Option<&str>, text: &str) -> String {
    match prefix {
        Some(prefix) => format!("{prefix}{text}"),
        None => text.to_owned(),
    }
}

/// Shape of `output_name` for a one-input batch, from a probe inference.
#[cfg(feature = "dense")]
fn probe_output_shape(
    session: &mut ort::session::Session,
    tokenizer: &tokenizer::Tokenizer,
    graph_inputs: &GraphInputs,
    output_name: &str,
) -> Option<Vec<usize>> {
    let dummy = tokenizer.encode("test").ok()?;
    let seq_len = dummy.input_ids.len();
    if seq_len == 0 {
        return None;
    }

    let outputs = match graph_inputs {
        GraphInputs::Transformer {
            input_ids,
            attention_mask,
            token_type_ids,
        } => {
            let ids_vec: Vec<i64> = dummy.input_ids.iter().map(|&x| x as i64).collect();
            let mask_vec: Vec<i64> = dummy.attention_mask.iter().map(|&x| x as i64).collect();
            let ids_array = ndarray::Array2::from_shape_vec((1, seq_len), ids_vec).ok()?;
            let mask_array = ndarray::Array2::from_shape_vec((1, seq_len), mask_vec).ok()?;
            let ids_tensor = ort::value::Tensor::from_array(ids_array).ok()?;
            let mask_tensor = ort::value::Tensor::from_array(mask_array).ok()?;

            if let Some(type_id) = token_type_ids {
                let type_vec: Vec<i64> = dummy.token_type_ids.iter().map(|&x| x as i64).collect();
                let type_array = ndarray::Array2::from_shape_vec((1, seq_len), type_vec).ok()?;
                let type_tensor = ort::value::Tensor::from_array(type_array).ok()?;
                session
                    .run(ort::inputs![
                        input_ids.as_str() => ids_tensor,
                        attention_mask.as_str() => mask_tensor,
                        type_id.as_str() => type_tensor,
                    ])
                    .ok()?
            } else {
                session
                    .run(ort::inputs![
                        input_ids.as_str() => ids_tensor,
                        attention_mask.as_str() => mask_tensor,
                    ])
                    .ok()?
            }
        }
        GraphInputs::EmbeddingBag { input_ids, offsets } => {
            let ids_vec: Vec<i64> = dummy.input_ids.iter().map(|&x| x as i64).collect();
            let ids_array = ndarray::Array1::from_shape_vec(seq_len, ids_vec).ok()?;
            let offsets_array = ndarray::Array1::from_shape_vec(1, vec![0i64]).ok()?;
            let ids_tensor = ort::value::Tensor::from_array(ids_array).ok()?;
            let offsets_tensor = ort::value::Tensor::from_array(offsets_array).ok()?;
            session
                .run(ort::inputs![
                    input_ids.as_str() => ids_tensor,
                    offsets.as_str() => offsets_tensor,
                ])
                .ok()?
        }
    };

    let (shape, _) = outputs[output_name].try_extract_tensor::<f32>().ok()?;
    shape.iter().map(|&d| usize::try_from(d).ok()).collect()
}

/// Compute cosine similarity between two L2-normalized vectors.
/// Both vectors must be pre-normalized for correct results.
///
/// Uses the chunked, autovectorizable dot product from [`crate::core::embedding_quant`]
/// so every semantic-search hot path gets SIMD throughput.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have equal dimensions");
    crate::core::embedding_quant::dot_f32(a, b)
}

/// Compute cosine similarity without requiring pre-normalization.
pub fn cosine_similarity_raw(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    use crate::core::embedding_quant::dot_f32;
    let dot = dot_f32(a, b);
    let norm_a = dot_f32(a, a).sqrt();
    let norm_b = dot_f32(b, b).sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(feature = "dense")]
static SHARED_ENGINE: std::sync::OnceLock<anyhow::Result<EmbeddingEngine>> =
    std::sync::OnceLock::new();

/// Global singleton embedding engine. Loaded once, shared across all consumers.
/// Returns None if the dense feature is disabled or the model fails to load.
/// NOTE: This function BLOCKS on first call while loading the ONNX model.
#[cfg(feature = "dense")]
pub fn shared_engine() -> Option<&'static EmbeddingEngine> {
    shared_engine_result().ok()
}

/// Global singleton embedding engine with the load error preserved for callers
/// that can surface diagnostics to users.
#[cfg(feature = "dense")]
pub fn shared_engine_result() -> anyhow::Result<&'static EmbeddingEngine> {
    SHARED_ENGINE
        .get_or_init(EmbeddingEngine::load_default)
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_raw_unnormalized() {
        let a = vec![3.0, 4.0];
        let b = vec![3.0, 4.0];
        assert!((cosine_similarity_raw(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_raw_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        assert_eq!(cosine_similarity_raw(&a, &b), 0.0);
    }

    #[test]
    fn model_directory_follows_cache_dir() {
        // Serialize env mutation: SOBRE_CACHE_DIR is process-global and
        // shared with `ensure_downloaded_is_offline_noop_when_model_present`.
        let _lock = crate::core::data_dir::test_env_lock();
        let unique = "/tmp/sobre_test_embed_42xyz";
        crate::test_env::set_var("SOBRE_CACHE_DIR", unique);
        let dir = EmbeddingEngine::model_directory().unwrap();
        assert_eq!(dir, std::path::Path::new(unique).join("models"));
        assert!(!EmbeddingEngine::is_available());
        crate::test_env::remove_var("SOBRE_CACHE_DIR");
    }

    /// With a complete model cache, `ensure_downloaded` is a no-op that
    /// never touches the network and agrees with `is_available` on the model
    /// directory. The build path relies on this so a cold availability check can
    /// route through the (real) download without redundantly re-fetching a model
    /// that is already present. Both gates use plain existence checks, so tiny
    /// placeholder files are enough to exercise the short-circuit offline.
    #[test]
    fn ensure_downloaded_is_offline_noop_when_model_present() {
        // Serialize env mutation (SOBRE_CACHE_DIR is process-global) so a
        // sibling env test can't clobber the dir between set_var and the checks.
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = std::env::temp_dir().join(format!("sobre-embed-{}", std::process::id()));
        crate::test_env::set_var("SOBRE_CACHE_DIR", tmp.to_str().unwrap());

        let selected = model_registry::resolve_model();
        let model_dir = tmp.join("models").join(selected.storage_dir_name());
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join(download::MODEL_FILE), b"onnx").unwrap();
        std::fs::write(model_dir.join(download::TOKENIZER_FILE), b"tokenizer").unwrap();

        assert!(EmbeddingEngine::is_available());
        assert!(EmbeddingEngine::ensure_downloaded().is_ok());

        crate::test_env::remove_var("SOBRE_CACHE_DIR");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The EmbeddingBag detection is purely name-based — exactly two
    /// inputs with the second named `offsets`. Everything else (classic 2-/
    /// 3-input transformers, unnamed graphs) must stay on the transformer path.
    #[test]
    #[cfg(feature = "dense")]
    fn embedding_bag_signature_detection() {
        // model2vec / potion export.
        assert!(is_embedding_bag_signature(&[
            "input_ids".to_string(),
            "offsets".to_string()
        ]));
        // Transformers: mask second, optional token types third.
        assert!(!is_embedding_bag_signature(&[
            "input_ids".to_string(),
            "attention_mask".to_string()
        ]));
        assert!(!is_embedding_bag_signature(&[
            "input_ids".to_string(),
            "attention_mask".to_string(),
            "token_type_ids".to_string()
        ]));
        // Wrong arity never flips the topology.
        assert!(!is_embedding_bag_signature(&["input_ids".to_string()]));
        assert!(!is_embedding_bag_signature(&[
            "input_ids".to_string(),
            "offsets".to_string(),
            "extra".to_string()
        ]));
    }

    /// Live proof: loads a real HuggingFace repo through the
    /// `org/repo@rev` spec (download → SHA-256 lockfile → tokenizer.json →
    /// ONNX inference → dimension probe). Ignored by default (network + ~91MB);
    /// run explicitly:
    /// `cargo test --lib --features dense -- --ignored custom_hf_model_end_to_end`
    #[test]
    #[ignore = "downloads a real model from HuggingFace (~91MB)"]
    #[cfg(feature = "dense")]
    fn custom_hf_model_end_to_end() {
        let model =
            model_registry::EmbeddingModel::parse("hf:sentence-transformers/all-MiniLM-L6-v2@main")
                .expect("valid hf: spec");

        let base = std::env::temp_dir().join("sobre_test_custom_hf_e2e");
        download::ensure_model(&base.join(model.storage_dir_name()), &model).expect("download");
        let engine = EmbeddingEngine::load_model(&base, &model).expect("load custom model");

        assert_eq!(engine.dimensions(), 384, "probed dims from ONNX graph");
        assert_eq!(
            engine.model_name(),
            "sentence-transformers/all-MiniLM-L6-v2@main"
        );

        let v = engine.embed("fn main() { println!(\"hello\"); }").unwrap();
        assert_eq!(v.len(), 384);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "L2-normalized, got {norm}");

        // Lockfile must exist and pin both artifacts.
        let lock_path = base.join(model.storage_dir_name()).join("model.lock.json");
        let lock: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&lock_path).unwrap()).unwrap();
        assert!(lock.contains_key("model.onnx"));
        assert!(lock.contains_key("tokenizer.json"));

        // Semantic sanity: similar code closer than unrelated text.
        let a = engine.embed("read a file from disk").unwrap();
        let b = engine.embed("load file contents from filesystem").unwrap();
        let c = engine.embed("the weather in Zurich is sunny").unwrap();
        assert!(
            cosine_similarity(&a, &b) > cosine_similarity(&a, &c),
            "related texts must be closer"
        );
    }

    /// Live proof: a model2vec EmbeddingBag graph end-to-end
    /// through the same `hf:` scheme (potion-base-8M, ~30MB). Ignored by
    /// default (network); run explicitly:
    /// `cargo test --lib --features dense -- --ignored model2vec_potion_end_to_end`
    #[test]
    #[ignore = "downloads a real model from HuggingFace (~30MB)"]
    #[cfg(feature = "dense")]
    fn model2vec_potion_end_to_end() {
        let model = model_registry::EmbeddingModel::parse("hf:minishlab/potion-base-8M@main")
            .expect("valid hf: spec");

        let base = std::env::temp_dir().join("sobre_test_model2vec_e2e");
        download::ensure_model(&base.join(model.storage_dir_name()), &model).expect("download");
        let engine = EmbeddingEngine::load_model(&base, &model).expect("load model2vec model");

        // potion-base-8M is 256d; the probe must read it off the rank-2
        // output, not assume a [1, seq, dim] transformer shape.
        assert_eq!(engine.dimensions(), 256, "probed dims from EmbeddingBag");

        let code_vec = engine.embed("fn main() { println!(\"hello\"); }").unwrap();
        assert_eq!(code_vec.len(), 256);
        let norm: f32 = code_vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "L2-normalized, got {norm}");

        // Distinct inputs must not collapse to one vector.
        let sql_vec = engine.embed("SELECT * FROM users WHERE id = 1").unwrap();
        assert!(cosine_similarity(&code_vec, &sql_vec) < 0.999);

        // Semantic sanity survives the static-embedding quality trade-off.
        let read_vec = engine.embed("read a file from disk").unwrap();
        let load_vec = engine.embed("load file contents from filesystem").unwrap();
        let weather_vec = engine.embed("the weather in Zurich is sunny").unwrap();
        assert!(
            cosine_similarity(&read_vec, &load_vec) > cosine_similarity(&read_vec, &weather_vec),
            "related texts must be closer"
        );

        // Empty input: zero vector, no inference panic.
        let empty_vec = engine.embed("").unwrap();
        assert_eq!(empty_vec.len(), 256);
    }
}
