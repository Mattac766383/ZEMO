export type Identifier = string;

export interface SystemStatus {
  localFirst: boolean;
  readOnlyScan: boolean;
  networkDisabled: boolean;
  applyEnabled: boolean;
  applyGateReason?: string | null;
  displayLabel?: string | null;
  version?: string | null;
  recoveryRequired: boolean;
  journalLocked: boolean;
  journalDiagnostics: JournalDiagnostic[];
}

export interface JournalDiagnostic {
  scope: "database" | "external" | string;
  executionId?: Identifier | null;
  code: string;
  message: string;
  detectedAtUnixMs: number;
  recoveryAvailable: boolean;
  rollbackAvailable: boolean;
}

export interface RegisteredRoot {
  id: Identifier;
  displayLabel: string;
  selectedPath: string;
}

export type UserContentKind =
  | "desktop"
  | "documents"
  | "downloads"
  | "pictures"
  | "movies"
  | "music";

export type FolderAccessState =
  | "accessible"
  | "authorization_required"
  | "missing"
  | "unsupported"
  | "locked"
  | "permission_denied"
  | "temporarily_unavailable"
  | "unexpected_error"
  | string;

export interface UserContentLocation {
  kind: UserContentKind | string;
  displayLabel: string;
  absolutePath: string;
  exists: boolean;
  readable: boolean;
  recommended: boolean;
  accessState?: FolderAccessState;
  humanStatus?: string;
  writable?: boolean;
  rawOsError?: number | null;
  platformError?: string | null;
}

export interface FolderAccessProbe {
  logicalName: string;
  kind: string;
  displayLabel: string;
  resolvedPath: string;
  exists: boolean;
  isDir: boolean;
  readable: boolean;
  writable: boolean;
  recommended: boolean;
  rawOsError?: number | null;
  platformError?: string | null;
  accessState: FolderAccessState;
  humanStatus: string;
  canonicalPath?: string;
  failedStage?: string | null;
  errorKind?: string | null;
  inspectResult?: string | null;
  technicalDetails?: string;
}

export interface RegisterUserContentRootResult {
  root?: RegisteredRoot | null;
  kind: string;
  displayLabel: string;
  absolutePath: string;
  status:
    | "registered"
    | "denied"
    | "missing"
    | "unavailable"
    | "rejected"
    | "error"
    | FolderAccessState
    | string;
  accessState?: FolderAccessState;
  humanStatus?: string;
  message?: string | null;
}

export interface Workspace {
  id: Identifier;
  name: string;
  root?: RegisteredRoot | null;
  createdAt?: string | null;
}

export type ScanStatus =
  | "PENDING"
  | "RUNNING"
  | "COMPLETED"
  | "COMPLETED_WITH_ERRORS"
  | "FAILED"
  | "CANCELLED";

export type ScanPhase =
  | "DISCOVERING"
  | "INSPECTING"
  | "HASHING"
  | "PERSISTING"
  | "COMPLETED"
  | "CANCELLED";

export interface ScanResult {
  id: Identifier;
  status: ScanStatus | string;
  startedAt?: string | null;
  completedAt?: string | null;
  filesDiscovered: number;
  filesIndexed: number;
  directoriesDiscovered: number;
  bytesDiscovered: number;
  filesHashed: number;
  duplicateGroups: number;
  errors: number;
  skippedItems: number;
  truncated: boolean;
}

export interface ScanProgress {
  scanId: Identifier;
  phase: ScanPhase;
  filesDiscovered: number;
  filesIndexed: number;
  directoriesDiscovered: number;
  bytesDiscovered: number;
  filesHashed: number;
  duplicateGroups: number;
  errors: number;
  skippedItems: number;
}

export interface ScanFile {
  id: Identifier;
  filename: string;
  fileType?: string | null;
  extension?: string | null;
  byteSize: number;
  modifiedAt?: string | null;
  relativePath: string;
  status: string;
  hashingStatus: string;
  readable: boolean;
}

export interface ScanIssue {
  relativePath: string;
  category: string;
  message: string;
  isDirectory: boolean;
}

export interface DuplicateFile {
  id: Identifier;
  filename: string;
  relativePath: string;
}

export interface DuplicateGroup {
  digest: string;
  byteSize: number;
  files: DuplicateFile[];
}

export type ContentAnalysisStatus =
  | "PENDING"
  | "RUNNING"
  | "COMPLETED"
  | "CANCELLED"
  | "FAILED";

