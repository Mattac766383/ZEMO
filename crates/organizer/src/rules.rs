use domain::{
    LocalRule, PolicyPrecedence, PrecedenceCandidate, ResolvedValue, RuleAction, RuleCondition,
    RuleField, RuleId, RuleOperator, RulePartyRole, SemanticRuleField,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct BehaviorSignal {
    pub value: String,
    pub confidence: f32,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleEvaluationContext {
    pub source_path: String,
    pub document_type: Option<BehaviorSignal>,
    pub context: Option<BehaviorSignal>,
    pub supplier: Option<BehaviorSignal>,
    pub customer: Option<BehaviorSignal>,
    pub project: Option<BehaviorSignal>,
    pub parties: Vec<BehaviorSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedRule {
    pub id: RuleId,
    pub name: String,
    pub explanation: String,
    pub position: u32,
    pub action: RuleAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleEvaluation {
    pub matched_rules: Vec<MatchedRule>,
    pub semantic_overrides: HashMap<SemanticRuleField, (String, MatchedRule)>,
    pub classified_supplier: Option<(String, MatchedRule)>,
    pub classified_customer: Option<(String, MatchedRule)>,
    pub prefer_project_location: Option<MatchedRule>,
    pub destination: Option<(Vec<String>, MatchedRule)>,
    pub preserve_subtree: Option<MatchedRule>,
    pub use_year_folders: Option<(bool, MatchedRule)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuleValidationError {
    #[error("rule name or explanation is invalid")]
    InvalidDescription,
    #[error("rule must contain between one and eight valid conditions")]
    InvalidConditions,
    #[error("rule action is invalid or exceeds a safety bound")]
    InvalidAction,
}

#[derive(Debug, Default)]
pub struct LocalRuleEngine;

impl LocalRuleEngine {
    #[must_use]
    pub fn evaluate(&self, context: &RuleEvaluationContext, rules: &[LocalRule]) -> RuleEvaluation {
        let mut ordered = rules
            .iter()
            .filter(|rule| rule.enabled && validate_rule(rule).is_ok())
            .filter(|rule| {
                rule.conditions
                    .iter()
                    .all(|condition| condition_matches(condition, context))
            })
            .collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            left.position
                .cmp(&right.position)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut evaluation = RuleEvaluation::default();
        for rule in ordered {
            let matched = MatchedRule {
                id: rule.id,
                name: rule.name.clone(),
                explanation: rule.explanation.clone(),
                position: rule.position,
                action: rule.action.clone(),
            };
            evaluation.matched_rules.push(matched.clone());
            match &rule.action {
                RuleAction::SetSemanticField { field, value } => {
                    evaluation
                        .semantic_overrides
                        .entry(*field)
                        .or_insert_with(|| (value.clone(), matched));
                }
                RuleAction::ClassifyParty { party, role } => match role {
                    RulePartyRole::Supplier => {
                        evaluation
                            .classified_supplier
                            .get_or_insert_with(|| (party.clone(), matched));
                    }
                    RulePartyRole::Customer => {
                        evaluation
                            .classified_customer
                            .get_or_insert_with(|| (party.clone(), matched));
                    }
                },
                RuleAction::PreferProjectLocation => {
                    evaluation.prefer_project_location.get_or_insert(matched);
                }
                RuleAction::SetDestination { segments } => {
                    evaluation
                        .destination
                        .get_or_insert_with(|| (segments.clone(), matched));
                }
                RuleAction::PreserveSubtree => {
                    evaluation.preserve_subtree.get_or_insert(matched);
                }
                RuleAction::UseYearFolders { enabled } => {
                    evaluation
                        .use_year_folders
                        .get_or_insert((*enabled, matched));
                }
            }
        }
        evaluation
    }
}

pub fn validate_rule(rule: &LocalRule) -> Result<(), RuleValidationError> {
    let valid_description = !rule.name.trim().is_empty()
        && rule.name.chars().count() <= 120
        && !rule.explanation.trim().is_empty()
        && rule.explanation.chars().count() <= 512;
    if !valid_description {
        return Err(RuleValidationError::InvalidDescription);
    }
    if rule.conditions.is_empty()
        || rule.conditions.len() > 8
        || rule.conditions.iter().any(|condition| {
            let operator_is_valid = if condition.field == RuleField::SourcePath {
                condition.operator == RuleOperator::StartsWith
            } else {
                condition.operator != RuleOperator::StartsWith
            };
            let value = condition.value.as_deref().map(str::trim);
            !operator_is_valid
                || match condition.operator {
                    RuleOperator::Exists => value.is_some_and(|value| !value.is_empty()),
                    RuleOperator::Equals | RuleOperator::StartsWith => {
                        value.is_none_or(str::is_empty)
                            || value.is_some_and(|value| value.chars().count() > 512)
                    }
                }
        })
    {
        return Err(RuleValidationError::InvalidConditions);
    }
    let valid_action = match &rule.action {
        RuleAction::SetSemanticField { value, .. } => {
            !value.trim().is_empty() && value.chars().count() <= 512
        }
        RuleAction::ClassifyParty { party, .. } => {
            !party.trim().is_empty() && party.chars().count() <= 512
        }
        RuleAction::SetDestination { segments } => {
            !segments.is_empty()
                && segments.len() <= 8
                && segments
                    .iter()
                    .all(|segment| !segment.trim().is_empty() && segment.chars().count() <= 512)
        }
        RuleAction::PreferProjectLocation
        | RuleAction::PreserveSubtree
        | RuleAction::UseYearFolders { .. } => true,
    };
    if !valid_action {
        return Err(RuleValidationError::InvalidAction);
    }
    Ok(())
}

#[must_use]
pub fn resolve_precedence<T: Clone>(
    candidates: impl IntoIterator<Item = PrecedenceCandidate<T>>,
) -> Option<ResolvedValue<T>> {
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .source
            .cmp(&left.source)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.rule_id.cmp(&right.rule_id))
    });
    candidates.first().map(|candidate| ResolvedValue {
        value: candidate.value.clone(),
        source: candidate.source,
        rule_id: candidate.rule_id,
        explanation: candidate.explanation.clone(),
    })
}

#[must_use]
pub fn signal_candidate<T>(
    value: T,
    signal: &BehaviorSignal,
    order: u32,
) -> PrecedenceCandidate<T> {
    PrecedenceCandidate {
        value,
        source: if signal.user_confirmed {
            PolicyPrecedence::UserConfirmedField
        } else {
            PolicyPrecedence::HighConfidenceMachineInference
        },
        order,
        rule_id: None,
        explanation: None,
    }
}

fn condition_matches(condition: &RuleCondition, context: &RuleEvaluationContext) -> bool {
    let values = values_for(condition.field, context);
    match condition.operator {
        RuleOperator::Exists => values.iter().any(|value| !value.trim().is_empty()),
        RuleOperator::Equals => {
            let Some(expected) = condition.value.as_deref() else {
                return false;
            };
            values
                .iter()
                .any(|value| normalize(value) == normalize(expected))
        }
        RuleOperator::StartsWith => {
            let Some(expected) = condition.value.as_deref() else {
                return false;
            };
            let expected = normalized_path(expected);
            values.iter().any(|value| {
                let value = normalized_path(value);
                value == expected
                    || value
                        .strip_prefix(&expected)
                        .is_some_and(|remainder| remainder.starts_with('/'))
            })
        }
    }
}

fn values_for(field: RuleField, context: &RuleEvaluationContext) -> Vec<&str> {
    match field {
        RuleField::DocumentType => signal_values(&context.document_type),
        RuleField::Context => signal_values(&context.context),
        RuleField::Supplier => signal_values(&context.supplier),
        RuleField::Customer => signal_values(&context.customer),
        RuleField::Project => signal_values(&context.project),
        RuleField::AnyParty => {
            let mut values = context
                .parties
                .iter()
                .filter(|signal| signal_is_eligible(signal))
                .map(|signal| signal.value.as_str())
                .collect::<Vec<_>>();
            values.extend(signal_values(&context.supplier));
            values.extend(signal_values(&context.customer));
            values
        }
        RuleField::SourcePath => vec![context.source_path.as_str()],
    }
}

fn signal_values(value: &Option<BehaviorSignal>) -> Vec<&str> {
    value
        .as_ref()
        .filter(|signal| signal_is_eligible(signal))
        .map(|signal| signal.value.as_str())
        .into_iter()
        .collect()
}

fn signal_is_eligible(signal: &BehaviorSignal) -> bool {
    signal.user_confirmed || signal.confidence >= 0.85
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalized_path(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{LocalRule, LocalRuleInput, RuleOrigin, SemanticRuleField, WorkspaceId};

    fn rule(position: u32, name: &str, action: RuleAction) -> LocalRule {
        let input = LocalRuleInput {
            name: name.into(),
            explanation: format!("{name} exists because the user requested it."),
            enabled: true,
            conditions: vec![RuleCondition {
                field: RuleField::DocumentType,
                operator: RuleOperator::Equals,
                value: Some("invoice".into()),
            }],
            action,
        };
        LocalRule {
            id: RuleId::new(),
            workspace_id: WorkspaceId::new(),
            name: input.name,
            explanation: input.explanation,
            position,
            enabled: input.enabled,
            conditions: input.conditions,
            action: input.action,
            origin: RuleOrigin::UserCreated,
            source_suggestion_id: None,
            created_at: "2026-08-11T00:00:00Z".into(),
            updated_at: "2026-08-11T00:00:00Z".into(),
        }
    }

    #[test]
    fn explicit_rule_beats_every_lower_precedence_source() {
        let explicit = RuleId::new();
        let resolved = resolve_precedence([
            PrecedenceCandidate {
                value: "default",
                source: PolicyPrecedence::DefaultPolicy,
                order: 0,
                rule_id: None,
                explanation: None,
            },
            PrecedenceCandidate {
                value: "confirmed",
                source: PolicyPrecedence::UserConfirmedField,
                order: 0,
                rule_id: None,
                explanation: None,
            },
            PrecedenceCandidate {
                value: "rule",
                source: PolicyPrecedence::UserExplicitRule,
                order: 4,
                rule_id: Some(explicit),
                explanation: Some("explicit".into()),
            },
        ])
        .unwrap_or_else(|| panic!("a candidate should resolve"));
        assert_eq!(resolved.value, "rule");
        assert_eq!(resolved.rule_id, Some(explicit));
    }

    #[test]
    fn conflicting_rules_use_user_order_then_stable_id() {
        let first = rule(
            0,
            "Project invoices",
            RuleAction::SetSemanticField {
                field: SemanticRuleField::Context,
                value: "business".into(),
            },
        );
        let second = rule(
            1,
            "Personal invoices",
            RuleAction::SetSemanticField {
                field: SemanticRuleField::Context,
                value: "personal".into(),
            },
        );
        let context = RuleEvaluationContext {
            document_type: Some(BehaviorSignal {
                value: "invoice".into(),
                confidence: 0.99,
                user_confirmed: false,
            }),
            ..RuleEvaluationContext::default()
        };
        let result = LocalRuleEngine.evaluate(&context, &[second, first.clone()]);
        assert_eq!(
            result
                .semantic_overrides
                .get(&SemanticRuleField::Context)
                .map(|(value, _)| value.as_str()),
            Some("business")
        );
        assert_eq!(result.matched_rules[0].id, first.id);
    }

    #[test]
    fn disabled_rule_never_matches() {
        let mut disabled = rule(0, "Disabled", RuleAction::PreserveSubtree);
        disabled.enabled = false;
        let context = RuleEvaluationContext {
            document_type: Some(BehaviorSignal {
                value: "invoice".into(),
                confidence: 1.0,
                user_confirmed: true,
            }),
            ..RuleEvaluationContext::default()
        };
        assert!(
            LocalRuleEngine
                .evaluate(&context, &[disabled])
                .matched_rules
                .is_empty()
        );
    }

    #[test]
    fn semantic_conditions_require_confirmed_or_high_confidence_evidence() {
        let preserve = rule(0, "Invoice rule", RuleAction::PreserveSubtree);
        let low_confidence = RuleEvaluationContext {
            document_type: Some(BehaviorSignal {
                value: "invoice".into(),
                confidence: 0.60,
                user_confirmed: false,
            }),
            ..RuleEvaluationContext::default()
        };
        assert!(
            LocalRuleEngine
                .evaluate(&low_confidence, std::slice::from_ref(&preserve))
                .matched_rules
                .is_empty()
        );

        let confirmed = RuleEvaluationContext {
            document_type: Some(BehaviorSignal {
                value: "invoice".into(),
                confidence: 0.60,
                user_confirmed: true,
            }),
            ..RuleEvaluationContext::default()
        };
        assert_eq!(
            LocalRuleEngine
                .evaluate(&confirmed, &[preserve])
                .matched_rules
                .len(),
            1
        );
    }

    #[test]
    fn only_source_paths_accept_prefix_conditions() {
        let mut invalid = rule(0, "Invalid prefix", RuleAction::PreserveSubtree);
        invalid.conditions[0].operator = RuleOperator::StartsWith;
        assert_eq!(
            validate_rule(&invalid),
            Err(RuleValidationError::InvalidConditions)
        );

        invalid.conditions[0].field = RuleField::SourcePath;
        assert!(validate_rule(&invalid).is_ok());
    }

    #[test]
    fn source_path_prefix_is_separator_normalized() {
        let mut preserve = rule(0, "Preserve taxes", RuleAction::PreserveSubtree);
        preserve.conditions = vec![RuleCondition {
            field: RuleField::SourcePath,
            operator: RuleOperator::StartsWith,
            value: Some("Personal\\Administrative".into()),
        }];
        let context = RuleEvaluationContext {
            source_path: "Personal/Administrative/Taxes/file.pdf".into(),
            ..RuleEvaluationContext::default()
        };
        assert_eq!(
            LocalRuleEngine
                .evaluate(&context, &[preserve])
                .matched_rules
                .len(),
            1
        );

        let near_prefix = RuleEvaluationContext {
            source_path: "Personal/Administrator/file.pdf".into(),
            ..RuleEvaluationContext::default()
        };
        let preserve = {
            let mut value = rule(0, "Preserve taxes", RuleAction::PreserveSubtree);
            value.conditions = vec![RuleCondition {
                field: RuleField::SourcePath,
                operator: RuleOperator::StartsWith,
                value: Some("Personal/Admin".into()),
            }];
            value
        };
        assert!(
            LocalRuleEngine
                .evaluate(&near_prefix, &[preserve])
                .matched_rules
                .is_empty()
        );
    }
}
