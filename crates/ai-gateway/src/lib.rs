//! Provider-independent AI capability gateway.

use async_trait::async_trait;
use domain::{
    AiCapability, ContentEnvelope, ExecutionPolicy, FinishReason, InferenceRequest,
    InferenceResult, ModelProvenance, ProcessingLocation, ProviderDescriptor, SchemaRef,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;

use privacy::{ConsentLedger, DisclosureReceipt, EgressRequest, PrivacyError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntrustedText {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub label: String,
    pub raw_score: f32,
    pub evidence_offsets: Vec<[usize; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub entity_type: String,
    pub canonical_name: String,
    pub surface_text: String,
    pub start: usize,
    pub end: usize,
    pub raw_score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedFact {
    pub predicate: String,
    pub value: serde_json::Value,
    pub evidence_offsets: Vec<[usize; 2]>,
    pub raw_score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticExtraction {
    pub classifications: Vec<Classification>,
    pub entities: Vec<ExtractedEntity>,
    pub facts: Vec<ExtractedFact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedItem {
    pub id: String,
    pub score: f32,
}

#[async_trait]
pub trait TextEmbeddingProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn embed(
        &self,
        request: &InferenceRequest<UntrustedText>,
    ) -> Result<InferenceResult<Vec<f32>>, GatewayError>;
}

#[async_trait]
pub trait ImageEmbeddingProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn embed_image(
        &self,
        request: &InferenceRequest<Vec<u8>>,
    ) -> Result<InferenceResult<Vec<f32>>, GatewayError>;
}

#[async_trait]
pub trait StructuredClassificationProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn classify(
        &self,
        request: &InferenceRequest<UntrustedText>,
    ) -> Result<InferenceResult<SemanticExtraction>, GatewayError>;
}

#[async_trait]
pub trait VisionProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn describe(
        &self,
        request: &InferenceRequest<Vec<u8>>,
    ) -> Result<InferenceResult<UntrustedText>, GatewayError>;
}

#[async_trait]
pub trait RerankingProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn rerank(
        &self,
        query: &UntrustedText,
        candidates: &[UntrustedText],
        policy: &ExecutionPolicy,
    ) -> Result<Vec<RankedItem>, GatewayError>;
}

#[async_trait]
pub trait StructuredCloudTransport: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn invoke_json(
        &self,
        body: serde_json::Value,
        output_schema: &SchemaRef,
    ) -> Result<serde_json::Value, GatewayError>;
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("requested capability is unavailable locally")]
    LocalCapabilityUnavailable,
    #[error("cloud use was not explicitly authorized")]
    CloudNotAuthorized,
    #[error("provider does not support the requested capability")]
    UnsupportedCapability,
    #[error("provider output failed schema validation: {0}")]
    InvalidOutput(String),
    #[error("provider failed: {0}")]
    Provider(String),
    #[error("privacy policy denied cloud egress: {0}")]
    Privacy(#[from] PrivacyError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudInference<T> {
    pub value: T,
    pub disclosure: DisclosureReceipt,
    pub provider: ProviderDescriptor,
}

pub struct CloudCapabilityGateway {
    consent: Arc<ConsentLedger>,
    transport: Arc<dyn StructuredCloudTransport>,
}

impl std::fmt::Debug for CloudCapabilityGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CloudCapabilityGateway")
            .field("provider", &self.transport.descriptor().provider_id)
            .finish_non_exhaustive()
    }
}

impl CloudCapabilityGateway {
    #[must_use]
    pub fn new(consent: Arc<ConsentLedger>, transport: Arc<dyn StructuredCloudTransport>) -> Self {
        Self { consent, transport }
    }

    pub async fn invoke_once<T: DeserializeOwned>(
        &self,
        workspace_id: domain::WorkspaceId,
        request: &InferenceRequest<UntrustedText>,
        now_unix_ms: i64,
    ) -> Result<CloudInference<T>, GatewayError> {
        let grant_id = match request.policy {
            ExecutionPolicy::CloudOnce { grant_id } => grant_id,
            ExecutionPolicy::LocalOnly => return Err(GatewayError::CloudNotAuthorized),
        };
        let descriptor = self.transport.descriptor();
        if descriptor.location != ProcessingLocation::Cloud
            || !descriptor.capabilities.contains(&request.capability)
        {
            return Err(GatewayError::UnsupportedCapability);
        }
        if u64::try_from(request.input.payload.text.len()).unwrap_or(u64::MAX)
            > descriptor.max_input_bytes
        {
            return Err(GatewayError::Provider(
                "input exceeds the provider manifest".to_owned(),
            ));
        }
        let disclosure = self.consent.authorize_once(
            grant_id,
            &EgressRequest {
                workspace_id,
                task_id: request.job_id.to_string(),
                request_digest: request.input.digest,
                provider_id: descriptor.provider_id.clone(),
                model_release_id: descriptor.model_release_id,
                artifact_id: request.input.artifact_id,
                artifact_digest: request.input.digest,
                data_class: request.input.data_class,
                byte_count: request.input.byte_length,
                now_unix_ms,
            },
        )?;
        let response = self
            .transport
            .invoke_json(
                serde_json::json!({
                    "capability": request.capability,
                    "content": request.input.payload.text,
                    "schema": request.output_schema,
                }),
                &request.output_schema,
            )
            .await?;
        Ok(CloudInference {
            value: validate_structured_output(response)?,
            disclosure,
            provider: descriptor,
        })
    }
}

