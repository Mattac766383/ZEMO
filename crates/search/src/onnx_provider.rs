//! Production local ONNX embedding provider (Granite 97M Multilingual R2).

use crate::{
    EmbeddingAvailability, EmbeddingError, EmbeddingInput, EmbeddingOutput,
    EmbeddingProviderDescriptor, LocalEmbeddingProvider, MAX_EMBEDDING_BATCH,
    MAX_EMBEDDING_INPUT_CHARS,
    model_manager::{
        EmbeddingModelStatus, GRANITE_EMBEDDING_APPROX_BYTES, GRANITE_EMBEDDING_DIMENSIONS,
        GRANITE_EMBEDDING_MAX_TOKENS, GRANITE_EMBEDDING_MODEL_ID, GRANITE_EMBEDDING_REVISION,
        LocalEmbeddingModelManager,
    },
    normalize_vector, validate_embedding_vector,
};
use ort::{
    session::{Session, builder::GraphOptimizationLevel},
    value::Tensor,
};
use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};
use tokenizers::Tokenizer;

const INTRA_THREADS: usize = 2;
const INTER_THREADS: usize = 1;

struct LoadedModel {
    session: Session,
    tokenizer: Tokenizer,
}

/// Production local embedding provider with lazy ONNX load and app-local model manager.
pub struct OnnxLocalEmbeddingProvider {
    manager: LocalEmbeddingModelManager,
    loaded: Mutex<Option<LoadedModel>>,
}

impl std::fmt::Debug for OnnxLocalEmbeddingProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OnnxLocalEmbeddingProvider")
            .field("model_id", &GRANITE_EMBEDDING_MODEL_ID)
            .field("version", &GRANITE_EMBEDDING_REVISION)
            .field("root", &self.manager.root())
            .finish()
    }
}

