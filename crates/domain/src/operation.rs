use crate::{
    ActorId, ExecutionId, FileFingerprint, NativePath, OperationStepId, PlanId, ProposalId,
    ProposalItemId, RootId, WorkspaceId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    CreateDirectory,
    RemoveDirectoryIfEmpty,
    RenameEntrySameVolume,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepPrecondition {
    SourceMatches { fingerprint: Box<FileFingerprint> },
    DestinationAbsent { root_id: RootId, path: NativePath },
    ParentMatches { native_key: Vec<u8> },
    SameVolume { stable_volume_id: String },
    SingleLink,
    NotReparsePoint,
    LocalNtfsVolume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationStep {
    pub id: OperationStepId,
    pub proposal_item_id: Option<ProposalItemId>,
    pub sequence: u32,
    pub kind: OperationKind,
    pub source_root_id: Option<RootId>,
    pub source_path: Option<NativePath>,
    pub destination_root_id: Option<RootId>,
    pub destination_path: Option<NativePath>,
    pub preconditions: Vec<StepPrecondition>,
    pub inverse: Option<Box<OperationStep>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDraft {
    pub id: PlanId,
    pub workspace_id: WorkspaceId,
    pub proposal_id: ProposalId,
    pub proposal_digest: [u8; 32],
    pub steps: Vec<OperationStep>,
    pub created_at_unix_ms: i64,
}

impl PlanDraft {
    pub fn seal(self, simulated_proposal_digest: [u8; 32]) -> Result<SealedPlan, PlanSealError> {
        if self.proposal_digest != simulated_proposal_digest {
            return Err(PlanSealError::StaleSimulation);
        }
        if self.steps.is_empty() {
            return Err(PlanSealError::Empty);
        }
        if self
            .steps
            .iter()
            .any(|step| !step_is_safe_and_reversible(step))
        {
            return Err(PlanSealError::UnsafeStep);
        }

        let encoded = serde_json::to_vec(&self).map_err(PlanSealError::Serialization)?;
        let digest = *blake3::hash(&encoded).as_bytes();
        Ok(SealedPlan {
            id: self.id,
            workspace_id: self.workspace_id,
            proposal_id: self.proposal_id,
            proposal_digest: self.proposal_digest,
            digest,
            steps: self.steps,
            sealed_at_unix_ms: self.created_at_unix_ms,
        })
    }
}

fn step_is_safe_and_reversible(step: &OperationStep) -> bool {
    match step.kind {
        OperationKind::NoOp => true,
        OperationKind::CreateDirectory => {
            step.source_path.is_none()
                && step.destination_path.is_some()
                && step.inverse.as_ref().is_some_and(|inverse| {
                    inverse.kind == OperationKind::RemoveDirectoryIfEmpty
                        && inverse.destination_path == step.destination_path
                })
        }
        OperationKind::RemoveDirectoryIfEmpty => {
            step.source_path.is_none()
                && step.destination_path.is_some()
                && step.inverse.as_ref().is_some_and(|inverse| {
                    inverse.kind == OperationKind::CreateDirectory
                        && inverse.destination_path == step.destination_path
                })
        }
        OperationKind::RenameEntrySameVolume => {
            step.source_path.is_some()
                && step.destination_path.is_some()
                && step.inverse.as_ref().is_some_and(|inverse| {
                    inverse.kind == OperationKind::RenameEntrySameVolume
                        && inverse.source_path == step.destination_path
                        && inverse.destination_path == step.source_path
                })
                && step.preconditions.iter().any(|condition| {
                    matches!(condition, StepPrecondition::DestinationAbsent { .. })
                })
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlanSealError {
    #[error("operation plan cannot be empty")]
    Empty,
    #[error("simulation does not match the proposal revision")]
    StaleSimulation,
    #[error("operation plan contains a step that is not safely reversible")]
    UnsafeStep,
    #[error("operation plan could not be serialized: {0}")]
    Serialization(serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedPlan {
    pub id: PlanId,
    pub workspace_id: WorkspaceId,
    pub proposal_id: ProposalId,
    pub proposal_digest: [u8; 32],
    pub digest: [u8; 32],
    pub steps: Vec<OperationStep>,
    pub sealed_at_unix_ms: i64,
}

impl SealedPlan {
    #[must_use]
    pub fn digest_hex(&self) -> String {
        self.digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn verify_integrity(&self) -> Result<(), PlanIntegrityError> {
        let draft = PlanDraft {
            id: self.id,
            workspace_id: self.workspace_id,
            proposal_id: self.proposal_id,
            proposal_digest: self.proposal_digest,
            steps: self.steps.clone(),
            created_at_unix_ms: self.sealed_at_unix_ms,
        };
        let encoded = serde_json::to_vec(&draft).map_err(PlanIntegrityError::Serialization)?;
        let computed = *blake3::hash(&encoded).as_bytes();
        if computed != self.digest {
            return Err(PlanIntegrityError::DigestMismatch);
        }
        if self.steps.is_empty()
            || self
                .steps
                .iter()
                .any(|step| !step_is_safe_and_reversible(step))
        {
            return Err(PlanIntegrityError::UnsafeStructure);
        }
        Ok(())
    }

    pub fn approve(
        &self,
        actor_id: ActorId,
        presented_digest: [u8; 32],
        approved_at_unix_ms: i64,
    ) -> Result<ApprovalReceipt, ApprovalError> {
        self.verify_integrity()
            .map_err(|_| ApprovalError::InvalidPlan)?;
        if self.digest != presented_digest {
            return Err(ApprovalError::DigestMismatch);
        }
        Ok(ApprovalReceipt {
            plan_id: self.id,
            plan_digest: self.digest,
            actor_id,
            scope_digest: self.digest,
            approved_at_unix_ms,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    #[error("presented plan digest does not match the sealed plan")]
    DigestMismatch,
    #[error("sealed plan failed its integrity check")]
    InvalidPlan,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanIntegrityError {
    #[error("sealed plan digest does not match its canonical contents")]
    DigestMismatch,
    #[error("sealed plan contains an unsafe operation structure")]
    UnsafeStructure,
    #[error("sealed plan could not be serialized: {0}")]
    Serialization(serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalReceipt {
    pub plan_id: PlanId,
    pub plan_digest: [u8; 32],
    pub actor_id: ActorId,
    pub scope_digest: [u8; 32],
    pub approved_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Approved,
    Applying,
    Applied,
    Partial,
    Failed,
    RecoveryRequired,
    RollingBack,
    RolledBack,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEventKind {
    ApprovedDurable,
    IntentDurable,
    PreconditionsValidated,
    AppliedObserved,
    StepFailed,
    ExecutionFinished,
    RollbackIntent,
    RolledBackObserved,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationJournalEvent {
    pub execution_id: ExecutionId,
    pub sequence: u64,
    pub step_id: Option<OperationStepId>,
    pub kind: JournalEventKind,
    pub payload: Vec<u8>,
    pub payload_digest: [u8; 32],
    pub previous_event_digest: Option<[u8; 32]>,
    pub event_digest: [u8; 32],
    pub occurred_at_unix_ms: i64,
}

impl OperationJournalEvent {
    pub fn new(
        execution_id: ExecutionId,
        sequence: u64,
        step_id: Option<OperationStepId>,
        kind: JournalEventKind,
        payload: &[u8],
        previous_event_digest: Option<[u8; 32]>,
        occurred_at_unix_ms: i64,
    ) -> Self {
        let payload_digest = *blake3::hash(payload).as_bytes();
        let mut hasher = blake3::Hasher::new();
        hasher.update(execution_id.to_string().as_bytes());
        hasher.update(&sequence.to_le_bytes());
        if let Some(step_id) = step_id {
            hasher.update(step_id.to_string().as_bytes());
        }
        hasher.update(&[kind as u8]);
        hasher.update(&payload_digest);
        if let Some(previous) = previous_event_digest {
            hasher.update(&previous);
        }
        hasher.update(&occurred_at_unix_ms.to_le_bytes());
        let event_digest = *hasher.finalize().as_bytes();

        Self {
            execution_id,
            sequence,
            step_id,
            kind,
            payload: payload.to_vec(),
            payload_digest,
            previous_event_digest,
            event_digest,
            occurred_at_unix_ms,
        }
    }

    #[must_use]
    pub fn verify(&self, expected_previous: Option<[u8; 32]>) -> bool {
        if self.previous_event_digest != expected_previous {
            return false;
        }
        let rebuilt = Self::new(
            self.execution_id,
            self.sequence,
            self.step_id,
            self.kind,
            &self.payload,
            self.previous_event_digest,
            self.occurred_at_unix_ms,
        );
        rebuilt.payload_digest == self.payload_digest && rebuilt.event_digest == self.event_digest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryObservation {
    NotApplied,
    Applied,
    BothEntriesPresent,
    NeitherEntryPresent,
    IdentityMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_rejects_a_different_digest() {
        let plan = SealedPlan {
            id: PlanId::new(),
            workspace_id: WorkspaceId::new(),
            proposal_id: ProposalId::new(),
            proposal_digest: [1; 32],
            digest: [2; 32],
            steps: Vec::new(),
            sealed_at_unix_ms: 1,
        };

        assert!(matches!(
            plan.approve(ActorId::new(), [3; 32], 2),
            Err(ApprovalError::InvalidPlan | ApprovalError::DigestMismatch)
        ));
    }
}
