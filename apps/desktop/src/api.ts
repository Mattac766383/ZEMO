import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ContentAnalysis,
  ContentAnalysisProgress,
  ContentDetail,
  DuplicateGroup,
  EmbeddingModelStatus,
  ExecutionDetail,
  ExecutionProgress,
  ExecutionSession,
  ExtractionRetry,
  FileReviewItem,
  FileReviewPage,
  IdentityDetail,
  IdentityMutation,
  IdentityResolution,
  IdentityResolutionProgress,
  IdentityReviewPage,
  InventorySort,
  LocalFileDetail,
  LocalRule,
  LocalRuleInput,
  LocalSearchPage,
  LocalSearchQuery,
  MonitoringDashboard,
  MonitoringExclusion,
  OrganizationProposal,
  OrganizationProposalProgress,
  OrganizationPreferences,
  RegisteredRoot,
  RegisterUserContentRootResult,
  RestoredWorkspaceSession,
  RecoveryAssessment,
  ReviewReasonFilter,
  ReviewStatusFilter,
  RuleSuggestion,
  RulesPreferencesState,
  ScanFile,
  ScanIssue,
  ScanProgress,
  ScanResult,
  SemanticAnalysis,
  SemanticAnalysisProgress,
  SemanticCorrection,
  SystemStatus,
  UserContentLocation,
  Workspace,
} from "./types";

