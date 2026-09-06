use crate::{
    ApplicationError, ApplicationError::InvalidExecution, ApprovedExecutorClient,
    ApprovedExecutorSession, ExecutorDispatchResult,
};
use domain::{
    ApprovedExecutionPlan, ExecutionConsent, ExecutionConsentState, ExecutionDetail,
    ExecutionFailureCategory, ExecutionId, ExecutionOperation, ExecutionOperationKind,
    ExecutionOperationStatus, ExecutionProgress, ExecutionRecoveryState, ExecutionRootBinding,
    ExecutionSafetyPolicyBinding, ExecutionSession, ExecutionSummary, ExecutorRequestDirection,
    ExecutorRequestIdentity, ExecutorRequestState, FileFingerprint, JournalDiagnostic,
    JournalDiagnosticScope, JournalDiagnosticState, JournalEventKind, NativePath,
    OperationJournalEvent, OperationStepId, OrganizationExecutionStatus,
    OrganizationProposalOperation, OrganizationProposalStatus, PathEncoding, PlanId, ProposalId,
    ProposalItemId,
};
use ipc_contracts::executor_v2::{
    CommittedJournalEventBinding, ConsentAttestationBinding, ExecutorAttemptAudit, ExecutorOutcome,
    FixedBytes32, ImmutableExecutionEnvelope, OperationDirection, ProtocolRefusal,
    RollbackEligibility, RollbackEligibilityState, SessionAuthorization,
    derive_consent_authority_key, sign_consent_attestation,
};
use operations::{
    ApplyGate, DurableJournal, ExecutionSafetyPolicy, OperationsError, STAGING_DIRECTORY_NAME,
    SafetyPolicyError,
};
use persistence::Database;
use platform::{FingerprintProgress, PlatformError, ReadOnlyPlatform};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;
use zeroize::Zeroize;

pub const EXECUTION_CONSENT_LIFETIME_MS: i64 = 10 * 60 * 1_000;

pub struct ExecutionApplicationService {
    database: Arc<Database>,
    reader: Arc<dyn ReadOnlyPlatform>,
    executor_client: Arc<dyn ApprovedExecutorClient>,
    journal: Arc<dyn DurableJournal>,
    gate: ApplyGate,
    policy: ExecutionSafetyPolicy,
    consent_authority: ExecutionConsentAuthorityKey,
    journal_diagnostics: RwLock<Vec<JournalDiagnostic>>,
    recovery_in_progress: AtomicBool,
}

impl std::fmt::Debug for ExecutionApplicationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionApplicationService")
            .field("database", &self.database)
            .field("gate", &self.gate)
            .field("policy", &self.policy)
            .field("consent_authority", &"<redacted>")
            .finish_non_exhaustive()
    }
}

pub struct ExecutionConsentAuthorityKey([u8; 32]);

impl ExecutionConsentAuthorityKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn derive(root_authority_key: &[u8; 32]) -> Self {
        Self(derive_consent_authority_key(root_authority_key))
    }
}

impl std::fmt::Debug for ExecutionConsentAuthorityKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ExecutionConsentAuthorityKey(<redacted>)")
    }
}

impl Drop for ExecutionConsentAuthorityKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeExecutionConfirmation {
    pub file_count: u64,
    pub folder_count: u64,
    pub destination_root_display: String,
    pub plan_verification_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionVerificationProgress {
    pub execution_id: ExecutionId,
    pub operation_id: OperationStepId,
    pub bytes_hashed: u64,
    pub total_bytes: u64,
}

pub struct ExecutionConsentChallenge {
    material: ConsentAttestationBinding,
    authenticator: [u8; 32],
    summary: NativeExecutionConfirmation,
}

impl ExecutionConsentChallenge {
    #[must_use]
    pub const fn summary(&self) -> &NativeExecutionConfirmation {
        &self.summary
    }
}

impl std::fmt::Debug for ExecutionConsentChallenge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionConsentChallenge")
            .field("execution_id", &self.material.execution_id)
            .field("expires_at_unix_ms", &self.material.expires_at_unix_ms)
            .field("summary", &self.summary)
            .field("authenticator", &"<redacted>")
            .finish()
    }
}

impl Drop for ExecutionConsentChallenge {
    fn drop(&mut self) {
        self.authenticator.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSystemStatus {
    pub apply_gate: ApplyGate,
    pub recovery_required: bool,
    pub journal_locked: bool,
    pub journal_diagnostics: Vec<JournalDiagnostic>,
}

#[derive(Debug)]
struct PlannedCandidate {
    operation: ExecutionOperation,
    blocked: bool,
}

#[derive(Debug, Clone, Copy)]
struct JournalCursor {
    sequence: u64,
    previous: Option<[u8; 32]>,
}

struct RecoveryGuard<'a>(&'a AtomicBool);

impl Drop for RecoveryGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl RecoveryGuard<'_> {
    fn try_enter(flag: &AtomicBool) -> Option<RecoveryGuard<'_>> {
        if flag.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(RecoveryGuard(flag))
        }
    }
}

impl ExecutionApplicationService {
    pub fn new(
        database: Arc<Database>,
        reader: Arc<dyn ReadOnlyPlatform>,
        executor_client: Arc<dyn ApprovedExecutorClient>,
        journal: Arc<dyn DurableJournal>,
        gate: ApplyGate,
        policy: ExecutionSafetyPolicy,
        consent_authority: ExecutionConsentAuthorityKey,
    ) -> Result<Self, ApplicationError> {
        let service = Self::construct(
            database,
            reader,
            executor_client,
            journal,
            gate,
            policy,
            consent_authority,
        )?;
        service.reconcile_interrupted_executions()?;
        Ok(service)
    }

