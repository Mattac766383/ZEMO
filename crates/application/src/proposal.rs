use crate::{ApplicationError, ScannerApplicationService};
use domain::{
    FileId, FileVersionId, LocalRule, OrganizationProposal, OrganizationProposalOverride,
    OrganizationProposalStatus, OrganizationRevisionId, ProposalId, ProposalOverrideAction,
    ProposalOverrideId, RootId, RuleFileMatch, SemanticRuleField, WorkspaceId,
};
use organizer::{
    BehaviorSignal, ConsumerRootKind, IncrementalOrganizationBuildRequest,
    LocalOrganizationProposalEngine, LocalRuleEngine, OrganizationBuildOutcome,
    OrganizationBuildRequest, OrganizationSourceInput, ProposalBuildProgress, ProposalRebuildMode,
    ProposalRelationship, ProposalSignal, RuleEvaluationContext, VirtualPathPolicy,
    compute_invalidation_neighborhood,
};
use persistence::{
    PersistenceError, ProposalRelationshipSourceRecord, ProposalSemanticSignalRecord,
    ProposalSourceFileRecord,
};
use std::collections::HashSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

impl ScannerApplicationService {
    pub fn generate_organization_proposal(
        &self,
        workspace_id: WorkspaceId,
        recompute_current: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationProposal, ApplicationError> {
        let root_id = self.database.unambiguous_organization_root(workspace_id)?;
        self.generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            recompute_current,
            is_cancelled,
            on_progress,
        )
    }

    pub fn generate_organization_proposal_for_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        recompute_current: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationProposal, ApplicationError> {
        let current_id = self
            .database
            .current_organization_proposal_id_for_root(workspace_id, root_id)?;
        let proposal_id = if recompute_current {
            current_id.unwrap_or_default()
        } else {
            ProposalId::new()
        };
        let trigger_kind = if recompute_current && current_id.is_some() {
            "manual_recompute"
        } else {
            "initial"
        };
        self.build_organization_proposal(
            workspace_id,
            root_id,
            proposal_id,
            trigger_kind,
            false,
            is_cancelled,
            on_progress,
        )
    }

    pub fn generate_consumer_organization_proposal_for_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        recompute_current: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationProposal, ApplicationError> {
        let current_id = self
            .database
            .current_organization_proposal_id_for_root(workspace_id, root_id)?;
        let proposal_id = if recompute_current {
            current_id.unwrap_or_default()
        } else {
            ProposalId::new()
        };
        let trigger_kind = if recompute_current && current_id.is_some() {
            "manual_recompute"
        } else {
            "initial"
        };
        self.build_organization_proposal(
            workspace_id,
            root_id,
            proposal_id,
            trigger_kind,
            true,
            is_cancelled,
            on_progress,
        )
    }

