//! Versioned renderer-facing DTOs.
//!
//! Only the directory explicitly selected by the user may cross IPC as an
//! absolute path; inventory and issue paths remain relative to that scope.

/// Internal, authenticated coordinator-to-executor protocol.
///
/// This is deliberately separate from renderer-facing DTOs.
pub mod executor_v2;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatusDto {
    pub local_first: bool,
    pub read_only_scan: bool,
    pub network_disabled: bool,
    pub apply_enabled: bool,
    pub apply_gate_reason: Option<String>,
    pub display_label: Option<String>,
    pub version: Option<String>,
    pub recovery_required: bool,
    pub journal_locked: bool,
    pub journal_diagnostics: Vec<JournalDiagnosticDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalDiagnosticDto {
    pub scope: String,
    pub execution_id: Option<String>,
    pub code: String,
    pub message: String,
    pub detected_at_unix_ms: i64,
    pub recovery_available: bool,
    pub rollback_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredRootDto {
    pub id: String,
    pub display_label: String,
    pub selected_path: String,
}

/// Standard user-content locations for Whole Computer mode.
/// Paths are resolved in Rust from trusted OS/home APIs — never from renderer input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserContentLocationDto {
    pub kind: String,
    pub display_label: String,
    pub absolute_path: String,
    pub exists: bool,
    pub readable: bool,
    pub recommended: bool,
    pub access_state: String,
    pub human_status: String,
    pub writable: bool,
    pub raw_os_error: Option<i32>,
    pub platform_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderAccessProbeDto {
    pub logical_name: String,
    pub kind: String,
    pub display_label: String,
    pub resolved_path: String,
    pub exists: bool,
    pub is_dir: bool,
    pub readable: bool,
    pub writable: bool,
    pub recommended: bool,
    pub raw_os_error: Option<i32>,
    pub platform_error: Option<String>,
    pub access_state: String,
    pub human_status: String,
    pub canonical_path: String,
    pub failed_stage: Option<String>,
    pub error_kind: Option<String>,
    pub inspect_result: Option<String>,
    pub technical_details: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterUserContentRootResultDto {
    pub root: Option<RegisteredRootDto>,
    pub kind: String,
    pub display_label: String,
    pub absolute_path: String,
    pub status: String,
    pub access_state: String,
    pub human_status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub id: String,
    pub name: String,
    pub root: Option<RegisteredRootDto>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResultDto {
    pub id: String,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub files_discovered: u64,
    pub files_indexed: u64,
    pub directories_discovered: u64,
    pub bytes_discovered: u64,
    pub files_hashed: u64,
    pub duplicate_groups: u64,
    pub errors: u64,
    pub skipped_items: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressDto {
    pub scan_id: String,
    pub phase: String,
    pub files_discovered: u64,
    pub files_indexed: u64,
    pub directories_discovered: u64,
    pub bytes_discovered: u64,
    pub files_hashed: u64,
    pub duplicate_groups: u64,
    pub errors: u64,
    pub skipped_items: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanFileDto {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanIssueDto {
    pub relative_path: String,
    pub category: String,
    pub message: String,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFileDto {
    pub id: String,
    pub filename: String,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupDto {
    pub digest: String,
    pub byte_size: u64,
    pub files: Vec<DuplicateFileDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentAnalysisDto {
    pub id: String,
    pub scan_id: String,
    pub status: String,
    pub files_queued: u64,
    pub files_completed: u64,
    pub successful: u64,
    pub partial: u64,
    pub unsupported: u64,
    pub skipped: u64,
    pub failed: u64,
    pub ocr_processed: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentAnalysisProgressDto {
    pub batch_id: String,
    pub scan_id: String,
    pub phase: String,
    pub files_queued: u64,
    pub files_completed: u64,
    pub successful: u64,
    pub partial: u64,
    pub unsupported: u64,
    pub skipped: u64,
    pub failed: u64,
    pub ocr_processed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDetailDto {
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalSearchFiltersDto {
    pub file_type: String,
    pub modified: String,
    pub extraction: String,
    pub ocr: String,
    pub minimum_size: Option<u64>,
    pub maximum_size: Option<u64>,
    pub document_type: String,
    pub context: String,
    pub customer: Option<String>,
    pub supplier: Option<String>,
    pub project: Option<String>,
    pub year: Option<i32>,
    pub amount_minimum_minor: Option<i64>,
    pub amount_maximum_minor: Option<i64>,
    pub currency: Option<String>,
    pub semantic_status: String,
    pub minimum_confidence_percent: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalSearchQueryDto {
    pub text: String,
    pub filters: LocalSearchFiltersDto,
    pub sort: String,
    pub page: usize,
    pub page_size: usize,
    pub semantic_search: Option<bool>,
    pub disabled_intents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSearchResultDto {
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub detected_type: Option<String>,
    pub extension: Option<String>,
    pub byte_size: u64,
    pub modified_at: Option<String>,
    pub extraction_status: Option<String>,
    pub ocr_status: Option<String>,
    pub duplicate: bool,
    pub match_source: String,
    pub relevance: f64,
    pub snippet: String,
    pub why_matched: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryChipDto {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingSearchStatusDto {
    pub availability: String,
    pub provider_id: String,
    pub version: String,
    pub production_ready: bool,
    pub indexed_files: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ann_index_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelStatusDto {
    pub model_id: String,
    pub version: String,
    pub dimensions: usize,
    pub status: String,
    pub approximate_disk_bytes: u64,
    pub license: String,
    pub local_only: bool,
    pub download_implemented: bool,
    pub last_error: Option<String>,
    pub install_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTimingsDto {
    pub total_ms: u64,
    pub lexical_and_structured_ms: u64,
    pub query_embed_ms: u64,
    pub ann_ms: u64,
    pub vector_ms: u64,
    pub fusion_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSearchPageDto {
    pub query: String,
    pub page: usize,
    pub page_size: usize,
    pub total: u64,
    pub has_more: bool,
    pub results: Vec<LocalSearchResultDto>,
    pub interpreted_query: Vec<QueryChipDto>,
    pub embeddings: EmbeddingSearchStatusDto,
    pub timings: SearchTimingsDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReviewItemDto {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReviewPageDto {
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
    pub items: Vec<FileReviewItemDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticAnalysisDto {
    pub id: String,
    pub scan_id: String,
    pub status: String,
    pub files_queued: u64,
    pub files_completed: u64,
    pub high_confidence: u64,
    pub needs_review: u64,
    pub unknown: u64,
    pub partial: u64,
    pub failed: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticAnalysisProgressDto {
    pub batch_id: String,
    pub scan_id: String,
    pub phase: String,
    pub files_queued: u64,
    pub files_completed: u64,
    pub high_confidence: u64,
    pub needs_review: u64,
    pub unknown: u64,
    pub partial: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEvidenceDto {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCandidateValueDto {
    pub display_value: String,
    pub normalized_value: serde_json::Value,
    pub confidence: f32,
    pub status: String,
    pub source_method: String,
    pub evidence: Vec<SemanticEvidenceDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticFieldDto {
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
    pub evidence: Vec<SemanticEvidenceDto>,
    pub candidates: Vec<SemanticCandidateValueDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEntityDto {
    pub entity_id: String,
    pub candidate_key: String,
    pub entity_type: String,
    pub original_value: String,
    pub normalized_value: String,
    pub confidence: f32,
    pub status: String,
    pub source_method: String,
    pub evidence: Vec<SemanticEvidenceDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticAnalysisDetailDto {
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
    pub fields: Vec<SemanticFieldDto>,
    pub entities: Vec<SemanticEntityDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCorrectionDto {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileDetailDto {
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
    pub review_items: Vec<FileReviewItemDto>,
    pub semantic_analysis: Option<SemanticAnalysisDetailDto>,
    pub relationships: Vec<IdentityRelationshipDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityResolutionDto {
    pub run_id: String,
    pub workspace_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityResolutionProgressDto {
    pub run_id: String,
    pub workspace_id: String,
    pub phase: String,
    pub files_considered: u64,
    pub occurrences_processed: u64,
    pub blocking_memberships: u64,
    pub comparisons: u64,
    pub candidates_created: u64,
    pub auto_links_created: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySummaryDto {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityMatchEvidenceDto {
    pub evidence_type: String,
    pub strength: String,
    pub polarity: String,
    pub left_value: String,
    pub right_value: String,
    pub weight: f32,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCandidateDto {
    pub candidate_id: String,
    pub review_group_key: String,
    pub score: f32,
    pub policy_decision: String,
    pub status: String,
    pub resolver_version: String,
    pub left: IdentitySummaryDto,
    pub right: IdentitySummaryDto,
    pub evidence: Vec<IdentityMatchEvidenceDto>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityReviewGroupDto {
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
    pub candidates: Vec<IdentityCandidateDto>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityReviewPageDto {
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
    pub items: Vec<IdentityReviewGroupDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityOccurrenceDto {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityRelationshipDto {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityIdentifierDto {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityAuditEventDto {
    pub event_type: String,
    pub decision_source: String,
    pub related_identity_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDetailDto {
    pub identity: IdentitySummaryDto,
    pub occurrences: Vec<IdentityOccurrenceDto>,
    pub occurrence_total: u64,
    pub occurrences_truncated: bool,
    pub identifiers: Vec<IdentityIdentifierDto>,
    pub relationships: Vec<IdentityRelationshipDto>,
    pub projects: Vec<IdentitySummaryDto>,
    pub audit_events: Vec<IdentityAuditEventDto>,
    pub resolver_version: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityMutationDto {
    pub decision_id: String,
    pub primary_identity_id: String,
    pub secondary_identity_id: Option<String>,
    pub occurrence_id: Option<String>,
    pub action: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionRetryDto {
    pub review_id: String,
    pub batch_id: Option<String>,
    pub file_id: Option<String>,
    pub status: String,
    pub extraction_status: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringSummaryDto {
    pub change_hints: usize,
    pub reconciliation_required: bool,
    pub auto_apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoredFolderDto {
    pub root_id: String,
    pub display_label: String,
    pub selected_path: String,
    pub enabled: bool,
    pub status: String,
    pub pending_jobs: u64,
    pub last_reconciled_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringCountsDto {
    pub files_analyzed: u64,
    pub ready_to_organize: u64,
    pub needs_review: u64,
    pub pending_proposals: u64,
    pub pending_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringActivityDto {
    pub id: String,
    pub summary: String,
    pub files_analyzed: u64,
    pub ready_to_organize: u64,
    pub needs_review: u64,
    pub failed: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringExclusionDto {
    pub id: String,
    pub root_id: Option<String>,
    pub kind: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringDashboardDto {
    pub workspace_id: String,
    pub mode: String,
    pub paused: bool,
    pub startup_reconciliation_pending: bool,
    pub automatic_execution_enabled: bool,
    pub folders: Vec<MonitoredFolderDto>,
    pub counts: MonitoringCountsDto,
    pub recent_activity: Vec<MonitoringActivityDto>,
    pub exclusions: Vec<MonitoringExclusionDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredWorkspaceSessionDto {
    pub workspace: WorkspaceDto,
    pub root: Option<RegisteredRootDto>,
    pub scan: Option<ScanResultDto>,
    pub safe_read_only: bool,
    pub filesystem_execution_resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceDto {
    pub id: String,
    pub display_label: String,
    pub excerpt: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultDto {
    pub id: String,
    pub display_label: String,
    pub summary: String,
    pub score: Option<f32>,
    pub evidence: Vec<EvidenceDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponseDto {
    pub query: String,
    pub results: Vec<SearchResultDto>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalCountsDto {
    pub ready: usize,
    pub to_review: usize,
    pub blocked: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalItemDto {
    pub id: String,
    pub display_label: String,
    pub status: String,
    pub rationale: String,
    pub decision: Option<String>,
    pub evidence: Vec<EvidenceDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalDto {
    pub id: String,
    pub status: String,
    pub summary: Option<String>,
    pub counts: ProposalCountsDto,
    pub items: Vec<ProposalItemDto>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalDiffDto {
    pub item_id: String,
    pub display_label: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalSimulationDto {
    pub proposal_id: String,
    pub status: String,
    pub summary: Option<String>,
    pub diffs: Vec<ProposalDiffDto>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDto {
    pub id: String,
    pub proposal_id: String,
    pub plan_digest: String,
    pub status: String,
    pub item_count: usize,
    pub sealed_at: Option<String>,
    pub approved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionDto {
    pub id: String,
    pub status: String,
    pub display_label: String,
    pub changed_items: usize,
    pub rollback_available: bool,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub rolled_back_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewDecisionDto {
    Accept,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationProposalSummaryDto {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationProposalChangeDto {
    pub destinations_changed: u64,
    pub files_added: u64,
    pub conflicts_resolved: u64,
    pub moved_to_review: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleFieldDto {
    DocumentType,
    Context,
    Supplier,
    Customer,
    Project,
    AnyParty,
    SourcePath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperatorDto {
    Equals,
    Exists,
    StartsWith,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleConditionDto {
    pub field: RuleFieldDto,
    pub operator: RuleOperatorDto,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRuleFieldDto {
    DocumentType,
    Context,
    Supplier,
    Customer,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulePartyRoleDto {
    Supplier,
    Customer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleActionDto {
    SetSemanticField {
        field: SemanticRuleFieldDto,
        value: String,
    },
    ClassifyParty {
        party: String,
        role: RulePartyRoleDto,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRuleInputDto {
    pub name: String,
    pub explanation: String,
    pub enabled: bool,
    pub conditions: Vec<RuleConditionDto>,
    pub action: RuleActionDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRuleDto {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub explanation: String,
    pub position: u32,
    pub enabled: bool,
    pub conditions: Vec<RuleConditionDto>,
    pub action: RuleActionDto,
    pub origin: String,
    pub source_suggestion_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSuggestionDto {
    pub id: String,
    pub workspace_id: String,
    pub signature: String,
    pub title: String,
    pub explanation: String,
    pub evidence_count: u64,
    pub status: String,
    pub proposed_rule: LocalRuleInputDto,
    pub accepted_rule_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationPreferencesDto {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesPreferencesStateDto {
    pub rules: Vec<LocalRuleDto>,
    pub suggestions: Vec<RuleSuggestionDto>,
    pub preferences: OrganizationPreferencesDto,
}

impl From<domain::RuleField> for RuleFieldDto {
    fn from(value: domain::RuleField) -> Self {
        match value {
            domain::RuleField::DocumentType => Self::DocumentType,
            domain::RuleField::Context => Self::Context,
            domain::RuleField::Supplier => Self::Supplier,
            domain::RuleField::Customer => Self::Customer,
            domain::RuleField::Project => Self::Project,
            domain::RuleField::AnyParty => Self::AnyParty,
            domain::RuleField::SourcePath => Self::SourcePath,
        }
    }
}

impl From<RuleFieldDto> for domain::RuleField {
    fn from(value: RuleFieldDto) -> Self {
        match value {
            RuleFieldDto::DocumentType => Self::DocumentType,
            RuleFieldDto::Context => Self::Context,
            RuleFieldDto::Supplier => Self::Supplier,
            RuleFieldDto::Customer => Self::Customer,
            RuleFieldDto::Project => Self::Project,
            RuleFieldDto::AnyParty => Self::AnyParty,
            RuleFieldDto::SourcePath => Self::SourcePath,
        }
    }
}

impl From<domain::RuleOperator> for RuleOperatorDto {
    fn from(value: domain::RuleOperator) -> Self {
        match value {
            domain::RuleOperator::Equals => Self::Equals,
            domain::RuleOperator::Exists => Self::Exists,
            domain::RuleOperator::StartsWith => Self::StartsWith,
        }
    }
}

impl From<RuleOperatorDto> for domain::RuleOperator {
    fn from(value: RuleOperatorDto) -> Self {
        match value {
            RuleOperatorDto::Equals => Self::Equals,
            RuleOperatorDto::Exists => Self::Exists,
            RuleOperatorDto::StartsWith => Self::StartsWith,
        }
    }
}

impl From<domain::SemanticRuleField> for SemanticRuleFieldDto {
    fn from(value: domain::SemanticRuleField) -> Self {
        match value {
            domain::SemanticRuleField::DocumentType => Self::DocumentType,
            domain::SemanticRuleField::Context => Self::Context,
            domain::SemanticRuleField::Supplier => Self::Supplier,
            domain::SemanticRuleField::Customer => Self::Customer,
            domain::SemanticRuleField::Project => Self::Project,
        }
    }
}

impl From<SemanticRuleFieldDto> for domain::SemanticRuleField {
    fn from(value: SemanticRuleFieldDto) -> Self {
        match value {
            SemanticRuleFieldDto::DocumentType => Self::DocumentType,
            SemanticRuleFieldDto::Context => Self::Context,
            SemanticRuleFieldDto::Supplier => Self::Supplier,
            SemanticRuleFieldDto::Customer => Self::Customer,
            SemanticRuleFieldDto::Project => Self::Project,
        }
    }
}

impl From<domain::RuleAction> for RuleActionDto {
    fn from(value: domain::RuleAction) -> Self {
        match value {
            domain::RuleAction::SetSemanticField { field, value } => Self::SetSemanticField {
                field: field.into(),
                value,
            },
            domain::RuleAction::ClassifyParty { party, role } => Self::ClassifyParty {
                party,
                role: match role {
                    domain::RulePartyRole::Supplier => RulePartyRoleDto::Supplier,
                    domain::RulePartyRole::Customer => RulePartyRoleDto::Customer,
                },
            },
            domain::RuleAction::PreferProjectLocation => Self::PreferProjectLocation,
            domain::RuleAction::SetDestination { segments } => Self::SetDestination { segments },
            domain::RuleAction::PreserveSubtree => Self::PreserveSubtree,
            domain::RuleAction::UseYearFolders { enabled } => Self::UseYearFolders { enabled },
        }
    }
}

impl From<RuleActionDto> for domain::RuleAction {
    fn from(value: RuleActionDto) -> Self {
        match value {
            RuleActionDto::SetSemanticField { field, value } => Self::SetSemanticField {
                field: field.into(),
                value,
            },
            RuleActionDto::ClassifyParty { party, role } => Self::ClassifyParty {
                party,
                role: match role {
                    RulePartyRoleDto::Supplier => domain::RulePartyRole::Supplier,
                    RulePartyRoleDto::Customer => domain::RulePartyRole::Customer,
                },
            },
            RuleActionDto::PreferProjectLocation => Self::PreferProjectLocation,
            RuleActionDto::SetDestination { segments } => Self::SetDestination { segments },
            RuleActionDto::PreserveSubtree => Self::PreserveSubtree,
            RuleActionDto::UseYearFolders { enabled } => Self::UseYearFolders { enabled },
        }
    }
}

impl From<domain::LocalRuleInput> for LocalRuleInputDto {
    fn from(value: domain::LocalRuleInput) -> Self {
        Self {
            name: value.name,
            explanation: value.explanation,
            enabled: value.enabled,
            conditions: value
                .conditions
                .into_iter()
                .map(|condition| RuleConditionDto {
                    field: condition.field.into(),
                    operator: condition.operator.into(),
                    value: condition.value,
                })
                .collect(),
            action: value.action.into(),
        }
    }
}

impl From<LocalRuleInputDto> for domain::LocalRuleInput {
    fn from(value: LocalRuleInputDto) -> Self {
        Self {
            name: value.name,
            explanation: value.explanation,
            enabled: value.enabled,
            conditions: value
                .conditions
                .into_iter()
                .map(|condition| domain::RuleCondition {
                    field: condition.field.into(),
                    operator: condition.operator.into(),
                    value: condition.value,
                })
                .collect(),
            action: value.action.into(),
        }
    }
}

impl From<domain::LocalRule> for LocalRuleDto {
    fn from(value: domain::LocalRule) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            name: value.name,
            explanation: value.explanation,
            position: value.position,
            enabled: value.enabled,
            conditions: value
                .conditions
                .into_iter()
                .map(|condition| RuleConditionDto {
                    field: condition.field.into(),
                    operator: condition.operator.into(),
                    value: condition.value,
                })
                .collect(),
            action: value.action.into(),
            origin: value.origin.database_name().to_owned(),
            source_suggestion_id: value.source_suggestion_id.map(|id| id.to_string()),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<domain::RuleSuggestion> for RuleSuggestionDto {
    fn from(value: domain::RuleSuggestion) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            signature: value.signature,
            title: value.title,
            explanation: value.explanation,
            evidence_count: value.evidence_count,
            status: value.status.database_name().to_owned(),
            proposed_rule: value.proposed_rule.into(),
            accepted_rule_id: value.accepted_rule_id.map(|id| id.to_string()),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<domain::OrganizationPreferences> for OrganizationPreferencesDto {
    fn from(value: domain::OrganizationPreferences) -> Self {
        Self {
            client_first: value.client_first,
            include_year_folders: value.include_year_folders,
            maximum_depth: value.maximum_depth,
            minimum_group_size: value.minimum_group_size,
            keep_photos_inside_projects: value.keep_photos_inside_projects,
            supplier_invoices_inside_projects: value.supplier_invoices_inside_projects,
            naming_language: value.naming_language,
            preserve_existing_folders: value.preserve_existing_folders,
            personal_root_name: value.personal_root_name,
            business_root_name: value.business_root_name,
            rename_template: value.rename_template,
            review_threshold: value.review_threshold,
        }
    }
}

impl From<OrganizationPreferencesDto> for domain::OrganizationPreferences {
    fn from(value: OrganizationPreferencesDto) -> Self {
        Self {
            client_first: value.client_first,
            include_year_folders: value.include_year_folders,
            maximum_depth: value.maximum_depth,
            minimum_group_size: value.minimum_group_size,
            keep_photos_inside_projects: value.keep_photos_inside_projects,
            supplier_invoices_inside_projects: value.supplier_invoices_inside_projects,
            naming_language: value.naming_language,
            preserve_existing_folders: value.preserve_existing_folders,
            personal_root_name: value.personal_root_name,
            business_root_name: value.business_root_name,
            rename_template: value.rename_template,
            review_threshold: value.review_threshold,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationReasonDto {
    pub code: String,
    pub explanation: String,
    pub evidence_references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationOperationDto {
    pub id: String,
    pub file_id: String,
    pub file_version_id: String,
    pub source_relative_path: String,
    pub source_name: String,
    pub source_hash: Option<String>,
    pub source_byte_size: u64,
    pub source_modified_at: Option<String>,
    pub machine_destination: Vec<String>,
    pub machine_name: String,
    pub proposed_destination: Vec<String>,
    pub proposed_name: String,
    pub proposed_relative_path: String,
    pub operation_kind: String,
    pub confidence_score: f32,
    pub confidence_level: String,
    pub reasons: Vec<OrganizationReasonDto>,
    pub conflict_state: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualProposalNodeDto {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub virtual_path: String,
    pub operation_id: Option<String>,
    pub child_count: u64,
    pub needs_review_count: u64,
    pub conflict_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationProposalDto {
    pub id: String,
    pub revision_id: String,
    pub workspace_id: String,
    pub root_id: String,
    pub source_scan_id: String,
    pub revision: u32,
    pub status: String,
    pub engine_version: String,
    pub policy_version: String,
    pub source_semantic_version: Option<String>,
    pub source_relationship_version: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub summary: OrganizationProposalSummaryDto,
    pub change: OrganizationProposalChangeDto,
    pub nodes: Vec<VirtualProposalNodeDto>,
    pub operations: Vec<OrganizationOperationDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationProposalProgressDto {
    pub proposal_id: String,
    pub phase: String,
    pub files_total: u64,
    pub files_evaluated: u64,
    pub high_confidence: u64,
    pub needs_review: u64,
    pub conflicts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionSummaryDto {
    pub affected_files: u64,
    pub folders_to_create: u64,
    pub files_to_move: u64,
    pub files_to_rename: u64,
    pub files_unchanged: u64,
    pub conflicts: u64,
    pub needs_review: u64,
    pub preflight_ok: u64,
    pub applied: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub failed: u64,
    pub rolled_back: u64,
    pub rollback_blocked: u64,
    pub rollback_failed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionSessionDto {
    pub id: String,
    pub plan_id: String,
    pub proposal_id: String,
    pub proposal_revision: u32,
    pub workspace_id: String,
    pub status: String,
    pub recovery_state: String,
    pub plan_digest: String,
    pub approved_operation_count: u64,
    pub consent_state: String,
    pub consent_issued_at_unix_ms: Option<i64>,
    pub consent_expires_at_unix_ms: Option<i64>,
    pub consent_attested_at_unix_ms: Option<i64>,
    pub consent_consumed_at_unix_ms: Option<i64>,
    pub consent_invalidated_at_unix_ms: Option<i64>,
    pub summary: ExecutionSummaryDto,
    pub current_operation: Option<String>,
    pub rollback_available: bool,
    pub confirmation_phrase_required: bool,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub rolled_back_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOperationDto {
    pub id: String,
    pub proposal_operation_id: Option<String>,
    pub kind: String,
    pub source_relative_path: Option<String>,
    pub destination_relative_path: String,
    pub sequence: u32,
    pub status: String,
    pub reason: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionDetailDto {
    pub session: ExecutionSessionDto,
    pub operations: Vec<ExecutionOperationDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionProgressDto {
    pub execution_id: String,
    pub status: String,
    pub completed: u64,
    pub total: u64,
    pub applied: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub failed: u64,
    pub current: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAssessmentDto {
    pub execution_id: String,
    pub state: String,
    pub affected_count: u64,
    pub not_started: u64,
    pub applied: u64,
    pub ambiguous: u64,
    pub verified_applied_items: Vec<RecoveryItemDto>,
    pub verified_not_started_items: Vec<RecoveryItemDto>,
    pub ambiguous_items: Vec<RecoveryItemDto>,
    pub rollback_available: bool,
    pub executor_sessions: Vec<ExecutorSessionFactDto>,
    pub executor_requests: Vec<ExecutorRequestFactDto>,
    pub journal_diagnostics: JournalDiagnosticStateDto,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryItemDto {
    pub operation_id: String,
    pub direction: String,
    pub item: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorSessionFactDto {
    pub session_id: String,
    pub execution_id: String,
    pub plan_id: String,
    pub purpose: String,
    pub coordinator_pid: u32,
    pub child_pid: Option<u32>,
    pub opened_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorRequestFactDto {
    pub request_id: String,
    pub session_id: String,
    pub operation_id: String,
    pub direction: String,
    pub request_sequence: u64,
    pub intent_event_sequence: u64,
    pub outcome_class: Option<String>,
    pub attempt_count: Option<u8>,
    pub error_class: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalDiagnosticStateDto {
    pub locked: bool,
    pub diagnostics: Vec<JournalDiagnosticDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_json_uses_renderer_camel_case() {
        let value = serde_json::to_value(SystemStatusDto {
            local_first: true,
            read_only_scan: true,
            network_disabled: true,
            apply_enabled: false,
            apply_gate_reason: None,
            display_label: None,
            version: None,
            recovery_required: false,
            journal_locked: false,
            journal_diagnostics: Vec::new(),
        })
        .unwrap_or_else(|error| panic!("serialization should succeed: {error}"));
        assert_eq!(value["localFirst"], true);
        assert!(value.get("local_first").is_none());
    }
}