export interface ContentAnalysis {
  id: Identifier;
  scanId: Identifier;
  status: ContentAnalysisStatus | string;
  filesQueued: number;
  filesCompleted: number;
  successful: number;
  partial: number;
  unsupported: number;
  skipped: number;
  failed: number;
  ocrProcessed: number;
  startedAt?: string | null;
  completedAt?: string | null;
}

export interface ContentAnalysisProgress {
  batchId: Identifier;
  scanId: Identifier;
  phase: "RUNNING" | "COMPLETED" | "CANCELLED" | "FAILED";
  filesQueued: number;
  filesCompleted: number;
  successful: number;
  partial: number;
  unsupported: number;
  skipped: number;
  failed: number;
  ocrProcessed: number;
}

export interface SemanticAnalysis {
  id: Identifier;
  scanId: Identifier;
  status: "PENDING" | "RUNNING" | "COMPLETED" | "CANCELLED" | "FAILED" | string;
  filesQueued: number;
  filesCompleted: number;
  highConfidence: number;
  needsReview: number;
  unknown: number;
  partial: number;
  failed: number;
  startedAt?: string | null;
  completedAt?: string | null;
}

export interface SemanticAnalysisProgress {
  batchId: Identifier;
  scanId: Identifier;
  phase: "RUNNING" | "COMPLETED" | "CANCELLED" | "FAILED";
  filesQueued: number;
  filesCompleted: number;
  highConfidence: number;
  needsReview: number;
  unknown: number;
  partial: number;
  failed: number;
}

export interface ContentDetail {
  fileVersionId: Identifier;
  filename: string;
  relativePath: string;
  extension?: string | null;
  status: string;
  extractorType?: string | null;
  extractorVersion?: string | null;
  detectedContentType?: string | null;
  typeMismatch: boolean;
  textPreview: string;
  characterCount: number;
  pageCount?: number | null;
  sheetCount?: number | null;
  slideCount?: number | null;
  imageWidth?: number | null;
  imageHeight?: number | null;
  requiresOcr: boolean;
  ocrUsed: boolean;
  ocrConfidence?: number | null;
  languageHint?: string | null;
  extractionDurationMs: number;
  truncated: boolean;
  structuredMetadata: Record<string, unknown>;
  errorCategory?: string | null;
  errorMessage?: string | null;
  extractedAt?: string | null;
}

export type InventorySort =
  | "filename"
  | "type"
  | "size"
  | "modified"
  | "location"
  | "status";

export type SearchFileType =
  | "all"
  | "pdf"
  | "documents"
  | "spreadsheets"
  | "presentations"
  | "images"
  | "archives"
  | "other";

export type SearchModified =
  | "any"
  | "today"
  | "last_7_days"
  | "last_30_days"
  | "this_year";

export type SearchExtraction =
  | "any"
  | "success"
  | "partial"
  | "failed"
  | "unsupported";

export type SearchOcr = "any" | "used" | "not_used" | "unavailable";
export type SearchSort = "relevance" | "newest" | "oldest" | "filename" | "size";
export type SearchDocumentType =
  | "any"
  | "invoice"
  | "quote"
  | "contract"
  | "purchase_order"
  | "delivery_note"
  | "bank_statement"
  | "tax_document"
  | "payslip"
  | "employment_contract"
  | "insurance_document"
  | "legal_document"
  | "administrative_document"
  | "receipt"
  | "report"
  | "letter"
  | "cv"
  | "photo"
  | "video"
  | "spreadsheet"
  | "presentation"
  | "archive"
  | "other"
  | "unknown";
export type SearchContext = "any" | "personal" | "business" | "mixed" | "unknown";
export type SearchSemanticStatus =
  | "any"
  | "success"
  | "partial"
  | "unknown"
  | "failed"
  | "pending";

export interface LocalSearchFilters {
  fileType: SearchFileType;
  modified: SearchModified;
  extraction: SearchExtraction;
  ocr: SearchOcr;
  minimumSize?: number | null;
  maximumSize?: number | null;
  documentType: SearchDocumentType;
  context: SearchContext;
  customer?: string | null;
  supplier?: string | null;
  project?: string | null;
  year?: number | null;
  amountMinimumMinor?: number | null;
  amountMaximumMinor?: number | null;
  currency?: string | null;
  semanticStatus: SearchSemanticStatus;
  minimumConfidencePercent?: number | null;
}

