use crate::{ApplicationError, ScannerApplicationService};
use domain::WorkspaceId;
use knowledge::{IdentityResolutionPolicy, ResolutionDecision, assess_match, blocking_keys};
use persistence::{
    IdentityCandidateAction, IdentityDetailRecord, IdentityMutationRecord,
    IdentityResolverRunRecord, IdentityReviewPageRecord,
};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityResolutionPhase {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityResolutionProgress {
    pub run_id: String,
    pub workspace_id: WorkspaceId,
    pub phase: IdentityResolutionPhase,
    pub files_considered: u64,
    pub occurrences_processed: u64,
    pub blocking_memberships: u64,
    pub comparisons: u64,
    pub candidates_created: u64,
    pub auto_links_created: u64,
}

impl ScannerApplicationService {
    pub fn resolve_workspace_identities(
        &self,
        workspace_id: WorkspaceId,
        trigger_kind: &str,
        force: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(IdentityResolutionProgress),
    ) -> Result<IdentityResolverRunRecord, ApplicationError> {
        let run = self
            .database
            .begin_identity_resolver_run(workspace_id, trigger_kind)?;
        let mut progress = progress_from_run(&run, IdentityResolutionPhase::Running);
        on_progress(progress.clone());
        let result = self.process_identity_run(
            workspace_id,
            &run.run_id,
            force,
            is_cancelled,
            &mut progress,
            on_progress,
        );
        let cancelled = is_cancelled();
        let (status, phase, error_message) = if cancelled {
            ("cancelled", IdentityResolutionPhase::Cancelled, None)
        } else if result.is_err() {
            (
                "failed",
                IdentityResolutionPhase::Failed,
                Some("local identity resolution stopped unexpectedly"),
            )
        } else {
            ("completed", IdentityResolutionPhase::Completed, None)
        };
        let final_run = self.database.finish_identity_resolver_run(
            &run.run_id,
            status,
            progress.files_considered,
            progress.occurrences_processed,
            progress.blocking_memberships,
            progress.comparisons,
            progress.candidates_created,
            progress.auto_links_created,
            error_message,
        )?;
        on_progress(progress_from_run(&final_run, phase));
        result?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(final_run)
    }

    pub fn identity_review_groups(
        &self,
        workspace_id: WorkspaceId,
        status: &str,
        limit: usize,
        offset: usize,
    ) -> Result<IdentityReviewPageRecord, ApplicationError> {
        self.database
            .identity_review_groups(workspace_id, status, limit, offset)
            .map_err(ApplicationError::Persistence)
    }

    pub fn identity_detail(
        &self,
        identity_id: &str,
    ) -> Result<IdentityDetailRecord, ApplicationError> {
        validate_uuid(identity_id)?;
        self.database
            .identity_detail(identity_id)
            .map_err(ApplicationError::Persistence)
    }

    pub fn decide_identity_candidate(
        &self,
        candidate_id: &str,
        action: IdentityCandidateAction,
        reason: Option<&str>,
    ) -> Result<IdentityMutationRecord, ApplicationError> {
        validate_uuid(candidate_id)?;
        validate_reason(reason)?;
        self.database
            .decide_identity_candidate(candidate_id, action, reason)
            .map_err(ApplicationError::Persistence)
    }

    pub fn merge_identity_records(
        &self,
        primary_identity_id: &str,
        secondary_identity_id: &str,
        reason: Option<&str>,
    ) -> Result<IdentityMutationRecord, ApplicationError> {
        validate_uuid(primary_identity_id)?;
        validate_uuid(secondary_identity_id)?;
        if primary_identity_id == secondary_identity_id {
            return Err(ApplicationError::InvalidIdentityDecision);
        }
        validate_reason(reason)?;
        self.database
            .merge_identity_records(primary_identity_id, secondary_identity_id, reason)
            .map_err(ApplicationError::Persistence)
    }

    pub fn unlink_identity_occurrence(
        &self,
        identity_id: &str,
        occurrence_id: &str,
        reason: Option<&str>,
    ) -> Result<IdentityMutationRecord, ApplicationError> {
        validate_uuid(identity_id)?;
        validate_uuid(occurrence_id)?;
        validate_reason(reason)?;
        self.database
            .unlink_identity_occurrence(identity_id, occurrence_id, reason)
            .map_err(ApplicationError::Persistence)
    }

    pub(crate) fn resolve_after_semantic_batch(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<IdentityResolverRunRecord, ApplicationError> {
        self.resolve_workspace_identities(
            workspace_id,
            "semantic_analysis",
            false,
            is_cancelled,
            &mut |_| {},
        )
    }

    pub(crate) fn resolve_after_semantic_correction(
        &self,
        file_id: &str,
    ) -> Result<IdentityResolverRunRecord, ApplicationError> {
        let workspace_id = self.database.identity_workspace_for_file(file_id)?;
        self.database
            .invalidate_identity_resolution_for_file(file_id)?;
        self.resolve_workspace_identities(
            workspace_id,
            "semantic_correction",
            false,
            &|| false,
            &mut |_| {},
        )
    }

    fn process_identity_run(
        &self,
        workspace_id: WorkspaceId,
        run_id: &str,
        force: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        progress: &mut IdentityResolutionProgress,
        on_progress: &mut dyn FnMut(IdentityResolutionProgress),
    ) -> Result<(), ApplicationError> {
        let policy = IdentityResolutionPolicy::default();
        let mut assessed_pairs = HashSet::<(String, String)>::new();
        loop {
            if is_cancelled() {
                break;
            }
            let files = self
                .database
                .identity_files_to_process(workspace_id, run_id, force, 32)?;
            if files.is_empty() {
                break;
            }
            for file_id in files {
                if is_cancelled() {
                    break;
                }
                let sync = self
                    .database
                    .synchronize_identity_occurrences(&file_id, run_id)?;
                progress.files_considered = progress.files_considered.saturating_add(1);
                for occurrence_id in &sync.occurrence_ids {
                    if is_cancelled() {
                        break;
                    }
                    let left = self.database.identity_occurrence(occurrence_id)?;
                    progress.occurrences_processed =
                        progress.occurrences_processed.saturating_add(1);
                    progress.blocking_memberships = progress.blocking_memberships.saturating_add(
                        u64::try_from(blocking_keys(&left).len()).unwrap_or(u64::MAX),
                    );
                    let candidates = self
                        .database
                        .blocked_identity_occurrences(occurrence_id, 64)?;
                    for right in candidates {
                        if is_cancelled() {
                            break;
                        }
                        let pair =
                            ordered_occurrence_pair(&left.occurrence_key, &right.occurrence_key);
                        if !assessed_pairs.insert(pair) {
                            continue;
                        }
                        progress.comparisons = progress.comparisons.saturating_add(1);
                        let assessment = assess_match(&left, &right, policy);
                        if assessment.decision == ResolutionDecision::Unknown {
                            continue;
                        }
                        let stored = self.database.store_identity_candidate(
                            &left.occurrence_key,
                            &right.occurrence_key,
                            &assessment,
                            if force { "resolver" } else { "incremental" },
                        )?;
                        if stored.created {
                            progress.candidates_created =
                                progress.candidates_created.saturating_add(1);
                        }
                        if stored.status == "auto_linked" {
                            progress.auto_links_created =
                                progress.auto_links_created.saturating_add(1);
                        }
                    }
                }
                self.database.mark_identity_file_resolution(
                    &file_id,
                    run_id,
                    if is_cancelled() {
                        "cancelled"
                    } else {
                        "completed"
                    },
                )?;
                on_progress(progress.clone());
            }
        }
        Ok(())
    }
}

fn progress_from_run(
    run: &IdentityResolverRunRecord,
    phase: IdentityResolutionPhase,
) -> IdentityResolutionProgress {
    IdentityResolutionProgress {
        run_id: run.run_id.clone(),
        workspace_id: run.workspace_id,
        phase,
        files_considered: run.files_considered,
        occurrences_processed: run.occurrences_processed,
        blocking_memberships: run.blocking_memberships,
        comparisons: run.comparisons,
        candidates_created: run.candidates_created,
        auto_links_created: run.auto_links_created,
    }
}

fn ordered_occurrence_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_owned(), right.to_owned())
    } else {
        (right.to_owned(), left.to_owned())
    }
}

fn validate_uuid(value: &str) -> Result<(), ApplicationError> {
    value
        .parse::<uuid::Uuid>()
        .map(|_| ())
        .map_err(|_| ApplicationError::InvalidIdentityDecision)
}

fn validate_reason(reason: Option<&str>) -> Result<(), ApplicationError> {
    if reason.is_some_and(|value| value.chars().count() > 512 || value.contains('\0')) {
        return Err(ApplicationError::InvalidIdentityDecision);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrence_pair_order_is_deterministic() {
        assert_eq!(
            ordered_occurrence_pair("z", "a"),
            ("a".to_owned(), "z".to_owned())
        );
    }
}