    #[inline(never)]
    fn construct(
        database: Arc<Database>,
        reader: Arc<dyn ReadOnlyPlatform>,
        executor_client: Arc<dyn ApprovedExecutorClient>,
        journal: Arc<dyn DurableJournal>,
        gate: ApplyGate,
        policy: ExecutionSafetyPolicy,
        consent_authority: ExecutionConsentAuthorityKey,
    ) -> Result<Self, ApplicationError> {
        let initial_diagnostics = journal.diagnostics();
        let service = Self {
            database,
            reader,
            executor_client,
            journal,
            gate,
            policy,
            consent_authority,
            journal_diagnostics: RwLock::new(initial_diagnostics),
            recovery_in_progress: AtomicBool::new(false),
        };
        for execution_id in service.database.execution_ids_with_journal()? {
            match service.database.validate_execution_journal(execution_id) {
                Ok(true) => {}
                Ok(false) => service.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::Database,
                    execution_id: Some(execution_id),
                    code: "database_journal_chain_invalid".to_owned(),
                    message: "The authenticated database execution journal chain is invalid."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                }),
                Err(_) => service.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::Database,
                    execution_id: Some(execution_id),
                    code: "database_journal_unavailable".to_owned(),
                    message: "The authenticated database execution journal is unavailable."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                }),
            }
            if service.journal_is_locked() {
                continue;
            }
            if service
                .synchronize_external_journal(execution_id, false)
                .is_err()
            {
                service.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::External,
                    execution_id: Some(execution_id),
                    code: "external_journal_consistency_failed".to_owned(),
                    message: "The encrypted recovery journal does not match the database journal."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                });
            }
        }
        Ok(service)
    }

    #[inline(never)]
    fn reconcile_interrupted_executions(&self) -> Result<(), ApplicationError> {
        if self.journal_is_locked() {
            return Ok(());
        }
        self.database.mark_interrupted_executions_for_recovery()?;
        for execution_id in self.database.recovery_execution_ids()? {
            // Observation only: never re-enters start_execution / Apply.
            let _ = self.recover_execution(execution_id);
        }
        Ok(())
    }

    pub fn system_status(&self) -> Result<ExecutionSystemStatus, ApplicationError> {
        let diagnostics = self.journal_diagnostic_state();
        let apply_gate = if diagnostics.locked {
            ApplyGate {
                enabled: false,
                reason: "Authenticated execution journal diagnostics are unresolved.".to_owned(),
            }
        } else {
            self.gate.clone()
        };
        Ok(ExecutionSystemStatus {
            apply_gate,
            recovery_required: diagnostics.locked
                || !self.database.recovery_execution_ids()?.is_empty(),
            journal_locked: diagnostics.locked,
            journal_diagnostics: diagnostics.diagnostics,
        })
    }

    pub fn prepare_execution(
        &self,
        proposal_id: ProposalId,
        revision: u32,
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.require_apply_gate()?;
        if self.database.blocking_execution_exists()? {
            return Err(ApplicationError::ExecutionAlreadyActive);
        }
        let proposal = self.database.organization_proposal(proposal_id)?;
        if proposal.status != OrganizationProposalStatus::ApprovedForFutureApply
            || proposal.revision != revision
        {
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        let root = self
            .database
            .root_by_id(proposal.workspace_id, proposal.root_id)?;
        let canonical_root = self.policy.validate_root(&root.absolute_path_native)?;
        let volume = self.reader.inspect_volume(&canonical_root)?;
        if !volume.local || volume.removable {
            return Err(ApplicationError::ExecutionSafety(
                SafetyPolicyError::ProtectedPath,
            ));
        }
        if cfg!(windows)
            && !volume
                .filesystem_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("NTFS"))
        {
            return Err(ApplicationError::Operations(OperationsError::Platform(
                PlatformError::Unsupported(
                    "same-volume apply currently requires a local fixed NTFS volume".to_owned(),
                ),
            )));
        }
        if cfg!(target_os = "macos")
            && !volume.filesystem_type.as_deref().is_some_and(|value| {
                value.eq_ignore_ascii_case("apfs") || value.eq_ignore_ascii_case("hfs")
            })
        {
            return Err(ApplicationError::Operations(OperationsError::Platform(
                PlatformError::Unsupported(
                    "same-volume apply currently requires a local APFS or HFS volume".to_owned(),
                ),
            )));
        }
        let destination_root = ExecutionRootBinding {
            canonical_path: native_path(&canonical_root),
            display_path: canonical_root.to_string_lossy().into_owned(),
            volume: volume.clone(),
        };
        let safety_policy = self
            .policy
            .binding()
            .map_err(|_| ApplicationError::InvalidExecution)?;

        let execution_id = ExecutionId::new();
        let plan_id = PlanId::new();
        let mut candidates = proposal
            .operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation.operation_kind,
                    domain::ProposalOperationKind::MoveProposal
                        | domain::ProposalOperationKind::RenameProposal
                ) || (operation.user_override
                    && operation.operation_kind == domain::ProposalOperationKind::KeepInPlace
                    && is_case_only_proposal(operation))
            })
            .map(|operation| {
                self.preflight_candidate(execution_id, operation, &canonical_root, &volume)
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err(ApplicationError::ExecutionPreflightBlocked);
        }

        detect_plan_collisions(&mut candidates);
        self.resolve_destination_dependencies(&canonical_root, &mut candidates);
        propagate_dependency_blocks(&mut candidates);

        let mut approved_operation_ids = candidates
            .iter()
            .filter(|candidate| !candidate.blocked)
            .filter_map(|candidate| candidate.operation.proposal_operation_id)
            .collect::<Vec<_>>();
        approved_operation_ids.sort_unstable();
        let safe_candidate_count = candidates
            .iter()
            .filter(|candidate| !candidate.blocked)
            .count();
        if safe_candidate_count == 0
            || (!self.policy.allow_independent_safe_subset
                && safe_candidate_count != candidates.len())
        {
            return Err(ApplicationError::ExecutionPreflightBlocked);
        }

        let (ordered, cyclic) = dependency_order(&candidates);
        let mut executable = Vec::new();
        let mut staging_destinations = Vec::new();
        for index in ordered {
            let mut operation = candidates[index].operation.clone();
            if self.policy.allow_qualified_case_only_rename && is_case_only_operation(&operation) {
                let source = operation
                    .source_relative_path
                    .clone()
                    .ok_or(InvalidExecution)?;
                let staging_path =
                    self.unique_staging_path(&canonical_root, execution_id, &staging_destinations)?;
                let stage_id = OperationStepId::new();
                let stage = ExecutionOperation {
                    id: stage_id,
                    execution_id,
                    proposal_operation_id: None,
                    kind: ExecutionOperationKind::InternalStage,
                    source_relative_path: Some(source.clone()),
                    destination_relative_path: staging_path.clone(),
                    original_source_relative_path: Some(source),
                    expected_source_hash: operation.expected_source_hash.clone(),
                    expected_source_size: operation.expected_source_size,
                    expected_source_modified_at: operation.expected_source_modified_at.clone(),
                    live_fingerprint: operation.live_fingerprint.clone(),
                    post_fingerprint: None,
                    preconditions: execution_preconditions(true),
                    dependencies: operation.dependencies.clone(),
                    sequence: 0,
                    status: ExecutionOperationStatus::PreflightOk,
                    directory_existed_before: None,
                    reason: Some("Qualification-only case rename staging transition.".to_owned()),
                    error_code: None,
                    error_message: None,
                    started_at: None,
                    completed_at: None,
                    rolled_back_at: None,
                };
                operation.source_relative_path = Some(staging_path.clone());
                operation.dependencies = vec![stage_id];
                operation.reason =
                    Some("Qualification-only final case rename transition.".to_owned());
                staging_destinations.push(staging_path);
                executable.push(stage);
            }
            executable.push(operation);
        }

        let mut cycle_operations = Vec::new();
        if !cyclic.is_empty() {
            let mut stages = Vec::new();
            for index in cyclic {
                let candidate = &mut candidates[index];
                let source = candidate
                    .operation
                    .source_relative_path
                    .clone()
                    .ok_or(InvalidExecution)?;
                let staging_path =
                    self.unique_staging_path(&canonical_root, execution_id, &staging_destinations)?;
                let stage_id = OperationStepId::new();
                stages.push(ExecutionOperation {
                    id: stage_id,
                    execution_id,
                    proposal_operation_id: None,
                    kind: ExecutionOperationKind::InternalStage,
                    source_relative_path: Some(source.clone()),
                    destination_relative_path: staging_path.clone(),
                    original_source_relative_path: Some(source),
                    expected_source_hash: candidate.operation.expected_source_hash.clone(),
                    expected_source_size: candidate.operation.expected_source_size,
                    expected_source_modified_at: candidate
                        .operation
                        .expected_source_modified_at
                        .clone(),
                    live_fingerprint: candidate.operation.live_fingerprint.clone(),
                    post_fingerprint: None,
                    preconditions: execution_preconditions(true),
                    dependencies: Vec::new(),
                    sequence: 0,
                    status: ExecutionOperationStatus::PreflightOk,
                    directory_existed_before: None,
                    reason: Some("Internal collision-safe staging step.".to_owned()),
                    error_code: None,
                    error_message: None,
                    started_at: None,
                    completed_at: None,
                    rolled_back_at: None,
                });
                candidate.operation.source_relative_path = Some(staging_path.clone());
                staging_destinations.push(staging_path);
                candidate.operation.dependencies = vec![stage_id];
                cycle_operations.push(candidate.operation.clone());
            }
            let all_stage_ids = stages
                .iter()
                .map(|operation| operation.id)
                .collect::<Vec<_>>();
            for operation in &mut cycle_operations {
                operation.dependencies = all_stage_ids.clone();
            }
            executable.extend(stages);
            executable.extend(cycle_operations);
        }

        let user_destinations = executable
            .iter()
            .filter(|operation| operation.kind != ExecutionOperationKind::InternalStage)
            .map(|operation| operation.destination_relative_path.clone())
            .collect::<Vec<_>>();
        let mut all_destinations = user_destinations.clone();
        all_destinations.extend(staging_destinations);
        let (mut directories, user_directory_count) = self.plan_directories(
            execution_id,
            &canonical_root,
            &all_destinations,
            &user_destinations,
        )?;
        let source_cleanup = self.plan_source_directory_cleanup(
            execution_id,
            &canonical_root,
            &executable,
            &user_destinations,
        )?;
        directories.extend(executable);
        directories.extend(source_cleanup);
        directories.extend(
            candidates
                .iter()
                .filter(|candidate| candidate.blocked)
                .map(|candidate| candidate.operation.clone()),
        );
        for (index, operation) in directories.iter_mut().enumerate() {
            operation.sequence =
                u32::try_from(index).map_err(|_| ApplicationError::InvalidExecution)?;
        }

        let summary = execution_summary(
            &proposal,
            &candidates,
            u64::try_from(user_directory_count).map_err(|_| ApplicationError::InvalidExecution)?,
        )?;
        let created_at = execution_now_iso();
        let operation_count =
            u64::try_from(approved_operation_ids.len()).map_err(|_| InvalidExecution)?;
        let digest_hex = plan_digest(
            execution_id,
            plan_id,
            proposal.id,
            proposal.revision_id,
            proposal.revision,
            proposal.source_scan_id,
            &approved_operation_ids,
            operation_count,
            proposal.root_id,
            &destination_root,
            &safety_policy,
            &directories,
        )?;
        let approval = ApprovedExecutionPlan {
            material_version: domain::EXECUTION_PLAN_MATERIAL_VERSION,
            execution_id,
            plan_id,
            proposal_id: proposal.id,
            proposal_revision_id: proposal.revision_id,
            proposal_revision: proposal.revision,
            source_snapshot_version: proposal.source_scan_id,
            approved_operation_ids,
            operation_count,
            destination_root,
            safety_policy,
            approval_timestamp: None,
            user_confirmed: false,
            digest_hex: digest_hex.clone(),
        };
        let detail = ExecutionDetail {
            session: ExecutionSession {
                id: execution_id,
                plan_id,
                proposal_id: proposal.id,
                proposal_revision_id: proposal.revision_id,
                proposal_revision: proposal.revision,
                workspace_id: proposal.workspace_id,
                root_id: proposal.root_id,
                source_scan_id: proposal.source_scan_id,
                status: OrganizationExecutionStatus::AwaitingConfirmation,
                recovery_state: ExecutionRecoveryState::RecoveryNotRequired,
                plan_digest_hex: digest_hex,
                approval,
                consent: ExecutionConsent::pending(),
                summary,
                current_operation: None,
                rollback_available: false,
                confirmation_phrase_required: operation_count
                    >= self.policy.large_batch_confirmation_threshold,
                created_at,
                approved_at: None,
                started_at: None,
                completed_at: None,
                rolled_back_at: None,
                error: None,
            },
            operations: directories,
        };
        verify_plan_digest(&detail)?;
        self.database.persist_prepared_execution(&detail)?;
        self.database
            .execution_detail(execution_id)
            .map_err(Into::into)
    }

    pub fn create_execution_consent_challenge(
        &self,
        execution_id: ExecutionId,
        confirmation_phrase: Option<&str>,
    ) -> Result<ExecutionConsentChallenge, ApplicationError> {
        self.create_execution_consent_challenge_at(
            execution_id,
            confirmation_phrase,
            execution_now_unix_ms(),
        )
    }

    #[doc(hidden)]
    pub fn create_execution_consent_challenge_at(
        &self,
        execution_id: ExecutionId,
        confirmation_phrase: Option<&str>,
        issued_at_unix_ms: i64,
    ) -> Result<ExecutionConsentChallenge, ApplicationError> {
        self.require_apply_gate()?;
        let detail = self.database.execution_detail(execution_id)?;
        self.require_valid_database_journal(execution_id)?;
        self.synchronize_external_journal(execution_id, false)?;
        if detail.session.status != OrganizationExecutionStatus::AwaitingConfirmation
            || !matches!(
                detail.session.consent.state,
                ExecutionConsentState::Pending | ExecutionConsentState::Expired
            )
        {
            return Err(ApplicationError::InvalidExecution);
        }
        self.policy.validate_confirmation(
            detail.session.approval.operation_count,
            true,
            confirmation_phrase,
        )?;
        self.revalidate_consent_context_or_invalidate(&detail)?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| ApplicationError::InvalidExecution)?;
        let expires_at_unix_ms = issued_at_unix_ms
            .checked_add(EXECUTION_CONSENT_LIFETIME_MS)
            .ok_or(ApplicationError::InvalidExecution)?;
        let material =
            consent_attestation_material(&detail, nonce, issued_at_unix_ms, expires_at_unix_ms)?;
        let authenticator = self.consent_authenticator(&material)?;
        let summary = native_confirmation_summary(&detail);
        self.database.issue_execution_consent_challenge(
            execution_id,
            nonce,
            issued_at_unix_ms,
            expires_at_unix_ms,
        )?;
        Ok(ExecutionConsentChallenge {
            material,
            authenticator,
            summary,
        })
    }

    pub fn finalize_execution_consent(
        &self,
        challenge: ExecutionConsentChallenge,
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.finalize_execution_consent_at(challenge, execution_now_unix_ms())
    }

    #[doc(hidden)]
    pub fn finalize_execution_consent_at(
        &self,
        challenge: ExecutionConsentChallenge,
        attested_at_unix_ms: i64,
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.require_apply_gate()?;
        let execution_id = challenge
            .material
            .execution_id
            .parse()
            .map_err(|_| ApplicationError::InvalidExecution)?;
        self.require_valid_database_journal(execution_id)?;
        self.synchronize_external_journal(execution_id, false)?;
        let provided_authenticator = self.consent_authenticator(&challenge.material)?;
        if !constant_time_equal(&provided_authenticator, &challenge.authenticator) {
            return Err(ApplicationError::InvalidExecution);
        }
        if attested_at_unix_ms >= challenge.material.expires_at_unix_ms {
            let _ = self
                .database
                .expire_execution_consent(execution_id, attested_at_unix_ms)?;
            return Err(ApplicationError::ExecutionConsentExpired);
        }
        let detail = self.database.execution_detail(execution_id)?;
        if detail.session.status != OrganizationExecutionStatus::AwaitingConfirmation
            || detail.session.consent.state != ExecutionConsentState::Pending
        {
            return Err(ApplicationError::InvalidExecution);
        }
        self.revalidate_consent_context_or_invalidate(&detail)?;
        let expected_material = consent_attestation_material(
            &detail,
            *challenge.material.consent_nonce.as_bytes(),
            challenge.material.issued_at_unix_ms,
            challenge.material.expires_at_unix_ms,
        )?;
        let expected_authenticator = self.consent_authenticator(&expected_material)?;
        if expected_material != challenge.material
            || !constant_time_equal(&expected_authenticator, &provided_authenticator)
        {
            return Err(ApplicationError::InvalidExecution);
        }
        let detail = self.database.attest_execution_consent(
            execution_id,
            *challenge.material.consent_nonce.as_bytes(),
            challenge.material.issued_at_unix_ms,
            challenge.material.expires_at_unix_ms,
            challenge.authenticator,
            attested_at_unix_ms,
        )?;
        let events = self.database.execution_journal_events(execution_id)?;
        if events.is_empty() {
            let mut cursor = JournalCursor {
                sequence: 0,
                previous: None,
            };
            let payload = json!({
                "event": "approved_durable",
                "plan_id": detail.session.plan_id,
                "plan_digest": detail.session.plan_digest_hex,
                "proposal_id": detail.session.proposal_id,
                "proposal_revision": detail.session.proposal_revision,
                "approved_operation_ids": detail.session.approval.approved_operation_ids,
                "native_confirmation_attested": true,
                "consent_issued_at_unix_ms": challenge.material.issued_at_unix_ms,
                "consent_expires_at_unix_ms": challenge.material.expires_at_unix_ms,
                "plan_verification_code": challenge.summary.plan_verification_code,
                "large_batch_phrase_verified":
                    detail.session.confirmation_phrase_required,
            });
            self.persist_event(
                execution_id,
                None,
                JournalEventKind::ApprovedDurable,
                payload,
                None,
                Some(OrganizationExecutionStatus::Approved),
                None,
                None,
                None,
                &mut cursor,
            )?;
        } else if events.first().map(|event| event.kind) != Some(JournalEventKind::ApprovedDurable)
        {
            return Err(ApplicationError::InvalidExecution);
        } else {
            self.synchronize_external_journal(execution_id, false)?;
        }
        self.database
            .execution_detail(execution_id)
            .map_err(Into::into)
    }

    pub fn discard_execution_consent_challenge(
        &self,
        challenge: ExecutionConsentChallenge,
    ) -> Result<bool, ApplicationError> {
        self.require_mutations_unlocked()?;
        let expected = self.consent_authenticator(&challenge.material)?;
        if !constant_time_equal(&expected, &challenge.authenticator) {
            return Err(ApplicationError::InvalidExecution);
        }
        self.database
            .clear_execution_consent_challenge(
                challenge
                    .material
                    .execution_id
                    .parse()
                    .map_err(|_| ApplicationError::InvalidExecution)?,
                *challenge.material.consent_nonce.as_bytes(),
                challenge.material.issued_at_unix_ms,
                challenge.material.expires_at_unix_ms,
                execution_now_unix_ms(),
            )
            .map_err(Into::into)
    }

    pub fn start_execution(
        &self,
        execution_id: ExecutionId,
        on_progress: &mut dyn FnMut(ExecutionProgress),
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.start_execution_at(execution_id, execution_now_unix_ms(), on_progress)
    }

    #[doc(hidden)]
    #[inline(never)]
    pub fn start_execution_at(
        &self,
        execution_id: ExecutionId,
        authorization_time_unix_ms: i64,
        on_progress: &mut dyn FnMut(ExecutionProgress),
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.require_apply_gate()?;
        let mut detail = self.database.execution_detail(execution_id)?;
        if detail.session.recovery_state != ExecutionRecoveryState::RecoveryNotRequired {
            return Err(ApplicationError::ExecutionRecoveryRequired);
        }
        self.require_valid_database_journal(execution_id)?;
        self.synchronize_external_journal(execution_id, false)?;
        let (root, mut executor_session) = match detail.session.status {
            OrganizationExecutionStatus::Approved => {
                if detail.session.consent.state != ExecutionConsentState::Attested {
                    return Err(ApplicationError::ExecutionApprovalRequired);
                }
                let root = self.revalidate_consent_context_or_invalidate(&detail)?;
                self.verify_stored_consent_attestation(&detail)?;
                let expires_at = detail
                    .session
                    .consent
                    .expires_at_unix_ms
                    .ok_or(ApplicationError::InvalidExecution)?;
                if authorization_time_unix_ms >= expires_at {
                    let _ = self
                        .database
                        .expire_execution_consent(execution_id, authorization_time_unix_ms)?;
                    return Err(ApplicationError::ExecutionConsentExpired);
                }
                let nonce = detail
                    .session
                    .consent
                    .nonce
                    .ok_or(ApplicationError::InvalidExecution)?;
                let issued_at = detail
                    .session
                    .consent
                    .issued_at_unix_ms
                    .ok_or(ApplicationError::InvalidExecution)?;
                let attestation_mac = detail
                    .session
                    .consent
                    .attestation_mac
                    .ok_or(ApplicationError::InvalidExecution)?;
                let envelope = ImmutableExecutionEnvelope::try_from_execution_detail(&detail)
                    .map_err(|_| ApplicationError::InvalidExecution)?;
                let executor_session = self
                    .executor_client
                    .open_session(envelope, SessionAuthorization::Forward)?;
                self.database
                    .persist_executor_session(executor_session.identity())?;
                detail = self.database.consume_execution_consent(
                    execution_id,
                    nonce,
                    issued_at,
                    expires_at,
                    attestation_mac,
                    authorization_time_unix_ms,
                )?;
                (root, executor_session)
            }
            _ => return Err(ApplicationError::InvalidExecution),
        };
        let mut cursor = self.journal_cursor(execution_id)?;
        let mut applied = detail
            .operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation.status,
                    ExecutionOperationStatus::Applied | ExecutionOperationStatus::Recovered
                )
            })
            .map(|operation| operation.id)
            .collect::<HashSet<_>>();
        let schedulable = detail
            .operations
            .iter()
            .filter(|operation| operation.status == ExecutionOperationStatus::PreflightOk)
            .cloned()
            .collect::<Vec<_>>();
        let stage_ids = schedulable
            .iter()
            .filter(|operation| operation.kind == ExecutionOperationKind::InternalStage)
            .map(|operation| operation.id)
            .collect::<HashSet<_>>();
        let cycle_group_ids = schedulable
            .iter()
            .filter(|operation| {
                operation.kind == ExecutionOperationKind::InternalStage
                    || operation
                        .dependencies
                        .iter()
                        .any(|dependency| stage_ids.contains(dependency))
            })
            .map(|operation| operation.id)
            .collect::<HashSet<_>>();
        let mut cycle_group_started = false;
        let total = detail.session.summary.preflight_ok;

        for operation in schedulable {
            let (pause, cancel) = self.database.execution_control(execution_id)?;
            let must_finish_cycle_group =
                cycle_group_started && cycle_group_ids.contains(&operation.id);
            if (cancel || pause) && !must_finish_cycle_group {
                let terminal = if cancel {
                    OrganizationExecutionStatus::Cancelled
                } else {
                    OrganizationExecutionStatus::Paused
                };
                self.finish_execution(
                    execution_id,
                    terminal,
                    if cancel {
                        "Execution cancelled after the current safe unit."
                    } else {
                        "Execution paused between operations."
                    },
                    &mut cursor,
                )?;
                return self
                    .database
                    .execution_detail(execution_id)
                    .map_err(Into::into);
            }
            if operation
                .dependencies
                .iter()
                .any(|dependency| !applied.contains(dependency))
            {
                self.fail_operation(
                    &operation,
                    ExecutionFailureCategory::DependencyFailure,
                    "dependency_not_applied",
                    "A required earlier operation was not applied.",
                    &mut cursor,
                )?;
                return self
                    .database
                    .execution_detail(execution_id)
                    .map_err(Into::into);
            }

            match self.apply_scheduled_operation(
                execution_id,
                &root,
                &operation,
                &mut executor_session,
                &mut cursor,
                total,
                on_progress,
            )? {
                ApplyScheduledOutcome::Applied => {
                    applied.insert(operation.id);
                    if operation.kind == ExecutionOperationKind::InternalStage {
                        cycle_group_started = true;
                    }
                }
                ApplyScheduledOutcome::Terminal(detail) => return Ok(*detail),
            }
        }

        let completed = self.database.execution_detail(execution_id)?;
        let terminal =
            if completed.session.summary.blocked > 0 || completed.session.summary.failed > 0 {
                OrganizationExecutionStatus::Partial
            } else {
                OrganizationExecutionStatus::Completed
            };
        self.finish_execution(
            execution_id,
            terminal,
            "All scheduled safe operations reached a durable terminal state.",
            &mut cursor,
        )?;
        let completed = self.database.execution_detail(execution_id)?;
        on_progress(progress_from_detail(&completed, total, None));
        Ok(completed)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn apply_scheduled_operation(
        &self,
        execution_id: ExecutionId,
        root: &Path,
        operation: &ExecutionOperation,
        executor_session: &mut Box<dyn ApprovedExecutorSession>,
        cursor: &mut JournalCursor,
        total: u64,
        on_progress: &mut dyn FnMut(ExecutionProgress),
    ) -> Result<ApplyScheduledOutcome, ApplicationError> {
        let current = match self.revalidate_operation(root, operation) {
            Ok(value) => value,
            Err(error) => {
                self.fail_operation(
                    operation,
                    ExecutionFailureCategory::CriticalExecutionFailure,
                    "execution_drift",
                    &error.to_string(),
                    cursor,
                )?;
                return self
                    .database
                    .execution_detail(execution_id)
                    .map(|detail| ApplyScheduledOutcome::Terminal(Box::new(detail)))
                    .map_err(Into::into);
            }
        };
        self.persist_event(
            execution_id,
            Some(operation.id),
            JournalEventKind::PreconditionsValidated,
            json!({
                "event": "preconditions_validated",
                "operation_id": operation.id,
                "source": operation.source_relative_path,
                "destination": operation.destination_relative_path,
                "preconditions": operation.preconditions,
                "expected_source_hash": operation.expected_source_hash,
                "expected_source_size": operation.expected_source_size,
                "expected_source_modified_at": operation.expected_source_modified_at,
                "live_fingerprint": operation.live_fingerprint,
            }),
            Some(ExecutionOperationStatus::PreflightOk),
            Some(OrganizationExecutionStatus::Running),
            None,
            None,
            None,
            cursor,
        )?;
        let prepared =
            executor_session.prepare_operation(operation.id, OperationDirection::Forward)?;
        let intent_binding = self.persist_request_intent(
            JournalEventKind::IntentDurable,
            json!({
                "event": "intent_durable",
                "operation_id": operation.id,
                "kind": operation.kind,
                "source": operation.source_relative_path,
                "destination": operation.destination_relative_path,
                "rollback_destination": operation.source_relative_path,
                "original_source": operation.original_source_relative_path,
                "expected_source_state": current,
                "directory_existed_before": operation.directory_existed_before,
            }),
            &prepared,
            executor_session.identity(),
            cursor,
        )?;

        let dispatch = executor_session.dispatch_prepared(prepared.clone(), intent_binding);
        let dispatch = match dispatch {
            Ok(dispatch) => dispatch,
            Err(error) => {
                self.database.transition_executor_request_proof(
                    &prepared.request_id,
                    ExecutorRequestState::RecoveryRequired,
                    &execution_now_iso(),
                )?;
                self.mark_executor_recovery_required(
                    operation,
                    "executor_response_ambiguous",
                    &error.to_string(),
                    None,
                    cursor,
                )?;
                return Err(ApplicationError::ExecutionRecoveryRequired);
            }
        };
        self.record_executor_response(&prepared, &dispatch)?;
        let executor_audit = match dispatch.outcome {
            ExecutorOutcome::Success { audit, .. } => audit,
            ExecutorOutcome::ProvenNotApplied {
                code,
                detail,
                audit,
            } => {
                if let Err(error) = self.revalidate_operation(root, operation) {
                    self.database.transition_executor_request_proof(
                        &prepared.request_id,
                        ExecutorRequestState::RecoveryRequired,
                        &execution_now_iso(),
                    )?;
                    self.mark_executor_recovery_required(
                        operation,
                        "proven_not_applied_contradicted",
                        &error.to_string(),
                        Some(&audit),
                        cursor,
                    )?;
                    return Err(ApplicationError::ExecutionRecoveryRequired);
                }
                self.database.transition_executor_request_proof(
                    &prepared.request_id,
                    ExecutorRequestState::ProvenNotStarted,
                    &execution_now_iso(),
                )?;
                self.fail_operation_with_executor_audit(
                    operation,
                    ExecutionFailureCategory::CriticalExecutionFailure,
                    &code,
                    &detail,
                    &audit,
                    cursor,
                )?;
                return self
                    .database
                    .execution_detail(execution_id)
                    .map(|detail| ApplyScheduledOutcome::Terminal(Box::new(detail)))
                    .map_err(Into::into);
            }
            ExecutorOutcome::ProtocolRefusal {
                refusal: ProtocolRefusal { code, detail, .. },
            } => {
                if let Err(error) = self.revalidate_operation(root, operation) {
                    self.mark_executor_recovery_required(
                        operation,
                        "proven_not_applied_contradicted",
                        &error.to_string(),
                        None,
                        cursor,
                    )?;
                    return Err(ApplicationError::ExecutionRecoveryRequired);
                }
                self.database.transition_executor_request_proof(
                    &prepared.request_id,
                    ExecutorRequestState::ProvenNotStarted,
                    &execution_now_iso(),
                )?;
                self.fail_operation(
                    operation,
                    ExecutionFailureCategory::CriticalExecutionFailure,
                    &code,
                    &detail,
                    cursor,
                )?;
                return self
                    .database
                    .execution_detail(execution_id)
                    .map(|detail| ApplyScheduledOutcome::Terminal(Box::new(detail)))
                    .map_err(Into::into);
            }
            ExecutorOutcome::RecoveryRequired {
                code,
                detail,
                audit,
            } => {
                self.mark_executor_recovery_required(
                    operation,
                    &code,
                    &detail,
                    Some(&audit),
                    cursor,
                )?;
                return Err(ApplicationError::ExecutionRecoveryRequired);
            }
        };
        let post = match self.verify_postcondition(root, operation) {
            Ok(value) => value,
            Err(error) => {
                self.database.transition_executor_request_proof(
                    &prepared.request_id,
                    ExecutorRequestState::RecoveryRequired,
                    &execution_now_iso(),
                )?;
                self.persist_event(
                    execution_id,
                    Some(operation.id),
                    JournalEventKind::StepFailed,
                    json!({
                        "event": "postcondition_failed",
                        "operation_id": operation.id,
                        "error": error.to_string(),
                        "executor_audit": executor_audit,
                    }),
                    // The mutation returned success but its postcondition
                    // is unproven. Recovery, not a terminal failure state,
                    // must classify the observed filesystem state.
                    Some(ExecutionOperationStatus::Running),
                    Some(OrganizationExecutionStatus::RecoveryRequired),
                    None,
                    Some("postcondition_failed"),
                    Some(&error.to_string()),
                    cursor,
                )?;
                return Err(ApplicationError::ExecutionRecoveryRequired);
            }
        };
        self.persist_event_with_request_proof(
            execution_id,
            Some(operation.id),
            JournalEventKind::AppliedObserved,
            json!({
                "event": "applied_observed",
                "operation_id": operation.id,
                "destination": operation.destination_relative_path,
                "postcondition": "verified",
                "executor_audit": executor_audit,
            }),
            Some(ExecutionOperationStatus::Applied),
            Some(OrganizationExecutionStatus::Running),
            post.as_ref(),
            None,
            None,
            &prepared.request_id,
            ExecutorRequestState::ProvenApplied,
            cursor,
        )?;
        let current_detail = self.database.execution_detail(execution_id)?;
        on_progress(progress_from_detail(
            &current_detail,
            total,
            Some(operation.destination_relative_path.clone()),
        ));
        Ok(ApplyScheduledOutcome::Applied)
    }

    pub fn pause_execution(&self, execution_id: ExecutionId) -> Result<bool, ApplicationError> {
        self.require_mutations_unlocked()?;
        self.database
            .request_execution_pause(execution_id)
            .map_err(Into::into)
    }

    pub fn cancel_execution(&self, execution_id: ExecutionId) -> Result<bool, ApplicationError> {
        self.require_mutations_unlocked()?;
        let detail = self.database.execution_detail(execution_id)?;
        match detail.session.status {
            OrganizationExecutionStatus::Running => self
                .database
                .request_execution_cancel(execution_id)
                .map_err(Into::into),
            OrganizationExecutionStatus::AwaitingConfirmation => {
                let _ = self.database.invalidate_execution_consent(
                    execution_id,
                    "execution_cancelled",
                    execution_now_unix_ms(),
                )?;
                self.database
                    .cancel_unstarted_execution(execution_id, &execution_now_iso())
                    .map_err(Into::into)
            }
            OrganizationExecutionStatus::Approved => {
                self.synchronize_external_journal(execution_id, false)?;
                let _ = self.database.invalidate_execution_consent(
                    execution_id,
                    "execution_cancelled",
                    execution_now_unix_ms(),
                )?;
                let mut cursor = self.journal_cursor(execution_id)?;
                self.finish_execution(
                    execution_id,
                    OrganizationExecutionStatus::Cancelled,
                    "Execution cancelled between safe operation units.",
                    &mut cursor,
                )?;
                Ok(true)
            }
            OrganizationExecutionStatus::Paused => {
                self.synchronize_external_journal(execution_id, false)?;
                let mut cursor = self.journal_cursor(execution_id)?;
                self.finish_execution(
                    execution_id,
                    OrganizationExecutionStatus::Cancelled,
                    "Execution cancelled between safe operation units.",
                    &mut cursor,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn execution_status(
        &self,
        execution_id: ExecutionId,
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.database
            .execution_detail(execution_id)
            .map_err(Into::into)
    }

    pub fn execution_history(
        &self,
        workspace_id: domain::WorkspaceId,
        limit: usize,
    ) -> Result<Vec<ExecutionSession>, ApplicationError> {
        self.database
            .execution_history(workspace_id, limit)
            .map_err(Into::into)
    }

    /// Streams a read-only verification of one operation already frozen into
    /// an execution plan. This exposes real coordinator-side byte progress
    /// without widening the mutation executor protocol.
    pub fn verify_approved_source_streaming(
        &self,
        execution_id: ExecutionId,
        operation_id: OperationStepId,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ExecutionVerificationProgress),
    ) -> Result<FileFingerprint, ApplicationError> {
        let detail = self.database.execution_detail(execution_id)?;
        verify_plan_digest(&detail)?;
        if self.policy.binding().map_err(|_| InvalidExecution)?
            != detail.session.approval.safety_policy
        {
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        let operation = detail
            .operations
            .iter()
            .find(|operation| operation.id == operation_id)
            .ok_or(InvalidExecution)?;
        let proposal_id = operation.proposal_operation_id.ok_or(InvalidExecution)?;
        if !detail
            .session
            .approval
            .approved_operation_ids
            .contains(&proposal_id)
        {
            return Err(InvalidExecution);
        }
        let source_relative = operation
            .original_source_relative_path
            .as_deref()
            .or(operation.source_relative_path.as_deref())
            .ok_or(InvalidExecution)?;
        let root = self.execution_root(&detail)?;
        let source = self
            .policy
            .resolve_existing_source(&root, &relative_path(source_relative)?)?;
        let fingerprint = self.reader.fingerprint_streaming(
            &source,
            true,
            self.policy.maximum_rehash_bytes,
            is_cancelled,
            &mut |progress: FingerprintProgress| {
                on_progress(ExecutionVerificationProgress {
                    execution_id,
                    operation_id,
                    bytes_hashed: progress.bytes_hashed,
                    total_bytes: progress.total_bytes,
                });
            },
        )?;
        if !same_file_state(
            operation
                .live_fingerprint
                .as_ref()
                .ok_or(InvalidExecution)?,
            &fingerprint,
        ) {
            return Err(ApplicationError::Operations(OperationsError::Platform(
                PlatformError::Precondition(
                    "approved source changed during streaming verification".to_owned(),
                ),
            )));
        }
        Ok(fingerprint)
    }

    #[inline(never)]
    pub fn recover_execution(
        &self,
        execution_id: ExecutionId,
    ) -> Result<domain::RecoveryAssessment, ApplicationError> {
        let _guard = RecoveryGuard::try_enter(&self.recovery_in_progress)
            .ok_or(ApplicationError::ExecutionAlreadyActive)?;
        self.require_mutations_unlocked()?;
        let detail = self.database.execution_detail(execution_id)?;
        if !matches!(
            detail.session.status,
            OrganizationExecutionStatus::RecoveryRequired
                | OrganizationExecutionStatus::RecoveryAvailable
                | OrganizationExecutionStatus::RecoveryAmbiguous
        ) {
            return Err(ApplicationError::InvalidExecution);
        }
        self.require_valid_database_journal(execution_id)?;
        self.synchronize_external_journal(execution_id, false)?;
        let root = self.execution_root(&detail)?;
        let mut cursor = self.journal_cursor(execution_id)?;
        let events = self.database.execution_journal_events(execution_id)?;
        let executor_sessions = self.database.executor_session_facts(execution_id)?;
        let mut executor_requests = self.database.executor_request_facts(execution_id)?;
        let operations = detail
            .operations
            .iter()
            .map(|operation| (operation.id, operation))
            .collect::<HashMap<_, _>>();
        let mut request_operations = HashSet::new();
        let mut verified_applied_items = Vec::new();
        let mut verified_not_started_items = Vec::new();
        let mut ambiguous_items = Vec::new();

        for request in &executor_requests {
            request_operations.insert((request.operation_id, request.direction));
            let operation = operations
                .get(&request.operation_id)
                .copied()
                .ok_or(ApplicationError::InvalidExecution)?;
            match self.reconcile_one_interrupted_request(
                execution_id,
                &root,
                request,
                operation,
                &executor_sessions,
                &events,
                &mut cursor,
            )? {
                RecoveredRequestClass::Skip => {}
                RecoveredRequestClass::Applied(item) => verified_applied_items.push(item),
                RecoveredRequestClass::NotStarted(item) => verified_not_started_items.push(item),
                RecoveredRequestClass::Ambiguous(item) => ambiguous_items.push(item),
            }
        }

        for operation in &detail.operations {
            if request_operations
                .iter()
                .any(|(operation_id, _)| *operation_id == operation.id)
            {
                continue;
            }
            match operation.status {
                ExecutionOperationStatus::PreflightOk => {
                    verified_not_started_items.push(domain::RecoveryItem {
                        operation_id: operation.id,
                        direction: ExecutorRequestDirection::Forward,
                        item: operation.destination_relative_path.clone(),
                        reason: Some("No durable executor request exists.".to_owned()),
                    });
                }
                ExecutionOperationStatus::Applied | ExecutionOperationStatus::Recovered => {
                    let journal_proven = events.iter().any(|event| {
                        event.step_id == Some(operation.id)
                            && event.kind == JournalEventKind::AppliedObserved
                    });
                    if journal_proven {
                        verified_applied_items.push(domain::RecoveryItem {
                            operation_id: operation.id,
                            direction: ExecutorRequestDirection::Forward,
                            item: operation.destination_relative_path.clone(),
                            reason: Some("Authenticated legacy applied event verified.".to_owned()),
                        });
                    } else {
                        ambiguous_items.push(domain::RecoveryItem {
                            operation_id: operation.id,
                            direction: ExecutorRequestDirection::Forward,
                            item: operation.destination_relative_path.clone(),
                            reason: Some(
                                "Applied state has no authenticated request proof.".to_owned(),
                            ),
                        });
                    }
                }
                ExecutionOperationStatus::Running | ExecutionOperationStatus::RollingBack => {
                    ambiguous_items.push(domain::RecoveryItem {
                        operation_id: operation.id,
                        direction: if operation.status == ExecutionOperationStatus::RollingBack {
                            ExecutorRequestDirection::Rollback
                        } else {
                            ExecutorRequestDirection::Forward
                        },
                        item: operation.destination_relative_path.clone(),
                        reason: Some(
                            "Interrupted operation has no exact executor request identity."
                                .to_owned(),
                        ),
                    });
                }
                _ => {}
            }
        }

        executor_requests = self.database.executor_request_facts(execution_id)?;
        let not_started =
            u64::try_from(verified_not_started_items.len()).map_err(|_| InvalidExecution)?;
        let applied = u64::try_from(verified_applied_items.len()).map_err(|_| InvalidExecution)?;
        let ambiguous = u64::try_from(ambiguous_items.len()).map_err(|_| InvalidExecution)?;
        let affected_count = not_started
            .checked_add(applied)
            .and_then(|value| value.checked_add(ambiguous))
            .ok_or(InvalidExecution)?;
        let state = if ambiguous > 0 {
            ExecutionRecoveryState::RecoveryAmbiguous
        } else {
            ExecutionRecoveryState::RecoveryAvailable
        };
        self.persist_recovery_assessment_result(
            execution_id,
            state,
            not_started,
            applied,
            ambiguous,
            affected_count,
            verified_applied_items,
            verified_not_started_items,
            ambiguous_items,
            executor_sessions,
            executor_requests,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn persist_recovery_assessment_result(
        &self,
        execution_id: ExecutionId,
        state: ExecutionRecoveryState,
        not_started: u64,
        applied: u64,
        ambiguous: u64,
        affected_count: u64,
        verified_applied_items: Vec<domain::RecoveryItem>,
        verified_not_started_items: Vec<domain::RecoveryItem>,
        ambiguous_items: Vec<domain::RecoveryItem>,
        executor_sessions: Vec<domain::ExecutorSessionFact>,
        executor_requests: Vec<domain::ExecutorRequestFact>,
    ) -> Result<domain::RecoveryAssessment, ApplicationError> {
        let observations = Box::new(json!({
            "verified_applied_items": verified_applied_items,
            "verified_not_started_items": verified_not_started_items,
            "ambiguous_items": ambiguous_items,
            "executor_sessions": executor_sessions,
            "executor_requests": executor_requests,
        }));
        self.database.persist_recovery_assessment(
            execution_id,
            state,
            not_started,
            applied,
            ambiguous,
            &serde_json::to_string(observations.as_ref())
                .map_err(|_| ApplicationError::InvalidExecution)?,
            &execution_now_iso(),
        )?;
        let reconciled = self.database.execution_detail(execution_id)?;
        let rollback_available =
            ambiguous == 0 && reconciled.session.rollback_available && applied > 0;
        Ok(domain::RecoveryAssessment {
            execution_id,
            state,
            affected_count,
            not_started,
            applied,
            ambiguous,
            verified_applied_items,
            verified_not_started_items,
            ambiguous_items,
            rollback_available,
            executor_sessions,
            executor_requests,
            journal_diagnostics: self.journal_diagnostic_state(),
            message: if ambiguous > 0 {
                "Recovery is ambiguous; no further mutation is allowed.".to_owned()
            } else if rollback_available {
                "Interrupted requests were reconciled; verified applied operations can be rolled back."
                    .to_owned()
            } else {
                "Interrupted requests were proven not started; forward resume remains unavailable."
                    .to_owned()
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn reconcile_one_interrupted_request(
        &self,
        execution_id: ExecutionId,
        root: &Path,
        request: &domain::ExecutorRequestFact,
        operation: &ExecutionOperation,
        executor_sessions: &[domain::ExecutorSessionFact],
        events: &[OperationJournalEvent],
        cursor: &mut JournalCursor,
    ) -> Result<RecoveredRequestClass, ApplicationError> {
        let item = operation.destination_relative_path.clone();
        if let Some(reason) = executor_request_binding_error(request, executor_sessions, events) {
            self.persist_event_with_request_proof(
                execution_id,
                Some(operation.id),
                JournalEventKind::Conflict,
                json!({
                    "event": "recovery_observed",
                    "operation_id": operation.id,
                    "direction": request.direction,
                    "observation": "ambiguous",
                    "reason": reason,
                    "request_id": request.request_id,
                }),
                None,
                Some(OrganizationExecutionStatus::RecoveryAmbiguous),
                None,
                Some("executor_request_binding_invalid"),
                Some(&reason),
                &request.request_id,
                ExecutorRequestState::Ambiguous,
                cursor,
            )?;
            return Ok(RecoveredRequestClass::Ambiguous(domain::RecoveryItem {
                operation_id: operation.id,
                direction: request.direction,
                item,
                reason: Some(reason),
            }));
        }
        let operation_interrupted = matches!(
            (operation.status, request.direction),
            (
                ExecutionOperationStatus::Running,
                ExecutorRequestDirection::Forward
            ) | (
                ExecutionOperationStatus::RollingBack,
                ExecutorRequestDirection::Rollback
            )
        );
        let interrupted_request = operation_interrupted
            || matches!(
                request.state,
                ExecutorRequestState::IntentDurable
                    | ExecutorRequestState::AcknowledgedSuccess
                    | ExecutorRequestState::RecoveryRequired
                    | ExecutorRequestState::Ambiguous
            )
            || request_has_recovery_observation(events, &request.request_id);
        if !interrupted_request {
            return Ok(RecoveredRequestClass::Skip);
        }

        Ok(match request.state {
            ExecutorRequestState::ProvenNotApplied | ExecutorRequestState::ProvenNotStarted
                if !operation_interrupted =>
            {
                RecoveredRequestClass::NotStarted(domain::RecoveryItem {
                    operation_id: operation.id,
                    direction: request.direction,
                    item,
                    reason: Some("Exact executor request was proven not applied.".to_owned()),
                })
            }
            ExecutorRequestState::ProvenApplied if !operation_interrupted => {
                RecoveredRequestClass::Applied(domain::RecoveryItem {
                    operation_id: operation.id,
                    direction: request.direction,
                    item,
                    reason: Some(
                        "Authenticated journal and postcondition proof recorded.".to_owned(),
                    ),
                })
            }
            ExecutorRequestState::Ambiguous => {
                RecoveredRequestClass::Ambiguous(domain::RecoveryItem {
                    operation_id: operation.id,
                    direction: request.direction,
                    item,
                    reason: Some("Executor request remains ambiguous.".to_owned()),
                })
            }
            ExecutorRequestState::IntentDurable
            | ExecutorRequestState::AcknowledgedSuccess
            | ExecutorRequestState::RecoveryRequired
            | ExecutorRequestState::ProvenNotApplied
            | ExecutorRequestState::ProvenNotStarted
            | ExecutorRequestState::ProvenApplied => {
                let rollback = request.direction == ExecutorRequestDirection::Rollback;
                let observation = if rollback {
                    self.observe_rollback_recovery(root, operation)?
                } else {
                    self.observe_recovery(root, operation)?
                };
                match observation {
                    RecoveryObservation::NotStarted => {
                        if request.state == ExecutorRequestState::ProvenApplied {
                            let reason = "Filesystem reality contradicts the stored applied proof."
                                .to_owned();
                            self.persist_event_with_request_proof(
                                execution_id,
                                Some(operation.id),
                                JournalEventKind::Conflict,
                                json!({
                                    "event": "recovery_observed",
                                    "operation_id": operation.id,
                                    "direction": request.direction,
                                    "observation": "ambiguous",
                                    "reason": reason,
                                    "request_id": request.request_id,
                                }),
                                None,
                                Some(OrganizationExecutionStatus::RecoveryAmbiguous),
                                None,
                                Some("recovery_proof_contradicted"),
                                Some(&reason),
                                &request.request_id,
                                ExecutorRequestState::Ambiguous,
                                cursor,
                            )?;
                            RecoveredRequestClass::Ambiguous(domain::RecoveryItem {
                                operation_id: operation.id,
                                direction: request.direction,
                                item,
                                reason: Some(reason),
                            })
                        } else {
                            let reason =
                                "Exact identity and both paths prove mutation did not start."
                                    .to_owned();
                            self.persist_event_with_request_proof(
                                execution_id,
                                Some(operation.id),
                                JournalEventKind::Conflict,
                                json!({
                                    "event": "recovery_observed",
                                    "operation_id": operation.id,
                                    "direction": request.direction,
                                    "observation": "not_started",
                                    "request_id": request.request_id,
                                }),
                                Some(if rollback {
                                    ExecutionOperationStatus::Applied
                                } else {
                                    ExecutionOperationStatus::PreflightOk
                                }),
                                Some(OrganizationExecutionStatus::RecoveryRequired),
                                None,
                                None,
                                None,
                                &request.request_id,
                                ExecutorRequestState::ProvenNotStarted,
                                cursor,
                            )?;
                            RecoveredRequestClass::NotStarted(domain::RecoveryItem {
                                operation_id: operation.id,
                                direction: request.direction,
                                item,
                                reason: Some(reason),
                            })
                        }
                    }
                    RecoveryObservation::Applied(fingerprint) => {
                        if matches!(
                            request.state,
                            ExecutorRequestState::ProvenNotApplied
                                | ExecutorRequestState::ProvenNotStarted
                        ) {
                            let reason =
                                "Filesystem reality contradicts the stored not-applied proof."
                                    .to_owned();
                            self.persist_event_with_request_proof(
                                execution_id,
                                Some(operation.id),
                                JournalEventKind::Conflict,
                                json!({
                                    "event": "recovery_observed",
                                    "operation_id": operation.id,
                                    "direction": request.direction,
                                    "observation": "ambiguous",
                                    "reason": reason,
                                    "request_id": request.request_id,
                                }),
                                None,
                                Some(OrganizationExecutionStatus::RecoveryAmbiguous),
                                None,
                                Some("recovery_proof_contradicted"),
                                Some(&reason),
                                &request.request_id,
                                ExecutorRequestState::Ambiguous,
                                cursor,
                            )?;
                            RecoveredRequestClass::Ambiguous(domain::RecoveryItem {
                                operation_id: operation.id,
                                direction: request.direction,
                                item,
                                reason: Some(reason),
                            })
                        } else {
                            let reason =
                                "Exact native identity, metadata, and content prove mutation applied."
                                    .to_owned();
                            self.persist_event_with_request_proof(
                                execution_id,
                                Some(operation.id),
                                JournalEventKind::AppliedObserved,
                                json!({
                                    "event": "recovery_observed",
                                    "operation_id": operation.id,
                                    "direction": request.direction,
                                    "observation": "applied",
                                    "request_id": request.request_id,
                                }),
                                Some(if rollback {
                                    ExecutionOperationStatus::RolledBack
                                } else {
                                    ExecutionOperationStatus::Recovered
                                }),
                                Some(OrganizationExecutionStatus::RecoveryRequired),
                                (!rollback).then_some(fingerprint.as_deref()).flatten(),
                                None,
                                None,
                                &request.request_id,
                                ExecutorRequestState::ProvenApplied,
                                cursor,
                            )?;
                            RecoveredRequestClass::Applied(domain::RecoveryItem {
                                operation_id: operation.id,
                                direction: request.direction,
                                item,
                                reason: Some(reason),
                            })
                        }
                    }
                    RecoveryObservation::Ambiguous(reason) => {
                        self.persist_event_with_request_proof(
                            execution_id,
                            Some(operation.id),
                            JournalEventKind::Conflict,
                            json!({
                                "event": "recovery_observed",
                                "operation_id": operation.id,
                                "direction": request.direction,
                                "observation": "ambiguous",
                                "reason": reason,
                                "request_id": request.request_id,
                            }),
                            None,
                            Some(OrganizationExecutionStatus::RecoveryAmbiguous),
                            None,
                            Some("ambiguous_recovery"),
                            Some(&reason),
                            &request.request_id,
                            ExecutorRequestState::Ambiguous,
                            cursor,
                        )?;
                        RecoveredRequestClass::Ambiguous(domain::RecoveryItem {
                            operation_id: operation.id,
                            direction: request.direction,
                            item,
                            reason: Some(reason),
                        })
                    }
                }
            }
        })
    }

    #[inline(never)]
    pub fn rollback_execution(
        &self,
        execution_id: ExecutionId,
        on_progress: &mut dyn FnMut(ExecutionProgress),
    ) -> Result<ExecutionDetail, ApplicationError> {
        self.require_apply_gate()?;
        let detail = self.database.execution_detail(execution_id)?;
        if detail.session.recovery_state == ExecutionRecoveryState::RecoveryAmbiguous
            || !matches!(
                detail.session.status,
                OrganizationExecutionStatus::Completed
                    | OrganizationExecutionStatus::Partial
                    | OrganizationExecutionStatus::Cancelled
                    | OrganizationExecutionStatus::Failed
                    | OrganizationExecutionStatus::RecoveryAvailable
                    | OrganizationExecutionStatus::Paused
            )
        {
            return Err(ApplicationError::ExecutionRecoveryRequired);
        }
        self.require_valid_database_journal(execution_id)?;
        self.synchronize_external_journal(execution_id, false)?;
        let root = self.execution_root(&detail)?;
        let mut cursor = self.journal_cursor(execution_id)?;
        let rollback = detail
            .operations
            .iter()
            .rev()
            .filter(|operation| {
                matches!(
                    operation.status,
                    ExecutionOperationStatus::Applied | ExecutionOperationStatus::Recovered
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let total = u64::try_from(rollback.len()).map_err(|_| InvalidExecution)?;
        if rollback.is_empty() {
            self.finish_execution(
                execution_id,
                OrganizationExecutionStatus::RolledBack,
                "No journal-proven applied operation remained to roll back.",
                &mut cursor,
            )?;
            return self
                .database
                .execution_detail(execution_id)
                .map_err(Into::into);
        }
        let authorization = self.rollback_authorization(&detail, &rollback)?;
        let envelope = ImmutableExecutionEnvelope::try_from_execution_detail_for_rollback(&detail)
            .map_err(|_| ApplicationError::InvalidExecution)?;
        let mut executor_session = self.executor_client.open_session(envelope, authorization)?;
        self.database
            .persist_executor_session(executor_session.identity())?;

        for operation in rollback {
            let preflight = match self.revalidate_rollback(&root, &operation) {
                Ok(value) => value,
                Err(error) => {
                    self.persist_event(
                        execution_id,
                        Some(operation.id),
                        JournalEventKind::Conflict,
                        json!({
                            "event": "rollback_blocked",
                            "operation_id": operation.id,
                            "error": error.to_string(),
                        }),
                        Some(ExecutionOperationStatus::RollbackBlocked),
                        Some(OrganizationExecutionStatus::RollbackPartial),
                        None,
                        Some("rollback_precondition_failed"),
                        Some(&error.to_string()),
                        &mut cursor,
                    )?;
                    self.finish_execution(
                        execution_id,
                        OrganizationExecutionStatus::RollbackPartial,
                        "Rollback stopped because a live precondition was no longer safe.",
                        &mut cursor,
                    )?;
                    return self
                        .database
                        .execution_detail(execution_id)
                        .map_err(Into::into);
                }
            };
            self.persist_event(
                execution_id,
                Some(operation.id),
                JournalEventKind::PreconditionsValidated,
                json!({
                    "event": "rollback_preconditions_validated",
                    "operation_id": operation.id,
                    "source": operation.destination_relative_path,
                    "destination": operation.source_relative_path,
                    "expected_applied_state": preflight,
                }),
                Some(operation.status),
                Some(OrganizationExecutionStatus::RollingBack),
                None,
                None,
                None,
                &mut cursor,
            )?;
            let prepared =
                executor_session.prepare_operation(operation.id, OperationDirection::Rollback)?;
            let intent_binding = self.persist_request_intent(
                JournalEventKind::RollbackIntent,
                json!({
                    "event": "rollback_intent",
                    "operation_id": operation.id,
                    "source": operation.destination_relative_path,
                    "destination": operation.source_relative_path,
                }),
                &prepared,
                executor_session.identity(),
                &mut cursor,
            )?;
            if let Err(error) = self.revalidate_rollback(&root, &operation) {
                self.database.transition_executor_request_proof(
                    &prepared.request_id,
                    ExecutorRequestState::ProvenNotStarted,
                    &execution_now_iso(),
                )?;
                self.persist_event(
                    execution_id,
                    Some(operation.id),
                    JournalEventKind::Conflict,
                    json!({
                        "event": "rollback_blocked",
                        "operation_id": operation.id,
                        "error": error.to_string(),
                        "phase": "immediately_before_worker_dispatch",
                    }),
                    Some(ExecutionOperationStatus::RollbackBlocked),
                    Some(OrganizationExecutionStatus::RollbackPartial),
                    None,
                    Some("rollback_precondition_failed"),
                    Some(&error.to_string()),
                    &mut cursor,
                )?;
                self.finish_execution(
                    execution_id,
                    OrganizationExecutionStatus::RollbackPartial,
                    "Rollback stopped because the exact destination identity changed.",
                    &mut cursor,
                )?;
                return self
                    .database
                    .execution_detail(execution_id)
                    .map_err(Into::into);
            }
            let dispatch = executor_session.dispatch_prepared(prepared.clone(), intent_binding);
            let dispatch = match dispatch {
                Ok(dispatch) => dispatch,
                Err(error) => {
                    self.database.transition_executor_request_proof(
                        &prepared.request_id,
                        ExecutorRequestState::RecoveryRequired,
                        &execution_now_iso(),
                    )?;
                    self.mark_rollback_recovery_required(
                        &operation,
                        "executor_response_ambiguous",
                        &error.to_string(),
                        None,
                        &mut cursor,
                    )?;
                    return Err(ApplicationError::ExecutionRecoveryRequired);
                }
            };
            self.record_executor_response(&prepared, &dispatch)?;
            let executor_audit = match dispatch.outcome {
                ExecutorOutcome::Success { audit, .. } => audit,
                ExecutorOutcome::ProvenNotApplied {
                    code,
                    detail,
                    audit,
                } => {
                    if let Err(error) = self.revalidate_rollback(&root, &operation) {
                        self.database.transition_executor_request_proof(
                            &prepared.request_id,
                            ExecutorRequestState::RecoveryRequired,
                            &execution_now_iso(),
                        )?;
                        self.mark_rollback_recovery_required(
                            &operation,
                            "proven_not_applied_contradicted",
                            &error.to_string(),
                            Some(&audit),
                            &mut cursor,
                        )?;
                        return Err(ApplicationError::ExecutionRecoveryRequired);
                    }
                    self.database.transition_executor_request_proof(
                        &prepared.request_id,
                        ExecutorRequestState::ProvenNotStarted,
                        &execution_now_iso(),
                    )?;
                    self.persist_event(
                        execution_id,
                        Some(operation.id),
                        JournalEventKind::Conflict,
                        json!({
                            "event": "rollback_blocked",
                            "operation_id": operation.id,
                            "code": code,
                            "detail": detail,
                            "executor_audit": audit,
                        }),
                        Some(ExecutionOperationStatus::RollbackBlocked),
                        Some(OrganizationExecutionStatus::RollbackPartial),
                        None,
                        Some(&code),
                        Some(&detail),
                        &mut cursor,
                    )?;
                    self.finish_execution(
                        execution_id,
                        OrganizationExecutionStatus::RollbackPartial,
                        "Rollback stopped after the executor proved no mutation occurred.",
                        &mut cursor,
                    )?;
                    return self
                        .database
                        .execution_detail(execution_id)
                        .map_err(Into::into);
                }
                ExecutorOutcome::ProtocolRefusal {
                    refusal: ProtocolRefusal { code, detail, .. },
                } => {
                    if let Err(error) = self.revalidate_rollback(&root, &operation) {
                        self.mark_rollback_recovery_required(
                            &operation,
                            "proven_not_applied_contradicted",
                            &error.to_string(),
                            None,
                            &mut cursor,
                        )?;
                        return Err(ApplicationError::ExecutionRecoveryRequired);
                    }
                    self.database.transition_executor_request_proof(
                        &prepared.request_id,
                        ExecutorRequestState::ProvenNotStarted,
                        &execution_now_iso(),
                    )?;
                    self.persist_event(
                        execution_id,
                        Some(operation.id),
                        JournalEventKind::Conflict,
                        json!({
                            "event": "rollback_blocked",
                            "operation_id": operation.id,
                            "code": code,
                            "detail": detail,
                        }),
                        Some(ExecutionOperationStatus::RollbackBlocked),
                        Some(OrganizationExecutionStatus::RollbackPartial),
                        None,
                        Some(&code),
                        Some(&detail),
                        &mut cursor,
                    )?;
                    self.finish_execution(
                        execution_id,
                        OrganizationExecutionStatus::RollbackPartial,
                        "Rollback stopped after the executor proved no mutation occurred.",
                        &mut cursor,
                    )?;
                    return self
                        .database
                        .execution_detail(execution_id)
                        .map_err(Into::into);
                }
                ExecutorOutcome::RecoveryRequired {
                    code,
                    detail,
                    audit,
                } => {
                    self.mark_rollback_recovery_required(
                        &operation,
                        &code,
                        &detail,
                        Some(&audit),
                        &mut cursor,
                    )?;
                    return Err(ApplicationError::ExecutionRecoveryRequired);
                }
            };
            if let Err(error) = self.verify_rollback_postcondition(&root, &operation) {
                self.database.transition_executor_request_proof(
                    &prepared.request_id,
                    ExecutorRequestState::RecoveryRequired,
                    &execution_now_iso(),
                )?;
                self.mark_rollback_recovery_required(
                    &operation,
                    "rollback_postcondition_failed",
                    &error.to_string(),
                    Some(&executor_audit),
                    &mut cursor,
                )?;
                return Err(ApplicationError::ExecutionRecoveryRequired);
            }
            self.persist_event_with_request_proof(
                execution_id,
                Some(operation.id),
                JournalEventKind::RolledBackObserved,
                json!({
                    "event": "rolled_back_observed",
                    "operation_id": operation.id,
                    "postcondition": "verified",
                    "executor_audit": executor_audit,
                }),
                Some(ExecutionOperationStatus::RolledBack),
                Some(OrganizationExecutionStatus::RollingBack),
                None,
                None,
                None,
                &prepared.request_id,
                ExecutorRequestState::ProvenApplied,
                &mut cursor,
            )?;
            let current = self.database.execution_detail(execution_id)?;
            on_progress(progress_from_detail(&current, total, None));
        }
        self.finish_execution(
            execution_id,
            OrganizationExecutionStatus::RolledBack,
            "Rollback completed in reverse dependency order.",
            &mut cursor,
        )?;
        let detail = self.database.execution_detail(execution_id)?;
        on_progress(progress_from_detail(&detail, total, None));
        Ok(detail)
    }

    fn consent_authenticator(
        &self,
        material: &ConsentAttestationBinding,
    ) -> Result<[u8; 32], ApplicationError> {
        sign_consent_attestation(material, &self.consent_authority.0)
            .map(|mac| *mac.as_bytes())
            .map_err(|_| ApplicationError::InvalidExecution)
    }

    fn verify_stored_consent_attestation(
        &self,
        detail: &ExecutionDetail,
    ) -> Result<(), ApplicationError> {
        let consent = &detail.session.consent;
        let material = consent_attestation_material(
            detail,
            consent.nonce.ok_or(ApplicationError::InvalidExecution)?,
            consent
                .issued_at_unix_ms
                .ok_or(ApplicationError::InvalidExecution)?,
            consent
                .expires_at_unix_ms
                .ok_or(ApplicationError::InvalidExecution)?,
        )?;
        let expected = self.consent_authenticator(&material)?;
        let stored = consent
            .attestation_mac
            .ok_or(ApplicationError::InvalidExecution)?;
        if !constant_time_equal(&expected, &stored) {
            self.invalidate_unstarted_consent(
                detail.session.id,
                "attestation_authentication_failed",
            )?;
            return Err(ApplicationError::InvalidExecution);
        }
        Ok(())
    }

    fn revalidate_consent_context_or_invalidate(
        &self,
        detail: &ExecutionDetail,
    ) -> Result<PathBuf, ApplicationError> {
        if verify_plan_digest(detail).is_err() {
            self.invalidate_unstarted_consent(detail.session.id, "plan_digest_changed")?;
            return Err(ApplicationError::InvalidExecution);
        }
        if !self
            .database
            .execution_proposal_approval_is_current(detail.session.id)?
        {
            self.invalidate_unstarted_consent(detail.session.id, "proposal_revision_changed")?;
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        let policy_binding = self
            .policy
            .binding()
            .map_err(|_| ApplicationError::InvalidExecution)?;
        if policy_binding != detail.session.approval.safety_policy {
            self.invalidate_unstarted_consent(detail.session.id, "safety_policy_changed")?;
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        let root_record = match self
            .database
            .root_by_id(detail.session.workspace_id, detail.session.root_id)
        {
            Ok(root) => root,
            Err(error) => {
                self.invalidate_unstarted_consent(
                    detail.session.id,
                    "destination_root_unavailable",
                )?;
                return Err(error.into());
            }
        };
        if root_record.id != detail.session.root_id {
            self.invalidate_unstarted_consent(detail.session.id, "destination_root_changed")?;
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        let canonical_root = match self
            .policy
            .validate_root(Path::new(&root_record.absolute_path))
        {
            Ok(root) => root,
            Err(error) => {
                self.invalidate_unstarted_consent(detail.session.id, "destination_root_invalid")?;
                return Err(error.into());
            }
        };
        let volume = match self.reader.inspect_volume(&canonical_root) {
            Ok(volume) => volume,
            Err(error) => {
                self.invalidate_unstarted_consent(
                    detail.session.id,
                    "destination_volume_unavailable",
                )?;
                return Err(error.into());
            }
        };
        let root_binding = ExecutionRootBinding {
            canonical_path: native_path(&canonical_root),
            display_path: canonical_root.to_string_lossy().into_owned(),
            volume,
        };
        if root_binding != detail.session.approval.destination_root {
            self.invalidate_unstarted_consent(
                detail.session.id,
                "destination_root_identity_changed",
            )?;
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        if let Err(error) = self.revalidate_approved_sources(detail, &canonical_root) {
            self.invalidate_unstarted_consent(detail.session.id, "source_fingerprint_changed")?;
            return Err(error);
        }
        Ok(canonical_root)
    }

    fn revalidate_approved_sources(
        &self,
        detail: &ExecutionDetail,
        root: &Path,
    ) -> Result<(), ApplicationError> {
        let approved = detail
            .session
            .approval
            .approved_operation_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut observed = BTreeSet::new();
        for operation in detail.operations.iter().filter(|operation| {
            operation
                .proposal_operation_id
                .is_some_and(|id| approved.contains(&id))
        }) {
            let proposal_operation_id = operation
                .proposal_operation_id
                .ok_or(ApplicationError::InvalidExecution)?;
            if !observed.insert(proposal_operation_id) {
                return Err(ApplicationError::InvalidExecution);
            }
            let source_relative = operation
                .original_source_relative_path
                .as_deref()
                .or(operation.source_relative_path.as_deref())
                .ok_or(ApplicationError::InvalidExecution)?;
            let source = self
                .policy
                .resolve_existing_source(root, &relative_path(source_relative)?)?;
            let current = execution_fingerprint(
                self.reader.as_ref(),
                &source,
                self.policy.maximum_rehash_bytes,
            )?;
            let expected = operation
                .live_fingerprint
                .as_ref()
                .ok_or(ApplicationError::InvalidExecution)?;
            if !same_file_state(expected, &current) {
                return Err(ApplicationError::Operations(OperationsError::Platform(
                    PlatformError::Precondition(
                        "approved source changed after execution plan preparation".to_owned(),
                    ),
                )));
            }
        }
        if observed != approved {
            return Err(ApplicationError::InvalidExecution);
        }
        Ok(())
    }

    fn invalidate_unstarted_consent(
        &self,
        execution_id: ExecutionId,
        reason: &str,
    ) -> Result<(), ApplicationError> {
        if self.database.invalidate_execution_consent(
            execution_id,
            reason,
            execution_now_unix_ms(),
        )? {
            Ok(())
        } else {
            Err(ApplicationError::InvalidExecution)
        }
    }

    fn require_apply_gate(&self) -> Result<(), ApplicationError> {
        self.require_mutations_unlocked()?;
        if self.gate.enabled {
            Ok(())
        } else {
            Err(ApplicationError::Operations(OperationsError::GateDisabled(
                self.gate.reason.clone(),
            )))
        }
    }

    fn require_mutations_unlocked(&self) -> Result<(), ApplicationError> {
        if self.journal_is_locked() {
            Err(ApplicationError::JournalLocked)
        } else {
            Ok(())
        }
    }

    fn require_valid_database_journal(
        &self,
        execution_id: ExecutionId,
    ) -> Result<(), ApplicationError> {
        match self.database.validate_execution_journal(execution_id) {
            Ok(true) => Ok(()),
            Ok(false) => {
                self.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::Database,
                    execution_id: Some(execution_id),
                    code: "database_journal_chain_invalid".to_owned(),
                    message: "The authenticated database execution journal chain is invalid."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                });
                Err(ApplicationError::JournalLocked)
            }
            Err(_) => {
                self.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::Database,
                    execution_id: Some(execution_id),
                    code: "database_journal_unavailable".to_owned(),
                    message: "The authenticated database execution journal is unavailable."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                });
                Err(ApplicationError::JournalLocked)
            }
        }
    }

    fn journal_is_locked(&self) -> bool {
        self.journal_diagnostics
            .read()
            .map_or(true, |diagnostics| !diagnostics.is_empty())
    }

    fn journal_diagnostic_state(&self) -> JournalDiagnosticState {
        match self.journal_diagnostics.read() {
            Ok(diagnostics) => JournalDiagnosticState {
                locked: !diagnostics.is_empty(),
                diagnostics: diagnostics.clone(),
            },
            Err(_) => JournalDiagnosticState {
                locked: true,
                diagnostics: vec![JournalDiagnostic {
                    scope: JournalDiagnosticScope::Database,
                    execution_id: None,
                    code: "journal_diagnostic_state_unavailable".to_owned(),
                    message: "Journal diagnostic state is unavailable; mutation remains locked."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                }],
            },
        }
    }

    fn record_journal_diagnostic(&self, diagnostic: JournalDiagnostic) {
        if let Ok(mut diagnostics) = self.journal_diagnostics.write()
            && !diagnostics.iter().any(|existing| {
                existing.scope == diagnostic.scope
                    && existing.execution_id == diagnostic.execution_id
                    && existing.code == diagnostic.code
            })
        {
            diagnostics.push(diagnostic);
        }
    }

    fn preflight_candidate(
        &self,
        execution_id: ExecutionId,
        proposal: &OrganizationProposalOperation,
        root: &Path,
        root_volume: &domain::VolumeIdentity,
    ) -> PlannedCandidate {
        let source = normalize_relative_string(&proposal.source.relative_path);
        let destination =
            destination_relative(&proposal.proposed_destination, &proposal.proposed_name);
        let kind = classify_operation(&source, &destination);
        let mut operation = ExecutionOperation {
            id: OperationStepId::new(),
            execution_id,
            proposal_operation_id: Some(proposal.id),
            kind,
            source_relative_path: Some(source.clone()),
            destination_relative_path: destination.clone(),
            original_source_relative_path: Some(source.clone()),
            expected_source_hash: proposal.source.content_hash.clone(),
            expected_source_size: Some(proposal.source.byte_size),
            expected_source_modified_at: proposal.source.modified_at.clone(),
            live_fingerprint: None,
            post_fingerprint: None,
            preconditions: execution_preconditions(false),
            dependencies: Vec::new(),
            sequence: 0,
            status: ExecutionOperationStatus::PreflightOk,
            directory_existed_before: None,
            reason: Some(
                proposal
                    .reasons
                    .iter()
                    .map(|reason| reason.explanation.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(1_024)
                    .collect(),
            ),
            error_code: None,
            error_message: None,
            started_at: None,
            completed_at: None,
            rolled_back_at: None,
        };
        let blocked = if proposal.stale {
            block_operation(
                &mut operation,
                true,
                "stale_source",
                "Source snapshot is stale.",
            );
            true
        } else if (proposal.needs_review || proposal.conflict_state.requires_review())
            && !proposal.user_override
        {
            block_operation(
                &mut operation,
                false,
                "proposal_needs_review",
                "The approved proposal operation still requires review.",
            );
            true
        } else if source == destination {
            block_operation(
                &mut operation,
                false,
                "no_effect",
                "The proposed operation has no filesystem effect.",
            );
            true
        } else if path_key(&source) == path_key(&destination)
            && !self.policy.allow_qualified_case_only_rename
        {
            block_operation(
                &mut operation,
                false,
                "case_only_rename_unqualified",
                "Case-only rename is blocked until native behavior is proven safe.",
            );
            true
        } else if let Err(error) = self.policy.validate_destination_components(
            &proposal.proposed_destination,
            &proposal.proposed_name,
        ) {
            block_operation(
                &mut operation,
                false,
                "invalid_destination",
                &error.to_string(),
            );
            true
        } else {
            match self.live_source_fingerprint(root, proposal, root_volume) {
                Ok(fingerprint) => {
                    operation.live_fingerprint = Some(fingerprint);
                    false
                }
                Err((code, message, stale)) => {
                    block_operation(&mut operation, stale, code, &message);
                    true
                }
            }
        };
        PlannedCandidate { operation, blocked }
    }

    fn live_source_fingerprint(
        &self,
        root: &Path,
        proposal: &OrganizationProposalOperation,
        root_volume: &domain::VolumeIdentity,
    ) -> Result<FileFingerprint, (&'static str, String, bool)> {
        let relative = relative_path(&proposal.source.relative_path)
            .map_err(|error| ("invalid_source_path", error.to_string(), true))?;
        let source = self
            .policy
            .resolve_existing_source(root, &relative)
            .map_err(|error| ("source_unavailable", error.to_string(), true))?;
        let fingerprint = self
            .reader
            .fingerprint_streaming(
                &source,
                true,
                self.policy.maximum_rehash_bytes,
                &|| false,
                &mut |_| {},
            )
            .map_err(|error| ("source_fingerprint_failed", error.to_string(), true))?;
        if !fingerprint.stable_for_apply()
            || fingerprint.native_identity.volume.stable_identifier != root_volume.stable_identifier
            || fingerprint.byte_size != proposal.source.byte_size
            || !modified_time_matches(
                proposal.source.modified_at.as_deref(),
                fingerprint.modified_at_ns,
            )
        {
            return Err((
                "source_drift",
                "Source identity, size, or modified time changed after proposal review.".to_owned(),
                true,
            ));
        }
        if let Some(expected_hash) = proposal.source.content_hash.as_deref() {
            let expected = decode_hex_digest(expected_hash).ok_or((
                "invalid_snapshot_hash",
                "The proposal source hash is malformed.".to_owned(),
                true,
            ))?;
            if fingerprint.content_digest != Some(expected) {
                return Err((
                    "source_hash_drift",
                    "Source content hash changed after proposal review.".to_owned(),
                    true,
                ));
            }
        }
        Ok(fingerprint)
    }

    fn resolve_destination_dependencies(&self, root: &Path, candidates: &mut [PlannedCandidate]) {
        let sources = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| !candidate.blocked)
            .filter_map(|(index, candidate)| {
                candidate
                    .operation
                    .source_relative_path
                    .as_deref()
                    .map(|source| (path_key(source), index))
            })
            .collect::<HashMap<_, _>>();
        for index in 0..candidates.len() {
            if candidates[index].blocked {
                continue;
            }
            let destination = candidates[index]
                .operation
                .destination_relative_path
                .clone();
            let relative = match relative_path(&destination) {
                Ok(value) => value,
                Err(error) => {
                    block_operation(
                        &mut candidates[index].operation,
                        false,
                        "invalid_destination",
                        &error.to_string(),
                    );
                    candidates[index].blocked = true;
                    continue;
                }
            };
            let absolute = root.join(&relative);
            match case_insensitive_existing(&absolute) {
                Ok(Some(_)) => {
                    if let Some(occupant) = sources.get(&path_key(&destination)).copied() {
                        let qualified_case_only = occupant == index
                            && self.policy.allow_qualified_case_only_rename
                            && candidates[index]
                                .operation
                                .source_relative_path
                                .as_deref()
                                .is_some_and(|source| {
                                    source != destination
                                        && path_key(source) == path_key(&destination)
                                });
                        if qualified_case_only {
                            continue;
                        }
                        if occupant == index || candidates[occupant].blocked {
                            block_operation(
                                &mut candidates[index].operation,
                                false,
                                "destination_exists",
                                "Destination already exists.",
                            );
                            candidates[index].blocked = true;
                        } else {
                            candidates[index]
                                .operation
                                .dependencies
                                .push(candidates[occupant].operation.id);
                        }
                    } else {
                        block_operation(
                            &mut candidates[index].operation,
                            false,
                            "destination_exists",
                            "Destination already exists and is not vacated by this plan.",
                        );
                        candidates[index].blocked = true;
                    }
                }
                Ok(None) => {
                    if let Err(error) = self
                        .policy
                        .resolve_absent_destination(root, &relative, false)
                    {
                        block_operation(
                            &mut candidates[index].operation,
                            false,
                            "unsafe_destination",
                            &error.to_string(),
                        );
                        candidates[index].blocked = true;
                    }
                }
                Err(error) => {
                    block_operation(
                        &mut candidates[index].operation,
                        false,
                        "destination_inspection_failed",
                        &error.to_string(),
                    );
                    candidates[index].blocked = true;
                }
            }
        }
    }

    fn plan_directories(
        &self,
        execution_id: ExecutionId,
        root: &Path,
        all_destinations: &[String],
        user_destinations: &[String],
    ) -> Result<(Vec<ExecutionOperation>, usize), ApplicationError> {
        let user_prefixes = directory_prefixes(user_destinations)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let all_prefixes = directory_prefixes(all_destinations)?;
        let mut operations = Vec::new();
        let mut user_count = 0_usize;
        for relative_string in all_prefixes {
            let relative = relative_path(&relative_string)?;
            let absolute = root.join(&relative);
            match fs::symlink_metadata(&absolute) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => continue,
                Ok(_) => return Err(ApplicationError::InvalidExecution),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ApplicationError::ExecutionSafety(SafetyPolicyError::Io(
                        error,
                    )));
                }
            }
            let internal = relative_string.starts_with(STAGING_DIRECTORY_NAME);
            self.policy
                .resolve_absent_destination(root, &relative, internal)?;
            if user_prefixes.contains(&relative_string) {
                user_count = user_count.saturating_add(1);
            }
            operations.push(ExecutionOperation {
                id: OperationStepId::new(),
                execution_id,
                proposal_operation_id: None,
                kind: ExecutionOperationKind::CreateDirectory,
                source_relative_path: None,
                destination_relative_path: relative_string,
                original_source_relative_path: None,
                expected_source_hash: None,
                expected_source_size: None,
                expected_source_modified_at: None,
                live_fingerprint: None,
                post_fingerprint: None,
                preconditions: vec![
                    "destination_absent".to_owned(),
                    "parent_inside_approved_root".to_owned(),
                    "no_symlink_or_reparse_traversal".to_owned(),
                    "remove_on_rollback_only_if_empty".to_owned(),
                ],
                dependencies: Vec::new(),
                sequence: 0,
                status: ExecutionOperationStatus::PreflightOk,
                directory_existed_before: Some(false),
                reason: Some(if internal {
                    "Application-controlled staging directory.".to_owned()
                } else {
                    "Approved destination directory required by the frozen plan.".to_owned()
                }),
                error_code: None,
                error_message: None,
                started_at: None,
                completed_at: None,
                rolled_back_at: None,
            });
        }
        Ok((operations, user_count))
    }

    fn plan_source_directory_cleanup(
        &self,
        execution_id: ExecutionId,
        root: &Path,
        executable: &[ExecutionOperation],
        user_destinations: &[String],
    ) -> Result<Vec<ExecutionOperation>, ApplicationError> {
        let planned_sources = executable
            .iter()
            .filter(|operation| {
                operation.proposal_operation_id.is_some()
                    && operation.kind != ExecutionOperationKind::InternalStage
            })
            .filter_map(|operation| {
                operation
                    .original_source_relative_path
                    .as_deref()
                    .or(operation.source_relative_path.as_deref())
            })
            .map(normalize_relative_string)
            .collect::<HashSet<_>>();
        if planned_sources.is_empty() {
            return Ok(Vec::new());
        }
        let destination_paths = user_destinations
            .iter()
            .map(|value| normalize_relative_string(value))
            .collect::<Vec<_>>();
        let mut top_levels = planned_sources
            .iter()
            .filter_map(|source| source.split('/').next())
            .filter(|top| !top.is_empty() && !source_cleanup_top_level_is_protected(top))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        let mut removable = Vec::<String>::new();
        top_levels.retain(|top| {
            let mut discovered = Vec::new();
            let safe = collect_fully_planned_directory_tree(
                root,
                top,
                &planned_sources,
                &destination_paths,
                &mut discovered,
            )
            .unwrap_or(false);
            if safe {
                removable.extend(discovered);
            }
            safe
        });
        removable.sort_by(|left, right| {
            let left_depth = left.split('/').count();
            let right_depth = right.split('/').count();
            right_depth.cmp(&left_depth).then_with(|| left.cmp(right))
        });
        removable.dedup();
        Ok(removable
            .into_iter()
            .map(|relative| ExecutionOperation {
                id: OperationStepId::new(),
                execution_id,
                proposal_operation_id: None,
                kind: ExecutionOperationKind::RemoveDirectoryIfEmpty,
                source_relative_path: Some(relative.clone()),
                destination_relative_path: relative.clone(),
                original_source_relative_path: Some(relative),
                expected_source_hash: None,
                expected_source_size: None,
                expected_source_modified_at: None,
                live_fingerprint: None,
                post_fingerprint: None,
                preconditions: vec![
                    "source_directory_is_unlinked".to_owned(),
                    "source_directory_is_empty_after_planned_moves".to_owned(),
                ],
                dependencies: Vec::new(),
                sequence: 0,
                status: ExecutionOperationStatus::PreflightOk,
                directory_existed_before: Some(true),
                reason: Some(
                    "Remove an emptied source folder so Ranger cleans the visible folder tree."
                        .to_owned(),
                ),
                error_code: None,
                error_message: None,
                started_at: None,
                completed_at: None,
                rolled_back_at: None,
            })
            .collect())
    }

    fn unique_staging_path(
        &self,
        root: &Path,
        execution_id: ExecutionId,
        reserved: &[String],
    ) -> Result<String, ApplicationError> {
        let execution_staging = format!("{STAGING_DIRECTORY_NAME}/{execution_id}");
        for _ in 0..16 {
            let candidate = format!("{execution_staging}/{}", Uuid::now_v7());
            let relative = relative_path(&candidate)?;
            if !reserved
                .iter()
                .any(|existing| path_key(existing) == path_key(&candidate))
                && case_insensitive_existing(&root.join(&relative))?.is_none()
                && self
                    .policy
                    .resolve_absent_destination(root, &relative, true)
                    .is_ok()
            {
                return Ok(candidate);
            }
        }
        Err(ApplicationError::ExecutionPreflightBlocked)
    }

    fn execution_root(&self, detail: &ExecutionDetail) -> Result<PathBuf, ApplicationError> {
        let root = self.database.active_root(detail.session.workspace_id)?;
        if root.id != detail.session.root_id {
            return Err(ApplicationError::ExecutionApprovalRequired);
        }
        self.policy
            .validate_root(&root.absolute_path_native)
            .map_err(Into::into)
    }

    fn revalidate_operation(
        &self,
        root: &Path,
        operation: &ExecutionOperation,
    ) -> Result<Option<FileFingerprint>, ApplicationError> {
        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {
            let source_relative = relative_path(
                operation
                    .source_relative_path
                    .as_deref()
                    .ok_or(ApplicationError::InvalidExecution)?,
            )?;
            let source = root.join(source_relative);
            let metadata = fs::symlink_metadata(&source)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ApplicationError::InvalidExecution);
            }
            let mut entries = fs::read_dir(&source)?;
            if entries.next().transpose()?.is_some() {
                return Err(ApplicationError::Operations(OperationsError::Platform(
                    PlatformError::Precondition(
                        "source directory is not empty after its planned moves".to_owned(),
                    ),
                )));
            }
            return Ok(None);
        }
        let destination_relative = relative_path(&operation.destination_relative_path)?;
        let internal = operation
            .destination_relative_path
            .starts_with(STAGING_DIRECTORY_NAME);
        self.policy
            .resolve_absent_destination(root, &destination_relative, internal)?;
        if operation.kind == ExecutionOperationKind::CreateDirectory {
            return Ok(None);
        }
        let source_relative = relative_path(
            operation
                .source_relative_path
                .as_deref()
                .ok_or(ApplicationError::InvalidExecution)?,
        )?;
        let source = self
            .policy
            .resolve_existing_source(root, &source_relative)?;
        let current = execution_fingerprint(
            self.reader.as_ref(),
            &source,
            self.policy.maximum_rehash_bytes,
        )?;
        let expected = operation
            .live_fingerprint
            .as_ref()
            .ok_or(ApplicationError::InvalidExecution)?;
        if !same_file_state(expected, &current) {
            return Err(ApplicationError::Operations(OperationsError::Platform(
                PlatformError::Precondition(
                    "source changed between preflight and mutation".to_owned(),
                ),
            )));
        }
        Ok(Some(current))
    }

    fn verify_postcondition(
        &self,
        root: &Path,
        operation: &ExecutionOperation,
    ) -> Result<Option<FileFingerprint>, ApplicationError> {
        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {
            let source = root.join(relative_path(
                operation
                    .source_relative_path
                    .as_deref()
                    .ok_or(ApplicationError::InvalidExecution)?,
            )?);
            return match fs::symlink_metadata(source) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(ApplicationError::InvalidExecution),
            };
        }
        let destination = root.join(relative_path(&operation.destination_relative_path)?);
        if operation.kind == ExecutionOperationKind::CreateDirectory {
            let metadata = fs::symlink_metadata(destination)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ApplicationError::InvalidExecution);
            }
            return Ok(None);
        }
        let source = root.join(relative_path(
            operation
                .source_relative_path
                .as_deref()
                .ok_or(ApplicationError::InvalidExecution)?,
        )?);
        if fs::symlink_metadata(source).is_ok() {
            return Err(ApplicationError::InvalidExecution);
        }
        let observed = execution_fingerprint(
            self.reader.as_ref(),
            &destination,
            self.policy.maximum_rehash_bytes,
        )?;
        let expected = operation
            .live_fingerprint
            .as_ref()
            .ok_or(ApplicationError::InvalidExecution)?;
        if !same_file_state(expected, &observed) {
            return Err(ApplicationError::InvalidExecution);
        }
        Ok(Some(observed))
    }

    fn revalidate_rollback(
        &self,
        root: &Path,
        operation: &ExecutionOperation,
    ) -> Result<Option<FileFingerprint>, ApplicationError> {
        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {
            let restore_relative = relative_path(
                operation
                    .source_relative_path
                    .as_deref()
                    .ok_or(ApplicationError::InvalidExecution)?,
            )?;
            self.policy
                .resolve_absent_destination(root, &restore_relative, false)?;
            return Ok(None);
        }
        let current_relative = relative_path(&operation.destination_relative_path)?;
        let current = root.join(&current_relative);
        if operation.kind == ExecutionOperationKind::CreateDirectory {
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    let mut entries = fs::read_dir(&current)?;
                    if entries.next().transpose()?.is_some() {
                        return Err(ApplicationError::Operations(OperationsError::Platform(
                            PlatformError::Precondition(
                                "directory created by this execution is no longer empty".to_owned(),
                            ),
                        )));
                    }
                    return Ok(None);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Ok(_) => return Err(ApplicationError::InvalidExecution),
                Err(error) => return Err(error.into()),
            }
        }
        let current = self
            .policy
            .resolve_existing_source(root, &current_relative)?;
        let fingerprint = execution_fingerprint(
            self.reader.as_ref(),
            &current,
            self.policy.maximum_rehash_bytes,
        )?;
        let expected = operation
            .post_fingerprint
            .as_ref()
            .ok_or(ApplicationError::InvalidExecution)?;
        if !same_file_state(expected, &fingerprint) {
            return Err(ApplicationError::Operations(OperationsError::Platform(
                PlatformError::Precondition(
                    "the applied file changed after execution; rollback is blocked".to_owned(),
                ),
            )));
        }
        let rollback_relative = relative_path(
            operation
                .source_relative_path
                .as_deref()
                .ok_or(ApplicationError::InvalidExecution)?,
        )?;
        let internal = operation
            .source_relative_path
            .as_deref()
            .is_some_and(|value| value.starts_with(STAGING_DIRECTORY_NAME));
        self.policy
            .resolve_absent_destination(root, &rollback_relative, internal)?;
        Ok(Some(fingerprint))
    }

    fn verify_rollback_postcondition(
        &self,
        root: &Path,
        operation: &ExecutionOperation,
    ) -> Result<(), ApplicationError> {
        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {
            let restored = root.join(relative_path(
                operation
                    .source_relative_path
                    .as_deref()
                    .ok_or(ApplicationError::InvalidExecution)?,
            )?);
            let metadata = fs::symlink_metadata(restored)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ApplicationError::InvalidExecution);
            }
            return Ok(());
        }
        let prior_destination = root.join(relative_path(&operation.destination_relative_path)?);
        if operation.kind == ExecutionOperationKind::CreateDirectory {
            if fs::symlink_metadata(prior_destination).is_ok() {
                return Err(ApplicationError::InvalidExecution);
            }
            return Ok(());
        }
        if fs::symlink_metadata(prior_destination).is_ok() {
            return Err(ApplicationError::InvalidExecution);
        }
        let restored = root.join(relative_path(
            operation
                .source_relative_path
                .as_deref()
                .ok_or(ApplicationError::InvalidExecution)?,
        )?);
        let observed = execution_fingerprint(
            self.reader.as_ref(),
            &restored,
            self.policy.maximum_rehash_bytes,
        )?;
        if !same_file_state(
            operation
                .post_fingerprint
                .as_ref()
                .ok_or(ApplicationError::InvalidExecution)?,
            &observed,
        ) {
            return Err(ApplicationError::InvalidExecution);
        }
        Ok(())
    }

    #[inline(never)]
    fn observe_recovery(
        &self,
        root: &Path,
        operation: &ExecutionOperation,
    ) -> Result<RecoveryObservation, ApplicationError> {
        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {
            let source = root.join(relative_path(
                operation
                    .source_relative_path
                    .as_deref()
                    .ok_or(ApplicationError::InvalidExecution)?,
            )?);
            return match fs::symlink_metadata(source) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(RecoveryObservation::Applied(None))
                }
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    Ok(RecoveryObservation::NotStarted)
                }
                Ok(_) => Ok(RecoveryObservation::Ambiguous(
                    "Cleanup source path contains an unexpected entry.".to_owned(),
                )),
                Err(error) => Err(error.into()),
            };
        }
        let destination = root.join(relative_path(&operation.destination_relative_path)?);
        if operation.kind == ExecutionOperationKind::CreateDirectory {
            return match fs::symlink_metadata(destination) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    Ok(RecoveryObservation::Applied(None))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(RecoveryObservation::NotStarted)
                }
                Ok(_) => Ok(RecoveryObservation::Ambiguous(
                    "Directory path contains an unexpected entry.".to_owned(),
                )),
                Err(error) => Err(error.into()),
            };
        }
        let source = root.join(relative_path(
            operation
                .source_relative_path
                .as_deref()
                .ok_or(ApplicationError::InvalidExecution)?,
        )?);
        let expected = operation
            .live_fingerprint
            .as_ref()
            .ok_or(ApplicationError::InvalidExecution)?;
        let source_observed = fingerprint_if_present(
            self.reader.as_ref(),
            &source,
            self.policy.maximum_rehash_bytes,
        )?;
        let destination_observed = fingerprint_if_present(
            self.reader.as_ref(),
            &destination,
            self.policy.maximum_rehash_bytes,
        )?;
        match (source_observed, destination_observed) {
            (Some(source), None) if same_file_state(expected, &source) => {
                Ok(RecoveryObservation::NotStarted)
            }
            (None, Some(destination)) if same_file_state(expected, &destination) => {
                Ok(RecoveryObservation::Applied(Some(Box::new(destination))))
            }
            (Some(_), Some(_)) => Ok(RecoveryObservation::Ambiguous(
                "Both source and destination exist.".to_owned(),
            )),
            (None, None) => Ok(RecoveryObservation::Ambiguous(
                "Neither source nor destination exists.".to_owned(),
            )),
            _ => Ok(RecoveryObservation::Ambiguous(
                "Observed file identity or content does not match the journal.".to_owned(),
            )),
        }
    }

    #[inline(never)]
    fn observe_rollback_recovery(
        &self,
        root: &Path,
        operation: &ExecutionOperation,
    ) -> Result<RecoveryObservation, ApplicationError> {
        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {
            let restored = root.join(relative_path(
                operation
                    .source_relative_path
                    .as_deref()
                    .ok_or(ApplicationError::InvalidExecution)?,
            )?);
            return match fs::symlink_metadata(restored) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    Ok(RecoveryObservation::Applied(None))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(RecoveryObservation::NotStarted)
                }
                Ok(_) => Ok(RecoveryObservation::Ambiguous(
                    "Rollback cleanup path contains an unexpected entry.".to_owned(),
                )),
                Err(error) => Err(error.into()),
            };
        }
        let current = root.join(relative_path(&operation.destination_relative_path)?);
        if operation.kind == ExecutionOperationKind::CreateDirectory {
            return match fs::symlink_metadata(current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    Ok(RecoveryObservation::NotStarted)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(RecoveryObservation::Applied(None))
                }
                Ok(_) => Ok(RecoveryObservation::Ambiguous(
                    "Rollback directory path contains an unexpected entry.".to_owned(),
                )),
                Err(error) => Err(error.into()),
            };
        }
        let restored = root.join(relative_path(
            operation
                .source_relative_path
                .as_deref()
                .ok_or(ApplicationError::InvalidExecution)?,
        )?);
        let expected = operation
            .post_fingerprint
            .as_ref()
            .ok_or(ApplicationError::InvalidExecution)?;
        let current_observed = fingerprint_if_present(
            self.reader.as_ref(),
            &current,
            self.policy.maximum_rehash_bytes,
        )?;
        let restored_observed = fingerprint_if_present(
            self.reader.as_ref(),
            &restored,
            self.policy.maximum_rehash_bytes,
        )?;
        match (current_observed, restored_observed) {
            (Some(current), None) if same_file_state(expected, &current) => {
                Ok(RecoveryObservation::NotStarted)
            }
            (None, Some(restored)) if same_file_state(expected, &restored) => {
                Ok(RecoveryObservation::Applied(Some(Box::new(restored))))
            }
            (Some(_), Some(_)) => Ok(RecoveryObservation::Ambiguous(
                "Both applied and restored paths exist during rollback recovery.".to_owned(),
            )),
            (None, None) => Ok(RecoveryObservation::Ambiguous(
                "Neither applied nor restored path exists during rollback recovery.".to_owned(),
            )),
            _ => Ok(RecoveryObservation::Ambiguous(
                "Rollback file identity or content does not match the journal.".to_owned(),
            )),
        }
    }

    fn rollback_authorization(
        &self,
        detail: &ExecutionDetail,
        rollback: &[ExecutionOperation],
    ) -> Result<SessionAuthorization, ApplicationError> {
        let events = self.database.execution_journal_events(detail.session.id)?;
        let mut eligible_operations = Vec::with_capacity(rollback.len());
        for operation in rollback {
            if !matches!(
                operation.kind,
                ExecutionOperationKind::CreateDirectory
                    | ExecutionOperationKind::RemoveDirectoryIfEmpty
            ) && operation.post_fingerprint.is_none()
            {
                return Err(ApplicationError::InvalidExecution);
            }
            let state = match operation.status {
                ExecutionOperationStatus::Applied => RollbackEligibilityState::Applied,
                ExecutionOperationStatus::Recovered => RollbackEligibilityState::Recovered,
                _ => return Err(ApplicationError::InvalidExecution),
            };
            let event = events
                .iter()
                .rev()
                .find(|event| {
                    event.step_id == Some(operation.id)
                        && event.kind == JournalEventKind::AppliedObserved
                })
                .ok_or(ApplicationError::InvalidExecution)?;
            eligible_operations.push(RollbackEligibility {
                operation_id: operation.id.to_string(),
                state,
                applied_event: CommittedJournalEventBinding {
                    database_sequence: event.sequence,
                    database_event_digest: FixedBytes32::from_bytes(event.event_digest),
                    external_sequence: event.sequence,
                    external_event_digest: FixedBytes32::from_bytes(event.event_digest),
                },
            });
        }
        Ok(SessionAuthorization::Rollback {
            eligible_operations,
        })
    }

    fn mark_executor_recovery_required(
        &self,
        operation: &ExecutionOperation,
        code: &str,
        detail: &str,
        executor_audit: Option<&ExecutorAttemptAudit>,
        cursor: &mut JournalCursor,
    ) -> Result<(), ApplicationError> {
        self.persist_event(
            operation.execution_id,
            Some(operation.id),
            JournalEventKind::StepFailed,
            json!({
                "event": "executor_outcome_ambiguous",
                "direction": "forward",
                "operation_id": operation.id,
                "code": code,
                "detail": detail,
                "executor_audit": executor_audit,
            }),
            Some(ExecutionOperationStatus::Running),
            Some(OrganizationExecutionStatus::RecoveryRequired),
            None,
            Some(code),
            Some(detail),
            cursor,
        )?;
        self.database.add_execution_error(
            operation.execution_id,
            Some(operation.id),
            ExecutionFailureCategory::CriticalExecutionFailure,
            code,
            "The isolated executor outcome could not be proven.",
            Some(detail),
            &execution_now_iso(),
        )?;
        Ok(())
    }

    fn mark_rollback_recovery_required(
        &self,
        operation: &ExecutionOperation,
        code: &str,
        detail: &str,
        executor_audit: Option<&ExecutorAttemptAudit>,
        cursor: &mut JournalCursor,
    ) -> Result<(), ApplicationError> {
        self.persist_event(
            operation.execution_id,
            Some(operation.id),
            JournalEventKind::StepFailed,
            json!({
                "event": "executor_outcome_ambiguous",
                "direction": "rollback",
                "operation_id": operation.id,
                "code": code,
                "detail": detail,
                "executor_audit": executor_audit,
            }),
            Some(ExecutionOperationStatus::RollingBack),
            Some(OrganizationExecutionStatus::RecoveryRequired),
            None,
            Some(code),
            Some(detail),
            cursor,
        )?;
        self.database.add_execution_error(
            operation.execution_id,
            Some(operation.id),
            ExecutionFailureCategory::CriticalExecutionFailure,
            code,
            "The isolated rollback outcome could not be proven.",
            Some(detail),
            &execution_now_iso(),
        )?;
        Ok(())
    }

    fn record_executor_response(
        &self,
        request: &ExecutorRequestIdentity,
        dispatch: &ExecutorDispatchResult,
    ) -> Result<(), ApplicationError> {
        let (outcome_class, state, audit) = match &dispatch.outcome {
            ExecutorOutcome::Success { audit, .. } => (
                "success",
                ExecutorRequestState::AcknowledgedSuccess,
                Some(audit),
            ),
            ExecutorOutcome::ProvenNotApplied { audit, .. } => (
                "proven_not_applied",
                ExecutorRequestState::ProvenNotApplied,
                Some(audit),
            ),
            ExecutorOutcome::RecoveryRequired { audit, .. } => (
                "recovery_required",
                ExecutorRequestState::RecoveryRequired,
                Some(audit),
            ),
            ExecutorOutcome::ProtocolRefusal { .. } => (
                "protocol_refusal",
                ExecutorRequestState::RecoveryRequired,
                None,
            ),
        };
        let attempt_count = audit.map(|value| value.attempt_count);
        let error_class = audit.and_then(|value| value.error_class).map(|value| {
            serde_json::to_value(value)
                .ok()
                .and_then(|encoded| encoded.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "executor_error".to_owned())
        });
        self.database.record_executor_response(
            &request.request_id,
            &dispatch.response_digest_hex,
            outcome_class,
            attempt_count,
            error_class.as_deref(),
            state,
            &execution_now_iso(),
        )?;
        Ok(())
    }

    #[inline(never)]
    fn persist_request_intent(
        &self,
        kind: JournalEventKind,
        mut payload: serde_json::Value,
        request: &ExecutorRequestIdentity,
        session: &domain::ExecutorSessionIdentity,
        cursor: &mut JournalCursor,
    ) -> Result<CommittedJournalEventBinding, ApplicationError> {
        let execution_id = request.execution_id;
        let operation_id = request.operation_id;
        self.require_valid_database_journal(execution_id)?;
        if request.session_id != session.session_id
            || !matches!(
                (kind, request.direction),
                (
                    JournalEventKind::IntentDurable,
                    ExecutorRequestDirection::Forward
                ) | (
                    JournalEventKind::RollbackIntent,
                    ExecutorRequestDirection::Rollback
                )
            )
        {
            return Err(ApplicationError::InvalidExecution);
        }
        let payload_object = payload
            .as_object_mut()
            .ok_or(ApplicationError::InvalidExecution)?;
        payload_object.insert(
            "executor_session".to_owned(),
            json!({
                "session_id": session.session_id,
                "execution_id": session.execution_id,
                "plan_id": session.plan_id,
                "plan_digest": session.plan_digest_hex,
                "purpose": session.purpose,
                "coordinator_pid": session.coordinator_pid,
                "child_pid": session.child_pid,
                "worker_nonce_hash": session.worker_nonce_hash_hex,
                "coordinator_nonce_hash": session.coordinator_nonce_hash_hex,
                "response_nonce_hash": session.response_nonce_hash_hex,
            }),
        );
        payload_object.insert(
            "executor_request".to_owned(),
            json!({
                "request_id": request.request_id,
                "session_id": request.session_id,
                "operation_id": request.operation_id,
                "direction": request.direction,
                "request_sequence": request.request_sequence,
                "request_nonce": encode_hex_bytes(&request.request_nonce),
                "request_digest": request.request_digest_hex,
            }),
        );
        let canonical =
            serde_json::to_string(&payload).map_err(|_| ApplicationError::InvalidExecution)?;
        let now_ms = execution_now_unix_ms();
        let event = OperationJournalEvent::new(
            execution_id,
            cursor.sequence,
            Some(operation_id),
            kind,
            canonical.as_bytes(),
            cursor.previous,
            now_ms,
        );
        let now = execution_now_iso();
        let (operation_status, session_status) = match kind {
            JournalEventKind::IntentDurable => (
                ExecutionOperationStatus::Running,
                OrganizationExecutionStatus::Running,
            ),
            JournalEventKind::RollbackIntent => (
                ExecutionOperationStatus::RollingBack,
                OrganizationExecutionStatus::RollingBack,
            ),
            _ => return Err(ApplicationError::InvalidExecution),
        };
        self.database.persist_executor_request_intent(
            &event,
            &canonical,
            operation_status,
            session_status,
            request,
            &now,
        )?;
        if let Err(error) = self.synchronize_external_journal(execution_id, true) {
            let _ = self.database.mark_execution_recovery_required(
                execution_id,
                "The durable request intent could not be committed to both journals; recovery review is required.",
            );
            return Err(error);
        }
        cursor.sequence = cursor.sequence.saturating_add(1);
        cursor.previous = Some(event.event_digest);
        Ok(CommittedJournalEventBinding {
            database_sequence: event.sequence,
            database_event_digest: FixedBytes32::from_bytes(event.event_digest),
            external_sequence: event.sequence,
            external_event_digest: FixedBytes32::from_bytes(event.event_digest),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn persist_event_with_request_proof(
        &self,
        execution_id: ExecutionId,
        operation_id: Option<OperationStepId>,
        kind: JournalEventKind,
        payload: serde_json::Value,
        operation_status: Option<ExecutionOperationStatus>,
        session_status: Option<OrganizationExecutionStatus>,
        post_fingerprint: Option<&FileFingerprint>,
        error_code: Option<&str>,
        error_message: Option<&str>,
        request_id: &str,
        proof_state: ExecutorRequestState,
        cursor: &mut JournalCursor,
    ) -> Result<CommittedJournalEventBinding, ApplicationError> {
        self.require_valid_database_journal(execution_id)?;
        let canonical =
            serde_json::to_string(&payload).map_err(|_| ApplicationError::InvalidExecution)?;
        let now_ms = execution_now_unix_ms();
        let event = OperationJournalEvent::new(
            execution_id,
            cursor.sequence,
            operation_id,
            kind,
            canonical.as_bytes(),
            cursor.previous,
            now_ms,
        );
        let now = execution_now_iso();
        self.database.persist_execution_event_with_request_proof(
            &event,
            &canonical,
            operation_status,
            session_status,
            post_fingerprint,
            error_code,
            error_message,
            &now,
            request_id,
            proof_state,
        )?;
        if let Err(error) = self.synchronize_external_journal(execution_id, true) {
            let _ = self.database.mark_execution_recovery_required(
                execution_id,
                "The durable external journal could not be synchronized; recovery review is required.",
            );
            return Err(error);
        }
        cursor.sequence = cursor.sequence.saturating_add(1);
        cursor.previous = Some(event.event_digest);
        Ok(CommittedJournalEventBinding {
            database_sequence: event.sequence,
            database_event_digest: FixedBytes32::from_bytes(event.event_digest),
            external_sequence: event.sequence,
            external_event_digest: FixedBytes32::from_bytes(event.event_digest),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn persist_event(
        &self,
        execution_id: ExecutionId,
        operation_id: Option<OperationStepId>,
        kind: JournalEventKind,
        payload: serde_json::Value,
        operation_status: Option<ExecutionOperationStatus>,
        session_status: Option<OrganizationExecutionStatus>,
        post_fingerprint: Option<&FileFingerprint>,
        error_code: Option<&str>,
        error_message: Option<&str>,
        cursor: &mut JournalCursor,
    ) -> Result<CommittedJournalEventBinding, ApplicationError> {
        self.require_valid_database_journal(execution_id)?;
        let canonical =
            serde_json::to_string(&payload).map_err(|_| ApplicationError::InvalidExecution)?;
        let now_ms = execution_now_unix_ms();
        let event = OperationJournalEvent::new(
            execution_id,
            cursor.sequence,
            operation_id,
            kind,
            canonical.as_bytes(),
            cursor.previous,
            now_ms,
        );
        let now = execution_now_iso();
        self.database.persist_execution_event(
            &event,
            &canonical,
            operation_status,
            session_status,
            post_fingerprint,
            error_code,
            error_message,
            &now,
        )?;
        if let Err(error) = self.synchronize_external_journal(execution_id, true) {
            let _ = self.database.mark_execution_recovery_required(
                execution_id,
                "The durable external journal could not be synchronized; recovery review is required.",
            );
            return Err(error);
        }
        cursor.sequence = cursor.sequence.saturating_add(1);
        cursor.previous = Some(event.event_digest);
        Ok(CommittedJournalEventBinding {
            database_sequence: event.sequence,
            database_event_digest: FixedBytes32::from_bytes(event.event_digest),
            external_sequence: event.sequence,
            external_event_digest: FixedBytes32::from_bytes(event.event_digest),
        })
    }

    fn synchronize_external_journal(
        &self,
        execution_id: ExecutionId,
        allow_single_append: bool,
    ) -> Result<(), ApplicationError> {
        let database_events = self.database.execution_journal_events(execution_id)?;
        let external = match self.journal.events(execution_id) {
            Ok(events) => events,
            Err(_error) => {
                self.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::External,
                    execution_id: Some(execution_id),
                    code: "external_journal_unavailable".to_owned(),
                    message: "The encrypted recovery journal became unavailable.".to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                });
                return Err(ApplicationError::JournalLocked);
            }
        };
        if external.len() > database_events.len()
            || !external
                .iter()
                .zip(&database_events)
                .all(|(left, right)| left == right)
        {
            self.record_journal_diagnostic(JournalDiagnostic {
                scope: JournalDiagnosticScope::External,
                execution_id: Some(execution_id),
                code: "external_journal_consistency_failed".to_owned(),
                message: "The encrypted recovery journal does not match the database journal."
                    .to_owned(),
                detected_at_unix_ms: execution_now_unix_ms(),
                recovery_available: false,
                rollback_available: false,
            });
            return Err(ApplicationError::JournalLocked);
        }
        let missing_events = database_events.len().saturating_sub(external.len());
        if missing_events > 0 && (!allow_single_append || missing_events != 1) {
            self.record_journal_diagnostic(JournalDiagnostic {
                scope: JournalDiagnosticScope::External,
                execution_id: Some(execution_id),
                code: "external_journal_incomplete".to_owned(),
                message: "The encrypted recovery journal is an authenticated but incomplete prefix; no automatic repair was attempted."
                    .to_owned(),
                detected_at_unix_ms: execution_now_unix_ms(),
                recovery_available: false,
                rollback_available: false,
            });
            return Err(ApplicationError::JournalLocked);
        }
        for event in database_events.into_iter().skip(external.len()) {
            if let Err(_error) = self.journal.append(event) {
                self.record_journal_diagnostic(JournalDiagnostic {
                    scope: JournalDiagnosticScope::External,
                    execution_id: Some(execution_id),
                    code: "external_journal_append_failed".to_owned(),
                    message: "The encrypted recovery journal could not durably append an event."
                        .to_owned(),
                    detected_at_unix_ms: execution_now_unix_ms(),
                    recovery_available: false,
                    rollback_available: false,
                });
                return Err(ApplicationError::JournalLocked);
            }
        }
        if let Err(_error) = self.journal.flush() {
            self.record_journal_diagnostic(JournalDiagnostic {
                scope: JournalDiagnosticScope::External,
                execution_id: Some(execution_id),
                code: "external_journal_flush_failed".to_owned(),
                message: "The encrypted recovery journal could not be flushed durably.".to_owned(),
                detected_at_unix_ms: execution_now_unix_ms(),
                recovery_available: false,
                rollback_available: false,
            });
            return Err(ApplicationError::JournalLocked);
        }
        Ok(())
    }

    fn journal_cursor(&self, execution_id: ExecutionId) -> Result<JournalCursor, ApplicationError> {
        let events = self.database.execution_journal_events(execution_id)?;
        Ok(JournalCursor {
            sequence: u64::try_from(events.len()).map_err(|_| InvalidExecution)?,
            previous: events.last().map(|event| event.event_digest),
        })
    }

    fn finish_execution(
        &self,
        execution_id: ExecutionId,
        status: OrganizationExecutionStatus,
        message: &str,
        cursor: &mut JournalCursor,
    ) -> Result<(), ApplicationError> {
        self.persist_event(
            execution_id,
            None,
            JournalEventKind::ExecutionFinished,
            json!({
                "event": "execution_finished",
                "status": status,
                "message": message,
            }),
            None,
            Some(status),
            None,
            None,
            None,
            cursor,
        )
        .map(|_| ())
    }

    fn fail_operation(
        &self,
        operation: &ExecutionOperation,
        category: ExecutionFailureCategory,
        code: &str,
        message: &str,
        cursor: &mut JournalCursor,
    ) -> Result<(), ApplicationError> {
        self.persist_event(
            operation.execution_id,
            Some(operation.id),
            JournalEventKind::StepFailed,
            json!({
                "event": "step_failed",
                "operation_id": operation.id,
                "category": category,
                "code": code,
                "message": message,
            }),
            Some(ExecutionOperationStatus::Failed),
            Some(OrganizationExecutionStatus::Failed),
            None,
            Some(code),
            Some(message),
            cursor,
        )?;
        self.finish_execution(
            operation.execution_id,
            OrganizationExecutionStatus::Failed,
            "Execution stopped before mutation because a critical precondition failed.",
            cursor,
        )?;
        self.database.add_execution_error(
            operation.execution_id,
            Some(operation.id),
            category,
            code,
            message,
            None,
            &execution_now_iso(),
        )?;
        Ok(())
    }

    fn fail_operation_with_executor_audit(
        &self,
        operation: &ExecutionOperation,
        category: ExecutionFailureCategory,
        code: &str,
        message: &str,
        executor_audit: &ExecutorAttemptAudit,
        cursor: &mut JournalCursor,
    ) -> Result<(), ApplicationError> {
        self.persist_event(
            operation.execution_id,
            Some(operation.id),
            JournalEventKind::StepFailed,
            json!({
                "event": "step_failed",
                "operation_id": operation.id,
                "category": category,
                "code": code,
                "message": message,
                "executor_audit": executor_audit,
            }),
            Some(ExecutionOperationStatus::Failed),
            Some(OrganizationExecutionStatus::Failed),
            None,
            Some(code),
            Some(message),
            cursor,
        )?;
        self.finish_execution(
            operation.execution_id,
            OrganizationExecutionStatus::Failed,
            "Execution stopped after the executor proved no mutation occurred.",
            cursor,
        )?;
        self.database.add_execution_error(
            operation.execution_id,
            Some(operation.id),
            category,
            code,
            message,
            None,
            &execution_now_iso(),
        )?;
        Ok(())
    }
}

fn consent_attestation_material(
    detail: &ExecutionDetail,
    nonce: [u8; 32],
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
) -> Result<ConsentAttestationBinding, ApplicationError> {
    ConsentAttestationBinding::try_from_execution_detail(
        detail,
        FixedBytes32::from_bytes(nonce),
        issued_at_unix_ms,
        expires_at_unix_ms,
    )
    .map_err(|_| ApplicationError::InvalidExecution)
}

fn native_confirmation_summary(detail: &ExecutionDetail) -> NativeExecutionConfirmation {
    NativeExecutionConfirmation {
        file_count: detail.session.approval.operation_count,
        folder_count: detail.session.summary.folders_to_create,
        destination_root_display: detail
            .session
            .approval
            .destination_root
            .display_path
            .clone(),
        plan_verification_code: plan_verification_code(&detail.session.plan_digest_hex),
    }
}

fn plan_verification_code(digest_hex: &str) -> String {
    let compact = digest_hex
        .chars()
        .take(8)
        .collect::<String>()
        .to_ascii_uppercase();
    if compact.len() == 8 {
        format!("{}-{}", &compact[..4], &compact[4..])
    } else {
        "INVALID".to_owned()
    }
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(unix)]
fn native_path(path: &Path) -> NativePath {
    use std::os::unix::ffi::OsStrExt;
    NativePath {
        encoding: PathEncoding::UnixBytes,
        bytes: path.as_os_str().as_bytes().to_vec(),
    }
}

#[cfg(windows)]
fn native_path(path: &Path) -> NativePath {
    use std::os::windows::ffi::OsStrExt;
    NativePath {
        encoding: PathEncoding::WindowsUtf16Le,
        bytes: path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect(),
    }
}

#[cfg(not(any(unix, windows)))]
fn native_path(path: &Path) -> NativePath {
    NativePath {
        encoding: PathEncoding::UnixBytes,
        bytes: path.to_string_lossy().as_bytes().to_vec(),
    }
}

#[derive(Debug)]
enum RecoveryObservation {
    NotStarted,
    Applied(Option<Box<FileFingerprint>>),
    Ambiguous(String),
}

enum ApplyScheduledOutcome {
    Applied,
    Terminal(Box<ExecutionDetail>),
}

enum RecoveredRequestClass {
    Skip,
    Applied(domain::RecoveryItem),
    NotStarted(domain::RecoveryItem),
    Ambiguous(domain::RecoveryItem),
}

fn execution_preconditions(internal_staging: bool) -> Vec<String> {
    let mut values = vec![
        "source_exists".to_owned(),
        "source_identity_matches".to_owned(),
        "source_size_matches".to_owned(),
        "source_modified_time_matches".to_owned(),
        "source_content_hash_revalidated".to_owned(),
        "destination_absent".to_owned(),
        "same_volume".to_owned(),
        "single_link".to_owned(),
        "no_symlink_or_reparse_traversal".to_owned(),
        "no_overwrite".to_owned(),
    ];
    if internal_staging {
        values.push("application_controlled_staging_name".to_owned());
    }
    values
}

fn block_operation(operation: &mut ExecutionOperation, stale: bool, code: &str, message: &str) {
    operation.status = if stale {
        ExecutionOperationStatus::Stale
    } else {
        ExecutionOperationStatus::Blocked
    };
    operation.error_code = Some(code.to_owned());
    operation.error_message = Some(message.chars().take(2_048).collect());
}

fn detect_plan_collisions(candidates: &mut [PlannedCandidate]) {
    let mut destinations = BTreeMap::<String, Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if !candidate.blocked {
            destinations
                .entry(path_key(&candidate.operation.destination_relative_path))
                .or_default()
                .push(index);
        }
    }
    for indexes in destinations.into_values().filter(|values| values.len() > 1) {
        for index in indexes {
            block_operation(
                &mut candidates[index].operation,
                false,
                "duplicate_plan_destination",
                "Multiple approved operations target the same case-insensitive destination.",
            );
            candidates[index].blocked = true;
        }
    }
}

fn propagate_dependency_blocks(candidates: &mut [PlannedCandidate]) {
    loop {
        let blocked_ids = candidates
            .iter()
            .filter(|candidate| candidate.blocked)
            .map(|candidate| candidate.operation.id)
            .collect::<HashSet<_>>();
        let mut changed = false;
        for candidate in candidates.iter_mut().filter(|candidate| !candidate.blocked) {
            if candidate
                .operation
                .dependencies
                .iter()
                .any(|dependency| blocked_ids.contains(dependency))
            {
                block_operation(
                    &mut candidate.operation,
                    false,
                    "blocked_dependency",
                    "A destination dependency is blocked.",
                );
                candidate.blocked = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn dependency_order(candidates: &[PlannedCandidate]) -> (Vec<usize>, Vec<usize>) {
    let mut remaining = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (!candidate.blocked).then_some(index))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::new();
    loop {
        let remaining_ids = remaining
            .iter()
            .map(|index| candidates[*index].operation.id)
            .collect::<HashSet<_>>();
        let next = remaining
            .iter()
            .filter(|index| {
                candidates[**index]
                    .operation
                    .dependencies
                    .iter()
                    .all(|dependency| !remaining_ids.contains(dependency))
            })
            .min_by_key(|index| {
                (
                    path_key(
                        candidates[**index]
                            .operation
                            .source_relative_path
                            .as_deref()
                            .unwrap_or_default(),
                    ),
                    candidates[**index].operation.id,
                )
            })
            .copied();
        let Some(next) = next else {
            break;
        };
        remaining.remove(&next);
        ordered.push(next);
    }
    let mut cyclic = remaining.into_iter().collect::<Vec<_>>();
    cyclic.sort_by_key(|index| {
        path_key(
            candidates[*index]
                .operation
                .source_relative_path
                .as_deref()
                .unwrap_or_default(),
        )
    });
    (ordered, cyclic)
}

fn source_cleanup_top_level_is_protected(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('.')
        || matches!(
            lower.as_str(),
            "documents"
                | "images"
                | "pictures"
                | "photos"
                | "vidéos"
                | "videos"
                | "archives"
                | "installateurs"
                | "à vérifier"
                | "a verifier"
                | "développement"
                | "developpement"
                | "applications"
                | "library"
                | "system"
                | "windows"
                | "program files"
                | "program files (x86)"
        )
}

fn collect_fully_planned_directory_tree(
    root: &Path,
    relative_directory: &str,
    planned_sources: &HashSet<String>,
    destination_paths: &[String],
    removable: &mut Vec<String>,
) -> Result<bool, ApplicationError> {
    let normalized_directory = normalize_relative_string(relative_directory);
    if destination_paths.iter().any(|destination| {
        destination == &normalized_directory
            || destination.starts_with(&(normalized_directory.clone() + "/"))
    }) {
        return Ok(false);
    }
    let absolute = root.join(relative_path(&normalized_directory)?);
    let metadata = fs::symlink_metadata(&absolute)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }
    let mut children = fs::read_dir(&absolute)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let entry_path = entry.path();
        let entry_metadata = fs::symlink_metadata(&entry_path)?;
        if entry_metadata.file_type().is_symlink() {
            return Ok(false);
        }
        let child_relative = entry_path
            .strip_prefix(root)
            .map_err(|_| ApplicationError::InvalidExecution)?
            .to_string_lossy()
            .replace('\\', "/");
        if entry_metadata.is_dir() {
            if !collect_fully_planned_directory_tree(
                root,
                &child_relative,
                planned_sources,
                destination_paths,
                removable,
            )? {
                return Ok(false);
            }
        } else if entry_metadata.is_file() {
            if !planned_sources.contains(&normalize_relative_string(&child_relative)) {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    removable.push(normalized_directory);
    Ok(true)
}

fn execution_summary(
    proposal: &domain::OrganizationProposal,
    candidates: &[PlannedCandidate],
    directory_count: u64,
) -> Result<ExecutionSummary, ApplicationError> {
    let files_to_move = candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.operation.kind,
                ExecutionOperationKind::Move | ExecutionOperationKind::MoveAndRename
            )
        })
        .count();
    let files_to_rename = candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.operation.kind,
                ExecutionOperationKind::Rename | ExecutionOperationKind::MoveAndRename
            )
        })
        .count();
    let blocked = candidates
        .iter()
        .filter(|candidate| candidate.blocked)
        .count();
    let preflight_ok = candidates.len().saturating_sub(blocked);
    Ok(ExecutionSummary {
        affected_files: u64::try_from(candidates.len()).map_err(|_| InvalidExecution)?,
        folders_to_create: directory_count,
        files_to_move: u64::try_from(files_to_move).map_err(|_| InvalidExecution)?,
        files_to_rename: u64::try_from(files_to_rename).map_err(|_| InvalidExecution)?,
        files_unchanged: proposal.summary.unchanged,
        conflicts: u64::try_from(
            candidates
                .iter()
                .filter(|candidate| {
                    candidate.operation.error_code.as_deref() == Some("duplicate_plan_destination")
                        || candidate.operation.error_code.as_deref() == Some("destination_exists")
                })
                .count(),
        )
        .map_err(|_| InvalidExecution)?,
        needs_review: u64::try_from(
            candidates
                .iter()
                .filter(|candidate| {
                    candidate.operation.error_code.as_deref() == Some("proposal_needs_review")
                        || candidate.operation.status == ExecutionOperationStatus::Stale
                })
                .count(),
        )
        .map_err(|_| InvalidExecution)?,
        preflight_ok: u64::try_from(preflight_ok).map_err(|_| InvalidExecution)?,
        blocked: u64::try_from(blocked).map_err(|_| InvalidExecution)?,
        ..ExecutionSummary::default()
    })
}

