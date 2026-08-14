use domain::{
    ExecutorRequestDirection, ExecutorRequestIdentity, ExecutorSessionIdentity,
    ExecutorSessionPurpose, OperationStepId,
};
use ipc_contracts::executor_v2::{
    CommittedJournalEventBinding, ExecutorOutcome, ImmutableExecutionEnvelope, OperationDirection,
    SessionAuthorization,
};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Narrow mutation capability for one immutable approved execution envelope.
///
/// Neither this interface nor its session exposes arbitrary filesystem paths.
pub trait ApprovedExecutorClient: Send + Sync {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError>;
}

/// An authenticated child session. Preparing a request reserves its exact
/// identity without dispatching it. The caller must durably bind that identity
/// to both journals before `dispatch_prepared` can cross the process boundary.
pub trait ApprovedExecutorSession: Send {
    fn identity(&self) -> &ExecutorSessionIdentity;

    fn prepare_operation(
        &mut self,
        operation_id: OperationStepId,
        direction: OperationDirection,
    ) -> Result<ExecutorRequestIdentity, ApprovedExecutorError>;

    fn dispatch_prepared(
        &mut self,
        request: ExecutorRequestIdentity,
        journal_intent: CommittedJournalEventBinding,
    ) -> Result<ExecutorDispatchResult, ApprovedExecutorError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorDispatchResult {
    pub outcome: ExecutorOutcome,
    pub response_digest_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovedExecutorError {
    #[error("the isolated approved-operation executor is unavailable: {0}")]
    Unavailable(String),
    #[error("the isolated executor outcome is ambiguous: {0}")]
    Ambiguous(String),
}

#[derive(Debug, Default)]
pub struct UnavailableApprovedExecutorClient;

impl ApprovedExecutorClient for UnavailableApprovedExecutorClient {
    fn open_session(
        &self,
        _envelope: ImmutableExecutionEnvelope,
        _authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        Err(ApprovedExecutorError::Unavailable(
            "native apply is not available on this platform".to_owned(),
        ))
    }
}

pub fn synthetic_executor_session_identity(
    envelope: &ImmutableExecutionEnvelope,
    authorization: &SessionAuthorization,
) -> Result<ExecutorSessionIdentity, ApprovedExecutorError> {
    let session_nonce = fresh_bytes()?;
    let worker_nonce = fresh_bytes()?;
    let coordinator_nonce = fresh_bytes()?;
    Ok(ExecutorSessionIdentity {
        session_id: encode_hex(&session_nonce),
        execution_id: envelope
            .execution_id
            .parse()
            .map_err(|_| ApprovedExecutorError::Unavailable("invalid execution id".to_owned()))?,
        plan_id: envelope
            .plan
            .plan_id
            .parse()
            .map_err(|_| ApprovedExecutorError::Unavailable("invalid plan id".to_owned()))?,
        plan_digest_hex: envelope.plan.digest.to_hex(),
        purpose: match authorization {
            SessionAuthorization::Forward => ExecutorSessionPurpose::Forward,
            SessionAuthorization::Rollback { .. } => ExecutorSessionPurpose::Rollback,
        },
        coordinator_pid: std::process::id(),
        child_pid: None,
        worker_nonce_hash_hex: executor_nonce_hash(&worker_nonce),
        coordinator_nonce_hash_hex: executor_nonce_hash(&coordinator_nonce),
        response_nonce_hash_hex: None,
        opened_at_unix_ms: now_unix_ms()?,
    })
}

pub fn prepare_executor_request_identity(
    session: &ExecutorSessionIdentity,
    operation_id: OperationStepId,
    direction: OperationDirection,
    request_sequence: u64,
    request_nonce: [u8; 32],
) -> Result<ExecutorRequestIdentity, ApprovedExecutorError> {
    if request_sequence == 0 || request_nonce.iter().all(|byte| *byte == 0) {
        return Err(ApprovedExecutorError::Ambiguous(
            "executor request identity is invalid".to_owned(),
        ));
    }
    let direction = match direction {
        OperationDirection::Forward => ExecutorRequestDirection::Forward,
        OperationDirection::Rollback => ExecutorRequestDirection::Rollback,
    };
    let request_id = request_id(session, request_sequence, &request_nonce);
    let material = CanonicalRequestMaterial {
        material_version: 1,
        session_id: &session.session_id,
        execution_id: session.execution_id,
        plan_id: session.plan_id,
        plan_digest_hex: &session.plan_digest_hex,
        operation_id,
        direction,
        request_sequence,
        request_id: &request_id,
        request_nonce,
    };
    let encoded = serde_json::to_vec(&material)
        .map_err(|_| ApprovedExecutorError::Ambiguous("request serialization failed".to_owned()))?;
    Ok(ExecutorRequestIdentity {
        request_id,
        session_id: session.session_id.clone(),
        execution_id: session.execution_id,
        operation_id,
        direction,
        request_sequence,
        request_nonce,
        request_digest_hex: blake3::hash(&encoded).to_hex().to_string(),
    })
}

pub fn executor_response_digest(
    request: &ExecutorRequestIdentity,
    outcome: &ExecutorOutcome,
) -> Result<String, ApprovedExecutorError> {
    let encoded = serde_json::to_vec(&CanonicalResponseMaterial {
        material_version: 1,
        request_id: &request.request_id,
        request_digest_hex: &request.request_digest_hex,
        outcome,
    })
    .map_err(|_| ApprovedExecutorError::Ambiguous("response serialization failed".to_owned()))?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

pub fn fresh_request_nonce() -> Result<[u8; 32], ApprovedExecutorError> {
    fresh_bytes()
}

#[derive(Serialize)]
struct CanonicalRequestMaterial<'a> {
    material_version: u32,
    session_id: &'a str,
    execution_id: domain::ExecutionId,
    plan_id: domain::PlanId,
    plan_digest_hex: &'a str,
    operation_id: OperationStepId,
    direction: ExecutorRequestDirection,
    request_sequence: u64,
    request_id: &'a str,
    request_nonce: [u8; 32],
}

#[derive(Serialize)]
struct CanonicalResponseMaterial<'a> {
    material_version: u32,
    request_id: &'a str,
    request_digest_hex: &'a str,
    outcome: &'a ExecutorOutcome,
}

fn request_id(
    session: &ExecutorSessionIdentity,
    request_sequence: u64,
    request_nonce: &[u8; 32],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"com.workingname.operation-executor/request-id/v1\0");
    hasher.update(session.session_id.as_bytes());
    hasher.update(&request_sequence.to_le_bytes());
    hasher.update(request_nonce);
    hasher.finalize().to_hex().to_string()
}

#[must_use]
pub fn executor_nonce_hash(nonce: &[u8; 32]) -> String {
    domain::executor_nonce_hash(nonce)
}

fn fresh_bytes() -> Result<[u8; 32], ApprovedExecutorError> {
    loop {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        if bytes.iter().any(|byte| *byte != 0) {
            return Ok(bytes);
        }
    }
}

fn now_unix_ms() -> Result<i64, ApprovedExecutorError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?
        .as_millis();
    i64::try_from(millis).map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
