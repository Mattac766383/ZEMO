use crate::{ApplicationError, ScannerApplicationService};
use domain::{
    LearningObservationInput, LearningPatternKind, LearningSourceKind, LocalRule, LocalRuleInput,
    OrganizationPreferences, OrganizationProposal, OrganizationProposalOperation,
    OrganizationProposalOverride, RuleAction, RuleCondition, RuleField, RuleId, RuleOperator,
    RuleOrigin, RuleSuggestion, RuleSuggestionId, RuleSuggestionSeed, SemanticRuleField,
    WorkspaceId,
};
use organizer::{ProposalBuildProgress, VirtualPathPolicy, validate_rule};
use persistence::{PersistenceError, SemanticCorrectionRecord};

#[derive(Debug, Clone)]
pub struct RulesPreferencesState {
    pub rules: Vec<LocalRule>,
    pub suggestions: Vec<RuleSuggestion>,
    pub preferences: OrganizationPreferences,
}

impl ScannerApplicationService {
    pub fn rules_preferences_state(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<RulesPreferencesState, ApplicationError> {
        self.database.workspace(workspace_id)?;
        Ok(RulesPreferencesState {
            rules: self.database.rules(workspace_id)?,
            suggestions: self.database.rule_suggestions(workspace_id)?,
            preferences: self.database.organization_preferences(workspace_id)?,
        })
    }

    pub fn create_local_rule(
        &self,
        workspace_id: WorkspaceId,
        input: &LocalRuleInput,
    ) -> Result<LocalRule, ApplicationError> {
        self.validate_rule_input(workspace_id, input)?;
        let rule = self.database.create_rule(workspace_id, input)?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(rule)
    }

    pub fn update_local_rule(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
        input: &LocalRuleInput,
    ) -> Result<LocalRule, ApplicationError> {
        self.validate_rule_input(workspace_id, input)?;
        let rule = self.database.update_rule(workspace_id, rule_id, input)?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(rule)
    }

    pub fn set_local_rule_enabled(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
        enabled: bool,
    ) -> Result<LocalRule, ApplicationError> {
        let rule = self
            .database
            .set_rule_enabled(workspace_id, rule_id, enabled)?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(rule)
    }

    pub fn delete_local_rule(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
    ) -> Result<(), ApplicationError> {
        self.database.delete_rule(workspace_id, rule_id)?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(())
    }

    pub fn reorder_local_rules(
        &self,
        workspace_id: WorkspaceId,
        ordered_ids: &[RuleId],
    ) -> Result<Vec<LocalRule>, ApplicationError> {
        let rules = self.database.reorder_rules(workspace_id, ordered_ids)?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(rules)
    }

    pub fn store_local_organization_preferences(
        &self,
        workspace_id: WorkspaceId,
        preferences: &OrganizationPreferences,
    ) -> Result<OrganizationPreferences, ApplicationError> {
        self.database.workspace(workspace_id)?;
        self.database
            .store_organization_preferences(workspace_id, preferences)
            .map_err(ApplicationError::Persistence)
    }

    pub fn accept_local_rule_suggestion(
        &self,
        workspace_id: WorkspaceId,
        suggestion_id: RuleSuggestionId,
    ) -> Result<LocalRule, ApplicationError> {
        let suggested = self
            .database
            .rule_suggestions(workspace_id)?
            .into_iter()
            .find(|value| value.id == suggestion_id)
            .ok_or(ApplicationError::NotFound)?;
        self.validate_rule_input(workspace_id, &suggested.proposed_rule)?;
        let rule = self
            .database
            .accept_rule_suggestion(workspace_id, suggestion_id)?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(rule)
    }

    pub fn dismiss_local_rule_suggestion(
        &self,
        workspace_id: WorkspaceId,
        suggestion_id: RuleSuggestionId,
    ) -> Result<RuleSuggestion, ApplicationError> {
        self.database
            .dismiss_rule_suggestion(workspace_id, suggestion_id)
            .map_err(ApplicationError::Persistence)
    }

    pub fn recompute_after_rule_change(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ProposalBuildProgress),
    ) -> Result<Option<OrganizationProposal>, ApplicationError> {
        let proposals = self.database.current_organization_proposals(workspace_id)?;
        if proposals.is_empty() {
            return Ok(None);
        }
        let mut updated = None;
        for (root_id, proposal_id) in proposals {
            if is_cancelled() {
                break;
            }
            updated = Some(self.build_organization_proposal(
                workspace_id,
                root_id,
                proposal_id,
                "algorithm_changed",
                is_cancelled,
                on_progress,
            )?);
        }
        Ok(updated)
    }