fn classify_operation(source: &str, destination: &str) -> ExecutionOperationKind {
    let source = Path::new(source);
    let destination = Path::new(destination);
    let moved = source.parent() != destination.parent();
    let renamed = source.file_name() != destination.file_name();
    match (moved, renamed) {
        (true, true) => ExecutionOperationKind::MoveAndRename,
        (true, false) => ExecutionOperationKind::Move,
        (false, true) => ExecutionOperationKind::Rename,
        (false, false) => ExecutionOperationKind::Rename,
    }
}

fn is_case_only_operation(operation: &ExecutionOperation) -> bool {
    operation
        .source_relative_path
        .as_deref()
        .is_some_and(|source| {
            source != operation.destination_relative_path
                && path_key(source) == path_key(&operation.destination_relative_path)
        })
}

fn is_case_only_proposal(operation: &OrganizationProposalOperation) -> bool {
    let source = normalize_relative_string(&operation.source.relative_path);
    let destination =
        destination_relative(&operation.proposed_destination, &operation.proposed_name);
    source != destination && path_key(&source) == path_key(&destination)
}

fn destination_relative(destination: &[String], filename: &str) -> String {
    destination
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(filename))
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_relative_string(value: &str) -> String {
    value.replace('\\', "/")
}

fn relative_path(value: &str) -> Result<PathBuf, ApplicationError> {
    let normalized = normalize_relative_string(value);
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.ends_with('/')
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|value| *value == b':')
    {
        return Err(ApplicationError::InvalidExecution);
    }
    let mut output = PathBuf::new();
    for component in normalized.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(ApplicationError::InvalidExecution);
        }
        output.push(component);
    }
    if output.is_absolute()
        || output
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ApplicationError::InvalidExecution);
    }
    Ok(output)
}

