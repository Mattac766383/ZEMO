use crate::{
    RuleEvaluation,
    path_safety::{VirtualPathPolicy, collision_key, collision_name, validate_component},
};
use domain::{
    FileId, FileVersionId, OrganizationPreferences, OrganizationProposal, OrganizationProposalDiff,
    OrganizationProposalOperation, OrganizationProposalOverride, OrganizationProposalStatus,
    OrganizationProposalSummary, OrganizationReason, OrganizationRevisionId,
    ProposalConfidenceLevel, ProposalConflictState, ProposalId, ProposalItemId, ProposalNodeId,
    ProposalOperationKind, ProposalOverrideAction, ProposalSourceSnapshot, RootId, ScanId,
    VirtualNodeKind, VirtualProposalNode, WorkspaceId,
};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const ORGANIZATION_ENGINE_VERSION: &str = "m7-local-organizer-1";
pub const ORGANIZATION_POLICY_VERSION: &str = "m7-safety-policy-1";

/// Maximum neighborhood size before falling back to a full rebuild.
pub const INCREMENTAL_NEIGHBORHOOD_LIMIT: usize = 2_048;
/// Absolute dirty-file ceiling for attempting incremental updates.
pub const INCREMENTAL_DIRTY_FILE_LIMIT: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalSignal {
    pub value: String,
    pub confidence: f32,
    pub status: String,
    pub user_confirmed: bool,
}