    pub(crate) fn observe_semantic_correction(
        &self,
        correction: &SemanticCorrectionRecord,
    ) -> Result<Option<RuleSuggestion>, ApplicationError> {
        if correction.correction_state != "user_corrected" {
            return Ok(None);
        }
        let file_id = correction
            .file_id
            .parse::<domain::FileId>()
            .map_err(|_| ApplicationError::NotFound)?;
        let workspace_id = self.database.file_workspace_id(file_id)?;
        let detail = self.database.file_detail(&correction.file_id)?;
        let machine_value = detail
            .semantic_analysis
            .as_ref()
            .and_then(|analysis| {
                analysis
                    .fields
                    .iter()
                    .find(|field| field.field_key == correction.field_key)
            })
            .and_then(|field| field.machine_display_value.clone());
        let require_machine_value = || {
            machine_value
                .clone()
                .filter(|value| !value.trim().is_empty())
        };
        let (condition_field, action_field, condition_value) = match correction.field_key.as_str() {
            "document_type" => {
                let Some(value) = require_machine_value() else {
                    return Ok(None);
                };
                (
                    RuleField::DocumentType,
                    SemanticRuleField::DocumentType,
                    value,
                )
            }
            "context" => {
                let Some(value) = require_machine_value() else {
                    return Ok(None);
                };
                (RuleField::Context, SemanticRuleField::Context, value)
            }
            "supplier_candidate" => (
                RuleField::AnyParty,
                SemanticRuleField::Supplier,
                correction.display_value.clone(),
            ),
            "customer_candidate" => (
                RuleField::AnyParty,
                SemanticRuleField::Customer,
                correction.display_value.clone(),
            ),
            "project_reference_candidate" => {
                let Some(value) = require_machine_value() else {
                    return Ok(None);
                };
                (RuleField::Project, SemanticRuleField::Project, value)
            }
            _ => return Ok(None),
        };
        if condition_value.trim().is_empty() {
            return Ok(None);
        }
        let pattern_key = format!(
            "{}\0{}\0{}",
            correction.field_key,
            normalize_pattern(&condition_value),
            normalize_pattern(&correction.display_value)
        );
        let proposed_rule = LocalRuleInput {
            name: format!(
                "Use {} for {}",
                correction.display_value, correction.field_key
            )
            .chars()
            .take(120)
            .collect(),
            explanation: format!(
                "When {} is {}, use {}. Suggested from repeated corrections and enabled only if you accept it.",
                correction.field_key, condition_value, correction.display_value
            )
            .chars()
            .take(512)
            .collect(),
            enabled: true,
            conditions: vec![RuleCondition {
                field: condition_field,
                operator: RuleOperator::Equals,
                value: Some(condition_value.clone()),
            }],
            action: RuleAction::SetSemanticField {
                field: action_field,
                value: correction.display_value.clone(),
            },
        };
        let suggestion = self.database.record_learning_observation(
            workspace_id,
            &LearningObservationInput {
                file_id: Some(file_id),
                source_kind: LearningSourceKind::SemanticCorrection,
                source_ref: correction.correction_id.clone(),
                pattern_kind: LearningPatternKind::SemanticField,
                pattern_key,
                evidence: serde_json::json!({
                    "field": correction.field_key,
                    "machineValue": machine_value,
                    "correctedValue": correction.display_value,
                }),
            },
            Some(&RuleSuggestionSeed {
                title: format!("Create a reusable rule for {}?", correction.display_value)
                    .chars()
                    .take(200)
                    .collect(),
                explanation: "The same correction has been made at least three times. Nothing is created until you accept."
                    .to_owned(),
                proposed_rule,
            }),
        )?;
        self.refresh_rule_matches_if_available(workspace_id)?;
        Ok(suggestion)
    }