    /// Incrementally update the current root proposal for a dirty file set.
    /// Falls back to a full rebuild when correctness cannot be proven.
    pub fn update_organization_proposal_incrementally(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        dirty_file_ids: &[FileId],
        deleted_file_ids: &[FileId],
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationBuildOutcome, ApplicationError> {
        let current_id = self
            .database
            .current_organization_proposal_id_for_root(workspace_id, root_id)?;
        let Some(proposal_id) = current_id else {
            let proposal = self.build_organization_proposal(
                workspace_id,
                root_id,
                ProposalId::new(),
                "initial",
                false,
                is_cancelled,
                on_progress,
            )?;
            return Ok(OrganizationBuildOutcome {
                dirty_file_count: proposal.summary.files_analyzed,
                proposal,
                rebuild_mode: ProposalRebuildMode::Full,
                rebuild_reason: Some("no_previous_proposal".to_owned()),
                affected_file_ids: Vec::new(),
            });
        };
        self.build_organization_proposal_incremental(
            workspace_id,
            root_id,
            proposal_id,
            dirty_file_ids,
            deleted_file_ids,
            "semantic_changed",
            is_cancelled,
            on_progress,
        )
    }

    pub fn latest_organization_proposal(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<OrganizationProposal, ApplicationError> {
        self.database
            .latest_organization_proposal(workspace_id)
            .map_err(ApplicationError::Persistence)
    }

    pub fn latest_organization_proposal_for_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
    ) -> Result<OrganizationProposal, ApplicationError> {
        self.database
            .latest_organization_proposal_for_root(workspace_id, root_id)
            .map_err(ApplicationError::Persistence)
    }

    pub fn latest_organization_proposal_for_ui(
        &self,
        workspace_id: WorkspaceId,
        root_id: Option<RootId>,
        operation_limit: usize,
    ) -> Result<OrganizationProposal, ApplicationError> {
        self.database
            .latest_organization_proposal_for_ui(workspace_id, root_id, operation_limit)
            .map_err(ApplicationError::Persistence)
    }

    pub fn organization_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<OrganizationProposal, ApplicationError> {
        self.database
            .organization_proposal(proposal_id)
            .map_err(ApplicationError::Persistence)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_organization_proposal_override(
        &self,
        proposal_id: ProposalId,
        file_id: FileId,
        action: ProposalOverrideAction,
        destination: Option<Vec<String>>,
        proposed_name: Option<String>,
        reason: Option<String>,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationProposal, ApplicationError> {
        let current = self.database.organization_proposal(proposal_id)?;
        let preferences = self
            .database
            .organization_preferences(current.workspace_id)?;
        let path_policy = VirtualPathPolicy {
            maximum_depth: preferences.maximum_depth,
            ..VirtualPathPolicy::default()
        };
        if let Some(segments) = &destination {
            path_policy
                .validate_user_destination(segments)
                .map_err(|_| ApplicationError::InvalidOrganizationProposal)?;
        }
        if let Some(filename) = &proposed_name {
            path_policy
                .validate_user_filename(filename)
                .map_err(|_| ApplicationError::InvalidOrganizationProposal)?;
        }
        if let (Some(segments), Some(filename)) = (&destination, &proposed_name)
            && path_policy.path_length_utf16(segments, filename) > path_policy.maximum_path_utf16
        {
            return Err(ApplicationError::InvalidOrganizationProposal);
        }
        let now = now_iso();
        let source_operation = current
            .operations
            .iter()
            .find(|operation| operation.file_id == file_id)
            .cloned();
        let stored_override = OrganizationProposalOverride {
            id: ProposalOverrideId::new(),
            proposal_id,
            file_id,
            action,
            destination,
            proposed_name,
            reason: reason.map(|value| value.chars().take(512).collect()),
            created_at: now.clone(),
            updated_at: now,
        };
        self.database
            .store_organization_override(&stored_override)?;
        if let Some(operation) = source_operation.as_ref() {
            self.observe_organization_override(current.workspace_id, &stored_override, operation)?;
        }
        self.build_organization_proposal(
            current.workspace_id,
            current.root_id,
            proposal_id,
            "user_override",
            false,
            is_cancelled,
            on_progress,
        )
    }

    pub fn set_organization_proposal_status(
        &self,
        proposal_id: ProposalId,
        status: OrganizationProposalStatus,
    ) -> Result<OrganizationProposal, ApplicationError> {
        self.database
            .set_organization_proposal_status(proposal_id, status)
            .map_err(ApplicationError::Persistence)
    }

    pub fn refresh_organization_proposal_drift(
        &self,
        proposal_id: ProposalId,
    ) -> Result<(u64, OrganizationProposal), ApplicationError> {
        let changed = self
            .database
            .refresh_organization_proposal_drift(proposal_id)?;
        let proposal = self.database.organization_proposal(proposal_id)?;
        Ok((changed, proposal))
    }

    pub(crate) fn build_organization_proposal(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        proposal_id: ProposalId,
        trigger_kind: &str,
        consumer_mode: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationProposal, ApplicationError> {
        let outcome = self.build_organization_proposal_full(
            workspace_id,
            root_id,
            proposal_id,
            trigger_kind,
            None,
            consumer_mode,
            is_cancelled,
            on_progress,
        )?;
        Ok(outcome.proposal)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_organization_proposal_full(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        proposal_id: ProposalId,
        trigger_kind: &str,
        rebuild_reason: Option<&str>,
        consumer_mode: bool,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationBuildOutcome, ApplicationError> {
        let source = self
            .database
            .organization_source_for_root(workspace_id, root_id)?;
        let mut preferences = self.database.organization_preferences(workspace_id)?;
        if consumer_mode {
            preferences.maximum_depth = 3;
            preferences.include_year_folders = false;
            preferences.naming_language = "fr".to_owned();
        }
        let consumer_root_kind = consumer_root_kind(self, workspace_id, root_id);
        let rules = self.database.rules(workspace_id)?;
        let revision = self
            .database
            .organization_proposal_revision_number(proposal_id)?
            .saturating_add(1);
        let previous = self.database.organization_proposal(proposal_id).ok();
        let overrides = if previous.is_some() {
            self.database.organization_proposal_overrides(proposal_id)?
        } else {
            Vec::new()
        };
        let now = now_iso();
        let created_at = previous
            .as_ref()
            .map_or_else(|| now.clone(), |proposal| proposal.created_at.clone());
        let inputs = source
            .files
            .into_iter()
            .map(|source| source_input(source, &rules))
            .collect::<Result<Vec<_>, _>>()?;
        let matches = inputs
            .iter()
            .flat_map(|input| {
                input
                    .rule_evaluation
                    .matched_rules
                    .iter()
                    .map(move |matched| RuleFileMatch {
                        rule_id: matched.id,
                        workspace_id,
                        file_id: input.file_id,
                        boost: 0.15,
                        explanation: format!("Matched your rule: {}", matched.explanation.trim()),
                    })
            })
            .collect::<Vec<_>>();
        self.database
            .replace_rule_file_matches(workspace_id, &matches)?;
        let outcome = LocalOrganizationProposalEngine.build_with_mode(
            OrganizationBuildRequest {
                proposal_id,
                revision_id: OrganizationRevisionId::new(),
                workspace_id,
                root_id: source.root_id,
                source_scan_id: source.scan_id,
                revision,
                created_at,
                updated_at: now,
                source_semantic_version: source.semantic_version,
                source_relationship_version: source.relationship_version,
                preferences,
                inputs,
                overrides,
                previous_operations: previous
                    .map(|proposal| proposal.operations)
                    .unwrap_or_default(),
                consumer_mode,
                consumer_root_kind,
            },
            is_cancelled,
            on_progress,
        );
        let mut outcome = outcome;
        if let Some(reason) = rebuild_reason {
            outcome.rebuild_reason = Some(reason.to_owned());
        }
        self.database.persist_organization_proposal_with_meta(
            &outcome.proposal,
            trigger_kind,
            outcome.rebuild_mode.database_name(),
            outcome.rebuild_reason.as_deref(),
            outcome.dirty_file_count,
        )?;
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_organization_proposal_incremental(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        proposal_id: ProposalId,
        dirty_file_ids: &[FileId],
        deleted_file_ids: &[FileId],
        trigger_kind: &str,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<OrganizationBuildOutcome, ApplicationError> {
        let previous = self.database.organization_proposal(proposal_id)?;
        let preferences = self.database.organization_preferences(workspace_id)?;
        let rules = self.database.rules(workspace_id)?;
        let neighborhood = compute_invalidation_neighborhood(
            dirty_file_ids,
            deleted_file_ids,
            &previous.operations,
        );
        if neighborhood.len() > organizer::INCREMENTAL_NEIGHBORHOOD_LIMIT
            || dirty_file_ids.len() + deleted_file_ids.len()
                > organizer::INCREMENTAL_DIRTY_FILE_LIMIT
        {
            return self.build_organization_proposal_full(
                workspace_id,
                root_id,
                proposal_id,
                "manual_recompute",
                Some("neighborhood_or_dirty_limit"),
                false,
                is_cancelled,
                on_progress,
            );
        }

        let neighborhood_ids = neighborhood
            .into_iter()
            .filter(|file_id| !deleted_file_ids.contains(file_id))
            .collect::<Vec<_>>();
        let source = self.database.organization_source_for_files(
            workspace_id,
            root_id,
            &neighborhood_ids,
        )?;
        let revision = self
            .database
            .organization_proposal_revision_number(proposal_id)?
            .saturating_add(1);
        let overrides = self.database.organization_proposal_overrides(proposal_id)?;
        let now = now_iso();
        let dirty_inputs = source
            .files
            .into_iter()
            .map(|source| source_input(source, &rules))
            .collect::<Result<Vec<_>, _>>()?;

        // Refresh rule matches for the affected neighborhood only, then leave
        // unrelated matches intact by merging into a workspace replace of known files.
        let mut match_file_ids = dirty_inputs
            .iter()
            .map(|input| input.file_id)
            .collect::<HashSet<_>>();
        match_file_ids.extend(deleted_file_ids.iter().copied());
        let matches = dirty_inputs
            .iter()
            .flat_map(|input| {
                input
                    .rule_evaluation
                    .matched_rules
                    .iter()
                    .map(move |matched| RuleFileMatch {
                        rule_id: matched.id,
                        workspace_id,
                        file_id: input.file_id,
                        boost: 0.15,
                        explanation: format!("Matched your rule: {}", matched.explanation.trim()),
                    })
            })
            .collect::<Vec<_>>();
        self.database.replace_rule_file_matches_for_files(
            workspace_id,
            &match_file_ids.into_iter().collect::<Vec<_>>(),
            &matches,
        )?;

        let previous_revision_id = previous.revision_id;
        let outcome = LocalOrganizationProposalEngine.build_incremental(
            IncrementalOrganizationBuildRequest {
                base: OrganizationBuildRequest {
                    proposal_id,
                    revision_id: OrganizationRevisionId::new(),
                    workspace_id,
                    root_id,
                    source_scan_id: source.scan_id,
                    revision,
                    created_at: previous.created_at,
                    updated_at: now,
                    source_semantic_version: source.semantic_version,
                    source_relationship_version: source.relationship_version,
                    preferences,
                    inputs: Vec::new(),
                    overrides,
                    previous_operations: previous.operations,
                    consumer_mode: false,
                    consumer_root_kind: consumer_root_kind(self, workspace_id, root_id),
                },
                dirty_file_ids: dirty_file_ids.to_vec(),
                neighborhood_inputs: dirty_inputs,
                deleted_file_ids: deleted_file_ids.to_vec(),
                force_full_rebuild: false,
            },
            is_cancelled,
            on_progress,
        );

        if outcome.rebuild_mode == ProposalRebuildMode::Full {
            // Engine declined incremental correctness; run authoritative full rebuild.
            return self.build_organization_proposal_full(
                workspace_id,
                root_id,
                proposal_id,
                "manual_recompute",
                outcome.rebuild_reason.as_deref(),
                false,
                is_cancelled,
                on_progress,
            );
        }

        let changed_ids = outcome
            .affected_file_ids
            .iter()
            .copied()
            .chain(deleted_file_ids.iter().copied())
            .collect::<HashSet<_>>();
        self.database.persist_organization_proposal_incremental(
            &outcome.proposal,
            trigger_kind,
            previous_revision_id,
            &changed_ids,
            outcome.rebuild_mode.database_name(),
            outcome.rebuild_reason.as_deref(),
            outcome.dirty_file_count,
        )?;
        Ok(outcome)
    }

    pub(crate) fn refresh_local_rule_matches(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(), ApplicationError> {
        let rules = self.database.rules(workspace_id)?;
        let mut inputs = Vec::new();
        for root in self.database.list_roots(workspace_id)? {
            match self
                .database
                .organization_source_for_root(workspace_id, root.id)
            {
                Ok(source) => inputs.extend(
                    source
                        .files
                        .into_iter()
                        .map(|source| source_input(source, &rules))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Err(PersistenceError::NotFound) => {}
                Err(error) => return Err(ApplicationError::Persistence(error)),
            }
        }
        let matches = inputs
            .iter()
            .flat_map(|input| {
                input
                    .rule_evaluation
                    .matched_rules
                    .iter()
                    .map(move |matched| RuleFileMatch {
                        rule_id: matched.id,
                        workspace_id,
                        file_id: input.file_id,
                        boost: 0.15,
                        explanation: format!("Matched your rule: {}", matched.explanation.trim()),
                    })
            })
            .collect::<Vec<_>>();
        self.database
            .replace_rule_file_matches(workspace_id, &matches)?;
        Ok(())
    }
}

pub(crate) fn source_input(
    source: ProposalSourceFileRecord,
    rules: &[LocalRule],
) -> Result<OrganizationSourceInput, ApplicationError> {
    let mut input = OrganizationSourceInput {
        file_id: source
            .file_id
            .parse::<FileId>()
            .map_err(|_| ApplicationError::InvalidOrganizationProposal)?,
        file_version_id: source
            .file_version_id
            .parse::<FileVersionId>()
            .map_err(|_| ApplicationError::InvalidOrganizationProposal)?,
        source_relative_path: source.relative_path,
        source_name: source.filename,
        byte_size: source.byte_size,
        modified_at: source.modified_at,
        content_hash: source.content_hash,
        extraction_status: source.extraction_status,
        semantic_status: source.semantic_status,
        input_quality: source.input_quality,
        context: source.context.map(signal),
        document_type: source.document_type.map(signal),
        issue_date: source.issue_date.map(signal),
        identifier: source.identifier.map(signal),
        amount: source.amount.map(signal),
        currency: source.currency.map(signal),
        relationships: source.relationships.into_iter().map(relationship).collect(),
        review_reasons: source.review_reasons,
        duplicate_group_id: source.duplicate_group_id,
        duplicate_canonical: source.duplicate_canonical,
        rule_evaluation: Default::default(),
    };
    let context = rule_context(&input);
    let evaluation = LocalRuleEngine.evaluate(&context, rules);
    apply_semantic_rule_overlays(&mut input, &evaluation);
    input.rule_evaluation = evaluation;
    Ok(input)
}

fn rule_context(input: &OrganizationSourceInput) -> RuleEvaluationContext {
    let relationship = |kind: &str| {
        input
            .relationships
            .iter()
            .filter(|value| value.relationship_type == kind)
            .max_by(|left, right| {
                left.confidence
                    .partial_cmp(&right.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| right.identity_id.cmp(&left.identity_id))
            })
            .map(|value| BehaviorSignal {
                value: value.display_name.clone(),
                confidence: value.confidence,
                user_confirmed: value.user_confirmed || value.status == "user_confirmed",
            })
    };
    let signal = |value: &Option<ProposalSignal>| {
        value.as_ref().map(|value| BehaviorSignal {
            value: value.value.clone(),
            confidence: value.confidence,
            user_confirmed: value.user_confirmed,
        })
    };
    RuleEvaluationContext {
        source_path: input.source_relative_path.clone(),
        document_type: signal(&input.document_type),
        context: signal(&input.context),
        supplier: relationship("file_supplier"),
        customer: relationship("file_customer"),
        project: relationship("file_project"),
        parties: input
            .relationships
            .iter()
            .map(|value| BehaviorSignal {
                value: value.display_name.clone(),
                confidence: value.confidence,
                user_confirmed: value.user_confirmed || value.status == "user_confirmed",
            })
            .collect(),
    }
}

fn apply_semantic_rule_overlays(
    input: &mut OrganizationSourceInput,
    evaluation: &organizer::RuleEvaluation,
) {
    if let Some((value, _)) = evaluation
        .semantic_overrides
        .get(&SemanticRuleField::DocumentType)
    {
        input.document_type = Some(rule_signal(value));
    }
    if let Some((value, _)) = evaluation
        .semantic_overrides
        .get(&SemanticRuleField::Context)
    {
        input.context = Some(rule_signal(value));
    }

    let semantic_party = |field| {
        evaluation
            .semantic_overrides
            .get(&field)
            .map(|(value, rule)| (value.clone(), rule.clone()))
    };
    let preferred_party = |semantic, classified: &Option<(String, organizer::MatchedRule)>| match (
        semantic,
        classified.clone(),
    ) {
        (Some(left), Some(right)) => {
            if left.1.position <= right.1.position {
                Some(left)
            } else {
                Some(right)
            }
        }
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    };
    let supplier = preferred_party(
        semantic_party(SemanticRuleField::Supplier),
        &evaluation.classified_supplier,
    );
    let customer = preferred_party(
        semantic_party(SemanticRuleField::Customer),
        &evaluation.classified_customer,
    );
    let project = semantic_party(SemanticRuleField::Project);
    for (kind, party) in [
        ("file_supplier", supplier),
        ("file_customer", customer),
        ("file_project", project),
    ] {
        let Some((display_name, matched)) = party else {
            continue;
        };
        input
            .relationships
            .retain(|value| value.relationship_type != kind);
        input.relationships.push(ProposalRelationship {
            relationship_type: kind.to_owned(),
            identity_id: format!("rule:{}", matched.id),
            display_name,
            confidence: 1.0,
            status: "user_rule".to_owned(),
            user_confirmed: true,
            project_customer_name: None,
        });
    }
}

fn rule_signal(value: &str) -> ProposalSignal {
    ProposalSignal {
        value: value.to_owned(),
        confidence: 1.0,
        status: "user_rule".to_owned(),
        user_confirmed: true,
    }
}

fn signal(source: ProposalSemanticSignalRecord) -> ProposalSignal {
    ProposalSignal {
        value: source.value,
        confidence: source.confidence,
        status: source.status,
        user_confirmed: source.user_confirmed,
    }
}

fn relationship(source: ProposalRelationshipSourceRecord) -> ProposalRelationship {
    ProposalRelationship {
        relationship_type: source.relationship_type,
        identity_id: source.identity_id,
        display_name: source.display_name,
        confidence: source.confidence,
        status: source.status,
        user_confirmed: source.user_confirmed,
        project_customer_name: source.project_customer_name,
    }
}

fn consumer_root_kind(
    service: &ScannerApplicationService,
    workspace_id: WorkspaceId,
    root_id: RootId,
) -> ConsumerRootKind {
    service
        .database
        .list_roots(workspace_id)
        .ok()
        .and_then(|roots| roots.into_iter().find(|root| root.id == root_id))
        .map(|root| ConsumerRootKind::from_path_and_label(&root.absolute_path, &root.display_label))
        .unwrap_or_default()
}

fn now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}