#[derive(Debug)]
pub struct LocalHashEmbedding {
    descriptor: ProviderDescriptor,
    dimensions: usize,
}

impl LocalHashEmbedding {
    #[must_use]
    pub fn new(descriptor: ProviderDescriptor, dimensions: usize) -> Self {
        Self {
            descriptor,
            dimensions: dimensions.max(32),
        }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0_f32; self.dimensions];
        for token in text
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
        {
            let digest = blake3::hash(token.to_lowercase().as_bytes());
            let bytes = digest.as_bytes();
            let index = usize::from(u16::from_le_bytes([bytes[0], bytes[1]])) % self.dimensions;
            let sign = if bytes[2] & 1 == 0 { 1.0 } else { -1.0 };
            vector[index] += sign;
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }
        vector
    }
}

#[async_trait]
impl TextEmbeddingProvider for LocalHashEmbedding {
    fn descriptor(&self) -> ProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: &InferenceRequest<UntrustedText>,
    ) -> Result<InferenceResult<Vec<f32>>, GatewayError> {
        if request.capability != AiCapability::TextEmbedding {
            return Err(GatewayError::UnsupportedCapability);
        }
        if !matches!(request.policy, ExecutionPolicy::LocalOnly) {
            return Err(GatewayError::CloudNotAuthorized);
        }
        let now = unix_ms();
        Ok(InferenceResult {
            value: self.vector(&request.input.payload.text),
            provenance: ModelProvenance {
                job_id: request.job_id,
                pipeline_version: "1".to_owned(),
                code_version: env!("CARGO_PKG_VERSION").to_owned(),
                adapter_version: "local-hash-1".to_owned(),
                schema: request.output_schema.clone(),
                provider_id: self.descriptor.provider_id.clone(),
                model_release_id: self.descriptor.model_release_id,
                model_name: self.descriptor.model_name.clone(),
                immutable_revision: self.descriptor.immutable_revision.clone(),
                model_digest: self.descriptor.model_digest(),
                location: ProcessingLocation::LocalBundled,
                prompt_digest: None,
                input_digests: vec![request.input.digest],
                config_digest: *blake3::hash(&self.dimensions.to_le_bytes()).as_bytes(),
                started_at_unix_ms: now,
                finished_at_unix_ms: now,
            },
            finish_reason: FinishReason::Success,
            warnings: vec![
                "embedding déterministe de secours; aucun modèle ML n’a été invoqué".to_owned(),
            ],
        })
    }
}

trait ProviderDescriptorExt {
    fn model_digest(&self) -> Option<[u8; 32]>;
}

impl ProviderDescriptorExt for ProviderDescriptor {
    fn model_digest(&self) -> Option<[u8; 32]> {
        Some(*blake3::hash(self.immutable_revision.as_bytes()).as_bytes())
    }
}

pub fn validate_structured_output<T: DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, GatewayError> {
    serde_json::from_value(value).map_err(|error| GatewayError::InvalidOutput(error.to_string()))
}

