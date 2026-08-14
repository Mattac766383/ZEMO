use domain::{FileObservation, FileVersionId, RootId, ScanId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootRecord {
    pub id: RootId,
    pub workspace_id: WorkspaceId,
    pub display_label: String,
    pub absolute_path: String,
    #[serde(skip)]
    pub absolute_path_native: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRecord {
    pub id: ScanId,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub status: String,
    pub started_at: Option<String>,
    pub discovered_count: u64,
    pub indexed_count: u64,
    pub directory_count: u64,
    pub byte_count: u64,
    pub hashed_count: u64,
    pub error_count: u64,
    pub skipped_count: u64,
    pub duplicate_group_count: u64,
    pub issue_count: u64,
    pub truncated: bool,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanFileInput {
    pub observation: FileObservation,
    pub extension: Option<String>,
    pub accessed_at_ns: Option<i128>,
    pub readability_status: String,
    pub scan_status: String,
    pub hashing_status: String,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssueInput {
    pub relative_path: String,
    pub code: String,
    pub message: String,
    pub is_directory: bool,
    pub is_error: bool,
    pub skipped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroupInput {
    pub digest: Vec<u8>,
    pub byte_size: u64,
    pub members: Vec<FileVersionId>,
}

#[derive(Debug, Clone)]
pub struct ScanCompletionInput {
    pub scan_id: ScanId,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub status: String,
    pub files_discovered: u64,
    pub directories_discovered: u64,
    pub bytes_discovered: u64,
    pub files_hashed: u64,
    pub errors: u64,
    pub skipped_items: u64,
    pub truncated: bool,
    pub files: Vec<ScanFileInput>,
    pub issues: Vec<ScanIssueInput>,
    pub duplicate_groups: Vec<DuplicateGroupInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventorySort {
    Filename,
    FileType,
    Size,
    Modified,
    RelativePath,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFileRecord {
    pub id: String,
    pub filename: String,
    pub file_type: Option<String>,
    pub extension: Option<String>,
    pub byte_size: u64,
    pub modified_at: Option<String>,
    pub relative_path: String,
    pub status: String,
    pub hashing_status: String,
    pub readable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanIssueRecord {
    pub relative_path: String,
    pub category: String,
    pub message: String,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateFileRecord {
    pub id: String,
    pub filename: String,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroupRecord {
    pub digest_hex: String,
    pub byte_size: u64,
    pub files: Vec<DuplicateFileRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow {
    pub file_id: String,
    pub file_version_id: String,
    pub display_label: String,
    pub excerpt: String,
    pub body: String,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEmbeddingIndexStats {
    pub file_count: u64,
    pub vector_count: u64,
    pub vector_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedFile {
    pub file_id: String,
    pub file_version_id: String,
    pub location_id: String,
    pub content_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedScan {
    pub scan: ScanRecord,
    pub files: Vec<PersistedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionCandidate {
    pub file_id: String,
    pub file_version_id: String,
    pub root_path: String,
    pub relative_path: String,
    pub filename: String,
    pub extension: Option<String>,
    pub declared_media_type: Option<String>,
    pub byte_size: u64,
    pub readable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionBatchRecord {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub scan_id: ScanId,
    pub status: String,
    pub files_queued: u64,
    pub files_completed: u64,
    pub successful_count: u64,
    pub partial_count: u64,
    pub unsupported_count: u64,
    pub skipped_count: u64,
    pub failed_count: u64,
    pub ocr_processed_count: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExtractionResultInput {
    pub status: String,
    pub extractor_type: Option<String>,
    pub extractor_version: Option<String>,
    pub detected_content_type: String,
    pub type_mismatch: bool,
    pub extracted_text: String,
    pub character_count: u64,
    pub page_count: Option<u32>,
    pub sheet_count: Option<u32>,
    pub slide_count: Option<u32>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub requires_ocr: bool,
    pub ocr_used: bool,
    pub ocr_confidence: Option<f32>,
    pub language_hint: Option<String>,
    pub extraction_duration_ms: u64,
    pub truncated: bool,
    pub structured_metadata_json: String,
    pub error_category: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionDetailRecord {
    pub file_version_id: String,
    pub filename: String,
    pub relative_path: String,
    pub extension: Option<String>,
    pub status: String,
    pub extractor_type: Option<String>,
    pub extractor_version: Option<String>,
    pub detected_content_type: Option<String>,
    pub type_mismatch: bool,
    pub text_preview: String,
    pub character_count: u64,
    pub page_count: Option<u32>,
    pub sheet_count: Option<u32>,
    pub slide_count: Option<u32>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub requires_ocr: bool,
    pub ocr_used: bool,
    pub ocr_confidence: Option<f32>,
    pub language_hint: Option<String>,
    pub extraction_duration_ms: u64,
    pub truncated: bool,
    pub structured_metadata: serde_json::Value,
    pub error_category: Option<String>,
    pub error_message: Option<String>,
    pub extracted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticAnalysisCandidate {
    pub workspace_id: WorkspaceId,
    pub scan_id: ScanId,
    pub file_id: String,
    pub file_version_id: String,
    pub extraction_result_id: String,
    pub filename: String,
    pub extension: Option<String>,
    pub detected_content_type: Option<String>,
    pub extraction_status: String,
    pub extracted_text: String,
    pub extractor_type: Option<String>,
    pub extractor_version: Option<String>,
    pub page_count: Option<u32>,
    pub sheet_count: Option<u32>,
    pub slide_count: Option<u32>,
    pub ocr_used: bool,
    pub ocr_confidence: Option<f32>,
    pub extraction_truncated: bool,
    pub language_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticAnalysisBatchRecord {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub scan_id: ScanId,
    pub status: String,
    pub files_queued: u64,
    pub files_completed: u64,
    pub high_confidence_count: u64,
    pub needs_review_count: u64,
    pub unknown_count: u64,
    pub partial_count: u64,
    pub failed_count: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEvidenceRecord {
    pub evidence_type: String,
    pub exact_text: String,
    pub start_offset: Option<u64>,
    pub end_offset: Option<u64>,
    pub page_number: Option<u32>,
    pub sheet_name: Option<String>,
    pub slide_number: Option<u32>,
    pub source_label: String,
    pub explanation: String,
    pub extraction_method: String,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCandidateValueRecord {
    pub display_value: String,
    pub normalized_value: serde_json::Value,
    pub confidence: f32,
    pub status: String,
    pub source_method: String,
    pub evidence: Vec<SemanticEvidenceRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticFieldRecord {
    pub field_id: String,
    pub field_key: String,
    pub value_kind: Option<String>,
    pub display_value: Option<String>,
    pub machine_display_value: Option<String>,
    pub normalized_value: serde_json::Value,
    pub confidence: f32,
    pub status: String,
    pub source_method: String,
    pub analyzer_version: String,
    pub value_source: String,
    pub user_state: Option<String>,
    pub evidence: Vec<SemanticEvidenceRecord>,
    pub candidates: Vec<SemanticCandidateValueRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEntityRecord {
    pub entity_id: String,
    pub candidate_key: String,
    pub entity_type: String,
    pub original_value: String,
    pub normalized_value: String,
    pub confidence: f32,
    pub status: String,
    pub source_method: String,
    pub evidence: Vec<SemanticEvidenceRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticAnalysisDetailRecord {
    pub analysis_id: String,
    pub status: String,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub provider_id: String,
    pub provider_version: String,
    pub schema_version: u32,
    pub input_quality: f32,
    pub input_quality_status: String,
    pub input_quality_reasons: Vec<String>,
    pub language: Option<String>,
    pub analyzed_at: Option<String>,
    pub fields: Vec<SemanticFieldRecord>,
    pub entities: Vec<SemanticEntityRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCorrectionInput {
    pub field_key: String,
    pub correction_state: String,
    pub value_kind: String,
    pub display_value: String,
    pub normalized_value_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCorrectionRecord {
    pub correction_id: String,
    pub file_id: String,
    pub field_key: String,
    pub correction_state: String,
    pub value_kind: String,
    pub display_value: String,
    pub normalized_value: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatusFilter {
    #[default]
    NeedsReview,
    Resolved,
    Ignored,
    All,
}

impl ReviewStatusFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::NeedsReview => Some("needs_review"),
            Self::Resolved => Some("resolved"),
            Self::Ignored => Some("ignored"),
            Self::All => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReasonFilter {
    #[default]
    All,
    Ocr,
    Unsupported,
    Permissions,
    Partial,
    Corrupt,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Resolve,
    Ignore,
}

impl ReviewAction {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Resolve => "resolved",
            Self::Ignore => "ignored",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItemRecord {
    pub review_id: String,
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub reason: String,
    pub source_subsystem: String,
    pub severity: String,
    pub explanation: String,
    pub technical_details: Option<String>,
    pub status: String,
    pub retry_available: bool,
    pub retry_count: u64,
    pub extraction_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPageRecord {
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
    pub items: Vec<ReviewItemRecord>,
}

#[derive(Debug, Clone)]
pub struct ReviewRetryCandidate {
    pub review_id: String,
    pub batch_id: String,
    pub scan_id: ScanId,
    pub candidate: ExtractionCandidate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDetailRecord {
    pub file_id: String,
    pub file_version_id: String,
    pub filename: String,
    pub relative_path: String,
    pub extension: Option<String>,
    pub detected_type: Option<String>,
    pub byte_size: u64,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub hash: Option<String>,
    pub duplicate: bool,
    pub extraction_status: Option<String>,
    pub extractor_type: Option<String>,
    pub extractor_version: Option<String>,
    pub ocr_status: Option<String>,
    pub text_preview: String,
    pub character_count: u64,
    pub review_items: Vec<ReviewItemRecord>,
    pub semantic_analysis: Option<SemanticAnalysisDetailRecord>,
    pub relationships: Vec<IdentityRelationshipRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityResolverRunRecord {
    pub run_id: String,
    pub workspace_id: WorkspaceId,
    pub trigger_kind: String,
    pub status: String,
    pub resolver_id: String,
    pub resolver_version: String,
    pub files_considered: u64,
    pub occurrences_processed: u64,
    pub blocking_memberships: u64,
    pub comparisons: u64,
    pub candidates_created: u64,
    pub auto_links_created: u64,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityOccurrenceSyncRecord {
    pub file_id: String,
    pub semantic_analysis_id: String,
    pub occurrence_ids: Vec<String>,
    pub created_count: u64,
    pub updated_count: u64,
    pub deactivated_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIdentityCandidateRecord {
    pub candidate_id: Option<String>,
    pub left_identity_id: String,
    pub right_identity_id: String,
    pub status: String,
    pub created: bool,
    pub rejected_by_user: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySummaryRecord {
    pub identity_id: String,
    pub identity_type: String,
    pub display_name: String,
    pub normalized_display_name: String,
    pub resolution_status: String,
    pub lifecycle_status: String,
    pub confidence: f32,
    pub user_locked: bool,
    pub occurrence_count: u64,
    pub file_count: u64,
    pub aliases: Vec<String>,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityMatchEvidenceRecord {
    pub evidence_type: String,
    pub strength: String,
    pub polarity: String,
    pub left_value: String,
    pub right_value: String,
    pub weight: f32,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCandidateRecord {
    pub candidate_id: String,
    pub review_group_key: String,
    pub score: f32,
    pub policy_decision: String,
    pub status: String,
    pub resolver_version: String,
    pub left: IdentitySummaryRecord,
    pub right: IdentitySummaryRecord,
    pub evidence: Vec<IdentityMatchEvidenceRecord>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityReviewGroupRecord {
    pub review_group_id: String,
    pub review_reason: String,
    pub group_key: String,
    pub title: String,
    pub explanation: String,
    pub max_score: f32,
    pub candidate_count: u64,
    pub occurrence_count: u64,
    pub file_count: u64,
    pub status: String,
    pub resolver_version: String,
    pub candidates: Vec<IdentityCandidateRecord>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityReviewPageRecord {
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
    pub items: Vec<IdentityReviewGroupRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityOccurrenceRecord {
    pub occurrence_id: String,
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub original_value: String,
    pub normalized_value: String,
    pub confidence: f32,
    pub role: Option<String>,
    pub analyzer_version: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityRelationshipRecord {
    pub relationship_id: String,
    pub relationship_type: String,
    pub identity_id: String,
    pub display_name: String,
    pub identity_type: String,
    pub confidence: f32,
    pub status: String,
    pub user_confirmation_state: Option<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityIdentifierRecord {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityAuditEventRecord {
    pub event_type: String,
    pub decision_source: String,
    pub related_identity_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDetailRecord {
    pub identity: IdentitySummaryRecord,
    pub occurrences: Vec<IdentityOccurrenceRecord>,
    /// Total occurrence rows for this identity (may exceed `occurrences.len()`).
    pub occurrence_total: u64,
    /// True when `occurrences` was capped for UI/memory safety.
    pub occurrences_truncated: bool,
    pub identifiers: Vec<IdentityIdentifierRecord>,
    pub relationships: Vec<IdentityRelationshipRecord>,
    pub projects: Vec<IdentitySummaryRecord>,
    pub audit_events: Vec<IdentityAuditEventRecord>,
    pub resolver_version: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityCandidateAction {
    Confirm,
    Reject,
    KeepSeparate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityMutationRecord {
    pub decision_id: String,
    pub primary_identity_id: String,
    pub secondary_identity_id: Option<String>,
    pub occurrence_id: Option<String>,
    pub action: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalSemanticSignalRecord {
    pub value: String,
    pub confidence: f32,
    pub status: String,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRelationshipSourceRecord {
    pub relationship_type: String,
    pub identity_id: String,
    pub display_name: String,
    pub confidence: f32,
    pub status: String,
    pub user_confirmed: bool,
    pub project_customer_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalSourceFileRecord {
    pub file_id: String,
    pub file_version_id: String,
    pub relative_path: String,
    pub filename: String,
    pub byte_size: u64,
    pub modified_at: Option<String>,
    pub content_hash: Option<String>,
    pub extraction_status: Option<String>,
    pub semantic_status: Option<String>,
    pub input_quality: f32,
    pub context: Option<ProposalSemanticSignalRecord>,
    pub document_type: Option<ProposalSemanticSignalRecord>,
    pub issue_date: Option<ProposalSemanticSignalRecord>,
    pub identifier: Option<ProposalSemanticSignalRecord>,
    pub amount: Option<ProposalSemanticSignalRecord>,
    pub currency: Option<ProposalSemanticSignalRecord>,
    pub relationships: Vec<ProposalRelationshipSourceRecord>,
    pub review_reasons: Vec<String>,
    pub duplicate_group_id: Option<String>,
    pub duplicate_canonical: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalWorkspaceSourceRecord {
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub scan_id: ScanId,
    pub semantic_version: Option<String>,
    pub relationship_version: Option<String>,
    pub files: Vec<ProposalSourceFileRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    Initial,
    Incremental,
    Reconciliation,
}

impl ScanKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Incremental => "incremental",
            Self::Reconciliation => "reconciliation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MonitoringMode {
    Prudent,
    Automatic,
    Rules,
}

impl MonitoringMode {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Prudent => "PRUDENT",
            Self::Automatic => "AUTOMATIC",
            Self::Rules => "RULES",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringRootStatus {
    Starting,
    Active,
    Paused,
    Reconciling,
    Overflowed,
    Offline,
    Failed,
    Stopped,
}

impl MonitoringRootStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Reconciling => "reconciling",
            Self::Overflowed => "overflowed",
            Self::Offline => "offline",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMonitoringStateRecord {
    pub workspace_id: WorkspaceId,
    pub mode: MonitoringMode,
    pub paused: bool,
    pub startup_reconciliation_pending: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootMonitoringConfiguration {
    pub enabled: bool,
    pub status: MonitoringRootStatus,
    pub size_threshold_bytes: u64,
    pub startup_entry_limit: u32,
}

impl Default for RootMonitoringConfiguration {
    fn default() -> Self {
        Self {
            enabled: false,
            status: MonitoringRootStatus::Paused,
            size_threshold_bytes: 4 * 1024 * 1024 * 1024,
            startup_entry_limit: 100_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootMonitoringSettingsRecord {
    pub root_id: RootId,
    pub workspace_id: WorkspaceId,
    pub enabled: bool,
    pub status: MonitoringRootStatus,
    pub size_threshold_bytes: u64,
    pub startup_entry_limit: u32,
    pub last_reconciliation_scan_id: Option<ScanId>,
    pub last_reconciled_at: Option<String>,
    pub last_checkpoint_sequence: Option<u64>,
    pub last_checkpoint_at: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoredRootRecord {
    pub root_id: RootId,
    pub workspace_id: WorkspaceId,
    pub display_label: String,
    pub selected_path: String,
    #[serde(skip)]
    pub selected_path_native: PathBuf,
    pub enabled: bool,
    pub status: MonitoringRootStatus,
    pub size_threshold_bytes: u64,
    pub startup_entry_limit: u32,
    pub pending_jobs: u64,
    pub last_reconciliation_scan_id: Option<ScanId>,
    pub last_reconciled_at: Option<String>,
    pub last_checkpoint_sequence: Option<u64>,
    pub last_checkpoint_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringExclusionKind {
    PathPrefix,
    Extension,
}

impl MonitoringExclusionKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::PathPrefix => "path_prefix",
            Self::Extension => "extension",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringExclusionRecord {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub root_id: Option<RootId>,
    pub kind: MonitoringExclusionKind,
    pub value: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchBackend {
    Fsevents,
    ReadDirectoryChanges,
    Inotify,
    Polling,
}

impl WatchBackend {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Fsevents => "fsevents",
            Self::ReadDirectoryChanges => "read_directory_changes",
            Self::Inotify => "inotify",
            Self::Polling => "polling",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchRegistrationStatus {
    Starting,
    Active,
    Paused,
    Overflowed,
    Failed,
    Stopped,
}

impl WatchRegistrationStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Overflowed => "overflowed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchRegistrationRecord {
    pub id: String,
    pub root_id: RootId,
    pub backend: WatchBackend,
    pub recursive: bool,
    pub status: WatchRegistrationStatus,
    pub backend_cursor: Option<String>,
    pub configuration_json: String,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchEventKind {
    Created,
    Modified,
    Moved,
    Removed,
    Metadata,
    Overflow,
    RescanRequired,
}

impl WatchEventKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Moved => "moved",
            Self::Removed => "removed",
            Self::Metadata => "metadata",
            Self::Overflow => "overflow",
            Self::RescanRequired => "rescan_required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEventScope {
    File,
    Directory,
    Unknown,
}

impl WatchEventScope {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEventInput {
    pub registration_id: String,
    pub kind: WatchEventKind,
    pub scope: WatchEventScope,
    pub path_before: Option<PathBuf>,
    pub path_after: Option<PathBuf>,
    pub native_identity_key: Option<Vec<u8>>,
    pub payload_json: String,
    pub debounce_ready_at_unix_ms: i64,
    pub maximum_attempts: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEventRecord {
    pub id: String,
    pub registration_id: String,
    pub resulting_scan_id: Option<ScanId>,
    pub sequence_number: u64,
    pub kind: WatchEventKind,
    pub scope: WatchEventScope,
    pub path_before: Option<PathBuf>,
    pub path_after: Option<PathBuf>,
    pub native_identity_key: Option<Vec<u8>>,
    pub payload_json: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchCheckpointRecord {
    pub id: String,
    pub registration_id: String,
    pub sequence_number: u64,
    pub backend_cursor: String,
    pub state_json: String,
    pub checkpointed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringJobStatus {
    Pending,
    Waiting,
    Processing,
    Completed,
    ToReview,
    Failed,
    Cancelled,
    Excluded,
}

impl MonitoringJobStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Waiting => "waiting",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::ToReview => "to_review",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Excluded => "excluded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitoringJobStage {
    Queued,
    Stability,
    Catalog,
    Content,
    Semantic,
    Relationships,
    Proposal,
    Search,
    Finalizing,
}

impl MonitoringJobStage {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Stability => "stability",
            Self::Catalog => "catalog",
            Self::Content => "content",
            Self::Semantic => "semantic",
            Self::Relationships => "relationships",
            Self::Proposal => "proposal",
            Self::Search => "search",
            Self::Finalizing => "finalizing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitoringJobRecord {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub watch_registration_id: Option<String>,
    pub event_kind: WatchEventKind,
    pub event_scope: WatchEventScope,
    pub path_before: Option<PathBuf>,
    pub path_after: Option<PathBuf>,
    pub coalescing_path: Option<PathBuf>,
    pub status: MonitoringJobStatus,
    pub attempt_count: u32,
    pub maximum_attempts: u32,
    pub sample_byte_size: Option<u64>,
    pub sample_modified_at_ns: Option<String>,
    pub stable_sample_count: u32,
    pub debounce_ready_at_unix_ms: i64,
    pub retry_after_unix_ms: Option<i64>,
    pub last_sampled_at_unix_ms: Option<i64>,
    pub event_count: u64,
    pub coalesced_event_count: u64,
    pub reconciliation_scan_id: Option<ScanId>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub claimed_at: Option<String>,
    pub claim_token: Option<String>,
    pub lease_expires_at_unix_ms: Option<i64>,
    pub processing_stage: MonitoringJobStage,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitoringStabilitySample {
    pub byte_size: u64,
    pub modified_at_ns: Option<String>,
    pub stable_sample_count: u32,
    pub sampled_at_unix_ms: i64,
    pub next_check_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoalescedWatchEventRecord {
    pub event: WatchEventRecord,
    pub job: MonitoringJobRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitoringActivityInput {
    pub batch_id: String,
    pub workspace_id: WorkspaceId,
    pub root_id: Option<RootId>,
    pub files_analyzed: u64,
    pub ready_to_organize: u64,
    pub needs_review: u64,
    pub failed: u64,
    pub summary: String,
    pub reconciliation_scan_id: Option<ScanId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringActivityRecord {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub root_id: Option<RootId>,
    pub files_analyzed: u64,
    pub ready_to_organize: u64,
    pub needs_review: u64,
    pub failed: u64,
    pub summary: String,
    pub reconciliation_scan_id: Option<ScanId>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringDashboardCountsRecord {
    pub files_analyzed: u64,
    pub ready_to_organize: u64,
    pub needs_review: u64,
    pub pending_proposals: u64,
    pub pending_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshotRecord {
    pub file_id: String,
    pub current_relative_path: PathBuf,
    pub byte_size: u64,
    pub modified_at_ns: Option<String>,
}