export interface LocalSearchQuery {
  text: string;
  filters: LocalSearchFilters;
  sort: SearchSort;
  page: number;
  pageSize: number;
  semanticSearch: boolean;
  disabledIntents: string[];
}

export interface LocalSearchResult {
  fileId: Identifier;
  filename: string;
  relativePath: string;
  detectedType?: string | null;
  extension?: string | null;
  byteSize: number;
  modifiedAt?: string | null;
  extractionStatus?: string | null;
  ocrStatus?: string | null;
  duplicate: boolean;
  matchSource:
    | "filename"
    | "path"
    | "content"
    | "metadata"
    | "structured"
    | "relationship"
    | "semantic";
  relevance: number;
  snippet: string;
  whyMatched: string[];
}

export interface QueryChip {
  id: string;
  kind: string;
  label: string;
  value: string;
}

export interface EmbeddingSearchStatus {
  availability:
    | "available_development"
    | "available_production"
    | "unavailable";
  providerId: string;
  version: string;
  productionReady: boolean;
  indexedFiles: number;
  annIndexStatus?: string | null;
}

export type EmbeddingModelLifecycleStatus =
  | "not_installed"
  | "downloading"
  | "installing"
  | "ready"
  | "loading"
  | "unavailable"
  | "corrupt"
  | "incompatible_version"
  | "failed";

export interface EmbeddingModelStatus {
  modelId: string;
  version: string;
  dimensions: number;
  status: EmbeddingModelLifecycleStatus;
  approximateDiskBytes: number;
  license: string;
  localOnly: boolean;
  downloadImplemented: boolean;
  lastError: string | null;
  installRoot: string;
}

export interface SearchTimings {
  totalMs: number;
  lexicalAndStructuredMs: number;
  queryEmbedMs: number;
  annMs: number;
  vectorMs: number;
  fusionMs: number;
}

export interface LocalSearchPage {
  query: string;
  page: number;
  pageSize: number;
  total: number;
  hasMore: boolean;
  results: LocalSearchResult[];
  interpretedQuery: QueryChip[];
  embeddings: EmbeddingSearchStatus;
  timings: SearchTimings;
}

export type ReviewStatus = "NEEDS_REVIEW" | "RESOLVED" | "IGNORED";
export type ReviewStatusFilter = "needs_review" | "resolved" | "ignored" | "all";
export type ReviewReasonFilter =
  | "all"
  | "ocr"
  | "unsupported"
  | "permissions"
  | "partial"
  | "corrupt"
  | "semantic";