#[must_use]
pub fn make_text_envelope(
    artifact_id: domain::ArtifactId,
    media_type: impl Into<String>,
    text: String,
) -> ContentEnvelope<UntrustedText> {
    let digest = *blake3::hash(text.as_bytes()).as_bytes();
    ContentEnvelope {
        artifact_id,
        digest,
        media_type: media_type.into(),
        byte_length: u64::try_from(text.len()).unwrap_or(u64::MAX),
        data_class: domain::DataClass::Text,
        payload: UntrustedText { text },
    }
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ArtifactId, ConsentGrantId, DataClass, JobId, ModelReleaseId, WorkspaceId};
    use privacy::ConsentGrant;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn descriptor() -> ProviderDescriptor {
        ProviderDescriptor {
            provider_id: "builtin".to_owned(),
            model_release_id: ModelReleaseId::new(),
            model_name: "local-hash".to_owned(),
            immutable_revision: "1".to_owned(),
            location: ProcessingLocation::LocalBundled,
            capabilities: vec![AiCapability::TextEmbedding],
            max_input_bytes: 1_000_000,
        }
    }

    #[test]
    fn local_embedding_is_stable_and_normalized() {
        let provider = LocalHashEmbedding::new(descriptor(), 64);
        let first = provider.vector("facture client acme");
        let second = provider.vector("facture client acme");
        assert_eq!(first, second);
        let norm = first.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.0001);
    }

    #[test]
    fn content_envelopes_never_contain_paths() {
        let envelope = make_text_envelope(
            ArtifactId::new(),
            "text/plain",
            "untrusted file content".to_owned(),
        );
        let serialized = serde_json::to_string(&envelope)
            .unwrap_or_else(|error| panic!("serialization should succeed: {error}"));
        assert!(!serialized.contains("path"));
        assert!(!serialized.contains("Users"));
    }

    #[test]
    fn request_types_are_capability_bound() {
        let request = InferenceRequest {
            job_id: JobId::new(),
            capability: AiCapability::TextEmbedding,
            input: make_text_envelope(ArtifactId::new(), "text/plain", "invoice".to_owned()),
            output_schema: SchemaRef {
                id: "embedding".to_owned(),
                version: "1".to_owned(),
                digest: [0; 32],
            },
            policy: ExecutionPolicy::LocalOnly,
            deadline_ms: 1_000,
        };
        assert_eq!(request.capability, AiCapability::TextEmbedding);
    }

    #[derive(Debug)]
    struct MockCloudTransport {
        descriptor: ProviderDescriptor,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl StructuredCloudTransport for MockCloudTransport {
        fn descriptor(&self) -> ProviderDescriptor {
            self.descriptor.clone()
        }

        async fn invoke_json(
            &self,
            _body: serde_json::Value,
            _output_schema: &SchemaRef,
        ) -> Result<serde_json::Value, GatewayError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"accepted": true}))
        }
    }

    #[test]
    fn cloud_transport_requires_an_exact_single_task_grant() {
        let workspace_id = WorkspaceId::new();
        let job_id = JobId::new();
        let model_release_id = ModelReleaseId::new();
        let descriptor = ProviderDescriptor {
            provider_id: "explicit-cloud-test".to_owned(),
            model_release_id,
            model_name: "structured".to_owned(),
            immutable_revision: "2026-08-09".to_owned(),
            location: ProcessingLocation::Cloud,
            capabilities: vec![AiCapability::StructuredClassification],
            max_input_bytes: 1_000,
        };
        let transport = Arc::new(MockCloudTransport {
            descriptor: descriptor.clone(),
            calls: AtomicUsize::new(0),
        });
        let consent = Arc::new(ConsentLedger::default());
        let gateway = CloudCapabilityGateway::new(consent.clone(), transport.clone());
        let envelope = make_text_envelope(
            ArtifactId::new(),
            "text/plain",
            "Facture client ACME".to_owned(),
        );
        let schema = SchemaRef {
            id: "classification".to_owned(),
            version: "1".to_owned(),
            digest: [7; 32],
        };
        let mut request = InferenceRequest {
            job_id,
            capability: AiCapability::StructuredClassification,
            input: envelope,
            output_schema: schema,
            policy: ExecutionPolicy::LocalOnly,
            deadline_ms: 1_000,
        };
        let denied = futures::executor::block_on(gateway.invoke_once::<serde_json::Value>(
            workspace_id,
            &request,
            10,
        ));
        assert!(matches!(denied, Err(GatewayError::CloudNotAuthorized)));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);

        let grant_id = ConsentGrantId::new();
        assert!(
            consent
                .grant(ConsentGrant {
                    id: grant_id,
                    workspace_id,
                    task_id: job_id.to_string(),
                    request_digest: request.input.digest,
                    provider_id: descriptor.provider_id,
                    model_release_id,
                    purpose: "structured_classification".to_owned(),
                    artifact_digests: vec![request.input.digest],
                    allowed_data_classes: vec![DataClass::Text],
                    max_bytes: request.input.byte_length,
                    max_calls: 1,
                    expires_at_unix_ms: 100,
                })
                .is_ok()
        );
        request.policy = ExecutionPolicy::CloudOnce { grant_id };
        let allowed = futures::executor::block_on(gateway.invoke_once::<serde_json::Value>(
            workspace_id,
            &request,
            10,
        ));
        assert!(allowed.is_ok());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        let replay = futures::executor::block_on(gateway.invoke_once::<serde_json::Value>(
            workspace_id,
            &request,
            10,
        ));
        assert!(matches!(
            replay,
            Err(GatewayError::Privacy(PrivacyError::CallBudgetExceeded))
        ));
    }
}