fn path_key(value: &str) -> String {
    normalize_relative_string(value).to_lowercase()
}

fn directory_prefixes(destinations: &[String]) -> Result<Vec<String>, ApplicationError> {
    let mut output = BTreeMap::<String, String>::new();
    for destination in destinations {
        let normalized = normalize_relative_string(destination);
        let components = normalized.split('/').collect::<Vec<_>>();
        if components.len() < 2 {
            continue;
        }
        for depth in 1..components.len() {
            let prefix = components[..depth].join("/");
            let _ = relative_path(&prefix)?;
            output
                .entry(path_key(&prefix))
                .and_modify(|existing| {
                    if prefix < *existing {
                        *existing = prefix.clone();
                    }
                })
                .or_insert(prefix);
        }
    }
    let mut ordered = output.into_values().collect::<Vec<_>>();
    ordered.sort_by_key(|value| (value.matches('/').count(), value.clone()));
    Ok(ordered)
}

fn case_insensitive_existing(path: &Path) -> Result<Option<PathBuf>, std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => return Ok(Some(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let Some(target_name) = path.file_name() else {
        return Ok(None);
    };
    let target_key = target_name.to_string_lossy().to_lowercase();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().to_lowercase() == target_key {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

fn modified_time_matches(expected: Option<&str>, observed: Option<i128>) -> bool {
    match expected {
        Some(expected) => expected.parse::<i128>().ok() == observed,
        None => true,
    }
}

fn decode_hex_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

fn encode_hex_bytes(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len().saturating_mul(2));
    for byte in value {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn same_file_state(expected: &FileFingerprint, observed: &FileFingerprint) -> bool {
    expected.native_identity.volume.stable_identifier
        == observed.native_identity.volume.stable_identifier
        && expected.native_identity.object_key == observed.native_identity.object_key
        && observed.native_identity.link_count == 1
        && observed.native_identity.reparse_tag.is_none()
        && expected.byte_size == observed.byte_size
        && expected.modified_at_ns == observed.modified_at_ns
        && expected.attributes == observed.attributes
        && expected.content_digest == observed.content_digest
        && expected.content_digest.is_some()
}

fn executor_request_binding_error(
    request: &domain::ExecutorRequestFact,
    sessions: &[domain::ExecutorSessionFact],
    events: &[OperationJournalEvent],
) -> Option<String> {
    let expected_purpose = match request.direction {
        ExecutorRequestDirection::Forward => domain::ExecutorSessionPurpose::Forward,
        ExecutorRequestDirection::Rollback => domain::ExecutorSessionPurpose::Rollback,
    };
    let Some(session) = sessions.iter().find(|session| {
        session.session_id == request.session_id && session.purpose == expected_purpose
    }) else {
        return Some("Executor session identity is missing or direction-mismatched.".to_owned());
    };
    let Some(event) = events
        .iter()
        .find(|event| event.sequence == request.intent_event_sequence)
    else {
        return Some("Intent event binding is missing from the authenticated journal.".to_owned());
    };
    let expected_kind = match request.direction {
        ExecutorRequestDirection::Forward => JournalEventKind::IntentDurable,
        ExecutorRequestDirection::Rollback => JournalEventKind::RollbackIntent,
    };
    if event.step_id != Some(request.operation_id)
        || event.kind != expected_kind
        || encode_hex_bytes(&event.event_digest) != request.intent_event_digest_hex
    {
        return Some("Intent event binding does not match the executor request.".to_owned());
    }
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&event.payload) else {
        return Some("Intent event payload is not valid canonical data.".to_owned());
    };
    let Some(session_material) = payload.get("executor_session") else {
        return Some("Intent event has no executor session identity.".to_owned());
    };
    let execution_id = session.execution_id.to_string();
    let plan_id = session.plan_id.to_string();
    if session_material
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        != Some(session.session_id.as_str())
        || session_material
            .get("execution_id")
            .and_then(serde_json::Value::as_str)
            != Some(execution_id.as_str())
        || session_material
            .get("plan_id")
            .and_then(serde_json::Value::as_str)
            != Some(plan_id.as_str())
        || session_material
            .get("plan_digest")
            .and_then(serde_json::Value::as_str)
            != Some(session.plan_digest_hex.as_str())
        || session_material
            .get("purpose")
            .and_then(serde_json::Value::as_str)
            != Some(session.purpose.database_name())
        || session_material
            .get("coordinator_pid")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(session.coordinator_pid))
        || session_material
            .get("child_pid")
            .and_then(serde_json::Value::as_u64)
            != session.child_pid.map(u64::from)
        || session_material
            .get("worker_nonce_hash")
            .and_then(serde_json::Value::as_str)
            != Some(session.worker_nonce_hash_hex.as_str())
        || session_material
            .get("coordinator_nonce_hash")
            .and_then(serde_json::Value::as_str)
            != Some(session.coordinator_nonce_hash_hex.as_str())
        || session_material
            .get("response_nonce_hash")
            .and_then(serde_json::Value::as_str)
            != session.response_nonce_hash_hex.as_deref()
    {
        return Some("Intent session identity does not match its session record.".to_owned());
    }
    let Some(material) = payload.get("executor_request") else {
        return Some("Intent event has no executor request identity.".to_owned());
    };
    let operation_id = request.operation_id.to_string();
    let request_nonce_matches = material
        .get("request_nonce")
        .and_then(serde_json::Value::as_str)
        .and_then(decode_hex_digest)
        .is_some_and(|nonce| domain::executor_nonce_hash(&nonce) == request.request_nonce_hash_hex);
    if material
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        != Some(request.request_id.as_str())
        || material
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            != Some(request.session_id.as_str())
        || material
            .get("operation_id")
            .and_then(serde_json::Value::as_str)
            != Some(operation_id.as_str())
        || material
            .get("request_sequence")
            .and_then(serde_json::Value::as_u64)
            != Some(request.request_sequence)
        || material
            .get("direction")
            .and_then(serde_json::Value::as_str)
            != Some(request.direction.database_name())
        || !request_nonce_matches
        || material
            .get("request_digest")
            .and_then(serde_json::Value::as_str)
            != Some(request.request_digest_hex.as_str())
    {
        return Some("Intent payload identity does not match its request record.".to_owned());
    }
    None
}

fn request_has_recovery_observation(events: &[OperationJournalEvent], request_id: &str) -> bool {
    events.iter().any(|event| {
        serde_json::from_slice::<serde_json::Value>(&event.payload)
            .ok()
            .is_some_and(|payload| {
                payload.get("event").and_then(serde_json::Value::as_str)
                    == Some("recovery_observed")
                    && payload
                        .get("request_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(request_id)
            })
    })
}

fn fingerprint_if_present(
    reader: &dyn ReadOnlyPlatform,
    path: &Path,
    maximum_bytes: u64,
) -> Result<Option<FileFingerprint>, ApplicationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ApplicationError::InvalidExecution)
        }
        Ok(_) => execution_fingerprint(reader, path, maximum_bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn execution_fingerprint(
    reader: &dyn ReadOnlyPlatform,
    path: &Path,
    maximum_bytes: u64,
) -> Result<FileFingerprint, ApplicationError> {
    reader
        .fingerprint_streaming(path, true, maximum_bytes, &|| false, &mut |_| {})
        .map_err(Into::into)
}

#[derive(Serialize)]
struct CanonicalPlanMaterial<'a> {
    material_version: u32,
    execution_id: ExecutionId,
    plan_id: PlanId,
    proposal_id: ProposalId,
    proposal_revision_id: domain::OrganizationRevisionId,
    proposal_revision: u32,
    source_scan_id: domain::ScanId,
    approved_operation_ids: &'a [ProposalItemId],
    approved_operation_count: u64,
    root_id: domain::RootId,
    destination_root: &'a ExecutionRootBinding,
    safety_policy: &'a ExecutionSafetyPolicyBinding,
    operations: Vec<CanonicalOperation<'a>>,
}

#[derive(Serialize)]
struct CanonicalOperation<'a> {
    id: OperationStepId,
    proposal_operation_id: Option<ProposalItemId>,
    kind: ExecutionOperationKind,
    source_relative_path: &'a Option<String>,
    destination_relative_path: &'a str,
    original_source_relative_path: &'a Option<String>,
    expected_source_hash: &'a Option<String>,
    expected_source_size: Option<u64>,
    expected_source_modified_at: &'a Option<String>,
    live_fingerprint: &'a Option<FileFingerprint>,
    preconditions: &'a [String],
    dependencies: &'a [OperationStepId],
    sequence: u32,
    directory_existed_before: Option<bool>,
}