    pub(crate) fn observe_organization_override(
        &self,
        workspace_id: WorkspaceId,
        value: &OrganizationProposalOverride,
        operation: &OrganizationProposalOperation,
    ) -> Result<Option<RuleSuggestion>, ApplicationError> {
        let Some(supplier) = operation.supplier_name.as_deref() else {
            return Ok(None);
        };
        if operation.document_type != "invoice" || operation.project_name.is_none() {
            return Ok(None);
        }
        let pattern_key = format!("supplier_invoice_project\0{}", normalize_pattern(supplier));
        let proposed_rule = LocalRuleInput {
            name: format!("Keep {supplier} invoices inside projects")
                .chars()
                .take(120)
                .collect(),
            explanation: format!(
                "Supplier invoices from {supplier} linked to a project stay inside that project."
            )
            .chars()
            .take(512)
            .collect(),
            enabled: true,
            conditions: vec![
                RuleCondition {
                    field: RuleField::DocumentType,
                    operator: RuleOperator::Equals,
                    value: Some("invoice".to_owned()),
                },
                RuleCondition {
                    field: RuleField::Supplier,
                    operator: RuleOperator::Equals,
                    value: Some(supplier.to_owned()),
                },
                RuleCondition {
                    field: RuleField::Project,
                    operator: RuleOperator::Exists,
                    value: None,
                },
            ],
            action: RuleAction::PreferProjectLocation,
        };
        self.database
            .record_learning_observation(
                workspace_id,
                &LearningObservationInput {
                    file_id: Some(value.file_id),
                    source_kind: LearningSourceKind::OrganizationOverride,
                    source_ref: value.id.to_string(),
                    pattern_kind: LearningPatternKind::ProjectSupplierInvoice,
                    pattern_key,
                    evidence: serde_json::json!({
                        "supplier": supplier,
                        "project": operation.project_name,
                        "destination": value.destination,
                    }),
                },
                Some(&RuleSuggestionSeed {
                    title: format!(
                        "Keep {supplier} invoices in their project automatically?"
                    )
                    .chars()
                    .take(200)
                    .collect(),
                    explanation: "This organization correction has repeated at least three times. Accepting is required before it affects proposals."
                        .to_owned(),
                    proposed_rule,
                }),
            )
            .map_err(ApplicationError::Persistence)
    }

    fn validate_rule_input(
        &self,
        workspace_id: WorkspaceId,
        input: &LocalRuleInput,
    ) -> Result<(), ApplicationError> {
        let temporary = LocalRule {
            id: RuleId::new(),
            workspace_id,
            name: input.name.clone(),
            explanation: input.explanation.clone(),
            position: 0,
            enabled: input.enabled,
            conditions: input.conditions.clone(),
            action: input.action.clone(),
            origin: RuleOrigin::UserCreated,
            source_suggestion_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        validate_rule(&temporary).map_err(|_| ApplicationError::InvalidRule)?;
        if let RuleAction::SetDestination { segments } = &input.action {
            let preferences = self.database.organization_preferences(workspace_id)?;
            VirtualPathPolicy {
                maximum_depth: preferences.maximum_depth.clamp(2, 8),
                ..VirtualPathPolicy::default()
            }
            .validate_user_destination(segments)
            .map_err(|_| ApplicationError::InvalidRule)?;
        }
        Ok(())
    }

    pub(crate) fn refresh_rule_matches_if_available(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(), ApplicationError> {
        match self.refresh_local_rule_matches(workspace_id) {
            Ok(()) | Err(ApplicationError::Persistence(PersistenceError::NotFound)) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn normalize_pattern(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
