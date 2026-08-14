use crate::{FileId, LearningObservationId, RuleId, RuleSuggestionId, WorkspaceId};
use serde::{Deserialize, Serialize};

pub const RULE_SUGGESTION_THRESHOLD: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleField {
    DocumentType,
    Context,
    Supplier,
    Customer,
    Project,
    AnyParty,
    SourcePath,
}

impl RuleField {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::DocumentType => "document_type",
            Self::Context => "context",
            Self::Supplier => "supplier",
            Self::Customer => "customer",
            Self::Project => "project",
            Self::AnyParty => "any_party",
            Self::SourcePath => "source_path",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperator {
    Equals,
    Exists,
    StartsWith,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCondition {
    pub field: RuleField,
    pub operator: RuleOperator,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRuleField {
    DocumentType,
    Context,
    Supplier,
    Customer,
    Project,
}

impl SemanticRuleField {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::DocumentType => "document_type",
            Self::Context => "context",
            Self::Supplier => "supplier_candidate",
            Self::Customer => "customer_candidate",
            Self::Project => "project_reference_candidate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulePartyRole {
    Supplier,
    Customer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleAction {
    SetSemanticField {
        field: SemanticRuleField,
        value: String,
    },
    ClassifyParty {
        party: String,
        role: RulePartyRole,
    },
    PreferProjectLocation,
    SetDestination {
        segments: Vec<String>,
    },
    PreserveSubtree,
    UseYearFolders {
        enabled: bool,
    },
}

impl RuleAction {
    #[must_use]
    pub const fn database_name(&self) -> &'static str {
        match self {
            Self::SetSemanticField { .. } => "set_semantic_field",
            Self::ClassifyParty { .. } => "classify_party",
            Self::PreferProjectLocation => "prefer_project_location",
            Self::SetDestination { .. } => "set_destination",
            Self::PreserveSubtree => "preserve_subtree",
            Self::UseYearFolders { .. } => "use_year_folders",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOrigin {
    UserCreated,
    AcceptedSuggestion,
}

impl RuleOrigin {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::UserCreated => "user_created",
            Self::AcceptedSuggestion => "accepted_suggestion",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRule {
    pub id: RuleId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub explanation: String,
    pub position: u32,
    pub enabled: bool,
    pub conditions: Vec<RuleCondition>,
    pub action: RuleAction,
    pub origin: RuleOrigin,
    pub source_suggestion_id: Option<RuleSuggestionId>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRuleInput {
    pub name: String,
    pub explanation: String,
    pub enabled: bool,
    pub conditions: Vec<RuleCondition>,
    pub action: RuleAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSuggestionStatus {
    Pending,
    Accepted,
    Dismissed,
}

impl RuleSuggestionStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSuggestion {
    pub id: RuleSuggestionId,
    pub workspace_id: WorkspaceId,
    pub signature: String,
    pub title: String,
    pub explanation: String,
    pub evidence_count: u64,
    pub status: RuleSuggestionStatus,
    pub proposed_rule: LocalRuleInput,
    pub accepted_rule_id: Option<RuleId>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningSourceKind {
    SemanticCorrection,
    OrganizationOverride,
}

impl LearningSourceKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::SemanticCorrection => "semantic_correction",
            Self::OrganizationOverride => "organization_override",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningPatternKind {
    SemanticField,
    ProjectSupplierInvoice,
    Destination,
}

impl LearningPatternKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::SemanticField => "semantic_field",
            Self::ProjectSupplierInvoice => "project_supplier_invoice",
            Self::Destination => "destination",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalLearningObservation {
    pub id: LearningObservationId,
    pub workspace_id: WorkspaceId,
    pub file_id: Option<FileId>,
    pub source_kind: LearningSourceKind,
    pub source_ref: String,
    pub pattern_kind: LearningPatternKind,
    pub pattern_key: String,
    pub evidence: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningObservationInput {
    pub file_id: Option<FileId>,
    pub source_kind: LearningSourceKind,
    pub source_ref: String,
    pub pattern_kind: LearningPatternKind,
    pub pattern_key: String,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSuggestionSeed {
    pub title: String,
    pub explanation: String,
    pub proposed_rule: LocalRuleInput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleFileMatch {
    pub rule_id: RuleId,
    pub workspace_id: WorkspaceId,
    pub file_id: FileId,
    pub boost: f64,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyPrecedence {
    DefaultPolicy,
    HighConfidenceMachineInference,
    UserPreference,
    UserConfirmedField,
    UserExplicitRule,
}

impl PolicyPrecedence {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::DefaultPolicy => "default_policy",
            Self::HighConfidenceMachineInference => "high_confidence_machine_inference",
            Self::UserPreference => "user_preference",
            Self::UserConfirmedField => "user_confirmed_field",
            Self::UserExplicitRule => "user_explicit_rule",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrecedenceCandidate<T> {
    pub value: T,
    pub source: PolicyPrecedence,
    pub order: u32,
    pub rule_id: Option<RuleId>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedValue<T> {
    pub value: T,
    pub source: PolicyPrecedence,
    pub rule_id: Option<RuleId>,
    pub explanation: Option<String>,
}
