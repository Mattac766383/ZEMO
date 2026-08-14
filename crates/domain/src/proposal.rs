use crate::{
    EvidenceRef, FileId, FileVersionId, OrganizationRevisionId, ProposalId, ProposalItemId,
    ProposalNodeId, ProposalOverrideId, ReviewReason, RootId, ScanId, WorkspaceId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationState {
    Calibrated,
    Uncalibrated,
    OutOfDistribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    pub raw_score: f32,
    pub probability: Option<f32>,
    pub calibration: CalibrationState,
}

impl Confidence {
    pub fn new(
        raw_score: f32,
        probability: Option<f32>,
        calibration: CalibrationState,
    ) -> Result<Self, ConfidenceError> {
        if !raw_score.is_finite() {
            return Err(ConfidenceError::NotFinite);
        }
        if probability.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
            return Err(ConfidenceError::ProbabilityOutOfRange);
        }
        Ok(Self {
            raw_score,
            probability,
            calibration,
        })
    }

    #[must_use]
    pub fn is_eligible(&self, threshold: f32) -> bool {
        self.calibration == CalibrationState::Calibrated
            && self.probability.is_some_and(|value| value >= threshold)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfidenceError {
    #[error("confidence score must be finite")]
    NotFinite,
    #[error("calibrated probability must be between zero and one")]
    ProbabilityOutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Ready,
    ToReview,
    Accepted,
    Rejected,
    Blocked,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationIntent {
    pub root_id: RootId,
    pub folder_components: Vec<String>,
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalAction {
    Keep,
    Move { destination: DestinationIntent },
    PlaceInReview { destination: DestinationIntent },
}

impl ProposalAction {
    #[must_use]
    pub const fn mutates_filesystem(&self) -> bool {
        !matches!(self, Self::Keep)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalItem {
    pub id: ProposalItemId,
    pub file_id: FileId,
    pub expected_file_version_id: FileVersionId,
    pub action: ProposalAction,
    pub review_state: ReviewState,
    pub confidence: Confidence,
    pub rationale: String,
    pub evidence: Vec<EvidenceRef>,
    pub counter_evidence: Vec<EvidenceRef>,
    pub uncertainty_reasons: Vec<ReviewReason>,
    pub alternatives: Vec<ProposalAction>,
}

impl ProposalItem {
    pub fn decide(&mut self, accept: bool) -> Result<(), ProposalDecisionError> {
        if matches!(self.review_state, ReviewState::Blocked | ReviewState::Stale) {
            return Err(ProposalDecisionError::NotReviewable);
        }
        self.review_state = if accept {
            ReviewState::Accepted
        } else {
            ReviewState::Rejected
        };
        Ok(())
    }

    #[must_use]
    pub fn requires_decision(&self) -> bool {
        self.action.mutates_filesystem()
            && !matches!(
                self.review_state,
                ReviewState::Accepted | ReviewState::Rejected | ReviewState::Blocked
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProposalDecisionError {
    #[error("blocked or stale proposal items cannot be accepted")]
    NotReviewable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalRevision {
    pub id: ProposalId,
    pub workspace_id: WorkspaceId,
    pub base_scan_id: ScanId,
    pub revision: u32,
    pub policy_digest: [u8; 32],
    pub items: Vec<ProposalItem>,
    pub created_at_unix_ms: i64,
}

impl ProposalRevision {
    #[must_use]
    pub fn counts(&self) -> ProposalCounts {
        self.items
            .iter()
            .fold(ProposalCounts::default(), |mut total, item| {
                match item.review_state {
                    ReviewState::Ready | ReviewState::Accepted => total.ready += 1,
                    ReviewState::ToReview => total.to_review += 1,
                    ReviewState::Blocked | ReviewState::Stale => total.blocked += 1,
                    ReviewState::Rejected => {}
                }
                total
            })
    }

    #[must_use]
    pub fn can_be_sealed(&self) -> bool {
        !self.items.is_empty()
            && self.items.iter().all(|item| {
                matches!(
                    item.review_state,
                    ReviewState::Accepted | ReviewState::Rejected
                ) || !item.action.mutates_filesystem()
            })
            && self
                .items
                .iter()
                .any(|item| item.review_state == ReviewState::Accepted)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCounts {
    pub ready: usize,
    pub to_review: usize,
    pub blocked: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSeverity {
    Warning,
    Blocker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationConflict {
    pub item_id: ProposalItemId,
    pub reason: ReviewReason,
    pub severity: ConflictSeverity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationDiff {
    pub item_id: ProposalItemId,
    pub display_label: String,
    pub before_label: Option<String>,
    pub after_label: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalSimulation {
    pub proposal_id: ProposalId,
    pub proposal_digest: [u8; 32],
    pub diffs: Vec<SimulationDiff>,
    pub conflicts: Vec<SimulationConflict>,
    pub simulated_at_unix_ms: i64,
}

impl ProposalSimulation {
    #[must_use]
    pub fn has_blockers(&self) -> bool {
        self.conflicts
            .iter()
            .any(|conflict| conflict.severity == ConflictSeverity::Blocker)
    }
}

/// Lifecycle of a review-only organization proposal. No state grants filesystem
/// mutation authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationProposalStatus {
    Draft,
    ReadyForReview,
    Reviewed,
    ApprovedForFutureApply,
    Superseded,
    Cancelled,
}

impl OrganizationProposalStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::ReadyForReview => "ready_for_review",
            Self::Reviewed => "reviewed",
            Self::ApprovedForFutureApply => "approved_for_future_apply",
            Self::Superseded => "superseded",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOperationKind {
    MoveProposal,
    RenameProposal,
    CreateFolderProposal,
    KeepInPlace,
    ToReview,
    NoAction,
}

impl ProposalOperationKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::MoveProposal => "move_proposal",
            Self::RenameProposal => "rename_proposal",
            Self::CreateFolderProposal => "create_folder_proposal",
            Self::KeepInPlace => "keep_in_place",
            Self::ToReview => "to_review",
            Self::NoAction => "no_action",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalConfidenceLevel {
    VeryHigh,
    High,
    Medium,
    Low,
}

impl ProposalConfidenceLevel {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::VeryHigh => "very_high",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalConflictState {
    None,
    AutoResolved,
    DestinationCollision,
    InvalidPath,
    PathTooLong,
    StaleSource,
}

impl ProposalConflictState {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AutoResolved => "auto_resolved",
            Self::DestinationCollision => "destination_collision",
            Self::InvalidPath => "invalid_path",
            Self::PathTooLong => "path_too_long",
            Self::StaleSource => "stale_source",
        }
    }

    #[must_use]
    pub const fn requires_review(self) -> bool {
        matches!(
            self,
            Self::DestinationCollision | Self::InvalidPath | Self::PathTooLong | Self::StaleSource
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationReason {
    pub code: String,
    pub explanation: String,
    pub evidence_references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalSourceSnapshot {
    pub relative_path: String,
    pub content_hash: Option<String>,
    pub byte_size: u64,
    pub modified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrganizationProposalOperation {
    pub id: ProposalItemId,
    pub file_id: FileId,
    pub file_version_id: FileVersionId,
    pub source: ProposalSourceSnapshot,
    pub source_name: String,
    pub machine_destination: Vec<String>,
    pub machine_name: String,
    pub proposed_destination: Vec<String>,
    pub proposed_name: String,
    pub operation_kind: ProposalOperationKind,
    pub confidence_score: f32,
    pub confidence_level: ProposalConfidenceLevel,
    pub reasons: Vec<OrganizationReason>,
    pub conflict_state: ProposalConflictState,
    pub needs_review: bool,
    pub stale: bool,
    pub user_override: bool,
    pub disruption_score: f32,
    pub proposed_path_length: usize,
    pub proposed_depth: usize,
    pub semantic_context: String,
    pub document_type: String,
    pub customer_name: Option<String>,
    pub supplier_name: Option<String>,
    pub project_name: Option<String>,
    pub duplicate_group_id: Option<String>,
    pub duplicate_canonical: bool,
}

impl OrganizationProposalOperation {
    #[must_use]
    pub fn proposed_relative_path(&self) -> String {
        self.proposed_destination
            .iter()
            .chain(std::iter::once(&self.proposed_name))
            .cloned()
            .collect::<Vec<_>>()
            .join("\\")
    }

    #[must_use]
    pub fn is_rename(&self) -> bool {
        !self.source_name.eq_ignore_ascii_case(&self.proposed_name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualNodeKind {
    Root,
    Folder,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualProposalNode {
    pub id: ProposalNodeId,
    pub parent_id: Option<ProposalNodeId>,
    pub kind: VirtualNodeKind,
    pub name: String,
    pub virtual_path: String,
    pub operation_id: Option<ProposalItemId>,
    pub child_count: u64,
    pub needs_review_count: u64,
    pub conflict_count: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrganizationProposalSummary {
    pub files_analyzed: u64,
    pub proposed_moves: u64,
    pub proposed_renames: u64,
    pub unchanged: u64,
    pub needs_review: u64,
    pub unresolved: u64,
    pub conflicts: u64,
    pub high_confidence: u64,
    pub medium_confidence: u64,
    pub low_confidence: u64,
    pub duplicate_no_action: u64,
    pub average_depth: f32,
    pub maximum_depth: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationProposalDiff {
    pub destinations_changed: u64,
    pub files_added: u64,
    pub conflicts_resolved: u64,
    pub moved_to_review: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrganizationProposal {
    pub id: ProposalId,
    pub revision_id: OrganizationRevisionId,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub source_scan_id: ScanId,
    pub revision: u32,
    pub status: OrganizationProposalStatus,
    pub engine_version: String,
    pub policy_version: String,
    pub source_semantic_version: Option<String>,
    pub source_relationship_version: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub summary: OrganizationProposalSummary,
    pub diff: OrganizationProposalDiff,
    pub nodes: Vec<VirtualProposalNode>,
    pub operations: Vec<OrganizationProposalOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrganizationPreferences {
    pub client_first: bool,
    pub include_year_folders: bool,
    pub maximum_depth: usize,
    pub minimum_group_size: usize,
    pub keep_photos_inside_projects: bool,
    pub supplier_invoices_inside_projects: bool,
    pub naming_language: String,
    pub preserve_existing_folders: bool,
    pub personal_root_name: String,
    pub business_root_name: String,
    pub rename_template: String,
    pub review_threshold: f32,
}

impl Default for OrganizationPreferences {
    fn default() -> Self {
        Self {
            client_first: true,
            include_year_folders: true,
            maximum_depth: 6,
            minimum_group_size: 2,
            keep_photos_inside_projects: true,
            supplier_invoices_inside_projects: true,
            naming_language: "en".to_owned(),
            preserve_existing_folders: true,
            personal_root_name: "Personal".to_owned(),
            business_root_name: "Business".to_owned(),
            rename_template: "{date}_{party}_{document_type}_{identifier}".to_owned(),
            review_threshold: 0.65,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOverrideAction {
    Destination,
    Rename,
    DestinationAndRename,
    KeepInPlace,
    ToReview,
    Reject,
}

impl ProposalOverrideAction {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Destination => "destination",
            Self::Rename => "rename",
            Self::DestinationAndRename => "destination_and_rename",
            Self::KeepInPlace => "keep_in_place",
            Self::ToReview => "to_review",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationProposalOverride {
    pub id: ProposalOverrideId,
    pub proposal_id: ProposalId,
    pub file_id: FileId,
    pub action: ProposalOverrideAction,
    pub destination: Option<Vec<String>>,
    pub proposed_name: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncalibrated_scores_are_never_eligible() {
        let confidence = Confidence::new(0.99, Some(0.99), CalibrationState::Uncalibrated)
            .expect("test confidence should be valid");

        assert!(!confidence.is_eligible(0.8));
    }
}