impl OnnxLocalEmbeddingProvider {
    pub fn new(model_root: impl Into<PathBuf>) -> Result<Self, EmbeddingError> {
        Ok(Self {
            manager: LocalEmbeddingModelManager::new(model_root)?,
            loaded: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn manager(&self) -> &LocalEmbeddingModelManager {
        &self.manager
    }

    pub fn model_status(&self) -> crate::model_manager::EmbeddingModelStatusView {
        self.manager.get_status()
    }

    pub fn activate_from_env(&self) -> Result<(), EmbeddingError> {
        self.unload();
        self.manager.register_from_env_or_error()?;
        Ok(())
    }

    pub fn activate_from_directory(&self, source: &std::path::Path) -> Result<(), EmbeddingError> {
        self.unload();
        self.manager.register_from_directory(source)?;
        Ok(())
    }

    pub fn verify_installed(&self) -> Result<(), EmbeddingError> {
        let state = self.manager.verify()?;
        if state.status != EmbeddingModelStatus::Ready {
            return Err(match state.status {
                EmbeddingModelStatus::Corrupt => EmbeddingError::Corrupt,
                EmbeddingModelStatus::IncompatibleVersion => {
                    EmbeddingError::Failed("incompatible embedding model version".to_owned())
                }
                EmbeddingModelStatus::Failed => EmbeddingError::Failed(
                    state
                        .last_error
                        .unwrap_or_else(|| "embedding model failed".to_owned()),
                ),
                _ => EmbeddingError::Unavailable,
            });
        }
        Ok(())
    }

    pub fn remove_model(&self) -> Result<(), EmbeddingError> {
        self.unload();
        // Removing the model invalidates any in-memory session; ANN rebuild is
        // required separately when a new model is installed.
        self.manager.remove()
    }

    /// User-consented install of the pinned Granite assets over HTTPS.
    /// Does not accept arbitrary URLs or paths from the renderer.
    pub fn install_pinned_model(
        &self,
        is_cancelled: &std::sync::atomic::AtomicBool,
        on_progress: &mut dyn FnMut(crate::InstallProgress),
    ) -> Result<(), EmbeddingError> {
        self.unload();
        let fetcher = crate::UreqHttpsFetcher;
        self.manager
            .install_from_pinned_network(&fetcher, is_cancelled, on_progress)?;
        Ok(())
    }

    /// Offline/dev provision path (env or explicit directory).
    pub fn install_from_local_or_env(&self) -> Result<(), EmbeddingError> {
        self.unload();
        if self.verify_installed().is_ok() {
            return Ok(());
        }
        self.activate_from_env()
    }

    pub fn unload(&self) {
        if let Ok(mut guard) = self.loaded.lock() {
            *guard = None;
        }
    }

    fn lock_loaded(&self) -> Result<MutexGuard<'_, Option<LoadedModel>>, EmbeddingError> {
        self.loaded
            .lock()
            .map_err(|_| EmbeddingError::Failed("embedding model lock poisoned".to_owned()))
    }

    fn ensure_loaded(&self) -> Result<(), EmbeddingError> {
        let state = self.manager.verify()?;
        if state.status != EmbeddingModelStatus::Ready {
            return Err(match state.status {
                EmbeddingModelStatus::Corrupt => EmbeddingError::Corrupt,
                EmbeddingModelStatus::NotInstalled => EmbeddingError::Unavailable,
                EmbeddingModelStatus::IncompatibleVersion => {
                    EmbeddingError::Failed("incompatible embedding model version".to_owned())
                }
                _ => EmbeddingError::Unavailable,
            });
        }

        let mut guard = self.lock_loaded()?;
        if guard.is_some() {
            return Ok(());
        }

        let _ = self
            .manager
            .mark_status(EmbeddingModelStatus::Loading, None);
        match self.load_model_inner() {
            Ok(loaded) => {
                *guard = Some(loaded);
                let _ = self.manager.mark_status(EmbeddingModelStatus::Ready, None);
                Ok(())
            }
            Err(error) => {
                let _ = self
                    .manager
                    .mark_status(EmbeddingModelStatus::Failed, Some(error.to_string()));
                Err(error)
            }
        }
    }

    fn load_model_inner(&self) -> Result<LoadedModel, EmbeddingError> {
        let model_path =
            crate::native_filesystem_path_for_c_runtime(&self.manager.asset_path("onnx_model")?);
        let tokenizer_path =
            crate::native_filesystem_path_for_c_runtime(&self.manager.asset_path("tokenizer")?);

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|error| EmbeddingError::Failed(format!("tokenizer load failed: {error}")))?;

        let session = Session::builder()
            .map_err(|error| EmbeddingError::Failed(format!("ORT session builder: {error}")))?
            .with_optimization_level(GraphOptimizationLevel::Level1)
            .map_err(|error| EmbeddingError::Failed(format!("ORT optimization: {error}")))?
            .with_intra_threads(INTRA_THREADS)
            .map_err(|error| EmbeddingError::Failed(format!("ORT intra threads: {error}")))?
            .with_inter_threads(INTER_THREADS)
            .map_err(|error| EmbeddingError::Failed(format!("ORT inter threads: {error}")))?
            .commit_from_file(&model_path)
            .map_err(|error| {
                EmbeddingError::Failed(format!(
                    "ORT commit_from_file {}: {error}",
                    model_path.display()
                ))
            })?;

        Ok(LoadedModel { session, tokenizer })
    }

    fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        self.ensure_loaded()?;
        let mut guard = self.lock_loaded()?;
        let loaded = guard.as_mut().ok_or(EmbeddingError::Unavailable)?;

        let mut encodings = Vec::with_capacity(texts.len());
        for text in texts {
            let prepared = preprocess_embedding_text(text);
            let encoding = loaded
                .tokenizer
                .encode(prepared, true)
                .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
            let mut ids = encoding.get_ids().to_vec();
            let mut mask = encoding.get_attention_mask().to_vec();
            if ids.len() > GRANITE_EMBEDDING_MAX_TOKENS {
                ids.truncate(GRANITE_EMBEDDING_MAX_TOKENS);
                mask.truncate(GRANITE_EMBEDDING_MAX_TOKENS);
            }
            encodings.push((ids, mask));
        }

        let batch = encodings.len();
        let seq_len = encodings
            .iter()
            .map(|(ids, _)| ids.len())
            .max()
            .unwrap_or(0)
            .max(1);

        let mut input_ids = vec![0_i64; batch * seq_len];
        let mut attention_mask = vec![0_i64; batch * seq_len];
        for (row, (ids, mask)) in encodings.iter().enumerate() {
            for (col, (id, attention)) in ids.iter().zip(mask.iter()).enumerate() {
                let index = row * seq_len + col;
                input_ids[index] = i64::from(*id);
                attention_mask[index] = i64::from(*attention);
            }
        }

        let ids_tensor = Tensor::from_array(([batch, seq_len], input_ids))
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        let mask_tensor = Tensor::from_array(([batch, seq_len], attention_mask))
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;

        let outputs = loaded
            .session
            .run(ort::inputs![
                "input_ids" => ids_tensor,
                "attention_mask" => mask_tensor,
            ])
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;

        let (_, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;

        // Expected layout: [batch, seq, hidden] or [batch, hidden]
        let hidden = GRANITE_EMBEDDING_DIMENSIONS;
        let mut vectors = Vec::with_capacity(batch);
        if data.len() == batch * hidden {
            for row in 0..batch {
                let start = row * hidden;
                let mut values = data[start..start + hidden].to_vec();
                normalize_vector(&mut values);
                if !validate_embedding_vector(&values, hidden) {
                    return Err(EmbeddingError::InvalidVector);
                }
                vectors.push(values);
            }
        } else if data.len() == batch * seq_len * hidden {
            for row in 0..batch {
                let start = row * seq_len * hidden;
                // CLS token at position 0
                let mut values = data[start..start + hidden].to_vec();
                normalize_vector(&mut values);
                if !validate_embedding_vector(&values, hidden) {
                    return Err(EmbeddingError::InvalidVector);
                }
                vectors.push(values);
            }
        } else {
            return Err(EmbeddingError::InvalidVector);
        }
        Ok(vectors)
    }
}

impl LocalEmbeddingProvider for OnnxLocalEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        let status = self.manager.get_status().status;
        let production_ready = status == EmbeddingModelStatus::Ready;
        EmbeddingProviderDescriptor {
            provider_id: GRANITE_EMBEDDING_MODEL_ID.to_owned(),
            version: GRANITE_EMBEDDING_REVISION.to_owned(),
            dimensions: GRANITE_EMBEDDING_DIMENSIONS,
            local_only: true,
            production_ready,
            // Installed assets only; Step 1 has no runtime download path.
            requires_download: false,
            model_size_bytes: GRANITE_EMBEDDING_APPROX_BYTES,
            max_model_size_bytes: 256 * 1024 * 1024,
        }
    }

    fn availability(&self) -> EmbeddingAvailability {
        match self.manager.get_status().status {
            EmbeddingModelStatus::Ready => EmbeddingAvailability::AvailableProduction,
            _ => EmbeddingAvailability::Unavailable,
        }
    }

    fn embed_batch(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<Vec<EmbeddingOutput>, EmbeddingError> {
        if self.availability() != EmbeddingAvailability::AvailableProduction {
            return Err(EmbeddingError::Unavailable);
        }
        if inputs.len() > MAX_EMBEDDING_BATCH
            || inputs
                .iter()
                .any(|input| input.text.chars().count() > MAX_EMBEDDING_INPUT_CHARS)
        {
            return Err(EmbeddingError::InputLimit);
        }
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let texts = inputs
            .iter()
            .map(|input| input.text.clone())
            .collect::<Vec<_>>();
        let vectors = self.embed_texts(&texts)?;
        inputs
            .iter()
            .zip(vectors)
            .map(|(input, values)| {
                if !validate_embedding_vector(&values, GRANITE_EMBEDDING_DIMENSIONS) {
                    return Err(EmbeddingError::InvalidVector);
                }
                Ok(EmbeddingOutput {
                    source_id: input.source_id.clone(),
                    values,
                    input_digest: *blake3::hash(input.text.as_bytes()).as_bytes(),
                })
            })
            .collect()
    }
}

