use crate::{ArtifactId, ConsentGrantId, JobId, ModelReleaseId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCapability {
    TextEmbedding,
    ImageEmbedding,
    Ocr,
    VisionDescription,
    StructuredClassification,
    EntityAndFactExtraction,
    RelationInference,
    QueryPlanning,
    Reranking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingLocation {
    LocalBundled,
    LocalExternal,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    Text,
    Image,
    Audio,
    Video,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExecutionPolicy {
    LocalOnly,
    CloudOnce { grant_id: ConsentGrantId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaRef {
    pub id: String,
    pub version: String,
    pub digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentEnvelope<T> {
    pub artifact_id: ArtifactId,
    pub digest: [u8; 32],
    pub media_type: String,
    pub byte_length: u64,
    pub data_class: DataClass,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceRequest<T> {
    pub job_id: JobId,
    pub capability: AiCapability,
    pub input: ContentEnvelope<T>,
    pub output_schema: SchemaRef,
    pub policy: ExecutionPolicy,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProvenance {
    pub job_id: JobId,
    pub pipeline_version: String,
    pub code_version: String,
    pub adapter_version: String,
    pub schema: SchemaRef,
    pub provider_id: String,
    pub model_release_id: ModelReleaseId,
    pub model_name: String,
    pub immutable_revision: String,
    pub model_digest: Option<[u8; 32]>,
    pub location: ProcessingLocation,
    pub prompt_digest: Option<[u8; 32]>,
    pub input_digests: Vec<[u8; 32]>,
    pub config_digest: [u8; 32],
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Success,
    Partial,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceResult<T> {
    pub value: T,
    pub provenance: ModelProvenance,
    pub finish_reason: FinishReason,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub provider_id: String,
    pub model_release_id: ModelReleaseId,
    pub model_name: String,
    pub immutable_revision: String,
    pub location: ProcessingLocation,
    pub capabilities: Vec<AiCapability>,
    pub max_input_bytes: u64,
}