impl ProposalSignal {
    #[must_use]
    pub fn is_high_confidence(&self) -> bool {
        self.user_confirmed
            || (self.confidence >= 0.85
                && matches!(
                    self.status.as_str(),
                    "confirmed" | "inferred" | "auto_linked"
                ))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRelationship {
    pub relationship_type: String,
    pub identity_id: String,
    pub display_name: String,
    pub confidence: f32,
    pub status: String,
    pub user_confirmed: bool,
    pub project_customer_name: Option<String>,
}

impl ProposalRelationship {
    #[must_use]
    pub fn is_eligible(&self) -> bool {
        self.user_confirmed
            || (self.status == "auto_linked" && self.confidence >= 0.9)
            || (self.status == "user_confirmed")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrganizationSourceInput {
    pub file_id: FileId,
    pub file_version_id: FileVersionId,
    pub source_relative_path: String,
    pub source_name: String,
    pub byte_size: u64,
    pub modified_at: Option<String>,
    pub content_hash: Option<String>,
    pub extraction_status: Option<String>,
    pub semantic_status: Option<String>,
    pub input_quality: f32,
    pub context: Option<ProposalSignal>,
    pub document_type: Option<ProposalSignal>,
    pub issue_date: Option<ProposalSignal>,
    pub identifier: Option<ProposalSignal>,
    pub amount: Option<ProposalSignal>,
    pub currency: Option<ProposalSignal>,
    pub relationships: Vec<ProposalRelationship>,
    pub review_reasons: Vec<String>,
    pub duplicate_group_id: Option<String>,
    pub duplicate_canonical: bool,
    pub rule_evaluation: RuleEvaluation,
}

#[derive(Debug, Clone)]
pub struct OrganizationBuildRequest {
    pub proposal_id: ProposalId,
    pub revision_id: OrganizationRevisionId,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub source_scan_id: ScanId,
    pub revision: u32,
    pub created_at: String,
    pub updated_at: String,
    pub source_semantic_version: Option<String>,
    pub source_relationship_version: Option<String>,
    pub preferences: OrganizationPreferences,
    pub inputs: Vec<OrganizationSourceInput>,
    pub overrides: Vec<OrganizationProposalOverride>,
    pub previous_operations: Vec<OrganizationProposalOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalBuildPhase {
    Evaluating,
    ResolvingGroups,
    DetectingConflicts,
    BuildingTree,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalBuildProgress {
    pub proposal_id: ProposalId,
    pub phase: ProposalBuildPhase,
    pub files_total: u64,
    pub files_evaluated: u64,
    pub high_confidence: u64,
    pub needs_review: u64,
    pub conflicts: u64,
}

#[derive(Debug, Clone)]
struct DraftOperation {
    operation: OrganizationProposalOperation,
    optional_tail: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalRebuildMode {
    Full,
    Incremental,
}

impl ProposalRebuildMode {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncrementalFallbackReason {
    NoPreviousProposal,
    EmptyDirtySet,
    DirtySetTooLarge { dirty: usize, limit: usize },
    NeighborhoodTooLarge { neighborhood: usize, limit: usize },
    PreferencesOrRulesChanged,
    Cancelled,
}

impl IncrementalFallbackReason {
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::NoPreviousProposal => "no_previous_proposal".to_owned(),
            Self::EmptyDirtySet => "empty_dirty_set".to_owned(),
            Self::DirtySetTooLarge { dirty, limit } => {
                format!("dirty_set_too_large:{dirty}>{limit}")
            }
            Self::NeighborhoodTooLarge {
                neighborhood,
                limit,
            } => format!("neighborhood_too_large:{neighborhood}>{limit}"),
            Self::PreferencesOrRulesChanged => "preferences_or_rules_changed".to_owned(),
            Self::Cancelled => "cancelled".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IncrementalOrganizationBuildRequest {
    pub base: OrganizationBuildRequest,
    /// Original changed file IDs (before neighborhood expansion).
    pub dirty_file_ids: Vec<FileId>,
    /// Fresh source inputs for dirty + neighborhood files to recompile.
    pub neighborhood_inputs: Vec<OrganizationSourceInput>,
    pub deleted_file_ids: Vec<FileId>,
    pub force_full_rebuild: bool,
}

#[derive(Debug, Clone)]
pub struct OrganizationBuildOutcome {
    pub proposal: OrganizationProposal,
    pub rebuild_mode: ProposalRebuildMode,
    pub rebuild_reason: Option<String>,
    pub dirty_file_count: u64,
    pub affected_file_ids: Vec<FileId>,
}

#[derive(Debug, Default)]
pub struct LocalOrganizationProposalEngine;

impl LocalOrganizationProposalEngine {
    pub fn build(
        &self,
        request: OrganizationBuildRequest,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> OrganizationProposal {
        self.build_with_mode(request, is_cancelled, on_progress)
            .proposal
    }

    pub fn build_with_mode(
        &self,
        request: OrganizationBuildRequest,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> OrganizationBuildOutcome {
        let proposal = self.build_full(request, is_cancelled, on_progress);
        OrganizationBuildOutcome {
            dirty_file_count: proposal.summary.files_analyzed,
            proposal,
            rebuild_mode: ProposalRebuildMode::Full,
            rebuild_reason: None,
            affected_file_ids: Vec::new(),
        }
    }

    pub fn build_incremental(
        &self,
        request: IncrementalOrganizationBuildRequest,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> OrganizationBuildOutcome {
        if request.force_full_rebuild {
            return self.fallback_full(
                request.base,
                IncrementalFallbackReason::PreferencesOrRulesChanged,
                is_cancelled,
                on_progress,
            );
        }
        if request.base.previous_operations.is_empty() {
            return self.fallback_full(
                request.base,
                IncrementalFallbackReason::NoPreviousProposal,
                is_cancelled,
                on_progress,
            );
        }
        let original_dirty = request
            .dirty_file_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if original_dirty.is_empty() && request.deleted_file_ids.is_empty() {
            return self.fallback_full(
                request.base,
                IncrementalFallbackReason::EmptyDirtySet,
                is_cancelled,
                on_progress,
            );
        }
        let dirty_count = original_dirty.len() + request.deleted_file_ids.len();
        if dirty_count > INCREMENTAL_DIRTY_FILE_LIMIT {
            return self.fallback_full(
                request.base,
                IncrementalFallbackReason::DirtySetTooLarge {
                    dirty: dirty_count,
                    limit: INCREMENTAL_DIRTY_FILE_LIMIT,
                },
                is_cancelled,
                on_progress,
            );
        }

        let mut policy = VirtualPathPolicy {
            maximum_depth: request.base.preferences.maximum_depth.clamp(2, 8),
            ..VirtualPathPolicy::default()
        };
        policy.maximum_depth = policy.maximum_depth.min(8);
        let overrides = request
            .base
            .overrides
            .iter()
            .map(|value| (value.file_id, value))
            .collect::<HashMap<_, _>>();

        let mut seed_drafts = Vec::with_capacity(request.neighborhood_inputs.len());
        for input in &request.neighborhood_inputs {
            if is_cancelled() {
                return self.fallback_full(
                    request.base,
                    IncrementalFallbackReason::Cancelled,
                    is_cancelled,
                    on_progress,
                );
            }
            let user_override = overrides.get(&input.file_id).copied();
            seed_drafts.push(compile_operation(
                input,
                &request.base.preferences,
                policy,
                user_override,
            ));
        }

        let provided = request
            .neighborhood_inputs
            .iter()
            .map(|input| input.file_id)
            .collect::<HashSet<_>>();
        let affected = expand_invalidation_neighborhood(
            &original_dirty
                .iter()
                .copied()
                .chain(request.deleted_file_ids.iter().copied())
                .collect::<HashSet<_>>(),
            &request.deleted_file_ids,
            &request.base.previous_operations,
            &seed_drafts,
        );
        if affected.len() > INCREMENTAL_NEIGHBORHOOD_LIMIT {
            return self.fallback_full(
                request.base,
                IncrementalFallbackReason::NeighborhoodTooLarge {
                    neighborhood: affected.len(),
                    limit: INCREMENTAL_NEIGHBORHOOD_LIMIT,
                },
                is_cancelled,
                on_progress,
            );
        }

        // Files in the affected set but absent from neighborhood_inputs are carried
        // forward from the previous revision. The application layer loads the
        // primary neighborhood; any second-order dependents keep prior ops
        // rather than forcing a full rebuild (correctness fallback still exists
        // for oversized neighborhoods and rule changes).

        let deleted = request
            .deleted_file_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let mut recompiled = seed_drafts
            .into_iter()
            .map(|draft| (draft.operation.file_id, draft))
            .collect::<HashMap<_, _>>();

        let mut drafts = Vec::with_capacity(request.base.previous_operations.len());
        let total = request
            .base
            .previous_operations
            .iter()
            .filter(|operation| !deleted.contains(&operation.file_id))
            .count()
            .saturating_add(
                provided
                    .iter()
                    .filter(|file_id| {
                        !request
                            .base
                            .previous_operations
                            .iter()
                            .any(|operation| operation.file_id == **file_id)
                    })
                    .count(),
            ) as u64;
        let mut progress = ProposalBuildProgress {
            proposal_id: request.base.proposal_id,
            phase: ProposalBuildPhase::Evaluating,
            files_total: total,
            files_evaluated: 0,
            high_confidence: 0,
            needs_review: 0,
            conflicts: 0,
        };
        on_progress(progress.clone());

        let mut seen = HashSet::new();
        for previous in &request.base.previous_operations {
            if deleted.contains(&previous.file_id) {
                continue;
            }
            let draft = if let Some(draft) = recompiled.remove(&previous.file_id) {
                draft
            } else {
                draft_from_previous_operation(previous)
            };
            seen.insert(draft.operation.file_id);
            progress.files_evaluated = progress.files_evaluated.saturating_add(1);
            if matches!(
                draft.operation.confidence_level,
                ProposalConfidenceLevel::VeryHigh | ProposalConfidenceLevel::High
            ) {
                progress.high_confidence = progress.high_confidence.saturating_add(1);
            }
            if draft.operation.needs_review {
                progress.needs_review = progress.needs_review.saturating_add(1);
            }
            drafts.push(draft);
            if progress.files_evaluated.is_multiple_of(128) || progress.files_evaluated == total {
                on_progress(progress.clone());
            }
        }
        for draft in recompiled.into_values() {
            if deleted.contains(&draft.operation.file_id) {
                continue;
            }
            if seen.insert(draft.operation.file_id) {
                progress.files_evaluated = progress.files_evaluated.saturating_add(1);
                drafts.push(draft);
            }
        }

        if is_cancelled() {
            return self.fallback_full(
                request.base,
                IncrementalFallbackReason::Cancelled,
                is_cancelled,
                on_progress,
            );
        }

        progress.phase = ProposalBuildPhase::ResolvingGroups;
        on_progress(progress.clone());
        apply_minimum_group_policy(
            &mut drafts,
            request.base.preferences.minimum_group_size.clamp(1, 20),
        );

        progress.phase = ProposalBuildPhase::DetectingConflicts;
        // Reset auto-resolved collision names in the affected collision neighborhood
        // so resolve_collisions stays deterministic vs a full rebuild.
        reset_collision_names_for_neighborhood(&mut drafts, &affected);
        resolve_collisions(&mut drafts, policy);
        progress.conflicts = drafts
            .iter()
            .filter(|draft| draft.operation.conflict_state != ProposalConflictState::None)
            .count() as u64;
        progress.needs_review = drafts
            .iter()
            .filter(|draft| draft.operation.needs_review)
            .count() as u64;
        on_progress(progress.clone());

        let operations = drafts
            .into_iter()
            .map(|draft| draft.operation)
            .collect::<Vec<_>>();
        progress.phase = ProposalBuildPhase::BuildingTree;
        on_progress(progress.clone());
        let nodes = build_virtual_tree(&operations);
        let summary = summarize(&operations);
        let diff = proposal_diff(&request.base.previous_operations, &operations);
        progress.phase = ProposalBuildPhase::Completed;
        on_progress(progress);

        let affected_file_ids = affected.into_iter().collect::<Vec<_>>();
        OrganizationBuildOutcome {
            dirty_file_count: dirty_count as u64,
            proposal: OrganizationProposal {
                id: request.base.proposal_id,
                revision_id: request.base.revision_id,
                workspace_id: request.base.workspace_id,
                root_id: request.base.root_id,
                source_scan_id: request.base.source_scan_id,
                revision: request.base.revision,
                status: OrganizationProposalStatus::ReadyForReview,
                engine_version: ORGANIZATION_ENGINE_VERSION.to_owned(),
                policy_version: ORGANIZATION_POLICY_VERSION.to_owned(),
                source_semantic_version: request.base.source_semantic_version,
                source_relationship_version: request.base.source_relationship_version,
                created_at: request.base.created_at,
                updated_at: request.base.updated_at,
                summary,
                diff,
                nodes,
                operations,
            },
            rebuild_mode: ProposalRebuildMode::Incremental,
            rebuild_reason: None,
            affected_file_ids,
        }
    }

    fn fallback_full(
        &self,
        request: OrganizationBuildRequest,
        reason: IncrementalFallbackReason,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> OrganizationBuildOutcome {
        let proposal = self.build_full(request, is_cancelled, on_progress);
        OrganizationBuildOutcome {
            dirty_file_count: proposal.summary.files_analyzed,
            proposal,
            rebuild_mode: ProposalRebuildMode::Full,
            rebuild_reason: Some(reason.as_str()),
            affected_file_ids: Vec::new(),
        }
    }

    fn build_full(
        &self,
        request: OrganizationBuildRequest,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> OrganizationProposal {
        let total = request.inputs.len() as u64;
        let mut progress = ProposalBuildProgress {
            proposal_id: request.proposal_id,
            phase: ProposalBuildPhase::Evaluating,
            files_total: total,
            files_evaluated: 0,
            high_confidence: 0,
            needs_review: 0,
            conflicts: 0,
        };
        on_progress(progress.clone());
        let mut policy = VirtualPathPolicy {
            maximum_depth: request.preferences.maximum_depth.clamp(2, 8),
            ..VirtualPathPolicy::default()
        };
        policy.maximum_depth = policy.maximum_depth.min(8);
        let overrides = request
            .overrides
            .iter()
            .map(|value| (value.file_id, value))
            .collect::<HashMap<_, _>>();
        let mut drafts = Vec::with_capacity(request.inputs.len());
        let mut cancelled = false;

        for input in &request.inputs {
            if is_cancelled() {
                cancelled = true;
                break;
            }
            let user_override = overrides.get(&input.file_id).copied();
            let draft = compile_operation(input, &request.preferences, policy, user_override);
            progress.files_evaluated = progress.files_evaluated.saturating_add(1);
            if matches!(
                draft.operation.confidence_level,
                ProposalConfidenceLevel::VeryHigh | ProposalConfidenceLevel::High
            ) {
                progress.high_confidence = progress.high_confidence.saturating_add(1);
            }
            if draft.operation.needs_review {
                progress.needs_review = progress.needs_review.saturating_add(1);
            }
            drafts.push(draft);
            if progress.files_evaluated.is_multiple_of(128) || progress.files_evaluated == total {
                on_progress(progress.clone());
            }
        }

        if !cancelled {
            progress.phase = ProposalBuildPhase::ResolvingGroups;
            on_progress(progress.clone());
            apply_minimum_group_policy(
                &mut drafts,
                request.preferences.minimum_group_size.clamp(1, 20),
            );

            progress.phase = ProposalBuildPhase::DetectingConflicts;
            resolve_collisions(&mut drafts, policy);
            progress.conflicts = drafts
                .iter()
                .filter(|draft| draft.operation.conflict_state != ProposalConflictState::None)
                .count() as u64;
            progress.needs_review = drafts
                .iter()
                .filter(|draft| draft.operation.needs_review)
                .count() as u64;
            on_progress(progress.clone());
        }

        let operations = drafts
            .into_iter()
            .map(|draft| draft.operation)
            .collect::<Vec<_>>();
        progress.phase = if cancelled {
            ProposalBuildPhase::Cancelled
        } else {
            ProposalBuildPhase::BuildingTree
        };
        on_progress(progress.clone());
        let nodes = build_virtual_tree(&operations);
        let summary = summarize(&operations);
        let diff = proposal_diff(&request.previous_operations, &operations);
        let status = if cancelled {
            OrganizationProposalStatus::Cancelled
        } else {
            OrganizationProposalStatus::ReadyForReview
        };
        progress.phase = if cancelled {
            ProposalBuildPhase::Cancelled
        } else {
            ProposalBuildPhase::Completed
        };
        on_progress(progress);

        OrganizationProposal {
            id: request.proposal_id,
            revision_id: request.revision_id,
            workspace_id: request.workspace_id,
            root_id: request.root_id,
            source_scan_id: request.source_scan_id,
            revision: request.revision,
            status,
            engine_version: ORGANIZATION_ENGINE_VERSION.to_owned(),
            policy_version: ORGANIZATION_POLICY_VERSION.to_owned(),
            source_semantic_version: request.source_semantic_version,
            source_relationship_version: request.source_relationship_version,
            created_at: request.created_at,
            updated_at: request.updated_at,
            summary,
            diff,
            nodes,
            operations,
        }
    }
}

/// Compute the proposal invalidation neighborhood for dirty/deleted files.
#[must_use]
pub fn compute_invalidation_neighborhood(
    dirty_file_ids: &[FileId],
    deleted_file_ids: &[FileId],
    previous_operations: &[OrganizationProposalOperation],
) -> HashSet<FileId> {
    expand_invalidation_neighborhood(
        &dirty_file_ids.iter().copied().collect(),
        deleted_file_ids,
        previous_operations,
        &[],
    )
}

fn expand_invalidation_neighborhood(
    dirty_file_ids: &HashSet<FileId>,
    deleted_file_ids: &[FileId],
    previous_operations: &[OrganizationProposalOperation],
    seed_drafts: &[DraftOperation],
) -> HashSet<FileId> {
    let mut affected = dirty_file_ids.clone();
    affected.extend(deleted_file_ids.iter().copied());

    let previous_by_id = previous_operations
        .iter()
        .map(|operation| (operation.file_id, operation))
        .collect::<HashMap<_, _>>();

    let mut customer_keys = HashSet::new();
    let mut supplier_keys = HashSet::new();
    let mut project_keys = HashSet::new();
    let mut destination_prefixes = HashSet::new();
    let mut collision_keys = HashSet::new();

    let mut absorb_operation = |operation: &OrganizationProposalOperation| {
        if let Some(value) = operation.customer_name.as_ref() {
            customer_keys.insert(value.to_ascii_lowercase());
        }
        if let Some(value) = operation.supplier_name.as_ref() {
            supplier_keys.insert(value.to_ascii_lowercase());
        }
        if let Some(value) = operation.project_name.as_ref() {
            project_keys.insert(value.to_ascii_lowercase());
        }
        for index in 1..=operation.proposed_destination.len() {
            destination_prefixes.insert(
                operation.proposed_destination[..index]
                    .iter()
                    .map(|segment| segment.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("\\"),
            );
        }
        for index in 1..=operation.machine_destination.len() {
            destination_prefixes.insert(
                operation.machine_destination[..index]
                    .iter()
                    .map(|segment| segment.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("\\"),
            );
        }
        collision_keys.insert(collision_key(
            &operation.machine_destination,
            &operation.machine_name,
        ));
    };

    for file_id in dirty_file_ids.iter().chain(deleted_file_ids.iter()) {
        if let Some(operation) = previous_by_id.get(file_id) {
            absorb_operation(operation);
        }
    }
    for draft in seed_drafts {
        absorb_operation(&draft.operation);
    }

    for operation in previous_operations {
        if affected.contains(&operation.file_id) {
            continue;
        }
        let customer_hit = operation
            .customer_name
            .as_ref()
            .is_some_and(|value| customer_keys.contains(&value.to_ascii_lowercase()));
        let supplier_hit = operation
            .supplier_name
            .as_ref()
            .is_some_and(|value| supplier_keys.contains(&value.to_ascii_lowercase()));
        let project_hit = operation
            .project_name
            .as_ref()
            .is_some_and(|value| project_keys.contains(&value.to_ascii_lowercase()));
        let collision_hit = collision_keys.contains(&collision_key(
            &operation.machine_destination,
            &operation.machine_name,
        ));
        let destination_hit = (1..=operation.proposed_destination.len()).any(|index| {
            destination_prefixes.contains(
                &operation.proposed_destination[..index]
                    .iter()
                    .map(|segment| segment.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("\\"),
            )
        }) || (1..=operation.machine_destination.len()).any(|index| {
            destination_prefixes.contains(
                &operation.machine_destination[..index]
                    .iter()
                    .map(|segment| segment.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("\\"),
            )
        });
        if customer_hit || supplier_hit || project_hit || collision_hit || destination_hit {
            affected.insert(operation.file_id);
        }
    }
    affected
}

fn draft_from_previous_operation(operation: &OrganizationProposalOperation) -> DraftOperation {
    DraftOperation {
        optional_tail: vec![false; operation.proposed_destination.len()],
        operation: OrganizationProposalOperation {
            id: ProposalItemId::new(),
            reasons: operation.reasons.clone(),
            ..operation.clone()
        },
    }
}

fn reset_collision_names_for_neighborhood(
    drafts: &mut [DraftOperation],
    affected: &HashSet<FileId>,
) {
    for draft in drafts.iter_mut() {
        if !affected.contains(&draft.operation.file_id) {
            continue;
        }
        if matches!(
            draft.operation.conflict_state,
            ProposalConflictState::AutoResolved | ProposalConflictState::DestinationCollision
        ) || draft.operation.proposed_name != draft.operation.machine_name
        {
            draft.operation.proposed_name = draft.operation.machine_name.clone();
            if !draft.operation.user_override {
                draft.operation.conflict_state = ProposalConflictState::None;
                draft.operation.reasons.retain(|reason| {
                    !matches!(
                        reason.code.as_str(),
                        "destination_collision" | "unresolved_collision" | "collision_resolved"
                    )
                });
            }
        }
    }
}

fn compile_operation(
    input: &OrganizationSourceInput,
    preferences: &OrganizationPreferences,
    policy: VirtualPathPolicy,
    user_override: Option<&OrganizationProposalOverride>,
) -> DraftOperation {
    let context = normalized_signal(input.context.as_ref(), "unknown");
    let document_type = normalized_signal(input.document_type.as_ref(), "unknown");
    let customer = strongest_relationship(input, "file_customer");
    let supplier = strongest_relationship(input, "file_supplier");
    let project = strongest_relationship(input, "file_project");
    let project_customer = project.and_then(|value| value.project_customer_name.clone());
    let effective_customer = customer
        .map(|value| value.display_name.clone())
        .or(project_customer);
    let supplier_name = supplier.map(|value| value.display_name.clone());
    let project_name = project.map(|value| value.display_name.clone());
    let mut reasons = Vec::new();
    let mut needs_review = false;
    let winning_location_rule = [
        input
            .rule_evaluation
            .prefer_project_location
            .as_ref()
            .map(|rule| ("project", rule)),
        input
            .rule_evaluation
            .destination
            .as_ref()
            .map(|(_, rule)| ("destination", rule)),
        input
            .rule_evaluation
            .preserve_subtree
            .as_ref()
            .map(|rule| ("preserve", rule)),
    ]
    .into_iter()
    .flatten()
    .min_by(|(_, left), (_, right)| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.id.cmp(&right.id))
    });
    let winning_location_kind = winning_location_rule.map(|(kind, _)| kind);
    push_applied_rule_reasons(
        &input.rule_evaluation,
        winning_location_rule.map(|(_, rule)| rule.id),
        &mut reasons,
    );

    let duplicate_no_action = input.duplicate_group_id.is_some() && !input.duplicate_canonical;
    let (confidence_score, confidence_level) =
        proposal_confidence(input, customer, supplier, project);
    if context == "unknown" || context == "mixed" {
        needs_review = true;
        reasons.push(reason(
            "ambiguous_context",
            "Personal versus business context is not strong enough for a definitive location.",
            input.context.as_ref().map(signal_reference),
        ));
    } else {
        reasons.push(reason(
            "context",
            &format!(
                "{} context is supported by local semantic evidence.",
                title(&context)
            ),
            input.context.as_ref().map(signal_reference),
        ));
    }
    if document_type == "unknown"
        || input
            .document_type
            .as_ref()
            .is_none_or(|signal| !signal.is_high_confidence())
    {
        needs_review = true;
        reasons.push(reason(
            "uncertain_document_type",
            "The document type is unknown or below the high-confidence policy threshold.",
            input.document_type.as_ref().map(signal_reference),
        ));
    } else {
        reasons.push(reason(
            "document_type",
            &format!("Document type: {}.", document_type_label(&document_type)),
            input.document_type.as_ref().map(signal_reference),
        ));
    }
    if input
        .semantic_status
        .as_deref()
        .is_none_or(|status| !matches!(status, "success" | "partial"))
        || input.input_quality < 0.65
    {
        needs_review = true;
        reasons.push(reason(
            "semantic_input_quality",
            "Semantic input is missing or degraded, so critical organization evidence may be absent.",
            None,
        ));
    }
    if input
        .extraction_status
        .as_deref()
        .is_some_and(|status| status != "success")
    {
        needs_review = true;
        reasons.push(reason(
            "partial_extraction",
            "Content extraction is not complete.",
            input.extraction_status.clone(),
        ));
    }
    if !input.review_reasons.is_empty() {
        needs_review = true;
        reasons.push(reason(
            "existing_review_state",
            "The source file already has unresolved local review items.",
            Some(input.review_reasons.join(",")),
        ));
    }
    if confidence_level == ProposalConfidenceLevel::Low {
        needs_review = true;
    }
    if confidence_score < preferences.review_threshold {
        needs_review = true;
        reasons.push(reason(
            "user_preference",
            &format!(
                "Review is required below your configured {:.0}% confidence threshold.",
                preferences.review_threshold * 100.0
            ),
            Some("review_threshold".to_owned()),
        ));
    }

    if duplicate_no_action {
        reasons.push(reason(
            "exact_duplicate",
            "This is a non-canonical member of an exact-duplicate group; no cleanup is proposed.",
            input.duplicate_group_id.clone(),
        ));
    }

    let (mut machine_destination, mut optional_tail) = if needs_review
        && (matches!(confidence_level, ProposalConfidenceLevel::Low)
            || context == "unknown"
            || context == "mixed")
    {
        (vec!["TO_REVIEW".to_owned()], vec![false])
    } else {
        destination_policy(
            &context,
            &document_type,
            effective_customer.as_deref(),
            supplier_name.as_deref(),
            project_name.as_deref(),
            input,
            preferences,
            input
                .rule_evaluation
                .use_year_folders
                .as_ref()
                .map_or(preferences.include_year_folders, |(enabled, _)| *enabled),
            winning_location_kind == Some("project"),
            &mut reasons,
        )
    };

    let source_parent = source_parent_components(&input.source_relative_path, &input.source_name);
    if winning_location_kind == Some("destination")
        && let Some((destination, matched)) = &input.rule_evaluation.destination
    {
        if policy.validate_user_destination(destination).is_ok() {
            machine_destination.clone_from(destination);
            optional_tail = vec![false; machine_destination.len()];
        } else {
            machine_destination = vec!["TO_REVIEW".to_owned()];
            optional_tail = vec![false];
            needs_review = true;
            reasons.push(reason(
                "invalid_user_rule",
                "The matched rule contains an unsafe destination and was not applied.",
                Some(matched.id.to_string()),
            ));
        }
    } else if winning_location_kind == Some("preserve") {
        machine_destination = source_parent.clone();
        optional_tail = vec![false; machine_destination.len()];
    } else if preferences.preserve_existing_folders
        && !needs_review
        && existing_structure_is_useful(&source_parent, &machine_destination, policy)
    {
        machine_destination = source_parent.clone();
        optional_tail = vec![false; machine_destination.len()];
        reasons.push(reason(
            "minimal_disruption",
            "The current folder structure already matches the supported organization signals.",
            Some(input.source_relative_path.clone()),
        ));
    }

    let machine_name = proposed_filename(
        input,
        &document_type,
        effective_customer.as_deref(),
        supplier_name.as_deref(),
        confidence_level,
        preferences,
        policy,
    );
    let (machine_destination, machine_name, path_adjusted, path_valid) =
        policy.fit_machine_path(&machine_destination, &machine_name);
    if path_adjusted {
        reasons.push(reason(
            "windows_path_safety",
            "Unsafe or excessive path components were sanitized or shortened for Windows.",
            None,
        ));
    }
    let mut conflict_state = if path_valid {
        ProposalConflictState::None
    } else {
        needs_review = true;
        ProposalConflictState::PathTooLong
    };

    let mut proposed_destination = machine_destination.clone();
    let mut proposed_name = machine_name.clone();
    let mut override_applied = false;
    let mut override_kind = None;
    if let Some(user_override) = user_override {
        override_applied = true;
        override_kind = Some(user_override.action);
        match user_override.action {
            ProposalOverrideAction::Destination => {
                if let Some(destination) = &user_override.destination {
                    proposed_destination.clone_from(destination);
                }
            }
            ProposalOverrideAction::Rename => {
                if let Some(name) = &user_override.proposed_name {
                    proposed_name.clone_from(name);
                }
            }
            ProposalOverrideAction::DestinationAndRename => {
                if let Some(destination) = &user_override.destination {
                    proposed_destination.clone_from(destination);
                }
                if let Some(name) = &user_override.proposed_name {
                    proposed_name.clone_from(name);
                }
            }
            ProposalOverrideAction::KeepInPlace | ProposalOverrideAction::Reject => {
                proposed_destination = source_parent.clone();
                proposed_name.clone_from(&input.source_name);
                needs_review = false;
            }
            ProposalOverrideAction::ToReview => {
                proposed_destination = vec!["TO_REVIEW".to_owned()];
                needs_review = true;
            }
        }
        optional_tail = vec![false; proposed_destination.len()];
        reasons.push(reason(
            "user_override",
            "A stored user decision is authoritative over this machine-generated suggestion.",
            user_override.reason.clone(),
        ));
        let destination_valid = policy
            .validate_user_destination(&proposed_destination)
            .is_ok();
        let filename_valid = policy.validate_user_filename(&proposed_name).is_ok();
        if !destination_valid || !filename_valid {
            conflict_state = ProposalConflictState::InvalidPath;
            needs_review = true;
        } else if policy.path_length_utf16(&proposed_destination, &proposed_name)
            > policy.maximum_path_utf16
        {
            conflict_state = ProposalConflictState::PathTooLong;
            needs_review = true;
        }
    }

    let disruption_score = disruption_score(
        &source_parent,
        &proposed_destination,
        &input.source_name,
        &proposed_name,
    );
    let same_destination = paths_equal(&source_parent, &proposed_destination);
    let same_name = input.source_name.eq_ignore_ascii_case(&proposed_name);
    let operation_kind = if duplicate_no_action {
        ProposalOperationKind::NoAction
    } else if matches!(
        override_kind,
        Some(ProposalOverrideAction::Reject | ProposalOverrideAction::KeepInPlace)
    ) {
        ProposalOperationKind::KeepInPlace
    } else if proposed_destination == ["TO_REVIEW"] {
        ProposalOperationKind::ToReview
    } else if same_destination && same_name {
        ProposalOperationKind::KeepInPlace
    } else if same_destination {
        ProposalOperationKind::RenameProposal
    } else {
        ProposalOperationKind::MoveProposal
    };
    let proposed_path_length = policy.path_length_utf16(&proposed_destination, &proposed_name);

    DraftOperation {
        optional_tail,
        operation: OrganizationProposalOperation {
            id: ProposalItemId::new(),
            file_id: input.file_id,
            file_version_id: input.file_version_id,
            source: ProposalSourceSnapshot {
                relative_path: input.source_relative_path.clone(),
                content_hash: input.content_hash.clone(),
                byte_size: input.byte_size,
                modified_at: input.modified_at.clone(),
            },
            source_name: input.source_name.clone(),
            machine_destination,
            machine_name,
            proposed_destination: proposed_destination.clone(),
            proposed_name,
            operation_kind,
            confidence_score,
            confidence_level,
            reasons,
            conflict_state,
            needs_review,
            stale: false,
            user_override: override_applied,
            disruption_score,
            proposed_path_length,
            proposed_depth: proposed_destination.len(),
            semantic_context: context,
            document_type,
            customer_name: effective_customer,
            supplier_name,
            project_name,
            duplicate_group_id: input.duplicate_group_id.clone(),
            duplicate_canonical: input.duplicate_canonical,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn destination_policy(
    context: &str,
    document_type: &str,
    customer: Option<&str>,
    supplier: Option<&str>,
    project: Option<&str>,
    input: &OrganizationSourceInput,
    preferences: &OrganizationPreferences,
    include_year_folders: bool,
    prefer_project_location: bool,
    reasons: &mut Vec<OrganizationReason>,
) -> (Vec<String>, Vec<bool>) {
    let year = useful_year(input, document_type, include_year_folders);
    let type_folder = type_folder(
        document_type,
        preferences.supplier_invoices_inside_projects && supplier.is_some() && project.is_some(),
    );
    let mut components = Vec::new();
    let mut optional = Vec::new();
    let push =
        |value: String, is_optional: bool, output: &mut Vec<String>, flags: &mut Vec<bool>| {
            output.push(value);
            flags.push(is_optional);
        };

    if context == "personal" {
        push(
            preferences.personal_root_name.clone(),
            false,
            &mut components,
            &mut optional,
        );
        match document_type {
            "tax_document" => {
                push(
                    localized_folder("administrative", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
                push(
                    localized_folder("taxes", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
            }
            "insurance_document" => {
                push(
                    localized_folder("administrative", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
                push(
                    localized_folder("insurance", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
            }
            "bank_statement" => {
                push(
                    localized_folder("administrative", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
                push(
                    localized_folder("banking", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
            }
            "photo" => push(
                localized_folder("photos", &preferences.naming_language).into(),
                false,
                &mut components,
                &mut optional,
            ),
            "video" => push(
                localized_folder("videos", &preferences.naming_language).into(),
                false,
                &mut components,
                &mut optional,
            ),
            "archive" => push(
                localized_folder("archives", &preferences.naming_language).into(),
                false,
                &mut components,
                &mut optional,
            ),
            _ => push(
                localized_folder("administrative", &preferences.naming_language).into(),
                false,
                &mut components,
                &mut optional,
            ),
        }
        if let Some(year) = year {
            push(year, true, &mut components, &mut optional);
        }
    } else if context == "business" {
        push(
            preferences.business_root_name.clone(),
            false,
            &mut components,
            &mut optional,
        );
        let project_photo = document_type == "photo"
            && preferences.keep_photos_inside_projects
            && project.is_some();
        let supplier_invoice_project = document_type == "invoice"
            && preferences.supplier_invoices_inside_projects
            && supplier.is_some()
            && project.is_some();
        let force_supplier_project =
            supplier_invoice_project && (!preferences.client_first || customer.is_none());
        if supplier_invoice_project {
            reasons.push(reason(
                "user_preference",
                "Linked supplier invoices stay inside their project because of your preference.",
                Some("supplier_invoices_inside_projects".to_owned()),
            ));
        }
        if (prefer_project_location || project_photo || force_supplier_project) && project.is_some()
        {
            push(
                localized_folder("projects", &preferences.naming_language).into(),
                false,
                &mut components,
                &mut optional,
            );
            push(
                project.unwrap_or_default().to_owned(),
                false,
                &mut components,
                &mut optional,
            );
            if project_photo {
                reasons.push(reason(
                    "user_preference",
                    "Project photos stay inside their linked project because of your preference.",
                    Some("keep_photos_inside_projects".to_owned()),
                ));
            }
            if let Some(folder) = type_folder {
                push(
                    localized_type_folder(folder, &preferences.naming_language).into(),
                    true,
                    &mut components,
                    &mut optional,
                );
            }
            if let Some(year) = year {
                push(year, true, &mut components, &mut optional);
            }
        } else if preferences.client_first {
            if let Some(customer) = customer {
                push(
                    localized_folder("clients", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
                push(customer.to_owned(), false, &mut components, &mut optional);
                reasons.push(reason(
                    "customer_identity",
                    &format!("Customer: {customer}."),
                    None,
                ));
                if let Some(project) = project {
                    push(project.to_owned(), false, &mut components, &mut optional);
                    reasons.push(reason(
                        "project_identity",
                        &format!("Project: {project}."),
                        None,
                    ));
                }
            } else if let Some(project) = project {
                push(
                    localized_folder("projects", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
                push(project.to_owned(), false, &mut components, &mut optional);
            } else if let Some(supplier) = supplier {
                push(
                    localized_folder("suppliers", &preferences.naming_language).into(),
                    false,
                    &mut components,
                    &mut optional,
                );
                push(supplier.to_owned(), false, &mut components, &mut optional);
                reasons.push(reason(
                    "supplier_identity",
                    &format!("Supplier: {supplier}."),
                    None,
                ));
            } else {
                push(
                    localized_type_folder(
                        business_area(document_type),
                        &preferences.naming_language,
                    )
                    .into(),
                    false,
                    &mut components,
                    &mut optional,
                );
            }
            if let Some(year) = year {
                push(year, true, &mut components, &mut optional);
            }
            if let Some(folder) = type_folder {
                push(
                    localized_type_folder(folder, &preferences.naming_language).into(),
                    true,
                    &mut components,
                    &mut optional,
                );
            }
        } else {
            push(
                localized_type_folder(
                    type_folder.unwrap_or_else(|| business_area(document_type)),
                    &preferences.naming_language,
                )
                .into(),
                false,
                &mut components,
                &mut optional,
            );
            if let Some(customer) = customer {
                push(customer.to_owned(), false, &mut components, &mut optional);
            } else if let Some(supplier) = supplier {
                push(supplier.to_owned(), false, &mut components, &mut optional);
            }
            if let Some(project) = project {
                push(project.to_owned(), false, &mut components, &mut optional);
            }
            if let Some(year) = year {
                push(year, true, &mut components, &mut optional);
            }
        }
    } else {
        components.push("TO_REVIEW".to_owned());
        optional.push(false);
    }
    (components, optional)
}

fn proposal_confidence(
    input: &OrganizationSourceInput,
    customer: Option<&ProposalRelationship>,
    supplier: Option<&ProposalRelationship>,
    project: Option<&ProposalRelationship>,
) -> (f32, ProposalConfidenceLevel) {
    let mut weighted = Vec::new();
    if let Some(context) = &input.context {
        weighted.push((
            if context.user_confirmed {
                1.0
            } else {
                context.confidence
            },
            0.35,
        ));
    }
    if let Some(document_type) = &input.document_type {
        weighted.push((
            if document_type.user_confirmed {
                1.0
            } else {
                document_type.confidence
            },
            0.3,
        ));
    }
    if let Some(relationship) = project.or(customer).or(supplier) {
        weighted.push((
            if relationship.user_confirmed {
                1.0
            } else {
                relationship.confidence
            },
            0.25,
        ));
    }
    weighted.push((input.input_quality.clamp(0.0, 1.0), 0.1));
    let denominator = weighted.iter().map(|(_, weight)| weight).sum::<f32>();
    let mut score = if denominator > 0.0 {
        weighted
            .iter()
            .map(|(value, weight)| value * weight)
            .sum::<f32>()
            / denominator
    } else {
        0.0
    };
    if input.semantic_status.as_deref() == Some("partial") {
        score *= 0.85;
    }
    if !input.review_reasons.is_empty() {
        score *= 0.8;
    }
    let score = score.clamp(0.0, 1.0);
    let level = if score >= 0.95 {
        ProposalConfidenceLevel::VeryHigh
    } else if score >= 0.85 {
        ProposalConfidenceLevel::High
    } else if score >= 0.65 {
        ProposalConfidenceLevel::Medium
    } else {
        ProposalConfidenceLevel::Low
    };
    (score, level)
}

fn strongest_relationship<'a>(
    input: &'a OrganizationSourceInput,
    relationship_type: &str,
) -> Option<&'a ProposalRelationship> {
    input
        .relationships
        .iter()
        .filter(|relationship| {
            relationship.relationship_type == relationship_type && relationship.is_eligible()
        })
        .max_by(|left, right| {
            left.confidence
                .partial_cmp(&right.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.identity_id.cmp(&left.identity_id))
        })
}

fn apply_minimum_group_policy(drafts: &mut [DraftOperation], minimum_group_size: usize) {
    if minimum_group_size <= 1 {
        return;
    }
    loop {
        let mut counts = HashMap::<String, usize>::new();
        for draft in drafts.iter() {
            for index in 1..=draft.operation.proposed_destination.len() {
                let key = draft.operation.proposed_destination[..index]
                    .iter()
                    .map(|segment| segment.to_lowercase())
                    .collect::<Vec<_>>()
                    .join("\\");
                *counts.entry(key).or_default() += 1;
            }
        }
        let mut changed = false;
        for draft in drafts.iter_mut() {
            let Some(true) = draft.optional_tail.last().copied() else {
                continue;
            };
            let key = draft
                .operation
                .proposed_destination
                .iter()
                .map(|segment| segment.to_lowercase())
                .collect::<Vec<_>>()
                .join("\\");
            if counts.get(&key).copied().unwrap_or_default() < minimum_group_size {
                draft.operation.proposed_destination.pop();
                draft.operation.machine_destination.pop();
                draft.optional_tail.pop();
                draft.operation.reasons.push(reason(
                    "minimum_group_policy",
                    "A singleton optional folder was omitted to avoid folder proliferation.",
                    None,
                ));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for draft in drafts {
        draft.operation.proposed_depth = draft.operation.proposed_destination.len();
        draft.operation.disruption_score = disruption_score(
            &source_parent_components(
                &draft.operation.source.relative_path,
                &draft.operation.source_name,
            ),
            &draft.operation.proposed_destination,
            &draft.operation.source_name,
            &draft.operation.proposed_name,
        );
    }
}

fn resolve_collisions(drafts: &mut [DraftOperation], policy: VirtualPathPolicy) {
    let mut seen = HashMap::<String, usize>::new();
    for draft in drafts {
        if matches!(
            draft.operation.operation_kind,
            ProposalOperationKind::KeepInPlace | ProposalOperationKind::NoAction
        ) {
            continue;
        }
        let initial_key = collision_key(
            &draft.operation.proposed_destination,
            &draft.operation.proposed_name,
        );
        let ordinal = seen.entry(initial_key).or_insert(0);
        *ordinal += 1;
        if *ordinal == 1 {
            continue;
        }
        if draft.operation.user_override {
            draft.operation.conflict_state = ProposalConflictState::DestinationCollision;
            draft.operation.needs_review = true;
            draft.operation.operation_kind = ProposalOperationKind::ToReview;
            draft.operation.reasons.push(reason(
                "destination_collision",
                "The authoritative user destination collides case-insensitively with another proposal.",
                None,
            ));
            continue;
        }

        let mut suffix = *ordinal;
        let mut candidate = collision_name(
            &draft.operation.proposed_name,
            suffix,
            policy.maximum_filename_utf16,
        );
        let mut candidate_key = collision_key(&draft.operation.proposed_destination, &candidate);
        while seen.contains_key(&candidate_key) && suffix < 10_000 {
            suffix += 1;
            candidate = collision_name(
                &draft.operation.proposed_name,
                suffix,
                policy.maximum_filename_utf16,
            );
            candidate_key = collision_key(&draft.operation.proposed_destination, &candidate);
        }
        if seen.contains_key(&candidate_key)
            || policy.path_length_utf16(&draft.operation.proposed_destination, &candidate)
                > policy.maximum_path_utf16
        {
            draft.operation.conflict_state = ProposalConflictState::DestinationCollision;
            draft.operation.needs_review = true;
            draft.operation.operation_kind = ProposalOperationKind::ToReview;
            draft.operation.reasons.push(reason(
                "unresolved_collision",
                "A unique Windows-safe virtual filename could not be generated.",
                None,
            ));
        } else {
            draft.operation.proposed_name = candidate;
            draft.operation.conflict_state = ProposalConflictState::AutoResolved;
            draft.operation.reasons.push(reason(
                "collision_resolved",
                "A deterministic numeric suffix avoids a case-insensitive destination collision.",
                None,
            ));
            seen.insert(candidate_key, 1);
        }
        draft.operation.proposed_path_length = policy.path_length_utf16(
            &draft.operation.proposed_destination,
            &draft.operation.proposed_name,
        );
    }
}

fn proposed_filename(
    input: &OrganizationSourceInput,
    document_type: &str,
    customer: Option<&str>,
    supplier: Option<&str>,
    confidence_level: ProposalConfidenceLevel,
    preferences: &OrganizationPreferences,
    policy: VirtualPathPolicy,
) -> String {
    if !matches!(
        confidence_level,
        ProposalConfidenceLevel::VeryHigh | ProposalConfidenceLevel::High
    ) || !is_generic_filename(&input.source_name)
    {
        return policy.sanitize_machine_filename(&input.source_name).value;
    }
    let mut components = Vec::new();
    if let Some(date) = input
        .issue_date
        .as_ref()
        .filter(|signal| signal.is_high_confidence())
        && valid_iso_date(&date.value)
    {
        components.push(date.value.clone());
    }
    if let Some(actor) = supplier.or(customer) {
        components.push(actor.to_owned());
    }
    if document_type != "unknown" {
        components.push(document_type_singular(document_type).to_owned());
    }
    if let Some(identifier) = input
        .identifier
        .as_ref()
        .filter(|signal| signal.is_high_confidence())
    {
        components.push(identifier.value.clone());
    }
    if let Some(amount) = input
        .amount
        .as_ref()
        .filter(|signal| signal.is_high_confidence())
    {
        let mut value = amount.value.clone();
        if let Some(currency) = input
            .currency
            .as_ref()
            .filter(|signal| signal.is_high_confidence())
        {
            value.push_str(&currency.value);
        }
        components.push(value);
    }
    components.truncate(5);
    if components.len() < 2 {
        return policy.sanitize_machine_filename(&input.source_name).value;
    }
    let extension = input
        .source_name
        .rsplit_once('.')
        .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
        .map(|(_, extension)| extension);
    let fallback_stem = components.join("_");
    let original_stem = input
        .source_name
        .rsplit_once('.')
        .map_or(input.source_name.as_str(), |(stem, _)| stem);
    let project = strongest_relationship(input, "file_project")
        .map(|relationship| relationship.display_name.as_str())
        .unwrap_or_default();
    let date = input
        .issue_date
        .as_ref()
        .filter(|signal| signal.is_high_confidence() && valid_iso_date(&signal.value))
        .map(|signal| signal.value.as_str())
        .unwrap_or_default();
    let identifier = input
        .identifier
        .as_ref()
        .filter(|signal| signal.is_high_confidence())
        .map(|signal| signal.value.as_str())
        .unwrap_or_default();
    let party = supplier.or(customer).unwrap_or_default();
    let rendered = preferences
        .rename_template
        .replace("{date}", date)
        .replace("{party}", party)
        .replace("{document_type}", document_type_singular(document_type))
        .replace("{identifier}", identifier)
        .replace("{project}", project)
        .replace("{original}", original_stem);
    let stem = rendered
        .split('_')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let stem = if stem.is_empty() || stem.contains(['{', '}']) {
        fallback_stem
    } else {
        stem
    };
    let candidate = extension.map_or(stem.clone(), |extension| format!("{stem}.{extension}"));
    policy.sanitize_machine_filename(&candidate).value
}

fn build_virtual_tree(operations: &[OrganizationProposalOperation]) -> Vec<VirtualProposalNode> {
    #[derive(Debug)]
    struct MutableNode {
        id: ProposalNodeId,
        parent_path: Option<String>,
        kind: VirtualNodeKind,
        name: String,
        operation_id: Option<ProposalItemId>,
        children: HashSet<String>,
        needs_review_count: u64,
        conflict_count: u64,
    }

    let root_id = ProposalNodeId::new();
    let mut nodes = BTreeMap::<String, MutableNode>::new();
    nodes.insert(
        String::new(),
        MutableNode {
            id: root_id,
            parent_path: None,
            kind: VirtualNodeKind::Root,
            name: "Organization Preview".to_owned(),
            operation_id: None,
            children: HashSet::new(),
            needs_review_count: 0,
            conflict_count: 0,
        },
    );
    let mut file_nodes = Vec::new();
    for operation in operations {
        let mut parent_path = String::new();
        if let Some(root) = nodes.get_mut("") {
            root.needs_review_count += u64::from(operation.needs_review);
            root.conflict_count +=
                u64::from(operation.conflict_state != ProposalConflictState::None);
        }
        for segment in &operation.proposed_destination {
            let path = if parent_path.is_empty() {
                segment.clone()
            } else {
                format!("{parent_path}\\{segment}")
            };
            if !nodes.contains_key(&path) {
                nodes.insert(
                    path.clone(),
                    MutableNode {
                        id: ProposalNodeId::new(),
                        parent_path: Some(parent_path.clone()),
                        kind: VirtualNodeKind::Folder,
                        name: segment.clone(),
                        operation_id: None,
                        children: HashSet::new(),
                        needs_review_count: 0,
                        conflict_count: 0,
                    },
                );
            }
            if let Some(parent) = nodes.get_mut(&parent_path) {
                parent.children.insert(path.clone());
            }
            if let Some(node) = nodes.get_mut(&path) {
                node.needs_review_count += u64::from(operation.needs_review);
                node.conflict_count +=
                    u64::from(operation.conflict_state != ProposalConflictState::None);
            }
            parent_path = path;
        }
        let virtual_path = if parent_path.is_empty() {
            operation.proposed_name.clone()
        } else {
            format!("{parent_path}\\{}", operation.proposed_name)
        };
        let file_id = ProposalNodeId::new();
        if let Some(parent) = nodes.get_mut(&parent_path) {
            parent.children.insert(virtual_path.clone());
        }
        file_nodes.push(VirtualProposalNode {
            id: file_id,
            parent_id: nodes.get(&parent_path).map(|node| node.id),
            kind: VirtualNodeKind::File,
            name: operation.proposed_name.clone(),
            virtual_path,
            operation_id: Some(operation.id),
            child_count: 0,
            needs_review_count: u64::from(operation.needs_review),
            conflict_count: u64::from(operation.conflict_state != ProposalConflictState::None),
        });
    }
    let mut output = nodes
        .iter()
        .map(|(path, node)| VirtualProposalNode {
            id: node.id,
            parent_id: node
                .parent_path
                .as_ref()
                .and_then(|parent| nodes.get(parent))
                .map(|parent| parent.id),
            kind: node.kind,
            name: node.name.clone(),
            virtual_path: path.clone(),
            operation_id: node.operation_id,
            child_count: node.children.len() as u64,
            needs_review_count: node.needs_review_count,
            conflict_count: node.conflict_count,
        })
        .collect::<Vec<_>>();
    output.extend(file_nodes);
    output
}

fn summarize(operations: &[OrganizationProposalOperation]) -> OrganizationProposalSummary {
    let mut summary = OrganizationProposalSummary {
        files_analyzed: operations.len() as u64,
        ..OrganizationProposalSummary::default()
    };
    let mut depth_sum = 0_u64;
    for operation in operations {
        match operation.operation_kind {
            ProposalOperationKind::MoveProposal => summary.proposed_moves += 1,
            ProposalOperationKind::RenameProposal => {}
            ProposalOperationKind::KeepInPlace | ProposalOperationKind::NoAction => {
                summary.unchanged += 1;
            }
            ProposalOperationKind::ToReview => summary.unresolved += 1,
            ProposalOperationKind::CreateFolderProposal => {}
        }
        summary.proposed_renames += u64::from(operation.is_rename());
        summary.needs_review += u64::from(operation.needs_review);
        summary.conflicts += u64::from(operation.conflict_state != ProposalConflictState::None);
        summary.duplicate_no_action +=
            u64::from(operation.duplicate_group_id.is_some() && !operation.duplicate_canonical);
        match operation.confidence_level {
            ProposalConfidenceLevel::VeryHigh | ProposalConfidenceLevel::High => {
                summary.high_confidence += 1;
            }
            ProposalConfidenceLevel::Medium => summary.medium_confidence += 1,
            ProposalConfidenceLevel::Low => summary.low_confidence += 1,
        }
        let depth = operation.proposed_depth as u64;
        depth_sum = depth_sum.saturating_add(depth);
        summary.maximum_depth = summary
            .maximum_depth
            .max(u32::try_from(depth).unwrap_or(u32::MAX));
    }
    if !operations.is_empty() {
        summary.average_depth = depth_sum as f32 / operations.len() as f32;
    }
    summary
}

fn proposal_diff(
    previous: &[OrganizationProposalOperation],
    current: &[OrganizationProposalOperation],
) -> OrganizationProposalDiff {
    if previous.is_empty() {
        return OrganizationProposalDiff {
            files_added: current.len() as u64,
            ..OrganizationProposalDiff::default()
        };
    }
    let previous = previous
        .iter()
        .map(|operation| (operation.file_id, operation))
        .collect::<HashMap<_, _>>();
    let mut diff = OrganizationProposalDiff::default();
    for operation in current {
        let Some(old) = previous.get(&operation.file_id) else {
            diff.files_added += 1;
            continue;
        };
        if !paths_equal(&old.proposed_destination, &operation.proposed_destination)
            || !old
                .proposed_name
                .eq_ignore_ascii_case(&operation.proposed_name)
        {
            diff.destinations_changed += 1;
        }
        if old.conflict_state != ProposalConflictState::None
            && operation.conflict_state == ProposalConflictState::None
        {
            diff.conflicts_resolved += 1;
        }
        if !old.needs_review && operation.needs_review {
            diff.moved_to_review += 1;
        }
    }
    diff
}

fn useful_year(
    input: &OrganizationSourceInput,
    document_type: &str,
    include_year: bool,
) -> Option<String> {
    if !include_year
        || !matches!(
            document_type,
            "invoice"
                | "contract"
                | "receipt"
                | "tax_document"
                | "payslip"
                | "bank_statement"
                | "photo"
                | "video"
        )
    {
        return None;
    }
    let signal = input.issue_date.as_ref()?;
    if !signal.is_high_confidence() || !valid_iso_date(&signal.value) {
        return None;
    }
    signal.value.get(..4).map(str::to_owned)
}

fn valid_iso_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value
            .get(..4)
            .and_then(|year| year.parse::<u16>().ok())
            .is_some_and(|year| (1900..=2200).contains(&year))
}

fn localized_folder<'a>(key: &'a str, language: &str) -> &'a str {
    if language != "fr" {
        return match key {
            "administrative" => "Administrative",
            "taxes" => "Taxes",
            "insurance" => "Insurance",
            "banking" => "Banking",
            "photos" => "Photos",
            "videos" => "Videos",
            "archives" => "Archives",
            "clients" => "Clients",
            "projects" => "Projects",
            "suppliers" => "Suppliers",
            _ => key,
        };
    }
    match key {
        "administrative" => "Administratif",
        "taxes" => "Impôts",
        "insurance" => "Assurances",
        "banking" => "Banque",
        "photos" => "Photos",
        "videos" => "Vidéos",
        "archives" => "Archives",
        "clients" => "Clients",
        "projects" => "Projets",
        "suppliers" => "Fournisseurs",
        _ => key,
    }
}

fn localized_type_folder<'a>(value: &'a str, language: &str) -> &'a str {
    if language != "fr" {
        return value;
    }
    match value {
        "Supplier Invoices" => "Factures fournisseurs",
        "Invoices" => "Factures",
        "Quotes" => "Devis",
        "Contracts" => "Contrats",
        "Receipts" => "Reçus",
        "Photos" => "Photos",
        "Videos" => "Vidéos",
        "Purchase Orders" => "Bons de commande",
        "Delivery Notes" => "Bons de livraison",
        "Taxes" => "Impôts",
        "Bank Statements" => "Relevés bancaires",
        "Payroll" => "Paie",
        "Legal" => "Juridique",
        "Accounting" => "Comptabilité",
        "HR" => "Ressources humaines",
        "Administration" => "Administratif",
        _ => value,
    }
}

fn type_folder(document_type: &str, supplier_inside_project: bool) -> Option<&'static str> {
    match document_type {
        "invoice" if supplier_inside_project => Some("Supplier Invoices"),
        "invoice" => Some("Invoices"),
        "quote" => Some("Quotes"),
        "contract" | "employment_contract" => Some("Contracts"),
        "receipt" => Some("Receipts"),
        "photo" => Some("Photos"),
        "video" => Some("Videos"),
        "purchase_order" => Some("Purchase Orders"),
        "delivery_note" => Some("Delivery Notes"),
        "tax_document" => Some("Taxes"),
        "bank_statement" => Some("Bank Statements"),
        "payslip" => Some("Payroll"),
        "legal_document" => Some("Legal"),
        "administrative_document" => Some("Administration"),
        _ => None,
    }
}

fn business_area(document_type: &str) -> &'static str {
    match document_type {
        "invoice" | "receipt" | "purchase_order" | "delivery_note" | "bank_statement"
        | "tax_document" => "Accounting",
        "contract" | "legal_document" => "Legal",
        "payslip" | "employment_contract" | "cv" => "HR",
        _ => "Administration",
    }
}

fn normalized_signal(signal: Option<&ProposalSignal>, default: &str) -> String {
    signal
        .map(|value| value.value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn document_type_label(document_type: &str) -> String {
    document_type.replace('_', " ")
}

fn document_type_singular(document_type: &str) -> &'static str {
    match document_type {
        "invoice" => "Invoice",
        "quote" => "Quote",
        "contract" => "Contract",
        "receipt" => "Receipt",
        "purchase_order" => "Purchase-Order",
        "delivery_note" => "Delivery-Note",
        "tax_document" => "Tax",
        "bank_statement" => "Bank-Statement",
        "payslip" => "Payslip",
        "photo" => "Photo",
        "video" => "Video",
        _ => "Document",
    }
}

fn source_parent_components(relative_path: &str, source_name: &str) -> Vec<String> {
    let mut parts = relative_path
        .split(['/', '\\'])
        .filter(|part| !part.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parts
        .last()
        .is_some_and(|last| last.eq_ignore_ascii_case(source_name))
    {
        parts.pop();
    }
    parts
}

fn existing_structure_is_useful(
    source: &[String],
    proposed: &[String],
    policy: VirtualPathPolicy,
) -> bool {
    if source.is_empty() || source.len() > policy.maximum_depth {
        return false;
    }
    if source
        .iter()
        .any(|segment| validate_component(segment, policy.maximum_segment_utf16).is_err())
    {
        return false;
    }
    let proposed = proposed
        .iter()
        .map(|value| value.to_lowercase())
        .collect::<HashSet<_>>();
    let matching = source
        .iter()
        .map(|value| value.to_lowercase())
        .filter(|value| proposed.contains(value))
        .count();
    let root_is_semantic = source.first().is_some_and(|root| {
        matches!(
            root.to_ascii_lowercase().as_str(),
            "business" | "entreprise" | "professional" | "professionnel" | "personal" | "personnel"
        )
    });
    root_is_semantic && matching >= 2 && matching * 2 >= source.len().min(proposed.len())
}

fn disruption_score(
    source: &[String],
    destination: &[String],
    source_name: &str,
    proposed_name: &str,
) -> f32 {
    if paths_equal(source, destination) && source_name.eq_ignore_ascii_case(proposed_name) {
        return 0.0;
    }
    let source_set = source
        .iter()
        .map(|value| value.to_lowercase())
        .collect::<HashSet<_>>();
    let destination_set = destination
        .iter()
        .map(|value| value.to_lowercase())
        .collect::<HashSet<_>>();
    let shared = source_set.intersection(&destination_set).count() as f32;
    let denominator = source_set.len().max(destination_set.len()).max(1) as f32;
    let path_score = 1.0 - shared / denominator;
    let rename_cost = if source_name.eq_ignore_ascii_case(proposed_name) {
        0.0
    } else {
        0.15
    };
    (path_score * 0.85 + rename_cost).clamp(0.0, 1.0)
}

fn paths_equal(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn is_generic_filename(filename: &str) -> bool {
    let stem = filename
        .rsplit_once('.')
        .map_or(filename, |(stem, _)| stem)
        .to_ascii_lowercase();
    ["scan", "img", "image", "document", "file"]
        .iter()
        .any(|prefix| {
            stem == *prefix
                || stem.starts_with(&format!("{prefix}_"))
                || stem.starts_with(&format!("{prefix}-"))
        })
        || matches!(stem.as_str(), "facture" | "invoice")
        || stem.chars().filter(char::is_ascii_digit).count() >= stem.len().saturating_div(2)
}

fn push_applied_rule_reasons(
    evaluation: &RuleEvaluation,
    winning_location_rule: Option<domain::RuleId>,
    reasons: &mut Vec<OrganizationReason>,
) {
    let mut matched = evaluation
        .semantic_overrides
        .values()
        .map(|(_, rule)| rule)
        .collect::<Vec<_>>();
    matched.extend(
        evaluation
            .classified_supplier
            .as_ref()
            .map(|(_, rule)| rule),
    );
    matched.extend(
        evaluation
            .classified_customer
            .as_ref()
            .map(|(_, rule)| rule),
    );
    matched.extend(
        evaluation
            .prefer_project_location
            .as_ref()
            .filter(|rule| Some(rule.id) == winning_location_rule),
    );
    matched.extend(
        evaluation
            .destination
            .as_ref()
            .map(|(_, rule)| rule)
            .filter(|rule| Some(rule.id) == winning_location_rule),
    );
    matched.extend(
        evaluation
            .preserve_subtree
            .as_ref()
            .filter(|rule| Some(rule.id) == winning_location_rule),
    );
    matched.extend(evaluation.use_year_folders.as_ref().map(|(_, rule)| rule));
    matched.sort_by(|left, right| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut seen = HashSet::new();
    for matched in matched {
        if seen.insert(matched.id) {
            reasons.push(reason(
                "user_rule",
                &format!(
                    "Placed here because of your rule: {}",
                    matched.explanation.trim()
                ),
                Some(matched.id.to_string()),
            ));
        }
    }
}

fn reason(code: &str, explanation: &str, evidence: Option<String>) -> OrganizationReason {
    OrganizationReason {
        code: code.chars().take(64).collect(),
        explanation: explanation.chars().take(512).collect(),
        evidence_references: evidence
            .into_iter()
            .map(|value| value.chars().take(512).collect())
            .collect(),
    }
}

fn signal_reference(signal: &ProposalSignal) -> String {
    format!(
        "{} ({:.0}%, {})",
        signal.value,
        signal.confidence * 100.0,
        if signal.user_confirmed {
            "user confirmed"
        } else {
            signal.status.as_str()
        }
    )
}

fn title(value: &str) -> String {
    let mut characters = value.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ProposalOverrideAction, ProposalOverrideId, RuleAction, RuleId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn signal(value: &str, confidence: f32) -> ProposalSignal {
        ProposalSignal {
            value: value.to_owned(),
            confidence,
            status: "inferred".to_owned(),
            user_confirmed: false,
        }
    }

    fn source(name: &str) -> OrganizationSourceInput {
        OrganizationSourceInput {
            file_id: FileId::new(),
            file_version_id: FileVersionId::new(),
            source_relative_path: format!("Downloads/{name}"),
            source_name: name.to_owned(),
            byte_size: 100,
            modified_at: Some("2026-06-17T10:00:00Z".into()),
            content_hash: Some("00".repeat(32)),
            extraction_status: Some("success".into()),
            semantic_status: Some("success".into()),
            input_quality: 1.0,
            context: Some(signal("business", 0.98)),
            document_type: Some(signal("invoice", 0.98)),
            issue_date: Some(signal("2026-06-17", 0.98)),
            identifier: Some(signal("FP-39482", 0.98)),
            amount: None,
            currency: None,
            relationships: Vec::new(),
            review_reasons: Vec::new(),
            duplicate_group_id: None,
            duplicate_canonical: true,
            rule_evaluation: RuleEvaluation::default(),
        }
    }

    fn relationship(
        relationship_type: &str,
        identity_id: &str,
        display_name: &str,
    ) -> ProposalRelationship {
        ProposalRelationship {
            relationship_type: relationship_type.into(),
            identity_id: identity_id.into(),
            display_name: display_name.into(),
            confidence: 0.98,
            status: "auto_linked".into(),
            user_confirmed: false,
            project_customer_name: None,
        }
    }

    fn request(inputs: Vec<OrganizationSourceInput>) -> OrganizationBuildRequest {
        OrganizationBuildRequest {
            proposal_id: ProposalId::new(),
            revision_id: OrganizationRevisionId::new(),
            workspace_id: WorkspaceId::new(),
            root_id: RootId::new(),
            source_scan_id: ScanId::new(),
            revision: 1,
            created_at: "2026-08-10T20:00:00Z".into(),
            updated_at: "2026-08-10T20:00:00Z".into(),
            source_semantic_version: Some("m5".into()),
            source_relationship_version: Some("m6".into()),
            preferences: OrganizationPreferences::default(),
            inputs,
            overrides: Vec::new(),
            previous_operations: Vec::new(),
        }
    }

    #[test]
    fn customer_project_supplier_invoice_has_one_primary_location() {
        let mut input = source("scan_38492.pdf");
        input.relationships = vec![
            ProposalRelationship {
                relationship_type: "file_customer".into(),
                identity_id: "customer".into(),
                display_name: "Dupont SARL".into(),
                confidence: 0.98,
                status: "auto_linked".into(),
                user_confirmed: false,
                project_customer_name: None,
            },
            ProposalRelationship {
                relationship_type: "file_supplier".into(),
                identity_id: "supplier".into(),
                display_name: "Point P".into(),
                confidence: 0.98,
                status: "auto_linked".into(),
                user_confirmed: false,
                project_customer_name: None,
            },
            ProposalRelationship {
                relationship_type: "file_project".into(),
                identity_id: "project".into(),
                display_name: "Project Bordeaux".into(),
                confidence: 0.98,
                status: "auto_linked".into(),
                user_confirmed: false,
                project_customer_name: Some("Dupont SARL".into()),
            },
        ];
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        let operation = &proposal.operations[0];
        assert!(operation.proposed_destination.starts_with(&[
            "Business".into(),
            "Clients".into(),
            "Dupont SARL".into(),
            "Project Bordeaux".into(),
        ]));
        assert!(!operation.proposed_destination.contains(&"Suppliers".into()));
        assert_eq!(operation.supplier_name.as_deref(), Some("Point P"));
    }

    #[test]
    fn user_order_resolves_conflicting_location_rule_kinds() {
        let preserve_id = RuleId::new();
        let destination_id = RuleId::new();
        let mut input = source("invoice.pdf");
        input.rule_evaluation.preserve_subtree = Some(crate::MatchedRule {
            id: preserve_id,
            name: "Preserve reviewed subtree".into(),
            explanation: "This reviewed subtree should remain in place.".into(),
            position: 0,
            action: RuleAction::PreserveSubtree,
        });
        input.rule_evaluation.destination = Some((
            vec!["Business".into(), "Lower priority".into()],
            crate::MatchedRule {
                id: destination_id,
                name: "Lower destination".into(),
                explanation: "This destination loses because it is lower.".into(),
                position: 1,
                action: RuleAction::SetDestination {
                    segments: vec!["Business".into(), "Lower priority".into()],
                },
            },
        ));

        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        let operation = &proposal.operations[0];
        assert_eq!(operation.proposed_destination, ["Downloads"]);
        assert!(operation.reasons.iter().any(|reason| {
            reason.code == "user_rule" && reason.evidence_references == [preserve_id.to_string()]
        }));
        assert!(operation.reasons.iter().all(|reason| {
            reason.code != "user_rule" || reason.evidence_references != [destination_id.to_string()]
        }));
    }

    #[test]
    fn supplier_invoice_preference_forces_project_level_placement() {
        let mut input = source("supplier-invoice.pdf");
        input.relationships = vec![
            relationship("file_supplier", "supplier", "Point P"),
            relationship("file_project", "project", "Project Bordeaux"),
        ];
        let mut inside_project = request(vec![input.clone()]);
        inside_project.preferences.client_first = false;
        inside_project.preferences.minimum_group_size = 1;
        inside_project.preferences.supplier_invoices_inside_projects = true;
        let proposal =
            LocalOrganizationProposalEngine.build(inside_project, &|| false, &mut |_| {});
        assert!(proposal.operations[0].proposed_destination.starts_with(&[
            "Business".into(),
            "Projects".into(),
            "Project Bordeaux".into(),
        ]));
        assert!(proposal.operations[0].reasons.iter().any(|reason| {
            reason.code == "user_preference"
                && reason
                    .evidence_references
                    .contains(&"supplier_invoices_inside_projects".to_owned())
        }));

        let mut type_first = request(vec![input]);
        type_first.preferences.client_first = false;
        type_first.preferences.minimum_group_size = 1;
        type_first.preferences.supplier_invoices_inside_projects = false;
        let proposal = LocalOrganizationProposalEngine.build(type_first, &|| false, &mut |_| {});
        assert!(proposal.operations[0].proposed_destination.starts_with(&[
            "Business".into(),
            "Invoices".into(),
            "Point P".into(),
        ]));
    }

    #[test]
    fn weak_context_and_unknown_type_go_to_review() {
        let mut input = source("notes.pdf");
        input.context = Some(signal("mixed", 0.55));
        input.document_type = Some(signal("unknown", 0.2));
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        assert_eq!(
            proposal.operations[0].operation_kind,
            ProposalOperationKind::ToReview
        );
        assert_eq!(
            proposal.operations[0].proposed_destination,
            vec!["TO_REVIEW"]
        );
    }

    #[test]
    fn personal_tax_insurance_bank_and_photos_use_clean_personal_hierarchy() {
        let mut inputs = Vec::new();
        for (index, document_type) in [
            "tax_document",
            "insurance_document",
            "bank_statement",
            "photo",
        ]
        .into_iter()
        .enumerate()
        {
            let mut input = source(&format!("scan_{index}.pdf"));
            input.context = Some(signal("personal", 0.98));
            input.document_type = Some(signal(document_type, 0.98));
            inputs.push(input);
        }
        let proposal =
            LocalOrganizationProposalEngine.build(request(inputs), &|| false, &mut |_| {});
        let destinations = proposal
            .operations
            .iter()
            .map(|operation| operation.proposed_destination.join("\\"))
            .collect::<Vec<_>>();
        assert!(
            destinations
                .iter()
                .any(|path| path.starts_with("Personal\\Administrative\\Taxes"))
        );
        assert!(
            destinations
                .iter()
                .any(|path| path.starts_with("Personal\\Administrative\\Insurance"))
        );
        assert!(
            destinations
                .iter()
                .any(|path| path.starts_with("Personal\\Administrative\\Banking"))
        );
        assert!(
            destinations
                .iter()
                .any(|path| path.starts_with("Personal\\Photos"))
        );
    }

    #[test]
    fn one_customer_with_multiple_projects_stays_separate() {
        let mut bordeaux = source("scan_bordeaux.pdf");
        bordeaux.relationships = vec![
            relationship("file_customer", "customer", "Dupont SARL"),
            relationship("file_project", "bordeaux", "Project Bordeaux"),
        ];
        let mut lyon = source("scan_lyon.pdf");
        lyon.relationships = vec![
            relationship("file_customer", "customer", "Dupont SARL"),
            relationship("file_project", "lyon", "Project Lyon"),
        ];
        let proposal = LocalOrganizationProposalEngine.build(
            request(vec![bordeaux, lyon]),
            &|| false,
            &mut |_| {},
        );
        assert!(
            proposal.operations[0]
                .proposed_destination
                .contains(&"Project Bordeaux".into())
        );
        assert!(
            proposal.operations[1]
                .proposed_destination
                .contains(&"Project Lyon".into())
        );
        assert_ne!(
            proposal.operations[0].proposed_destination,
            proposal.operations[1].proposed_destination
        );
    }

    #[test]
    fn customer_without_project_falls_back_to_customer_level() {
        let mut input = source("scan_customer.pdf");
        input.relationships = vec![relationship("file_customer", "customer", "Dupont SARL")];
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        let destination = &proposal.operations[0].proposed_destination;
        assert!(destination.starts_with(&[
            "Business".into(),
            "Clients".into(),
            "Dupont SARL".into()
        ]));
        assert!(!destination.contains(&"Projects".into()));
    }

    #[test]
    fn sensible_existing_tree_is_preserved_with_zero_disruption() {
        let mut input = source("Invoice-2026.pdf");
        input.source_relative_path =
            "Business/Clients/Dupont SARL/Invoices/Invoice-2026.pdf".into();
        input.relationships = vec![relationship("file_customer", "customer", "Dupont SARL")];
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        let operation = &proposal.operations[0];
        assert_eq!(operation.operation_kind, ProposalOperationKind::KeepInPlace);
        assert_eq!(operation.disruption_score, 0.0);
        assert!(
            operation
                .reasons
                .iter()
                .any(|reason| reason.code == "minimal_disruption")
        );
    }

    #[test]
    fn weak_supplier_mention_does_not_create_supplier_tree() {
        let mut input = source("invoice.pdf");
        let mut weak = relationship("file_supplier", "supplier", "Point P");
        weak.status = "candidate".into();
        weak.confidence = 0.7;
        input.relationships = vec![weak];
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        assert!(
            !proposal.operations[0]
                .proposed_destination
                .contains(&"Suppliers".into())
        );
    }

    #[test]
    fn windows_path_injection_is_sanitized_inside_virtual_root() {
        let mut input = source("CON?.pdf");
        input.relationships = vec![relationship(
            "file_customer",
            "customer",
            "../../Windows/System32",
        )];
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        let operation = &proposal.operations[0];
        assert!(
            operation
                .proposed_destination
                .iter()
                .all(|segment| !segment.contains(['/', '\\']) && segment != "..")
        );
        assert!(operation.proposed_name.starts_with('_') || !operation.proposed_name.contains('?'));
        assert!(
            operation
                .reasons
                .iter()
                .any(|reason| reason.code == "windows_path_safety")
        );
    }

    #[test]
    fn exact_duplicate_noncanonical_file_is_not_reorganized() {
        let canonical = source("invoice-original.pdf");
        let mut duplicate = source("invoice-copy.pdf");
        let group_id = "duplicate-group".to_owned();
        let mut canonical = canonical;
        canonical.duplicate_group_id = Some(group_id.clone());
        duplicate.duplicate_group_id = Some(group_id);
        duplicate.duplicate_canonical = false;
        let proposal = LocalOrganizationProposalEngine.build(
            request(vec![canonical, duplicate]),
            &|| false,
            &mut |_| {},
        );
        assert_eq!(
            proposal.operations[1].operation_kind,
            ProposalOperationKind::NoAction
        );
        assert_eq!(proposal.summary.duplicate_no_action, 1);
    }

    #[test]
    fn optional_singleton_folders_are_collapsed_and_depth_is_bounded() {
        let mut input = source("invoice.pdf");
        input.relationships = vec![
            relationship("file_customer", "customer", "Dupont SARL"),
            relationship("file_project", "project", "Project Bordeaux"),
        ];
        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        let operation = &proposal.operations[0];
        assert!(operation.proposed_depth <= 6);
        assert!(!operation.proposed_destination.contains(&"2026".into()));
        assert!(!operation.proposed_destination.contains(&"Invoices".into()));
    }

    #[test]
    fn case_insensitive_collisions_get_safe_deterministic_names() {
        let first = source("scan_1.pdf");
        let mut second = source("scan_2.PDF");
        second.file_id = FileId::new();
        second.file_version_id = FileVersionId::new();
        let proposal = LocalOrganizationProposalEngine.build(
            request(vec![first, second]),
            &|| false,
            &mut |_| {},
        );
        let keys = proposal
            .operations
            .iter()
            .map(|operation| {
                collision_key(&operation.proposed_destination, &operation.proposed_name)
            })
            .collect::<HashSet<_>>();
        assert_eq!(keys.len(), 2);
        assert!(
            proposal
                .operations
                .iter()
                .any(|operation| operation.conflict_state == ProposalConflictState::AutoResolved)
        );
    }

    #[test]
    fn explicit_destination_rule_changes_virtual_proposal_with_explanation() {
        let mut input = source("tax.pdf");
        let matched = crate::MatchedRule {
            id: RuleId::new(),
            name: "Tax destination".into(),
            explanation: "Tax documents stay under Personal Administration Taxes.".into(),
            position: 0,
            action: RuleAction::SetDestination {
                segments: vec!["Personal".into(), "Administrative".into(), "Taxes".into()],
            },
        };
        input.rule_evaluation.destination = Some((
            vec!["Personal".into(), "Administrative".into(), "Taxes".into()],
            matched.clone(),
        ));
        input.rule_evaluation.matched_rules.push(matched);

        let proposal =
            LocalOrganizationProposalEngine.build(request(vec![input]), &|| false, &mut |_| {});
        assert_eq!(
            proposal.operations[0].proposed_destination,
            ["Personal", "Administrative", "Taxes"]
        );
        assert!(proposal.operations[0].reasons.iter().any(|reason| {
            reason.code == "user_rule"
                && reason
                    .explanation
                    .starts_with("Placed here because of your rule:")
        }));
    }

    #[test]
    fn user_override_survives_as_authoritative_effective_destination() {
        let mut input = source("scan_1.pdf");
        let matched = crate::MatchedRule {
            id: RuleId::new(),
            name: "Tax destination".into(),
            explanation: "Tax documents stay in the chosen administrative folder.".into(),
            position: 0,
            action: RuleAction::SetDestination {
                segments: vec!["Personal".into(), "Administrative".into(), "Taxes".into()],
            },
        };
        input.rule_evaluation.destination = Some((
            vec!["Personal".into(), "Administrative".into(), "Taxes".into()],
            matched.clone(),
        ));
        input.rule_evaluation.matched_rules.push(matched);
        let file_id = input.file_id;
        let mut build = request(vec![input]);
        build.overrides.push(OrganizationProposalOverride {
            id: ProposalOverrideId::new(),
            proposal_id: build.proposal_id,
            file_id,
            action: ProposalOverrideAction::Destination,
            destination: Some(vec!["Business".into(), "Chosen".into()]),
            proposed_name: None,
            reason: Some("User choice".into()),
            created_at: build.created_at.clone(),
            updated_at: build.updated_at.clone(),
        });
        let proposal = LocalOrganizationProposalEngine.build(build, &|| false, &mut |_| {});
        assert_eq!(
            proposal.operations[0].proposed_destination,
            ["Business", "Chosen"]
        );
        assert!(proposal.operations[0].user_override);
        assert!(
            proposal.operations[0]
                .reasons
                .iter()
                .any(|reason| reason.code == "user_rule")
        );
    }

    #[test]
    fn cancellation_returns_consistent_partial_proposal() {
        let inputs = (0..10)
            .map(|index| source(&format!("scan_{index}.pdf")))
            .collect();
        let calls = AtomicUsize::new(0);
        let proposal = LocalOrganizationProposalEngine.build(
            request(inputs),
            &|| calls.fetch_add(1, Ordering::Relaxed) >= 3,
            &mut |_| {},
        );
        assert_eq!(proposal.status, OrganizationProposalStatus::Cancelled);
        assert_eq!(proposal.operations.len(), 3);
    }

    #[test]
    fn ten_thousand_file_scale_build_is_complete_and_bounded() {
        let inputs = (0..10_000)
            .map(|index| {
                let mut input = source(&format!("scan_{index:05}.pdf"));
                input.identifier = Some(signal(&format!("INV-{index:05}"), 0.98));
                input
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let mut tree_started = None;
        let mut tree_finished = None;
        let proposal =
            LocalOrganizationProposalEngine.build(request(inputs), &|| false, &mut |progress| {
                match progress.phase {
                    ProposalBuildPhase::BuildingTree => {
                        tree_started.get_or_insert_with(std::time::Instant::now);
                    }
                    ProposalBuildPhase::Completed => {
                        tree_finished = Some(std::time::Instant::now())
                    }
                    _ => {}
                }
            });
        let elapsed = started.elapsed();
        let tree_time = tree_started
            .zip(tree_finished)
            .map_or(std::time::Duration::ZERO, |(start, finish)| {
                finish.duration_since(start)
            });
        println!(
            "M7_SCALE files={} generation_ms={} tree_ms={} conflicts={} review={} average_depth={:.2} maximum_depth={}",
            proposal.summary.files_analyzed,
            elapsed.saturating_sub(tree_time).as_millis(),
            tree_time.as_millis(),
            proposal.summary.conflicts,
            proposal.summary.needs_review,
            proposal.summary.average_depth,
            proposal.summary.maximum_depth,
        );
        assert_eq!(proposal.summary.files_analyzed, 10_000);
        assert_eq!(proposal.operations.len(), 10_000);
        assert!(proposal.summary.maximum_depth <= 6);
        assert!(proposal.nodes.len() >= 10_000);
    }
}