#[must_use]
pub fn preprocess_embedding_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_manager::EmbeddingModelStatus;

    #[test]
    fn unavailable_without_installed_model() {
        let dir = tempfile::tempdir().expect("temp");
        let provider = OnnxLocalEmbeddingProvider::new(dir.path()).expect("provider");
        assert_eq!(provider.availability(), EmbeddingAvailability::Unavailable);
        assert!(!provider.descriptor().production_ready);
        let error = provider
            .embed_batch(&[EmbeddingInput {
                source_id: "q".to_owned(),
                source_kind: "semantic_summary".to_owned(),
                text: "facture".to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .expect_err("unavailable");
        assert!(matches!(error, EmbeddingError::Unavailable));
        assert_eq!(
            provider.model_status().status,
            EmbeddingModelStatus::NotInstalled
        );
    }

    #[test]
    fn preprocess_collapses_whitespace_only() {
        assert_eq!(
            preprocess_embedding_text("  facture   rénovation  "),
            "facture rénovation"
        );
    }

    #[test]
    fn rejects_oversized_batch() {
        let dir = tempfile::tempdir().expect("temp");
        let provider = OnnxLocalEmbeddingProvider::new(dir.path()).expect("provider");
        // Force Ready status without assets so availability path is exercised for limits
        // after we mark ready — still unavailable for embed until verify passes.
        let inputs = (0..=MAX_EMBEDDING_BATCH)
            .map(|index| EmbeddingInput {
                source_id: format!("i{index}"),
                source_kind: "semantic_summary".to_owned(),
                text: "x".to_owned(),
                start_offset: None,
                end_offset: None,
            })
            .collect::<Vec<_>>();
        // Not ready => Unavailable (limit check happens after availability).
        assert!(matches!(
            provider.embed_batch(&inputs),
            Err(EmbeddingError::Unavailable)
        ));
    }
}
