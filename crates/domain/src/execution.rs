use crate::{
    ExecutionId, FileFingerprint, NativePath, OperationStepId, OrganizationRevisionId, PlanId,
    ProposalId, ProposalItemId, RootId, ScanId, VolumeIdentity, WorkspaceId,
};
use serde::{Deserialize, Serialize};

pub const EXECUTION_PLAN_MATERIAL_VERSION: u32 = 2;
pub const MAX_EXECUTION_VERIFICATION_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const EXECUTION_SAFETY_POLICY_VERSION: &str = "execution-safety-v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationExecutionStatus {
    Prepared,
    AwaitingConfirmation,
    Approved,
    Running,
    Paused,
    Cancelled,
    Completed,
    Partial,
    Failed,
    RecoveryRequired,
    RecoveryAvailable,
    RecoveryAmbiguous,
    RollingBack,
    RolledBack,
    RollbackPartial,
}

impl OrganizationExecutionStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::Approved => "approved",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::RecoveryRequired => "recovery_required",
            Self::RecoveryAvailable => "recovery_available",
            Self::RecoveryAmbiguous => "recovery_ambiguous",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::RollbackPartial => "rollback_partial",
        }
    }

    #[must_use]
    pub const fn blocks_new_execution(self) -> bool {
        matches!(
            self,
            Self::Prepared
                | Self::AwaitingConfirmation
                | Self::Approved
                | Self::Running
                | Self::Paused
                | Self::RecoveryRequired
                | Self::RecoveryAvailable
                | Self::RecoveryAmbiguous
                | Self::RollingBack
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOperationKind {
    CreateDirectory,
    Move,
    Rename,
    MoveAndRename,
    InternalStage,
}

impl ExecutionOperationKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::CreateDirectory => "create_directory",
            Self::Move => "move",
            Self::Rename => "rename",
            Self::MoveAndRename => "move_and_rename",
            Self::InternalStage => "internal_stage",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOperationStatus {
    Planned,
    PreflightOk,
    Blocked,
    Running,
    Applied,
    Failed,
    Skipped,
    Stale,
    Recovered,
    RollingBack,
    RolledBack,
    RollbackBlocked,
    RollbackFailed,
}

impl ExecutionOperationStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::PreflightOk => "preflight_ok",
            Self::Blocked => "blocked",
            Self::Running => "running",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Stale => "stale",
            Self::Recovered => "recovered",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::RollbackBlocked => "rollback_blocked",
            Self::RollbackFailed => "rollback_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRecoveryState {
    RecoveryNotRequired,
    RecoveryAvailable,
    RecoveryRequired,
    RecoveryAmbiguous,
}

impl ExecutionRecoveryState {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::RecoveryNotRequired => "recovery_not_required",
            Self::RecoveryAvailable => "recovery_available",
            Self::RecoveryRequired => "recovery_required",
            Self::RecoveryAmbiguous => "recovery_ambiguous",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionFailureCategory {
    IsolatedFailure,
    DependencyFailure,
    CriticalExecutionFailure,
}

impl ExecutionFailureCategory {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::IsolatedFailure => "isolated_failure",
            Self::DependencyFailure => "dependency_failure",
            Self::CriticalExecutionFailure => "critical_execution_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionConsentState {
    Pending,
    Attested,
    Consumed,
    Expired,
    Invalidated,
}

impl ExecutionConsentState {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Attested => "attested",
            Self::Consumed => "consumed",
            Self::Expired => "expired",
            Self::Invalidated => "invalidated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRootBinding {
    pub canonical_path: NativePath,
    pub display_path: String,
    pub volume: VolumeIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSafetyPolicyBinding {
    pub version: String,
    pub maximum_rehash_bytes: u64,
    pub allow_qualified_case_only_rename: bool,
    pub digest_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionConsent {
    pub state: ExecutionConsentState,
    pub issued_at_unix_ms: Option<i64>,
    pub expires_at_unix_ms: Option<i64>,
    pub attested_at_unix_ms: Option<i64>,
    pub consumed_at_unix_ms: Option<i64>,
    pub invalidated_at_unix_ms: Option<i64>,
    pub invalidation_reason: Option<String>,
    pub nonce: Option<[u8; 32]>,
    pub attestation_mac: Option<[u8; 32]>,
}

impl ExecutionConsent {
    #[must_use]
    pub const fn pending() -> Self {
        Self {
            state: ExecutionConsentState::Pending,
            issued_at_unix_ms: None,
            expires_at_unix_ms: None,
            attested_at_unix_ms: None,
            consumed_at_unix_ms: None,
            invalidated_at_unix_ms: None,
            invalidation_reason: None,
            nonce: None,
            attestation_mac: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedExecutionPlan {
    pub material_version: u32,
    pub execution_id: ExecutionId,
    pub plan_id: PlanId,
    pub proposal_id: ProposalId,
    pub proposal_revision_id: OrganizationRevisionId,
    pub proposal_revision: u32,
    pub source_snapshot_version: ScanId,
    pub approved_operation_ids: Vec<ProposalItemId>,
    pub operation_count: u64,
    pub destination_root: ExecutionRootBinding,
    pub safety_policy: ExecutionSafetyPolicyBinding,
    pub approval_timestamp: Option<String>,
    pub user_confirmed: bool,
    pub digest_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionOperation {
    pub id: OperationStepId,
    pub execution_id: ExecutionId,
    pub proposal_operation_id: Option<ProposalItemId>,
    pub kind: ExecutionOperationKind,
    pub source_relative_path: Option<String>,
    pub destination_relative_path: String,
    pub original_source_relative_path: Option<String>,
    pub expected_source_hash: Option<String>,
    pub expected_source_size: Option<u64>,
    pub expected_source_modified_at: Option<String>,
    pub live_fingerprint: Option<FileFingerprint>,
    pub post_fingerprint: Option<FileFingerprint>,
    pub preconditions: Vec<String>,
    pub dependencies: Vec<OperationStepId>,
    pub sequence: u32,
    pub status: ExecutionOperationStatus,
    pub directory_existed_before: Option<bool>,
    pub reason: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub rolled_back_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub affected_files: u64,
    pub folders_to_create: u64,
    pub files_to_move: u64,
    pub files_to_rename: u64,
    pub files_unchanged: u64,
    pub conflicts: u64,
    pub needs_review: u64,
    pub preflight_ok: u64,
    pub applied: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub failed: u64,
    pub rolled_back: u64,
    pub rollback_blocked: u64,
    pub rollback_failed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSession {
    pub id: ExecutionId,
    pub plan_id: PlanId,
    pub proposal_id: ProposalId,
    pub proposal_revision_id: OrganizationRevisionId,
    pub proposal_revision: u32,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub source_scan_id: ScanId,
    pub status: OrganizationExecutionStatus,
    pub recovery_state: ExecutionRecoveryState,
    pub plan_digest_hex: String,
    pub approval: ApprovedExecutionPlan,
    pub consent: ExecutionConsent,
    pub summary: ExecutionSummary,
    pub current_operation: Option<String>,
    pub rollback_available: bool,
    pub confirmation_phrase_required: bool,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub rolled_back_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionDetail {
    pub session: ExecutionSession,
    pub operations: Vec<ExecutionOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionProgress {
    pub execution_id: ExecutionId,
    pub status: OrganizationExecutionStatus,
    pub completed: u64,
    pub total: u64,
    pub applied: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub failed: u64,
    pub current: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorSessionPurpose {
    Forward,
    Rollback,
}

impl ExecutorSessionPurpose {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorRequestDirection {
    Forward,
    Rollback,
}

impl ExecutorRequestDirection {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorRequestState {
    IntentDurable,
    AcknowledgedSuccess,
    ProvenNotApplied,
    RecoveryRequired,
    ProvenNotStarted,
    ProvenApplied,
    Ambiguous,
}

#[must_use]
pub fn executor_nonce_hash(nonce: &[u8; 32]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"com.workingname.operation-executor/nonce-hash/v1\0");
    hasher.update(nonce);
    hasher.finalize().to_hex().to_string()
}

impl ExecutorRequestState {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::IntentDurable => "intent_durable",
            Self::AcknowledgedSuccess => "acknowledged_success",
            Self::ProvenNotApplied => "proven_not_applied",
            Self::RecoveryRequired => "recovery_required",
            Self::ProvenNotStarted => "proven_not_started",
            Self::ProvenApplied => "proven_applied",
            Self::Ambiguous => "ambiguous",
        }
    }

    #[must_use]
    pub fn may_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (
                    Self::IntentDurable,
                    Self::AcknowledgedSuccess
                        | Self::ProvenNotApplied
                        | Self::RecoveryRequired
                        | Self::ProvenNotStarted
                        | Self::ProvenApplied
                        | Self::Ambiguous
                ) | (
                    Self::AcknowledgedSuccess,
                    Self::ProvenApplied | Self::RecoveryRequired | Self::Ambiguous
                ) | (
                    Self::RecoveryRequired,
                    Self::ProvenNotStarted | Self::ProvenApplied | Self::Ambiguous
                ) | (
                    Self::ProvenNotApplied,
                    Self::RecoveryRequired | Self::ProvenNotStarted
                ) | (
                    Self::ProvenNotApplied | Self::ProvenNotStarted | Self::ProvenApplied,
                    Self::Ambiguous
                )
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorSessionIdentity {
    pub session_id: String,
    pub execution_id: ExecutionId,
    pub plan_id: PlanId,
    pub plan_digest_hex: String,
    pub purpose: ExecutorSessionPurpose,
    pub coordinator_pid: u32,
    pub child_pid: Option<u32>,
    pub worker_nonce_hash_hex: String,
    pub coordinator_nonce_hash_hex: String,
    pub response_nonce_hash_hex: Option<String>,
    pub opened_at_unix_ms: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExecutorRequestIdentity {
    pub request_id: String,
    pub session_id: String,
    pub execution_id: ExecutionId,
    pub operation_id: OperationStepId,
    pub direction: ExecutorRequestDirection,
    pub request_sequence: u64,
    pub request_nonce: [u8; 32],
    pub request_digest_hex: String,
}

impl std::fmt::Debug for ExecutorRequestIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutorRequestIdentity")
            .field("request_id", &self.request_id)
            .field("session_id", &self.session_id)
            .field("execution_id", &self.execution_id)
            .field("operation_id", &self.operation_id)
            .field("direction", &self.direction)
            .field("request_sequence", &self.request_sequence)
            .field("request_nonce", &"<redacted>")
            .field("request_digest_hex", &self.request_digest_hex)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorSessionFact {
    pub session_id: String,
    pub execution_id: ExecutionId,
    pub plan_id: PlanId,
    pub plan_digest_hex: String,
    pub purpose: ExecutorSessionPurpose,
    pub coordinator_pid: u32,
    pub child_pid: Option<u32>,
    pub worker_nonce_hash_hex: String,
    pub coordinator_nonce_hash_hex: String,
    pub response_nonce_hash_hex: Option<String>,
    pub opened_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorRequestFact {
    pub request_id: String,
    pub session_id: String,
    pub operation_id: OperationStepId,
    pub direction: ExecutorRequestDirection,
    pub request_sequence: u64,
    pub request_nonce_hash_hex: String,
    pub request_digest_hex: String,
    pub intent_event_sequence: u64,
    pub intent_event_digest_hex: String,
    pub state: ExecutorRequestState,
    pub response_digest_hex: Option<String>,
    pub outcome_class: Option<String>,
    pub attempt_count: Option<u8>,
    pub error_class: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalDiagnosticScope {
    Database,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalDiagnostic {
    pub scope: JournalDiagnosticScope,
    pub execution_id: Option<ExecutionId>,
    pub code: String,
    pub message: String,
    pub detected_at_unix_ms: i64,
    pub recovery_available: bool,
    pub rollback_available: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalDiagnosticState {
    pub locked: bool,
    pub diagnostics: Vec<JournalDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryItem {
    pub operation_id: OperationStepId,
    pub direction: ExecutorRequestDirection,
    pub item: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRetentionMetadata {
    pub execution_id: ExecutionId,
    pub finalized_at: Option<String>,
    pub journal_retention_reason: String,
    pub rollback_retention_reason: String,
    pub minimum_retain_until: Option<String>,
    pub active_recovery: bool,
    pub rollback_eligible: bool,
    pub cleanup_eligible_at: Option<String>,
    pub cleanup_eligibility_reason: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAssessment {
    pub execution_id: ExecutionId,
    pub state: ExecutionRecoveryState,
    pub affected_count: u64,
    pub not_started: u64,
    pub applied: u64,
    pub ambiguous: u64,
    pub verified_applied_items: Vec<RecoveryItem>,
    pub verified_not_started_items: Vec<RecoveryItem>,
    pub ambiguous_items: Vec<RecoveryItem>,
    pub rollback_available: bool,
    pub executor_sessions: Vec<ExecutorSessionFact>,
    pub executor_requests: Vec<ExecutorRequestFact>,
    pub journal_diagnostics: JournalDiagnosticState,
    pub message: String,
}
