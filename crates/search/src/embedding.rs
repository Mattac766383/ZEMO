use crate::{normalize_search_text, tokenize};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DETERMINISTIC_TEST_EMBEDDING_PROVIDER_ID: &str = "deterministic-test-embedding";
pub const DETERMINISTIC_TEST_EMBEDDING_VERSION: &str = "deterministic-test-hash-v1";
/// Deprecated alias for tests that still reference the old development hash id.
pub const DEVELOPMENT_EMBEDDING_PROVIDER_ID: &str = DETERMINISTIC_TEST_EMBEDDING_PROVIDER_ID;
pub const DEVELOPMENT_EMBEDDING_VERSION: &str = DETERMINISTIC_TEST_EMBEDDING_VERSION;
pub const DEFAULT_EMBEDDING_DIMENSIONS: usize = 192;
pub const MAX_EMBEDDING_BATCH: usize = 32;
pub const MAX_EMBEDDING_INPUT_CHARS: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProviderDescriptor {
    pub provider_id: String,
    pub version: String,
    pub dimensions: usize,
    pub local_only: bool,
    pub production_ready: bool,
    pub requires_download: bool,
    pub model_size_bytes: u64,
    pub max_model_size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingAvailability {
    AvailableDevelopment,
    AvailableProduction,
    Unavailable,
}

impl EmbeddingAvailability {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::AvailableDevelopment => "available_development",
            Self::AvailableProduction => "available_production",
            Self::Unavailable => "unavailable",
        }
    }
}