export function redactPaths(value: string): string {
  return value
    .replace(
      /(?:file:\/\/)?\/(?:Users|home|Volumes|private|var|tmp|opt|usr|etc|Applications|Library)(?:\/[^,\n;:)"'<>]*)?/gi,
      "[chemin masqué]",
    )
    .replace(/[A-Za-z]:\\[^,\n;:)"'<>]*/g, "[chemin masqué]")
    .replace(/~\/[^,\n;:)"'<>]*/g, "[chemin masqué]");
}

export function getRawErrorText(error: unknown): string {
  if (error instanceof Error) {
    return redactPaths(error.message);
  }
  if (typeof error === "string") {
    return redactPaths(error);
  }
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return redactPaths(error.message);
  }
  return "La commande locale a échoué sans fournir de détail exploitable.";
}

export function getErrorMessage(error: unknown): string {
  const classified = classifyLoose(error);
  return classified.message;
}

export function getErrorTechnicalDetails(error: unknown): string | null {
  const raw = getRawErrorText(error);
  const human = humanizeErrorText(raw);
  if (!human || human === raw) {
    return null;
  }
  return raw;
}

function classifyLoose(error: unknown): { message: string } {
  const raw = getRawErrorText(error);
  const human = humanizeErrorText(raw);
  if (human) {
    return { message: human };
  }
  // Never surface the old catastrophic generic engine wording as-is.
  const normalized = raw.toLocaleLowerCase();
  if (normalized.includes("moteur local")) {
    return {
      message:
        "Cette action n’a pas pu être terminée pour le moment. Réessayez.",
    };
  }
  return { message: raw };
}

function humanizeErrorText(value: string): string | null {
  const normalized = value.toLocaleLowerCase();
  if (normalized.includes("file_in_use") || normalized.includes("sharing_violation") || normalized.includes("lock_violation")) {
    return "Ce fichier est actuellement utilisé par une autre application.";
  }
  if (
    normalized.includes("macos") ||
    normalized.includes("tcc") ||
    normalized.includes("n’autorise plus")
  ) {
    return "macOS n’autorise plus l’accès à ce dossier.";
  }
  if (
    normalized.includes("permission_denied") ||
    normalized.includes("access_denied") ||
    normalized.includes("permission denied") ||
    normalized.includes("eacces")
  ) {
    return "ZEMO a besoin d’accéder à ce dossier pour appliquer l’organisation.";
  }
  if (
    normalized.includes("destination_exists") ||
    normalized.includes("destination already exists")
  ) {
    return "Un fichier existe déjà à cet emplacement. Ce déplacement a été ignoré.";
  }
  if (
    normalized.includes("source_drift") ||
    normalized.includes("source_hash_drift") ||
    normalized.includes("execution_drift") ||
    normalized.includes("source_precondition")
  ) {
    return "Ce fichier a changé depuis l’aperçu.";
  }
  if (
    normalized.includes("rollback_blocked") ||
    normalized.includes("rollback blocked")
  ) {
    return "Impossible d’annuler ce déplacement car le fichier a été modifié ou remplacé depuis.";
  }
  if (
    normalized.includes("operation executor") ||
    normalized.includes("sidecar") ||
    (normalized.includes("exécuteur") &&
      (normalized.includes("indisponible") || normalized.includes("n’est pas disponible")))
  ) {
    return "L’application des fichiers n’est pas disponible dans cette session.";
  }
  if (
    normalized.includes("watcher") &&
    (normalized.includes("unavailable") ||
      normalized.includes("failed") ||
      normalized.includes("error"))
  ) {
    return "La surveillance de ce dossier est indisponible pour le moment.";
  }
  if (normalized.includes("not found") || normalized.includes("enoent")) {
    return "Cet élément est introuvable. Il a peut‑être été déplacé ou renommé.";
  }
  if (normalized.includes("timed out") || normalized.includes("timeout")) {
    return "L’opération a pris trop de temps. Réessayez dans un instant.";
  }
  if (normalized.includes("cancelled") || normalized.includes("canceled")) {
    return "L’opération a été annulée.";
  }
  if (
    normalized.includes("scan") &&
    (normalized.includes("fail") || normalized.includes("error"))
  ) {
    return "Le scan n’a pas pu se terminer correctement.";
  }
  if (normalized.includes("offline")) {
    return "Ce dossier ou cette surveillance est hors ligne pour le moment.";
  }
  if (normalized.includes("degraded")) {
    return "La surveillance fonctionne en mode dégradé.";
  }
  if (
    normalized.includes("search") &&
    (normalized.includes("unavailable") ||
      normalized.includes("failed") ||
      normalized.includes("error"))
  ) {
    return "La recherche est indisponible pour le moment.";
  }
  if (
    normalized.includes("moteur local") ||
    normalized.includes("opération interne") ||
    normalized.includes("action n’a pas pu")
  ) {
    return "Cette action n’a pas pu être terminée pour le moment. Réessayez.";
  }
  if (normalized.includes("blocked") || normalized.includes("operation_blocked")) {
    return "Cette opération est bloquée et n’a pas été appliquée.";
  }
  return null;
}

export function getSystemStatus(): Promise<SystemStatus> {
  return invoke<SystemStatus>("get_system_status");
}

export function createWorkspace(name: string): Promise<Workspace> {
  return invoke<Workspace>("create_workspace", { name });
}

export function selectAndRegisterRoot(
  workspaceId: string,
): Promise<RegisteredRoot> {
  return invoke<RegisteredRoot>("select_and_register_root", { workspaceId });
}

export function listUserContentLocations(): Promise<UserContentLocation[]> {
  return invoke<UserContentLocation[]>("list_user_content_locations");
}

export function registerUserContentRoot(
  workspaceId: string,
  kind: string,
): Promise<RegisterUserContentRootResult> {
  return invoke<RegisterUserContentRootResult>("register_user_content_root", {
    workspaceId,
    kind,
  });
}

export function scanWorkspace(workspaceId: string): Promise<ScanResult> {
  return invoke<ScanResult>("scan_workspace", { workspaceId });
}

export function cancelScan(workspaceId: string): Promise<boolean> {
  return invoke<boolean>("cancel_scan", { workspaceId });
}

export function subscribeScanProgress(
  handler: (progress: ScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<ScanProgress>("safe-scan-progress", (event) =>
    handler(event.payload),
  );
}

export function listScanFiles(
  scanId: string,
  sortBy: InventorySort,
  descending: boolean,
  limit = 500,
  offset = 0,
): Promise<ScanFile[]> {
  return invoke<ScanFile[]>("list_scan_files", {
    scanId,
    sortBy,
    descending,
    limit,
    offset,
  });
}

export function listScanDuplicates(
  scanId: string,
): Promise<DuplicateGroup[]> {
  return invoke<DuplicateGroup[]>("list_scan_duplicates", { scanId });
}

export function listScanErrors(scanId: string): Promise<ScanIssue[]> {
  return invoke<ScanIssue[]>("list_scan_errors", { scanId });
}

export function analyzeContent(scanId: string): Promise<ContentAnalysis> {
  return invoke<ContentAnalysis>("analyze_content", { scanId });
}

export function cancelContentAnalysis(scanId: string): Promise<boolean> {
  return invoke<boolean>("cancel_content_analysis", { scanId });
}

export function subscribeContentAnalysisProgress(
  handler: (progress: ContentAnalysisProgress) => void,
): Promise<UnlistenFn> {
  return listen<ContentAnalysisProgress>("content-analysis-progress", (event) =>
    handler(event.payload),
  );
}

export function analyzeSemantics(scanId: string): Promise<SemanticAnalysis> {
  return invoke<SemanticAnalysis>("analyze_semantics", { scanId });
}

export function cancelSemanticAnalysis(scanId: string): Promise<boolean> {
  return invoke<boolean>("cancel_semantic_analysis", { scanId });
}

export function subscribeSemanticAnalysisProgress(
  handler: (progress: SemanticAnalysisProgress) => void,
): Promise<UnlistenFn> {
  return listen<SemanticAnalysisProgress>("semantic-analysis-progress", (event) =>
    handler(event.payload),
  );
}

export function resolveIdentities(
  workspaceId: string,
  force = false,
): Promise<IdentityResolution> {
  return invoke<IdentityResolution>("resolve_identities", { workspaceId, force });
}

export function cancelIdentityResolution(workspaceId: string): Promise<boolean> {
  return invoke<boolean>("cancel_identity_resolution", { workspaceId });
}

export function subscribeIdentityResolutionProgress(
  handler: (progress: IdentityResolutionProgress) => void,
): Promise<UnlistenFn> {
  return listen<IdentityResolutionProgress>("identity-resolution-progress", (event) =>
    handler(event.payload),
  );
}

export function listIdentityReviewGroups(
  workspaceId: string,
  status: "needs_review" | "resolved" | "ignored" | "all" = "needs_review",
  limit = 50,
  offset = 0,
): Promise<IdentityReviewPage> {
  return invoke<IdentityReviewPage>("list_identity_review_groups", {
    workspaceId,
    status,
    limit,
    offset,
  });
}

export function getIdentityDetail(identityId: string): Promise<IdentityDetail> {
  return invoke<IdentityDetail>("get_identity_detail", { identityId });
}

export function decideIdentityCandidate(
  candidateId: string,
  action: "confirm" | "reject" | "keep_separate",
  reason?: string,
): Promise<IdentityMutation> {
  return invoke<IdentityMutation>("decide_identity_candidate", {
    candidateId,
    action,
    reason,
  });
}

export function mergeIdentities(
  primaryIdentityId: string,
  secondaryIdentityId: string,
  reason?: string,
): Promise<IdentityMutation> {
  return invoke<IdentityMutation>("merge_identities", {
    primaryIdentityId,
    secondaryIdentityId,
    reason,
  });
}

export function unlinkIdentityOccurrence(
  identityId: string,
  occurrenceId: string,
  reason?: string,
): Promise<IdentityMutation> {
  return invoke<IdentityMutation>("unlink_identity_occurrence", {
    identityId,
    occurrenceId,
    reason,
  });
}

export function generateOrganizationProposal(
  workspaceId: string,
  recomputeCurrent = false,
  rootId?: string,
): Promise<OrganizationProposal> {
  return invoke<OrganizationProposal>("generate_organization_proposal", {
    workspaceId,
    recomputeCurrent,
    rootId,
  });
}

export function cancelOrganizationProposal(workspaceId: string): Promise<boolean> {
  return invoke<boolean>("cancel_organization_proposal", { workspaceId });
}

export function subscribeOrganizationProposalProgress(
  handler: (progress: OrganizationProposalProgress) => void,
): Promise<UnlistenFn> {
  return listen<OrganizationProposalProgress>(
    "organization-proposal-progress",
    (event) => handler(event.payload),
  );
}

export function getLatestOrganizationProposal(
  workspaceId: string,
  rootId?: string,
  options?: { uiBound?: boolean; operationLimit?: number },
): Promise<OrganizationProposal> {
  return invoke<OrganizationProposal>("get_latest_organization_proposal", {
    workspaceId,
    rootId,
    uiBound: options?.uiBound ?? true,
    operationLimit: options?.operationLimit ?? 500,
  });
}

export function getOrganizationProposal(
  proposalId: string,
): Promise<OrganizationProposal> {
  return invoke<OrganizationProposal>("get_organization_proposal", { proposalId });
}

export function setOrganizationProposalOverride(
  proposalId: string,
  fileId: string,
  action:
    | "destination"
    | "rename"
    | "destination_and_rename"
    | "keep_in_place"
    | "to_review"
    | "reject",
  destination?: string[],
  proposedName?: string,
  reason?: string,
): Promise<OrganizationProposal> {
  return invoke<OrganizationProposal>("set_organization_proposal_override", {
    proposalId,
    fileId,
    action,
    destination,
    proposedName,
    reason,
  });
}

export function setOrganizationProposalStatus(
  proposalId: string,
  status: "reviewed" | "approved_for_future_apply" | "cancelled",
): Promise<OrganizationProposal> {
  return invoke<OrganizationProposal>("set_organization_proposal_status", {
    proposalId,
    status,
  });
}

export function refreshOrganizationProposalDrift(
  proposalId: string,
): Promise<OrganizationProposal> {
  return invoke<OrganizationProposal>("refresh_organization_proposal_drift", {
    proposalId,
  });
}

export function getRulesPreferences(
  workspaceId: string,
): Promise<RulesPreferencesState> {
  return invoke<RulesPreferencesState>("get_rules_preferences", { workspaceId });
}

export function createLocalRule(
  workspaceId: string,
  input: LocalRuleInput,
): Promise<LocalRule> {
  return invoke<LocalRule>("create_local_rule", { workspaceId, input });
}

export function updateLocalRule(
  workspaceId: string,
  ruleId: string,
  input: LocalRuleInput,
): Promise<LocalRule> {
  return invoke<LocalRule>("update_local_rule", { workspaceId, ruleId, input });
}

export function setLocalRuleEnabled(
  workspaceId: string,
  ruleId: string,
  enabled: boolean,
): Promise<LocalRule> {
  return invoke<LocalRule>("set_local_rule_enabled", {
    workspaceId,
    ruleId,
    enabled,
  });
}

export function deleteLocalRule(
  workspaceId: string,
  ruleId: string,
): Promise<boolean> {
  return invoke<boolean>("delete_local_rule", { workspaceId, ruleId });
}

export function reorderLocalRules(
  workspaceId: string,
  orderedIds: string[],
): Promise<LocalRule[]> {
  return invoke<LocalRule[]>("reorder_local_rules", { workspaceId, orderedIds });
}

export function storeLocalOrganizationPreferences(
  workspaceId: string,
  preferences: OrganizationPreferences,
): Promise<OrganizationPreferences> {
  return invoke<OrganizationPreferences>("store_local_organization_preferences", {
    workspaceId,
    preferences,
  });
}

export function acceptLocalRuleSuggestion(
  workspaceId: string,
  suggestionId: string,
): Promise<LocalRule> {
  return invoke<LocalRule>("accept_local_rule_suggestion", {
    workspaceId,
    suggestionId,
  });
}

export function dismissLocalRuleSuggestion(
  workspaceId: string,
  suggestionId: string,
): Promise<RuleSuggestion> {
  return invoke<RuleSuggestion>("dismiss_local_rule_suggestion", {
    workspaceId,
    suggestionId,
  });
}

export function recomputeRulesProposal(
  workspaceId: string,
): Promise<OrganizationProposal | null> {
  return invoke<OrganizationProposal | null>("recompute_rules_proposal", {
    workspaceId,
  });
}

export function prepareExecution(
  proposalId: string,
  revision: number,
): Promise<ExecutionDetail> {
  return invoke<ExecutionDetail>("prepare_execution", { proposalId, revision });
}

export function approveExecution(
  executionId: string,
  confirmationPhrase?: string,
): Promise<ExecutionDetail> {
  return invoke<ExecutionDetail>("approve_execution", {
    executionId,
    confirmationPhrase,
  });
}

export function startExecution(executionId: string): Promise<ExecutionDetail> {
  return invoke<ExecutionDetail>("start_execution", { executionId });
}

export function pauseExecution(executionId: string): Promise<boolean> {
  return invoke<boolean>("pause_execution", { executionId });
}

export function cancelExecution(executionId: string): Promise<boolean> {
  return invoke<boolean>("cancel_execution", { executionId });
}

export function getExecutionStatus(executionId: string): Promise<ExecutionDetail> {
  return invoke<ExecutionDetail>("get_execution_status", { executionId });
}

export function listExecutionHistory(
  workspaceId: string,
  limit = 20,
): Promise<ExecutionSession[]> {
  return invoke<ExecutionSession[]>("list_execution_history", { workspaceId, limit });
}

export function rollbackExecution(executionId: string): Promise<ExecutionDetail> {
  return invoke<ExecutionDetail>("rollback_execution", { executionId });
}

export function recoverExecution(executionId: string): Promise<RecoveryAssessment> {
  return invoke<RecoveryAssessment>("recover_execution", { executionId });
}

export function subscribeExecutionProgress(
  handler: (progress: ExecutionProgress) => void,
): Promise<UnlistenFn> {
  return listen<ExecutionProgress>("organization-execution-progress", (event) =>
    handler(event.payload),
  );
}

export function listContentResults(
  batchId: string,
  limit = 500,
  offset = 0,
): Promise<ContentDetail[]> {
  return invoke<ContentDetail[]>("list_content_results", {
    batchId,
    limit,
    offset,
  });
}

export function searchLocalFiles(
  workspaceId: string,
  query: LocalSearchQuery,
): Promise<LocalSearchPage> {
  return invoke<LocalSearchPage>("search_local_files", { workspaceId, query });
}

export function getEmbeddingModelStatus(): Promise<EmbeddingModelStatus> {
  return invoke<EmbeddingModelStatus>("get_embedding_model_status");
}

export function activateLocalEmbeddingModel(): Promise<EmbeddingModelStatus> {
  return invoke<EmbeddingModelStatus>("activate_local_embedding_model");
}

export function cancelLocalEmbeddingModelInstall(): Promise<EmbeddingModelStatus> {
  return invoke<EmbeddingModelStatus>("cancel_local_embedding_model_install");
}

export function retryLocalEmbeddingModel(): Promise<EmbeddingModelStatus> {
  return invoke<EmbeddingModelStatus>("retry_local_embedding_model");
}

export function removeLocalEmbeddingModel(): Promise<EmbeddingModelStatus> {
  return invoke<EmbeddingModelStatus>("remove_local_embedding_model");
}

export function rebuildSemanticAnnIndex(workspaceId: string): Promise<string> {
  return invoke<string>("rebuild_semantic_ann_index", { workspaceId });
}

export function listReviewItems(
  workspaceId: string,
  status: ReviewStatusFilter = "needs_review",
  reason: ReviewReasonFilter = "all",
  limit = 50,
  offset = 0,
): Promise<FileReviewPage> {
  return invoke<FileReviewPage>("list_review_items", {
    workspaceId,
    status,
    reason,
    limit,
    offset,
  });
}

export function updateReviewItem(
  reviewId: string,
  action: "resolve" | "ignore",
): Promise<FileReviewItem> {
  return invoke<FileReviewItem>("update_review_item", { reviewId, action });
}

export function getFileDetail(fileId: string): Promise<LocalFileDetail> {
  return invoke<LocalFileDetail>("get_file_detail", { fileId });
}

export function storeSemanticCorrection(
  fileId: string,
  fieldKey: string,
  action: "confirm" | "correct",
  value?: string,
): Promise<SemanticCorrection> {
  return invoke<SemanticCorrection>("store_semantic_correction", {
    fileId,
    fieldKey,
    action,
    value,
  });
}

export function retryExtraction(reviewId: string): Promise<ExtractionRetry> {
  return invoke<ExtractionRetry>("retry_extraction", { reviewId });
}

export function cancelExtractionRetry(reviewId: string): Promise<boolean> {
  return invoke<boolean>("cancel_extraction_retry", { reviewId });
}

export function getMonitoringDashboard(
  workspaceId: string,
): Promise<MonitoringDashboard> {
  return invoke<MonitoringDashboard>("get_monitoring_dashboard", { workspaceId });
}

export function restoreWorkspaceSession(): Promise<RestoredWorkspaceSession | null> {
  return invoke<RestoredWorkspaceSession | null>("restore_workspace_session");
}

export function pauseMonitoring(workspaceId: string): Promise<void> {
  return invoke<void>("pause_monitoring", { workspaceId });
}

export function resumeMonitoring(workspaceId: string): Promise<void> {
  return invoke<void>("resume_monitoring", { workspaceId });
}

export function setMonitoredFolderEnabled(
  rootId: string,
  enabled: boolean,
): Promise<void> {
  return invoke<void>("set_monitored_folder_enabled", { rootId, enabled });
}

export function addMonitoringExclusion(
  workspaceId: string,
  rootId: string | null | undefined,
  kind: MonitoringExclusion["kind"],
  value: string,
): Promise<void> {
  return invoke<void>("add_monitoring_exclusion", {
    workspaceId,
    rootId,
    kind,
    value,
  });
}

export function removeMonitoringExclusion(exclusionId: string): Promise<void> {
  return invoke<void>("remove_monitoring_exclusion", { exclusionId });
}

export function runMonitoringCycle(
  workspaceId: string,
): Promise<MonitoringDashboard> {
  return invoke<MonitoringDashboard>("run_monitoring_cycle", { workspaceId });
}

export function cancelMonitoring(workspaceId: string): Promise<void> {
  return invoke<void>("cancel_monitoring", { workspaceId });
}