#[allow(clippy::too_many_arguments)]
fn plan_digest(
    execution_id: ExecutionId,
    plan_id: PlanId,
    proposal_id: ProposalId,
    proposal_revision_id: domain::OrganizationRevisionId,
    proposal_revision: u32,
    source_scan_id: domain::ScanId,
    approved_operation_ids: &[ProposalItemId],
    approved_operation_count: u64,
    root_id: domain::RootId,
    destination_root: &ExecutionRootBinding,
    safety_policy: &ExecutionSafetyPolicyBinding,
    operations: &[ExecutionOperation],
) -> Result<String, ApplicationError> {
    let approved = approved_operation_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let material = CanonicalPlanMaterial {
        material_version: domain::EXECUTION_PLAN_MATERIAL_VERSION,
        execution_id,
        plan_id,
        proposal_id,
        proposal_revision_id,
        proposal_revision,
        source_scan_id,
        approved_operation_ids,
        approved_operation_count,
        root_id,
        destination_root,
        safety_policy,
        operations: operations
            .iter()
            .filter(|operation| {
                operation
                    .proposal_operation_id
                    .is_none_or(|operation_id| approved.contains(&operation_id))
            })
            .map(|operation| CanonicalOperation {
                id: operation.id,
                proposal_operation_id: operation.proposal_operation_id,
                kind: operation.kind,
                source_relative_path: &operation.source_relative_path,
                destination_relative_path: &operation.destination_relative_path,
                original_source_relative_path: &operation.original_source_relative_path,
                expected_source_hash: &operation.expected_source_hash,
                expected_source_size: operation.expected_source_size,
                expected_source_modified_at: &operation.expected_source_modified_at,
                live_fingerprint: &operation.live_fingerprint,
                preconditions: &operation.preconditions,
                dependencies: &operation.dependencies,
                sequence: operation.sequence,
                directory_existed_before: operation.directory_existed_before,
            })
            .collect(),
    };
    let encoded = serde_json::to_vec(&material).map_err(|_| ApplicationError::InvalidExecution)?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

fn verify_plan_digest(detail: &ExecutionDetail) -> Result<(), ApplicationError> {
    let computed = plan_digest(
        detail.session.id,
        detail.session.plan_id,
        detail.session.proposal_id,
        detail.session.proposal_revision_id,
        detail.session.proposal_revision,
        detail.session.source_scan_id,
        &detail.session.approval.approved_operation_ids,
        detail.session.approval.operation_count,
        detail.session.root_id,
        &detail.session.approval.destination_root,
        &detail.session.approval.safety_policy,
        &detail.operations,
    )?;
    let mut executable_operation_ids = detail
        .operations
        .iter()
        .filter(|operation| {
            !matches!(
                operation.status,
                ExecutionOperationStatus::Blocked | ExecutionOperationStatus::Stale
            )
        })
        .filter_map(|operation| operation.proposal_operation_id)
        .collect::<Vec<_>>();
    executable_operation_ids.sort_unstable();
    let approved_ids = &detail.session.approval.approved_operation_ids;
    if computed != detail.session.plan_digest_hex
        || computed != detail.session.approval.digest_hex
        || detail.session.approval.material_version != domain::EXECUTION_PLAN_MATERIAL_VERSION
        || detail.session.approval.execution_id != detail.session.id
        || detail.session.approval.plan_id != detail.session.plan_id
        || detail.session.approval.proposal_id != detail.session.proposal_id
        || detail.session.approval.proposal_revision_id != detail.session.proposal_revision_id
        || detail.session.approval.proposal_revision != detail.session.proposal_revision
        || detail.session.approval.source_snapshot_version != detail.session.source_scan_id
        || detail.session.approval.operation_count
            != u64::try_from(approved_ids.len()).map_err(|_| InvalidExecution)?
        || detail.session.approval.operation_count != detail.session.summary.preflight_ok
        || approved_ids.windows(2).any(|pair| pair[0] >= pair[1])
        || executable_operation_ids != *approved_ids
    {
        return Err(ApplicationError::InvalidExecution);
    }
    Ok(())
}

fn progress_from_detail(
    detail: &ExecutionDetail,
    total: u64,
    current: Option<String>,
) -> ExecutionProgress {
    ExecutionProgress {
        execution_id: detail.session.id,
        status: detail.session.status,
        completed: detail
            .session
            .summary
            .applied
            .saturating_add(detail.session.summary.failed)
            .saturating_add(detail.session.summary.skipped),
        total,
        applied: detail.session.summary.applied,
        blocked: detail.session.summary.blocked,
        skipped: detail.session.summary.skipped,
        failed: detail.session.summary.failed,
        current,
    }
}

fn execution_now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

fn execution_now_unix_ms() -> i64 {
    OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .saturating_div(1_000_000)
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod consent_plan_tests {
    use super::*;

    #[test]
    fn canonical_plan_digest_binds_exact_approved_set_and_every_step() {
        let execution_id = ExecutionId::new();
        let plan_id = PlanId::new();
        let proposal_id = ProposalId::new();
        let revision_id = domain::OrganizationRevisionId::new();
        let scan_id = domain::ScanId::new();
        let root_id = domain::RootId::new();
        let approved_id = ProposalItemId::new();
        let root = ExecutionRootBinding {
            canonical_path: NativePath {
                encoding: PathEncoding::UnixBytes,
                bytes: b"/safe/root".to_vec(),
            },
            display_path: "/safe/root".to_owned(),
            volume: domain::VolumeIdentity {
                platform: domain::PlatformKind::Other,
                stable_identifier: "volume-a".to_owned(),
                filesystem_type: Some("testfs".to_owned()),
                case_sensitive: true,
                removable: false,
                local: true,
            },
        };
        let policy = ExecutionSafetyPolicyBinding {
            version: domain::EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
            digest_hex: "1".repeat(64),
            maximum_rehash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
            allow_qualified_case_only_rename: false,
        };
        let stage_id = OperationStepId::new();
        let file_id = OperationStepId::new();
        let operations = vec![
            ExecutionOperation {
                id: stage_id,
                execution_id,
                proposal_operation_id: None,
                kind: ExecutionOperationKind::InternalStage,
                source_relative_path: Some("source.txt".to_owned()),
                destination_relative_path: ".supremacy-staging/nonce".to_owned(),
                original_source_relative_path: Some("source.txt".to_owned()),
                expected_source_hash: Some("2".repeat(64)),
                expected_source_size: Some(7),
                expected_source_modified_at: Some("snapshot".to_owned()),
                live_fingerprint: None,
                post_fingerprint: None,
                preconditions: vec!["source_exists".to_owned()],
                dependencies: Vec::new(),
                sequence: 0,
                status: ExecutionOperationStatus::PreflightOk,
                directory_existed_before: None,
                reason: None,
                error_code: None,
                error_message: None,
                started_at: None,
                completed_at: None,
                rolled_back_at: None,
            },
            ExecutionOperation {
                id: file_id,
                execution_id,
                proposal_operation_id: Some(approved_id),
                kind: ExecutionOperationKind::Move,
                source_relative_path: Some(".supremacy-staging/nonce".to_owned()),
                destination_relative_path: "organized/source.txt".to_owned(),
                original_source_relative_path: Some("source.txt".to_owned()),
                expected_source_hash: Some("2".repeat(64)),
                expected_source_size: Some(7),
                expected_source_modified_at: Some("snapshot".to_owned()),
                live_fingerprint: None,
                post_fingerprint: None,
                preconditions: vec!["destination_absent".to_owned()],
                dependencies: vec![stage_id],
                sequence: 1,
                status: ExecutionOperationStatus::PreflightOk,
                directory_existed_before: None,
                reason: None,
                error_code: None,
                error_message: None,
                started_at: None,
                completed_at: None,
                rolled_back_at: None,
            },
        ];
        let approved_ids = vec![approved_id];
        let baseline = plan_digest(
            execution_id,
            plan_id,
            proposal_id,
            revision_id,
            3,
            scan_id,
            &approved_ids,
            1,
            root_id,
            &root,
            &policy,
            &operations,
        )
        .unwrap_or_else(|error| panic!("baseline digest should build: {error}"));

        let mut mutated_steps = operations.clone();
        mutated_steps[0].destination_relative_path = ".supremacy-staging/other-nonce".to_owned();
        let mutated_step_digest = plan_digest(
            execution_id,
            plan_id,
            proposal_id,
            revision_id,
            3,
            scan_id,
            &approved_ids,
            1,
            root_id,
            &root,
            &policy,
            &mutated_steps,
        )
        .unwrap_or_else(|error| panic!("mutated digest should build: {error}"));
        assert_ne!(baseline, mutated_step_digest);

        let extra_approved_id = ProposalItemId::new();
        let exact_set_digest = plan_digest(
            execution_id,
            plan_id,
            proposal_id,
            revision_id,
            3,
            scan_id,
            &[approved_id, extra_approved_id],
            2,
            root_id,
            &root,
            &policy,
            &operations,
        )
        .unwrap_or_else(|error| panic!("changed approved set digest should build: {error}"));
        assert_ne!(baseline, exact_set_digest);

        let changed_count_digest = plan_digest(
            execution_id,
            plan_id,
            proposal_id,
            revision_id,
            3,
            scan_id,
            &approved_ids,
            2,
            root_id,
            &root,
            &policy,
            &operations,
        )
        .unwrap_or_else(|error| panic!("changed approved count digest should build: {error}"));
        assert_ne!(baseline, changed_count_digest);

        let mut changed_root = root.clone();
        changed_root.canonical_path.bytes = b"/other/root".to_vec();
        let changed_root_digest = plan_digest(
            execution_id,
            plan_id,
            proposal_id,
            revision_id,
            3,
            scan_id,
            &approved_ids,
            1,
            root_id,
            &changed_root,
            &policy,
            &operations,
        )
        .unwrap_or_else(|error| panic!("changed root digest should build: {error}"));
        assert_ne!(baseline, changed_root_digest);

        let mut changed_policy = policy;
        changed_policy.digest_hex = "3".repeat(64);
        let changed_policy_digest = plan_digest(
            execution_id,
            plan_id,
            proposal_id,
            revision_id,
            3,
            scan_id,
            &approved_ids,
            1,
            root_id,
            &root,
            &changed_policy,
            &operations,
        )
        .unwrap_or_else(|error| panic!("changed policy digest should build: {error}"));
        assert_ne!(baseline, changed_policy_digest);
    }
}

#[cfg(test)]
mod recovery_state_machine_tests {
    use super::*;

    #[test]
    fn recovery_guard_is_exclusive_and_releases_on_drop() {
        let flag = AtomicBool::new(false);
        {
            let first = RecoveryGuard::try_enter(&flag)
                .unwrap_or_else(|| panic!("first recovery entry must succeed"));
            assert!(flag.load(Ordering::SeqCst));
            assert!(
                RecoveryGuard::try_enter(&flag).is_none(),
                "nested recover_execution must be refused"
            );
            drop(first);
        }
        assert!(!flag.load(Ordering::SeqCst));
        assert!(
            RecoveryGuard::try_enter(&flag).is_some(),
            "recovery must be one-shot per entry and reusable after drop"
        );
    }

    #[test]
    fn recover_execution_must_not_call_start_execution() {
        const RECOVER_SOURCE: &str = include_str!("execution.rs");
        let recover = RECOVER_SOURCE
            .split("pub fn recover_execution(")
            .nth(1)
            .unwrap_or_else(|| panic!("recover_execution must exist"));
        let recover_body = recover
            .split("fn persist_recovery_assessment_result(")
            .next()
            .unwrap_or_else(|| {
                panic!("persist_recovery_assessment_result must follow recover_execution")
            });
        assert!(
            !recover_body.contains("self.start_execution(")
                && !recover_body.contains("self.start_execution_at("),
            "recovery must inspect durable state and must not recursively Apply"
        );
        assert!(
            !recover_body.contains("self.rollback_execution("),
            "recovery must not recursively invoke rollback"
        );
        assert!(
            !recover_body.contains("self.recover_execution("),
            "recovery must not re-enter recover_execution"
        );
        assert!(
            recover_body.contains("RecoveryGuard::try_enter"),
            "recovery must take the exclusive state-transition guard"
        );
        assert!(
            recover_body.contains("reconcile_one_interrupted_request"),
            "recovery must classify each request in a bounded helper, not nested Apply"
        );
        assert!(
            recover_body.contains("persist_recovery_assessment_result"),
            "recovery must persist through a separate frame, not grow recover_execution"
        );
    }
}