export interface FileReviewItem {
  reviewId: Identifier;
  fileId: Identifier;
  filename: string;
  relativePath: string;
  reason: string;
  sourceSubsystem: string;
  severity: string;
  explanation: string;
  technicalDetails?: string | null;
  status: ReviewStatus | string;
  retryAvailable: boolean;
  retryCount: number;
  extractionStatus?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface FileReviewPage {
  total: number;
  limit: number;
  offset: number;
  hasMore: boolean;
  items: FileReviewItem[];
}

export interface SemanticEvidence {
  evidenceType: string;
  exactText: string;
  startOffset?: number | null;
  endOffset?: number | null;
  pageNumber?: number | null;
  sheetName?: string | null;
  slideNumber?: number | null;
  sourceLabel: string;
  explanation: string;
  extractionMethod: string;
  analyzerVersion: string;
}

export interface SemanticCandidateValue {
  displayValue: string;
  normalizedValue: unknown;
  confidence: number;
  status: string;
  sourceMethod: string;
  evidence: SemanticEvidence[];
}

export interface SemanticField {
  fieldId: Identifier;
  fieldKey: string;
  valueKind?: string | null;
  displayValue?: string | null;
  machineDisplayValue?: string | null;
  normalizedValue: unknown;
  confidence: number;
  status: "CONFIRMED" | "INFERRED" | "AMBIGUOUS" | "UNKNOWN" | "CONFLICTING" | string;
  sourceMethod: string;
  analyzerVersion: string;
  valueSource: "MACHINE" | "USER" | string;
  userState?: "USER_CONFIRMED" | "USER_CORRECTED" | string | null;
  evidence: SemanticEvidence[];
  candidates: SemanticCandidateValue[];
}

export interface SemanticEntity {
  entityId: Identifier;
  candidateKey: string;
  entityType: string;
  originalValue: string;
  normalizedValue: string;
  confidence: number;
  status: string;
  sourceMethod: string;
  evidence: SemanticEvidence[];
}

export interface SemanticAnalysisDetail {
  analysisId: Identifier;
  status: string;
  analyzerId: string;
  analyzerVersion: string;
  providerId: string;
  providerVersion: string;
  schemaVersion: number;
  inputQuality: number;
  inputQualityStatus: string;
  inputQualityReasons: string[];
  language?: string | null;
  analyzedAt?: string | null;
  fields: SemanticField[];
  entities: SemanticEntity[];
}

export interface SemanticCorrection {
  correctionId: Identifier;
  fileId: Identifier;
  fieldKey: string;
  correctionState: string;
  valueKind: string;
  displayValue: string;
  normalizedValue: unknown;
  createdAt: string;
  updatedAt: string;
}

export interface LocalFileDetail {
  fileId: Identifier;
  fileVersionId: Identifier;
  filename: string;
  relativePath: string;
  extension?: string | null;
  detectedType?: string | null;
  byteSize: number;
  createdAt?: string | null;
  modifiedAt?: string | null;
  hash?: string | null;
  duplicate: boolean;
  extractionStatus?: string | null;
  extractorType?: string | null;
  extractorVersion?: string | null;
  ocrStatus?: string | null;
  textPreview: string;
  characterCount: number;
  reviewItems: FileReviewItem[];
  semanticAnalysis?: SemanticAnalysisDetail | null;
  relationships: IdentityRelationship[];
}

export interface IdentityResolution {
  runId: Identifier;
  workspaceId: Identifier;
  triggerKind: string;
  status: string;
  resolverId: string;
  resolverVersion: string;
  filesConsidered: number;
  occurrencesProcessed: number;
  blockingMemberships: number;
  comparisons: number;
  candidatesCreated: number;
  autoLinksCreated: number;
  startedAt: string;
  completedAt?: string | null;
}

export interface IdentityResolutionProgress {
  runId: Identifier;
  workspaceId: Identifier;
  phase: "RUNNING" | "COMPLETED" | "CANCELLED" | "FAILED";
  filesConsidered: number;
  occurrencesProcessed: number;
  blockingMemberships: number;
  comparisons: number;
  candidatesCreated: number;
  autoLinksCreated: number;
}

export interface IdentitySummary {
  identityId: Identifier;
  identityType: "ORGANIZATION" | "PERSON" | "PROJECT" | string;
  displayName: string;
  normalizedDisplayName: string;
  resolutionStatus: string;
  lifecycleStatus: string;
  confidence: number;
  userLocked: boolean;
  occurrenceCount: number;
  fileCount: number;
  aliases: string[];
  roles: string[];
}

export interface IdentityMatchEvidence {
  evidenceType: string;
  strength: "VERY_STRONG" | "STRONG" | "MEDIUM" | "WEAK" | "CONFLICTING" | string;
  polarity: "SUPPORTS" | "CONFLICTS" | string;
  leftValue: string;
  rightValue: string;
  weight: number;
  explanation: string;
}

export interface IdentityCandidate {
  candidateId: Identifier;
  reviewGroupKey: string;
  score: number;
  policyDecision: "AUTO_LINK" | "REVIEW" | "KEEP_SEPARATE" | "UNKNOWN" | string;
  status: string;
  resolverVersion: string;
  left: IdentitySummary;
  right: IdentitySummary;
  evidence: IdentityMatchEvidence[];
  createdAt: string;
  updatedAt: string;
}

export interface IdentityReviewGroup {
  reviewGroupId: Identifier;
  reviewReason: string;
  groupKey: string;
  title: string;
  explanation: string;
  maxScore: number;
  candidateCount: number;
  occurrenceCount: number;
  fileCount: number;
  status: string;
  resolverVersion: string;
  candidates: IdentityCandidate[];
  createdAt: string;
  updatedAt: string;
}

export interface IdentityReviewPage {
  total: number;
  limit: number;
  offset: number;
  hasMore: boolean;
  items: IdentityReviewGroup[];
}

export interface IdentityOccurrence {
  occurrenceId: Identifier;
  fileId: Identifier;
  filename: string;
  relativePath: string;
  originalValue: string;
  normalizedValue: string;
  confidence: number;
  role?: string | null;
  analyzerVersion: string;
  active: boolean;
}

export interface IdentityRelationship {
  relationshipId: Identifier;
  relationshipType: string;
  identityId: Identifier;
  displayName: string;
  identityType: string;
  confidence: number;
  status: string;
  userConfirmationState?: string | null;
  evidence: string[];
}

export interface IdentityIdentifier {
  kind: string;
  value: string;
}

export interface IdentityAuditEvent {
  eventType: string;
  decisionSource: string;
  relatedIdentityId?: Identifier | null;
  reason?: string | null;
  createdAt: string;
}

export interface IdentityDetail {
  identity: IdentitySummary;
  occurrences: IdentityOccurrence[];
  occurrenceTotal: number;
  occurrencesTruncated: boolean;
  identifiers: IdentityIdentifier[];
  relationships: IdentityRelationship[];
  projects: IdentitySummary[];
  auditEvents: IdentityAuditEvent[];
  resolverVersion: string;
  updatedAt: string;
}

export interface IdentityMutation {
  decisionId: Identifier;
  primaryIdentityId: Identifier;
  secondaryIdentityId?: Identifier | null;
  occurrenceId?: Identifier | null;
  action: string;
  createdAt: string;
}

export type RuleField =
  | "document_type"
  | "context"
  | "supplier"
  | "customer"
  | "project"
  | "any_party"
  | "source_path";

export type RuleOperator = "equals" | "exists" | "starts_with";

export interface RuleCondition {
  field: RuleField;
  operator: RuleOperator;
  value?: string | null;
}

export type SemanticRuleField =
  | "document_type"
  | "context"
  | "supplier"
  | "customer"
  | "project";

export type RuleAction =
  | {
      kind: "set_semantic_field";
      field: SemanticRuleField;
      value: string;
    }
  | {
      kind: "classify_party";
      party: string;
      role: "supplier" | "customer";
    }
  | { kind: "prefer_project_location" }
  | { kind: "set_destination"; segments: string[] }
  | { kind: "preserve_subtree" }
  | { kind: "use_year_folders"; enabled: boolean };

export interface LocalRuleInput {
  name: string;
  explanation: string;
  enabled: boolean;
  conditions: RuleCondition[];
  action: RuleAction;
}

export interface LocalRule extends LocalRuleInput {
  id: Identifier;
  workspaceId: Identifier;
  position: number;
  origin: "user_created" | "accepted_suggestion" | string;
  sourceSuggestionId?: Identifier | null;
  createdAt: string;
  updatedAt: string;
}

export interface RuleSuggestion {
  id: Identifier;
  workspaceId: Identifier;
  signature: string;
  title: string;
  explanation: string;
  evidenceCount: number;
  status: "pending" | "accepted" | "dismissed" | string;
  proposedRule: LocalRuleInput;
  acceptedRuleId?: Identifier | null;
  createdAt: string;
  updatedAt: string;
}

export interface OrganizationPreferences {
  clientFirst: boolean;
  includeYearFolders: boolean;
  maximumDepth: number;
  minimumGroupSize: number;
  keepPhotosInsideProjects: boolean;
  supplierInvoicesInsideProjects: boolean;
  namingLanguage: "en" | "fr";
  preserveExistingFolders: boolean;
  personalRootName: string;
  businessRootName: string;
  renameTemplate: string;
  reviewThreshold: number;
}

export interface RulesPreferencesState {
  rules: LocalRule[];
  suggestions: RuleSuggestion[];
  preferences: OrganizationPreferences;
}

export type OrganizationProposalStatus =
  | "DRAFT"
  | "READY_FOR_REVIEW"
  | "REVIEWED"
  | "APPROVED_FOR_FUTURE_APPLY"
  | "SUPERSEDED"
  | "CANCELLED";

export interface OrganizationProposalSummary {
  filesAnalyzed: number;
  proposedMoves: number;
  proposedRenames: number;
  unchanged: number;
  needsReview: number;
  unresolved: number;
  conflicts: number;
  highConfidence: number;
  mediumConfidence: number;
  lowConfidence: number;
  duplicateNoAction: number;
  averageDepth: number;
  maximumDepth: number;
}

export interface OrganizationProposalChange {
  destinationsChanged: number;
  filesAdded: number;
  conflictsResolved: number;
  movedToReview: number;
}

export interface OrganizationReason {
  code: string;
  explanation: string;
  evidenceReferences: string[];
}

export interface OrganizationOperation {
  id: Identifier;
  fileId: Identifier;
  fileVersionId: Identifier;
  sourceRelativePath: string;
  sourceName: string;
  sourceHash?: string | null;
  sourceByteSize: number;
  sourceModifiedAt?: string | null;
  machineDestination: string[];
  machineName: string;
  proposedDestination: string[];
  proposedName: string;
  proposedRelativePath: string;
  operationKind:
    | "MOVE_PROPOSAL"
    | "RENAME_PROPOSAL"
    | "CREATE_FOLDER_PROPOSAL"
    | "KEEP_IN_PLACE"
    | "TO_REVIEW"
    | "NO_ACTION"
    | string;
  confidenceScore: number;
  confidenceLevel: "VERY_HIGH" | "HIGH" | "MEDIUM" | "LOW" | string;
  reasons: OrganizationReason[];
  conflictState: string;
  needsReview: boolean;
  stale: boolean;
  userOverride: boolean;
  disruptionScore: number;
  proposedPathLength: number;
  proposedDepth: number;
  semanticContext: string;
  documentType: string;
  customerName?: string | null;
  supplierName?: string | null;
  projectName?: string | null;
  duplicateGroupId?: string | null;
  duplicateCanonical: boolean;
}

export interface VirtualProposalNode {
  id: Identifier;
  parentId?: Identifier | null;
  kind: "ROOT" | "FOLDER" | "FILE";
  name: string;
  virtualPath: string;
  operationId?: Identifier | null;
  childCount: number;
  needsReviewCount: number;
  conflictCount: number;
}

export interface OrganizationProposal {
  id: Identifier;
  revisionId: Identifier;
  workspaceId: Identifier;
  rootId: Identifier;
  sourceScanId: Identifier;
  revision: number;
  status: OrganizationProposalStatus | string;
  engineVersion: string;
  policyVersion: string;
  sourceSemanticVersion?: string | null;
  sourceRelationshipVersion?: string | null;
  createdAt: string;
  updatedAt: string;
  summary: OrganizationProposalSummary;
  change: OrganizationProposalChange;
  nodes: VirtualProposalNode[];
  operations: OrganizationOperation[];
}

export interface OrganizationProposalProgress {
  proposalId: Identifier;
  phase:
    | "EVALUATING"
    | "RESOLVING_GROUPS"
    | "DETECTING_CONFLICTS"
    | "BUILDING_TREE"
    | "COMPLETED"
    | "CANCELLED";
  filesTotal: number;
  filesEvaluated: number;
  highConfidence: number;
  needsReview: number;
  conflicts: number;
}

export type ExecutionStatus =
  | "PREPARED"
  | "AWAITING_CONFIRMATION"
  | "APPROVED"
  | "RUNNING"
  | "PAUSED"
  | "CANCELLED"
  | "COMPLETED"
  | "PARTIAL"
  | "FAILED"
  | "RECOVERY_REQUIRED"
  | "RECOVERY_AVAILABLE"
  | "RECOVERY_AMBIGUOUS"
  | "ROLLING_BACK"
  | "ROLLED_BACK"
  | "ROLLBACK_PARTIAL";

export type ExecutionConsentState =
  | "PENDING"
  | "ATTESTED"
  | "CONSUMED"
  | "EXPIRED"
  | "INVALIDATED";

export interface ExecutionSummary {
  affectedFiles: number;
  foldersToCreate: number;
  filesToMove: number;
  filesToRename: number;
  filesUnchanged: number;
  conflicts: number;
  needsReview: number;
  preflightOk: number;
  applied: number;
  blocked: number;
  skipped: number;
  failed: number;
  rolledBack: number;
  rollbackBlocked: number;
  rollbackFailed: number;
}

export interface ExecutionSession {
  id: Identifier;
  planId: Identifier;
  proposalId: Identifier;
  proposalRevision: number;
  workspaceId: Identifier;
  status: ExecutionStatus | string;
  recoveryState:
    | "RECOVERY_NOT_REQUIRED"
    | "RECOVERY_AVAILABLE"
    | "RECOVERY_REQUIRED"
    | "RECOVERY_AMBIGUOUS"
    | string;
  planDigest: string;
  approvedOperationCount: number;
  consentState: ExecutionConsentState | string;
  consentIssuedAtUnixMs?: number | null;
  consentExpiresAtUnixMs?: number | null;
  consentAttestedAtUnixMs?: number | null;
  consentConsumedAtUnixMs?: number | null;
  consentInvalidatedAtUnixMs?: number | null;
  summary: ExecutionSummary;
  currentOperation?: string | null;
  rollbackAvailable: boolean;
  confirmationPhraseRequired: boolean;
  createdAt: string;
  approvedAt?: string | null;
  startedAt?: string | null;
  completedAt?: string | null;
  rolledBackAt?: string | null;
  error?: string | null;
}

export interface ExecutionOperation {
  id: Identifier;
  proposalOperationId?: Identifier | null;
  kind: string;
  sourceRelativePath?: string | null;
  destinationRelativePath: string;
  sequence: number;
  status: string;
  reason?: string | null;
  errorCode?: string | null;
  errorMessage?: string | null;
}

export interface ExecutionDetail {
  session: ExecutionSession;
  operations: ExecutionOperation[];
}

export interface ExecutionProgress {
  executionId: Identifier;
  status: ExecutionStatus | string;
  completed: number;
  total: number;
  applied: number;
  blocked: number;
  skipped: number;
  failed: number;
  current?: string | null;
}

export interface RecoveryAssessment {
  executionId: Identifier;
  state: string;
  affectedCount: number;
  notStarted: number;
  applied: number;
  ambiguous: number;
  verifiedAppliedItems: RecoveryItem[];
  verifiedNotStartedItems: RecoveryItem[];
  ambiguousItems: RecoveryItem[];
  rollbackAvailable: boolean;
  executorSessions: ExecutorSessionFact[];
  executorRequests: ExecutorRequestFact[];
  journalDiagnostics: {
    locked: boolean;
    diagnostics: JournalDiagnostic[];
  };
  message: string;
}

export interface RecoveryItem {
  operationId: Identifier;
  direction: "FORWARD" | "ROLLBACK" | string;
  item: string;
  reason?: string | null;
}

export interface ExecutorSessionFact {
  sessionId: string;
  executionId: Identifier;
  planId: Identifier;
  purpose: "FORWARD" | "ROLLBACK" | string;
  coordinatorPid: number;
  childPid?: number | null;
  openedAtUnixMs: number;
}

export interface ExecutorRequestFact {
  requestId: string;
  sessionId: string;
  operationId: Identifier;
  direction: "FORWARD" | "ROLLBACK" | string;
  requestSequence: number;
  intentEventSequence: number;
  outcomeClass?: string | null;
  attemptCount?: number | null;
  errorClass?: string | null;
  state: string;
}

export interface ExtractionRetry {
  reviewId: Identifier;
  batchId?: Identifier | null;
  fileId?: Identifier | null;
  status: "SUCCEEDED" | "PARTIAL" | "FAILED" | "UNAVAILABLE" | "CANCELLED";
  extractionStatus?: string | null;
  message: string;
}

export interface MonitoredFolder {
  rootId: Identifier;
  displayLabel: string;
  selectedPath: string;
  enabled: boolean;
  status: string;
  pendingJobs: number;
  lastReconciledAt?: string | null;
  lastError?: string | null;
}

export interface MonitoringCounts {
  filesAnalyzed: number;
  readyToOrganize: number;
  needsReview: number;
  pendingProposals: number;
  pendingJobs: number;
}

export interface MonitoringActivity {
  id: Identifier;
  summary: string;
  filesAnalyzed: number;
  readyToOrganize: number;
  needsReview: number;
  failed: number;
  createdAt: string;
}

export interface MonitoringExclusion {
  id: Identifier;
  rootId?: Identifier | null;
  kind: "path_prefix" | "extension";
  value: string;
  enabled: boolean;
}

export interface MonitoringDashboard {
  workspaceId: Identifier;
  mode: "PRUDENT" | "AUTOMATIC" | "RULES" | (string & Record<never, never>);
  paused: boolean;
  startupReconciliationPending: boolean;
  automaticExecutionEnabled: boolean;
  folders: MonitoredFolder[];
  counts: MonitoringCounts;
  recentActivity: MonitoringActivity[];
  exclusions: MonitoringExclusion[];
}

export interface RestoredWorkspaceSession {
  workspace: Workspace;
  root?: RegisteredRoot | null;
  scan?: ScanResult | null;
  safeReadOnly: true;
  filesystemExecutionResumed: false;
}