#[must_use]
pub fn local_embedding_descriptor_is_safe(
    descriptor: &EmbeddingProviderDescriptor,
    availability: EmbeddingAvailability,
) -> bool {
    let dimensions_valid = match availability {
        EmbeddingAvailability::AvailableDevelopment
        | EmbeddingAvailability::AvailableProduction => (1..=4096).contains(&descriptor.dimensions),
        EmbeddingAvailability::Unavailable => descriptor.dimensions <= 4096,
    };
    let readiness_valid = match availability {
        EmbeddingAvailability::AvailableDevelopment | EmbeddingAvailability::Unavailable => {
            !descriptor.production_ready
        }
        EmbeddingAvailability::AvailableProduction => descriptor.production_ready,
    };
    !descriptor.provider_id.is_empty()
        && descriptor.provider_id.chars().count() <= 128
        && !descriptor.version.is_empty()
        && descriptor.version.chars().count() <= 64
        && descriptor.local_only
        && !descriptor.requires_download
        && descriptor.model_size_bytes <= descriptor.max_model_size_bytes
        && descriptor.max_model_size_bytes > 0
        && descriptor.max_model_size_bytes <= 1024 * 1024 * 1024
        && dimensions_valid
        && readiness_valid
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingInput {
    pub source_id: String,
    pub source_kind: String,
    pub text: String,
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingOutput {
    pub source_id: String,
    pub values: Vec<f32>,
    pub input_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingIndexEntry {
    pub source_id: String,
    pub source_kind: String,
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
    pub vector: Vec<u8>,
    pub input_digest: [u8; 32],
}

pub fn embedding_index_entries(
    inputs: &[EmbeddingInput],
    outputs: &[EmbeddingOutput],
) -> Result<Vec<EmbeddingIndexEntry>, EmbeddingError> {
    if inputs.len() != outputs.len() {
        return Err(EmbeddingError::InvalidVector);
    }
    inputs
        .iter()
        .zip(outputs)
        .map(|(input, output)| {
            if input.source_id != output.source_id || output.values.is_empty() {
                return Err(EmbeddingError::InvalidVector);
            }
            Ok(EmbeddingIndexEntry {
                source_id: input.source_id.clone(),
                source_kind: input.source_kind.clone(),
                start_offset: input.start_offset,
                end_offset: input.end_offset,
                vector: quantize_unit_vector(&output.values),
                input_digest: output.input_digest,
            })
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("local embedding provider is unavailable")]
    Unavailable,
    #[error("local embedding input exceeds bounded limits")]
    InputLimit,
    #[error("local embedding provider returned an invalid vector")]
    InvalidVector,
    #[error("local embedding model is corrupt or failed integrity checks")]
    Corrupt,
    #[error("local embedding model failed to load or run: {0}")]
    Failed(String),
}

pub trait LocalEmbeddingProvider: Send + Sync {
    fn descriptor(&self) -> EmbeddingProviderDescriptor;
    fn availability(&self) -> EmbeddingAvailability;
    fn embed_batch(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<Vec<EmbeddingOutput>, EmbeddingError>;
}

/// Deterministic non-semantic hash embeddings for unit tests and fixtures only.
/// Never use as the production semantic embedding path.
#[derive(Clone)]
pub struct DeterministicTestEmbeddingProvider {
    dimensions: usize,
}

impl DeterministicTestEmbeddingProvider {
    #[must_use]
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions: dimensions.clamp(64, 512),
        }
    }
}

impl Default for DeterministicTestEmbeddingProvider {
    fn default() -> Self {
        Self::new(DEFAULT_EMBEDDING_DIMENSIONS)
    }
}

impl fmt::Debug for DeterministicTestEmbeddingProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeterministicTestEmbeddingProvider")
            .field("dimensions", &self.dimensions)
            .field("production_ready", &false)
            .finish()
    }
}

impl LocalEmbeddingProvider for DeterministicTestEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor {
            provider_id: DETERMINISTIC_TEST_EMBEDDING_PROVIDER_ID.to_owned(),
            version: DETERMINISTIC_TEST_EMBEDDING_VERSION.to_owned(),
            dimensions: self.dimensions,
            local_only: true,
            production_ready: false,
            requires_download: false,
            model_size_bytes: 0,
            max_model_size_bytes: 256 * 1024 * 1024,
        }
    }

    fn availability(&self) -> EmbeddingAvailability {
        EmbeddingAvailability::AvailableDevelopment
    }

    fn embed_batch(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<Vec<EmbeddingOutput>, EmbeddingError> {
        if inputs.len() > MAX_EMBEDDING_BATCH
            || inputs
                .iter()
                .any(|input| input.text.chars().count() > MAX_EMBEDDING_INPUT_CHARS)
        {
            return Err(EmbeddingError::InputLimit);
        }
        inputs
            .iter()
            .map(|input| {
                let values = development_embedding(&input.text, self.dimensions);
                if values.len() != self.dimensions || values.iter().any(|value| !value.is_finite())
                {
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

#[derive(Debug, Clone, Default)]
pub struct UnavailableEmbeddingProvider;

impl LocalEmbeddingProvider for UnavailableEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor {
            provider_id: "unavailable-local-embedding".to_owned(),
            version: "none".to_owned(),
            dimensions: 0,
            local_only: true,
            production_ready: false,
            requires_download: false,
            model_size_bytes: 0,
            max_model_size_bytes: 256 * 1024 * 1024,
        }
    }

    fn availability(&self) -> EmbeddingAvailability {
        EmbeddingAvailability::Unavailable
    }

    fn embed_batch(
        &self,
        _inputs: &[EmbeddingInput],
    ) -> Result<Vec<EmbeddingOutput>, EmbeddingError> {
        Err(EmbeddingError::Unavailable)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbeddingDocument {
    pub filename: String,
    pub semantic_fields: Vec<(String, String)>,
    pub identities: Vec<(String, String)>,
    pub extracted_text: String,
}

/// Bounded document → embedding inputs using the Step-2 chunking policy.
#[must_use]
pub fn bounded_embedding_inputs(document: &EmbeddingDocument) -> Vec<EmbeddingInput> {
    crate::chunking::bounded_embedding_inputs_v2(document)
}

#[must_use]
pub fn quantize_unit_vector(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .map(|value| (value.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8)
        .collect()
}

#[must_use]
pub fn cosine_similarity_quantized(query: &[f32], stored: &[u8]) -> f32 {
    if query.len() != stored.len() || query.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut stored_norm = 0.0_f32;
    for (left, right) in query.iter().zip(stored) {
        let right = f32::from(i8::from_ne_bytes([*right])) / 127.0;
        dot += left * right;
        stored_norm += right * right;
    }
    if stored_norm <= f32::EPSILON {
        0.0
    } else {
        (dot / stored_norm.sqrt()).clamp(-1.0, 1.0)
    }
}

#[must_use]
pub fn development_embedding(value: &str, dimensions: usize) -> Vec<f32> {
    let dimensions = dimensions.clamp(64, 512);
    let expanded = expand_local_concepts(&normalize_search_text(value));
    let mut output = vec![0.0_f32; dimensions];
    let tokens = tokenize(&expanded);
    for token in &tokens {
        add_feature(&mut output, token.as_bytes(), 1.0);
        let characters = token.chars().collect::<Vec<_>>();
        for window in characters.windows(3) {
            let feature = window.iter().collect::<String>();
            add_feature(&mut output, feature.as_bytes(), 0.35);
        }
    }
    for pair in tokens.windows(2) {
        add_feature(
            &mut output,
            format!("{}:{}", pair[0], pair[1]).as_bytes(),
            0.55,
        );
    }
    normalize_vector(&mut output);
    output
}

fn add_feature(output: &mut [f32], feature: &[u8], weight: f32) {
    let digest = blake3::hash(feature);
    let bytes = digest.as_bytes();
    let index = usize::from(u16::from_le_bytes([bytes[0], bytes[1]])) % output.len();
    output[index] += if bytes[2] & 1 == 0 { weight } else { -weight };
}

pub fn normalize_vector(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector {
            *value /= norm;
        }
    }
}

/// Validates embedding vectors for dimension, finiteness, and nonzero norm.
#[must_use]
pub fn validate_embedding_vector(values: &[f32], expected_dimensions: usize) -> bool {
    if values.len() != expected_dimensions || values.is_empty() {
        return false;
    }
    if values.iter().any(|value| !value.is_finite()) {
        return false;
    }
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    norm_sq > f32::EPSILON
}

fn expand_local_concepts(value: &str) -> String {
    let mut output = value.to_owned();
    const CONCEPTS: &[(&str, &str)] = &[
        ("facture", " invoice billing"),
        ("devis", " quote estimate"),
        ("contrat", " contract agreement"),
        ("chantier", " project construction"),
        ("projet", " project"),
        ("fournisseur", " supplier vendor"),
        ("client", " customer"),
        ("photo", " image picture"),
        ("administratif", " administrative"),
        ("personnel", " personal private"),
    ];
    for (source, expansion) in CONCEPTS {
        if output.split_whitespace().any(|token| token == *source) {
            output.push_str(expansion);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunking::ChunkingPolicy;

    #[test]
    fn provider_is_explicitly_local_and_test_only() {
        let provider = DeterministicTestEmbeddingProvider::default();
        let descriptor = provider.descriptor();
        assert!(descriptor.local_only);
        assert!(!descriptor.production_ready);
        assert!(!descriptor.requires_download);
        assert_eq!(descriptor.model_size_bytes, 0);
        assert_eq!(
            EmbeddingAvailability::AvailableProduction.database_name(),
            "available_production"
        );
        let mut unsafe_descriptor = descriptor;
        unsafe_descriptor.local_only = false;
        unsafe_descriptor.requires_download = true;
        assert!(!local_embedding_descriptor_is_safe(
            &unsafe_descriptor,
            EmbeddingAvailability::AvailableDevelopment
        ));
    }

    #[test]
    fn validate_embedding_vector_rejects_nan_and_wrong_dims() {
        assert!(validate_embedding_vector(&[0.6, 0.8], 2));
        assert!(!validate_embedding_vector(&[0.0, 0.0], 2));
        assert!(!validate_embedding_vector(&[f32::NAN, 0.1], 2));
        assert!(!validate_embedding_vector(&[1.0], 2));
    }

    #[test]
    fn chunking_is_bounded_and_preserves_offsets() {
        let policy = ChunkingPolicy::default();
        let inputs = bounded_embedding_inputs(&EmbeddingDocument {
            filename: "scan.pdf".to_owned(),
            semantic_fields: vec![("document_type".to_owned(), "invoice".to_owned())],
            identities: vec![("supplier".to_owned(), "Point P".to_owned())],
            extracted_text: "x".repeat(10_000),
        });
        assert!(inputs.len() <= policy.max_chunks_total());
        assert!(inputs.len() >= 2);
        assert!(
            inputs
                .iter()
                .all(|input| input.text.chars().count() <= MAX_EMBEDDING_INPUT_CHARS)
        );
        assert!(inputs.iter().any(|input| input.start_offset.is_some()));
    }

    #[test]
    fn quantized_similarity_keeps_close_vectors_close() {
        let invoice = development_embedding("facture fournisseur Point P", 192);
        let query = development_embedding("invoice Point P", 192);
        let photo = development_embedding("vacances montagne", 192);
        assert!(
            cosine_similarity_quantized(&query, &quantize_unit_vector(&invoice))
                > cosine_similarity_quantized(&query, &quantize_unit_vector(&photo))
        );
    }
}
