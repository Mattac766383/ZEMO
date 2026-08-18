mod executor_client;
mod folder_access;

#[cfg(all(test, target_os = "macos"))]
mod packaged_macos_apply;

#[cfg(windows)]
const _: () = assert!(
    !platform_windows::MUTATION_CAPABILITY_COMPILED,
    "the live desktop dependency graph must not compile Windows mutation capability"
);
#[cfg(target_os = "macos")]
const _: () = assert!(
    !platform_macos::MUTATION_CAPABILITY_COMPILED,
    "the live desktop dependency graph must not compile macOS mutation capability"
);

use application::{
    ApplicationError, ApprovedExecutorClient, ContentAnalysisPhase, ContentAnalysisProgress,
    ExecutionApplicationService, ExecutionConsentAuthorityKey, ExtractionRetryOutcome,
    ExtractionRetryStatus, IdentityResolutionPhase, IdentityResolutionProgress,
    MonitoringDashboard, NativeExecutionConfirmation, ProposalBuildPhase, ProposalBuildProgress,
    ScannerApplicationService, SemanticAnalysisPhase, SemanticAnalysisProgress,
    SemanticCorrectionAction, UnavailableApprovedExecutorClient,
};
use catalog::{ScanPhase, ScanProgress};
use domain::{
    ExecutionDetail, ExecutionId, ExecutionProgress, ExecutionSession, FileId,
    OrganizationProposal, OrganizationProposalOperation, OrganizationProposalStatus, ProposalId,
    ProposalOverrideAction, RecoveryAssessment, RootId, RuleId, RuleSuggestionId, ScanId,
    VirtualProposalNode, WorkspaceId,
};
use extraction::LocalExtractionEngine;
use ipc_contracts::executor_v2::{ROOT_AUTHORITY_SECRET_NAME, ROOT_AUTHORITY_SECRET_SERVICE};
use ipc_contracts::{
    ContentAnalysisDto, ContentAnalysisProgressDto, ContentDetailDto, DuplicateFileDto,
    DuplicateGroupDto, EmbeddingModelStatusDto, EmbeddingSearchStatusDto, ExecutionDetailDto,
    ExecutionOperationDto, ExecutionProgressDto, ExecutionSessionDto, ExecutionSummaryDto,
    ExecutorRequestFactDto, ExecutorSessionFactDto, ExtractionRetryDto, FileReviewItemDto,
    FileReviewPageDto, IdentityAuditEventDto, IdentityCandidateDto, IdentityDetailDto,
    IdentityIdentifierDto, IdentityMatchEvidenceDto, IdentityMutationDto, IdentityOccurrenceDto,
    IdentityRelationshipDto, IdentityResolutionDto, IdentityResolutionProgressDto,
    IdentityReviewGroupDto, IdentityReviewPageDto, IdentitySummaryDto, JournalDiagnosticDto,
    JournalDiagnosticStateDto, LocalFileDetailDto, LocalRuleDto, LocalRuleInputDto,
    LocalSearchPageDto, LocalSearchQueryDto, LocalSearchResultDto, MonitoredFolderDto,
    MonitoringActivityDto, MonitoringCountsDto, MonitoringDashboardDto, MonitoringExclusionDto,
    OrganizationOperationDto, OrganizationPreferencesDto, OrganizationProposalChangeDto,
    OrganizationProposalDto, OrganizationProposalProgressDto, OrganizationProposalSummaryDto,
    OrganizationReasonDto, QueryChipDto, RecoveryAssessmentDto, RecoveryItemDto,
    FolderAccessProbeDto, RegisterUserContentRootResultDto, RegisteredRootDto,
    RestoredWorkspaceSessionDto,
    RuleSuggestionDto, RulesPreferencesStateDto, ScanFileDto, ScanIssueDto, ScanProgressDto,
    ScanResultDto, SearchTimingsDto, SemanticAnalysisDetailDto, SemanticAnalysisDto,
    SemanticAnalysisProgressDto, SemanticCandidateValueDto, SemanticCorrectionDto,
    SemanticEntityDto, SemanticEvidenceDto, SemanticFieldDto, SystemStatusDto,
    UserContentLocationDto, VirtualProposalNodeDto, WorkspaceDto,
};
use knowledge::DeterministicSemanticProvider;
use operations::{ApplyGate, ExecutionSafetyPolicy, FileJournal, JournalKey};
use persistence::{
    Database, DatabaseKey, ExtractionBatchRecord, ExtractionDetailRecord, FileDetailRecord,
    IdentityAuditEventRecord, IdentityCandidateAction, IdentityCandidateRecord,
    IdentityDetailRecord, IdentityIdentifierRecord, IdentityMatchEvidenceRecord,
    IdentityMutationRecord, IdentityOccurrenceRecord, IdentityRelationshipRecord,
    IdentityResolverRunRecord, IdentityReviewGroupRecord, IdentityReviewPageRecord,
    IdentitySummaryRecord, InventorySort, MonitoringExclusionKind, ReviewAction, ReviewItemRecord,
    ReviewReasonFilter, ReviewStatusFilter, ScanRecord, SemanticAnalysisBatchRecord,
    SemanticAnalysisDetailRecord, SemanticCandidateValueRecord, SemanticCorrectionRecord,
    SemanticEntityRecord, SemanticEvidenceRecord, SemanticFieldRecord,
};
use platform::{PlatformError, ReadOnlyPlatform, SecretStore};
use privacy::OsSecretStore;
use search::{
    ContextFilter, DocumentTypeFilter, EmbeddingAvailability, ExtractionFilter, FileTypeFilter,
    LocalEmbeddingProvider, MatchSource, ModifiedFilter, OcrFilter, OnnxLocalEmbeddingProvider,
    SearchFilters, SearchQuery, SearchSort, SemanticStatusFilter,
};
use std::{
    collections::HashMap,
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug)]
struct ManagedScanner {
    service: Arc<ScannerApplicationService>,
    execution_service: Arc<ExecutionApplicationService>,
    embedding_provider: Arc<OnnxLocalEmbeddingProvider>,
    model_install_cancel: Arc<AtomicBool>,
    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    content_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    semantic_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    identity_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    proposal_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    retry_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    monitoring_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_system_status(state: State<'_, ManagedScanner>) -> SystemStatusDto {
    let status = state.service.system_status();
    let execution = state.execution_service.system_status();
    let (apply_enabled, apply_gate_reason, recovery_required, journal_locked, journal_diagnostics) =
        match execution {
            Ok(execution) => (
                execution.apply_gate.enabled,
                Some(execution.apply_gate.reason),
                execution.recovery_required,
                execution.journal_locked,
                execution
                    .journal_diagnostics
                    .into_iter()
                    .map(journal_diagnostic_dto)
                    .collect(),
            ),
            Err(_) => (
                false,
                Some("L’exécution locale est indisponible sur cette plateforme.".to_owned()),
                true,
                true,
                vec![JournalDiagnosticDto {
                    scope: "database".to_owned(),
                    execution_id: None,
                    code: "execution_status_unavailable".to_owned(),
                    message: "Execution diagnostics are unavailable; mutations remain locked."
                        .to_owned(),
                    detected_at_unix_ms: 0,
                    recovery_available: false,
                    rollback_available: false,
                }],
            ),
        };
    SystemStatusDto {
        local_first: status.local_first,
        read_only_scan: status.read_only_scan,
        network_disabled: status.network_disabled,
        apply_enabled,
        apply_gate_reason,
        display_label: Some(
            "Inventaire local chiffré · mutations limitées aux plans approuvés".into(),
        ),
        version: Some(status.version),
        recovery_required,
        journal_locked,
        journal_diagnostics,
    }
}

#[tauri::command]
async fn restore_workspace_session(
    state: State<'_, ManagedScanner>,
) -> Result<Option<RestoredWorkspaceSessionDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .restore_workspace_session()
            .map(|session| session.map(restored_workspace_session_dto))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_monitoring_dashboard(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<MonitoringDashboardDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .monitoring_dashboard(parse_workspace_id(&workspace_id)?)
            .map(monitoring_dashboard_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn pause_monitoring(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<(), String> {
    let workspace_id = parse_workspace_id(&workspace_id).map_err(command_error)?;
    if let Some(cancellation) = state
        .monitoring_cancellations
        .lock()
        .map_err(|_| "monitoring cancellation registry is unavailable".to_owned())?
        .get(&workspace_id.to_string())
    {
        cancellation.store(true, Ordering::Relaxed);
    }
    let service = state.service.clone();
    run_blocking(move || {
        service.pause_monitoring(workspace_id)?;
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn resume_monitoring(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<(), String> {
    let service = state.service.clone();
    run_blocking(move || {
        service.resume_monitoring(parse_workspace_id(&workspace_id)?)?;
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn set_monitored_folder_enabled(
    state: State<'_, ManagedScanner>,
    root_id: String,
    enabled: bool,
) -> Result<(), String> {
    let service = state.service.clone();
    run_blocking(move || {
        service.set_monitored_root_enabled(parse_root_id(&root_id)?, enabled)?;
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn add_monitoring_exclusion(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    root_id: Option<String>,
    kind: String,
    value: String,
) -> Result<(), String> {
    let service = state.service.clone();
    run_blocking(move || {
        let kind = match kind.as_str() {
            "path_prefix" => MonitoringExclusionKind::PathPrefix,
            "extension" => MonitoringExclusionKind::Extension,
            _ => return Err(ApplicationError::InvalidMonitoringRequest),
        };
        service.add_monitoring_exclusion(
            parse_workspace_id(&workspace_id)?,
            root_id.as_deref().map(parse_root_id).transpose()?,
            kind,
            &value,
        )?;
        Ok(())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn remove_monitoring_exclusion(
    state: State<'_, ManagedScanner>,
    exclusion_id: String,
) -> Result<(), String> {
    let service = state.service.clone();
    run_blocking(move || service.remove_monitoring_exclusion(&exclusion_id)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn run_monitoring_cycle(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<MonitoringDashboardDto, String> {
    let workspace_id = parse_workspace_id(&workspace_id).map_err(command_error)?;
    let registry_key = workspace_id.to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut registry = state
            .monitoring_cancellations
            .lock()
            .map_err(|_| "monitoring cancellation registry is unavailable".to_owned())?;
        if registry.contains_key(&registry_key) {
            return Err("a monitoring check is already active for this workspace".to_owned());
        }
        registry.insert(registry_key.clone(), cancellation.clone());
    }
    let service = state.service.clone();
    let result = run_blocking(move || {
        service
            .run_monitoring_cycle(workspace_id, &|| cancellation.load(Ordering::Relaxed))
            .map(monitoring_dashboard_dto)
    })
    .await;
    state
        .monitoring_cancellations
        .lock()
        .map_err(|_| "monitoring cancellation registry is unavailable".to_owned())?
        .remove(&registry_key);
    result
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_monitoring(state: State<'_, ManagedScanner>, workspace_id: String) -> Result<(), String> {
    let registry_key = parse_workspace_id(&workspace_id)
        .map_err(command_error)?
        .to_string();
    if let Some(cancellation) = state
        .monitoring_cancellations
        .lock()
        .map_err(|_| "monitoring cancellation registry is unavailable".to_owned())?
        .get(&registry_key)
    {
        cancellation.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
async fn create_workspace(
    state: State<'_, ManagedScanner>,
    name: String,
) -> Result<WorkspaceDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let workspace = service.create_workspace(&name)?;
        Ok(WorkspaceDto {
            id: workspace.id.to_string(),
            name: workspace.name,
            root: None,
            created_at: Some(workspace.created_at),
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn select_and_register_root(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<RegisteredRootDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let selected = rfd::FileDialog::new()
            .set_title("Sélectionner le dossier exact à analyser en lecture seule")
            .pick_folder()
            .ok_or(ApplicationError::NotFound)?;
        let root = service.register_root(workspace_id, &selected)?;
        Ok(RegisteredRootDto {
            id: root.id.to_string(),
            display_label: root.display_label,
            selected_path: root.absolute_path,
        })
    })
    .await
}

fn folder_access_store_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_local_data_dir().ok()
}

fn probe_to_dto(probe: folder_access::FolderAccessProbe) -> FolderAccessProbeDto {
    FolderAccessProbeDto {
        logical_name: probe.logical_name,
        kind: probe.kind,
        display_label: probe.display_label,
        resolved_path: probe.resolved_path,
        exists: probe.exists,
        is_dir: probe.is_dir,
        readable: probe.readable,
        writable: probe.writable,
        recommended: probe.recommended,
        raw_os_error: probe.raw_os_error,
        platform_error: probe.platform_error,
        access_state: probe.access_state,
        human_status: probe.human_status,
        canonical_path: probe.canonical_path,
        failed_stage: probe.failed_stage,
        error_kind: probe.error_kind,
        inspect_result: probe.inspect_result,
        technical_details: probe.technical_details,
    }
}

fn enrich_probe(probe: folder_access::FolderAccessProbe) -> folder_access::FolderAccessProbe {
    let Some(path) = probe.resolved_path_buf() else {
        return probe;
    };
    if !probe.exists {
        return probe;
    }
    folder_access::with_inspect_outcome(probe, inspect_user_content_path(&path))
}

fn inspect_user_content_path(
    path: &std::path::Path,
) -> Result<String, (Option<i32>, String, String, &'static str)> {
    #[cfg(target_os = "macos")]
    {
        match platform_macos::MacOsPlatform.inspect_volume(path) {
            Ok(volume) => Ok(format!(
                "ok local={} fs={:?}",
                volume.local, volume.filesystem_type
            )),
            Err(error) => Err(describe_platform_inspect_error(&error)),
        }
    }
    #[cfg(windows)]
    {
        match platform_windows::WindowsPlatform.inspect_volume(path) {
            Ok(volume) => Ok(format!(
                "ok local={} fs={:?}",
                volume.local, volume.filesystem_type
            )),
            Err(error) => Err(describe_platform_inspect_error(&error)),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = path;
        Ok("inspect_skipped".to_owned())
    }
}

fn describe_platform_inspect_error(
    error: &PlatformError,
) -> (Option<i32>, String, String, &'static str) {
    let state = access_state_from_platform(error);
    match error {
        PlatformError::Io(inner) => (
            inner.raw_os_error(),
            format!("{:?}", inner.kind()),
            error.to_string(),
            state,
        ),
        PlatformError::PermissionDenied => {
            (Some(13), "PermissionDenied".to_owned(), error.to_string(), state)
        }
        PlatformError::ReparsePoint => {
            (None, "ReparsePoint".to_owned(), error.to_string(), state)
        }
        PlatformError::SourceMissing => {
            (Some(2), "NotFound".to_owned(), error.to_string(), state)
        }
        other => (None, format!("{other:?}"), other.to_string(), state),
    }
}

fn access_state_from_platform(error: &PlatformError) -> &'static str {
    match error {
        PlatformError::PermissionDenied => folder_access::ACCESS_AUTHORIZATION_REQUIRED,
        PlatformError::ReparsePoint => folder_access::ACCESS_AUTHORIZATION_REQUIRED,
        PlatformError::CloudPlaceholder => folder_access::ACCESS_TEMPORARILY_UNAVAILABLE,
        PlatformError::SourceMissing => folder_access::ACCESS_MISSING,
        PlatformError::SharingViolation | PlatformError::LockViolation => {
            folder_access::ACCESS_LOCKED
        }
        PlatformError::Unsupported(_) => folder_access::ACCESS_UNSUPPORTED,
        PlatformError::Io(inner) => folder_access::classify_io_error(inner),
        _ => folder_access::ACCESS_AUTHORIZATION_REQUIRED,
    }
}

fn access_state_from_register_error(error: &ApplicationError) -> &'static str {
    match error {
        ApplicationError::Platform(inner) => access_state_from_platform(inner),
        ApplicationError::Io(inner) => folder_access::classify_io_error(inner),
        ApplicationError::InvalidMonitoringRequest => folder_access::ACCESS_AUTHORIZATION_REQUIRED,
        _ => folder_access::ACCESS_AUTHORIZATION_REQUIRED,
    }
}

fn pick_folder_on_main_thread(
    app: &AppHandle,
    title: &str,
    directory: Option<PathBuf>,
) -> Result<Option<PathBuf>, String> {
    let title = title.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(directory) = directory.filter(|path| path.exists()) {
            dialog = dialog.set_directory(directory);
        }
        let _ = tx.send(dialog.pick_folder());
    })
    .map_err(|error| error.to_string())?;
    rx.recv().map_err(|error| error.to_string())
}

fn register_result_from_probe(
    probe: &folder_access::FolderAccessProbe,
    root: Option<RegisteredRootDto>,
    status: &str,
) -> RegisterUserContentRootResultDto {
    RegisterUserContentRootResultDto {
        root,
        kind: probe.kind.clone(),
        display_label: probe.display_label.clone(),
        absolute_path: probe.resolved_path.clone(),
        status: status.to_owned(),
        access_state: probe.access_state.clone(),
        human_status: probe.human_status.clone(),
        message: Some(folder_access::human_message_for(&probe.access_state).to_owned()),
    }
}

#[tauri::command(rename_all = "camelCase")]
async fn list_user_content_locations(
    app: AppHandle,
) -> Result<Vec<UserContentLocationDto>, String> {
    let store = folder_access_store_dir(&app);
    run_blocking_string(move || {
        let probes = folder_access::UserContentKind::all()
            .into_iter()
            .map(|kind| enrich_probe(folder_access::probe_kind(kind, store.as_deref())))
            .map(|probe| UserContentLocationDto {
                kind: probe.kind,
                display_label: probe.display_label,
                absolute_path: probe.resolved_path,
                exists: probe.exists,
                readable: probe.readable,
                recommended: probe.recommended,
                access_state: probe.access_state,
                human_status: probe.human_status,
                writable: probe.writable,
                raw_os_error: probe.raw_os_error,
                platform_error: probe.platform_error,
            })
            .collect();
        Ok(probes)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn probe_user_content_access(app: AppHandle) -> Result<Vec<FolderAccessProbeDto>, String> {
    let store = folder_access_store_dir(&app);
    run_blocking_string(move || {
        Ok(folder_access::probe_recommended(store.as_deref())
            .into_iter()
            .map(enrich_probe)
            .map(probe_to_dto)
            .collect())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn authorize_user_content_folder(
    app: AppHandle,
    kind: String,
) -> Result<FolderAccessProbeDto, String> {
    let parsed = folder_access::UserContentKind::parse(&kind)
        .ok_or_else(|| "Emplacement utilisateur inconnu.".to_owned())?;
    let hint = parsed.resolve_native_path().or_else(dirs::home_dir);
    let selected = pick_folder_on_main_thread(
        &app,
        "ZEMO a besoin de votre autorisation pour accéder à ce dossier.",
        hint,
    )?;
    let store = folder_access_store_dir(&app);
    run_blocking_string(move || {
        let Some(selected) = selected else {
            return Ok(probe_to_dto(enrich_probe(folder_access::probe_kind(
                parsed,
                store.as_deref(),
            ))));
        };
        let Some(accepted) = folder_access::accept_authorized_selection(parsed, &selected) else {
            return Ok(probe_to_dto(enrich_probe(folder_access::probe_kind(
                parsed,
                store.as_deref(),
            ))));
        };
        if let Some(store) = store.as_ref() {
            let _ = folder_access::persist_authorized_path(store, parsed.as_str(), &accepted);
        }
        Ok(probe_to_dto(enrich_probe(folder_access::probe_kind(
            parsed,
            store.as_deref(),
        ))))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn register_user_content_root(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    kind: String,
) -> Result<RegisterUserContentRootResultDto, String> {
    let service = state.service.clone();
    let store = folder_access_store_dir(&app);
    run_blocking_string(move || {
        let workspace_id = parse_workspace_id(&workspace_id).map_err(command_error)?;
        let kind = folder_access::UserContentKind::parse(&kind)
            .ok_or_else(|| "Emplacement utilisateur inconnu.".to_owned())?;
        let probe = enrich_probe(folder_access::probe_kind(kind, store.as_deref()));
        if !probe.can_scan() {
            return Ok(register_result_from_probe(&probe, None, &probe.access_state));
        }
        let path = probe
            .resolved_path_buf()
            .ok_or_else(|| "Ce dossier est introuvable.".to_owned())?;
        match service.register_root(workspace_id, &path) {
            Ok(root) => Ok(register_result_from_probe(
                &probe,
                Some(RegisteredRootDto {
                    id: root.id.to_string(),
                    display_label: root.display_label,
                    selected_path: root.absolute_path,
                }),
                "registered",
            )),
            Err(error) => {
                let state = access_state_from_register_error(&error);
                Ok(RegisterUserContentRootResultDto {
                    root: None,
                    kind: kind.as_str().to_owned(),
                    display_label: kind.display_label_fr().to_owned(),
                    absolute_path: path.to_string_lossy().into_owned(),
                    status: state.to_owned(),
                    access_state: state.to_owned(),
                    human_status: folder_access::human_status_for(state, kind.display_label_fr()),
                    message: Some(folder_access::human_message_for(state).to_owned()),
                })
            }
        }
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn scan_workspace(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<ScanResultDto, String> {
    let parsed_workspace = parse_workspace_id(&workspace_id).map_err(command_error)?;
    let registry_key = parsed_workspace.to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .cancellations
            .lock()
            .map_err(|_| "Le registre local des scans est indisponible.".to_owned())?;
        if active.contains_key(&registry_key) {
            return Err("Un scan est déjà en cours pour ce dossier.".to_owned());
        }
        active.insert(registry_key.clone(), cancellation.clone());
    }

    let service = state.service.clone();
    let registry = state.cancellations.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut last_phase = None;
        let mut last_discovered = 0_u64;
        let mut last_indexed = 0_u64;
        let mut last_hashed = 0_u64;
        let mut emit_progress = |progress: ScanProgress| {
            let now = Instant::now();
            let phase_changed = last_phase != Some(progress.phase);
            let batch_ready = progress.files_discovered.saturating_sub(last_discovered) >= 128
                || progress.files_indexed.saturating_sub(last_indexed) >= 128
                || progress.files_hashed.saturating_sub(last_hashed) >= 16;
            if phase_changed
                || batch_ready
                || now.duration_since(last_emit) >= Duration::from_millis(150)
            {
                let _ = app.emit("safe-scan-progress", progress_dto(progress));
                last_emit = now;
                last_phase = Some(progress.phase);
                last_discovered = progress.files_discovered;
                last_indexed = progress.files_indexed;
                last_hashed = progress.files_hashed;
            }
        };
        service.scan_workspace(
            parsed_workspace,
            &|| cancellation.load(Ordering::Relaxed),
            &mut emit_progress,
        )
    })
    .await
    .map_err(|_| "Le scanner local s’est interrompu de façon inattendue.".to_owned());

    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?.map(scan_result_dto).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_scan(state: State<'_, ManagedScanner>, workspace_id: String) -> Result<bool, String> {
    let registry_key = parse_workspace_id(&workspace_id)
        .map_err(command_error)?
        .to_string();
    let active = state
        .cancellations
        .lock()
        .map_err(|_| "Le registre local des scans est indisponible.".to_owned())?;
    let Some(cancellation) = active.get(&registry_key) else {
        return Ok(false);
    };
    cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command(rename_all = "camelCase")]
async fn list_scan_files(
    state: State<'_, ManagedScanner>,
    scan_id: String,
    sort_by: String,
    descending: bool,
    limit: usize,
    offset: usize,
) -> Result<Vec<ScanFileDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let scan_id = parse_scan_id(&scan_id)?;
        let sort = match sort_by.as_str() {
            "type" => InventorySort::FileType,
            "size" => InventorySort::Size,
            "modified" => InventorySort::Modified,
            "location" => InventorySort::RelativePath,
            "status" => InventorySort::Status,
            _ => InventorySort::Filename,
        };
        service
            .scan_files(scan_id, sort, descending, limit, offset)
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| ScanFileDto {
                        id: file.id,
                        filename: file.filename,
                        file_type: file.file_type,
                        extension: file.extension,
                        byte_size: file.byte_size,
                        modified_at: file.modified_at,
                        relative_path: file.relative_path,
                        status: file.status,
                        hashing_status: file.hashing_status,
                        readable: file.readable,
                    })
                    .collect()
            })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn list_scan_duplicates(
    state: State<'_, ManagedScanner>,
    scan_id: String,
) -> Result<Vec<DuplicateGroupDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .scan_duplicate_groups(parse_scan_id(&scan_id)?)
            .map(|groups| {
                groups
                    .into_iter()
                    .map(|group| DuplicateGroupDto {
                        digest: group.digest_hex,
                        byte_size: group.byte_size,
                        files: group
                            .files
                            .into_iter()
                            .map(|file| DuplicateFileDto {
                                id: file.id,
                                filename: file.filename,
                                relative_path: file.relative_path,
                            })
                            .collect(),
                    })
                    .collect()
            })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn list_scan_errors(
    state: State<'_, ManagedScanner>,
    scan_id: String,
) -> Result<Vec<ScanIssueDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service.scan_issues(parse_scan_id(&scan_id)?).map(|issues| {
            issues
                .into_iter()
                .map(|issue| ScanIssueDto {
                    relative_path: issue.relative_path,
                    category: issue.category,
                    message: issue.message,
                    is_directory: issue.is_directory,
                })
                .collect()
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn analyze_content(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    scan_id: String,
) -> Result<ContentAnalysisDto, String> {
    let parsed_scan = parse_scan_id(&scan_id).map_err(command_error)?;
    let registry_key = parsed_scan.to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .content_cancellations
            .lock()
            .map_err(|_| "Le registre local des analyses est indisponible.".to_owned())?;
        if active.contains_key(&registry_key) {
            return Err("Une analyse de contenu est déjà en cours pour ce scan.".to_owned());
        }
        active.insert(registry_key.clone(), cancellation.clone());
    }

    let service = state.service.clone();
    let registry = state.content_cancellations.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut last_completed = 0_u64;
        let mut last_phase = None;
        let mut emit_progress = |progress: ContentAnalysisProgress| {
            let now = Instant::now();
            let phase_changed = last_phase != Some(progress.phase);
            let batch_ready = progress.files_completed.saturating_sub(last_completed) >= 8;
            if phase_changed
                || batch_ready
                || now.duration_since(last_emit) >= Duration::from_millis(150)
            {
                let _ = app.emit("content-analysis-progress", content_progress_dto(&progress));
                last_emit = now;
                last_completed = progress.files_completed;
                last_phase = Some(progress.phase);
            }
        };
        service.analyze_scan_content(
            parsed_scan,
            &|| cancellation.load(Ordering::Relaxed),
            &mut emit_progress,
        )
    })
    .await
    .map_err(|_| "La lecture des documents s’est interrompue. Réessayez.".to_owned());

    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?.map(content_analysis_dto).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_content_analysis(
    state: State<'_, ManagedScanner>,
    scan_id: String,
) -> Result<bool, String> {
    let registry_key = parse_scan_id(&scan_id).map_err(command_error)?.to_string();
    let active = state
        .content_cancellations
        .lock()
        .map_err(|_| "Le registre local des analyses est indisponible.".to_owned())?;
    let Some(cancellation) = active.get(&registry_key) else {
        return Ok(false);
    };
    cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command(rename_all = "camelCase")]
async fn analyze_semantics(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    scan_id: String,
) -> Result<SemanticAnalysisDto, String> {
    let parsed_scan = parse_scan_id(&scan_id).map_err(command_error)?;
    let registry_key = parsed_scan.to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .semantic_cancellations
            .lock()
            .map_err(|_| "Le registre local de compréhension est indisponible.".to_owned())?;
        if active.contains_key(&registry_key) {
            return Err("Une compréhension sémantique est déjà en cours pour ce scan.".to_owned());
        }
        active.insert(registry_key.clone(), cancellation.clone());
    }

    let service = state.service.clone();
    let registry = state.semantic_cancellations.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut last_completed = 0_u64;
        let mut last_phase = None;
        let mut emit_progress = |progress: SemanticAnalysisProgress| {
            let now = Instant::now();
            let phase_changed = last_phase != Some(progress.phase);
            let batch_ready = progress.files_completed.saturating_sub(last_completed) >= 8;
            if phase_changed
                || batch_ready
                || now.duration_since(last_emit) >= Duration::from_millis(150)
            {
                let _ = app.emit(
                    "semantic-analysis-progress",
                    semantic_progress_dto(&progress),
                );
                last_emit = now;
                last_completed = progress.files_completed;
                last_phase = Some(progress.phase);
            }
        };
        service.analyze_scan_semantics(
            parsed_scan,
            &|| cancellation.load(Ordering::Relaxed),
            &mut emit_progress,
        )
    })
    .await
    .map_err(|_| "La compréhension des documents s’est interrompue. Réessayez.".to_owned());

    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?.map(semantic_analysis_dto).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_semantic_analysis(
    state: State<'_, ManagedScanner>,
    scan_id: String,
) -> Result<bool, String> {
    let registry_key = parse_scan_id(&scan_id).map_err(command_error)?.to_string();
    let active = state
        .semantic_cancellations
        .lock()
        .map_err(|_| "Le registre local de compréhension est indisponible.".to_owned())?;
    let Some(cancellation) = active.get(&registry_key) else {
        return Ok(false);
    };
    cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command(rename_all = "camelCase")]
async fn resolve_identities(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    force: bool,
) -> Result<IdentityResolutionDto, String> {
    let parsed_workspace = parse_workspace_id(&workspace_id).map_err(command_error)?;
    let registry_key = parsed_workspace.to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .identity_cancellations
            .lock()
            .map_err(|_| "Le registre local des relations est indisponible.".to_owned())?;
        if active.contains_key(&registry_key) {
            return Err("Une résolution d’identités est déjà en cours pour cet espace.".to_owned());
        }
        active.insert(registry_key.clone(), cancellation.clone());
    }
    let service = state.service.clone();
    let registry = state.identity_cancellations.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut last_files = 0_u64;
        let mut last_phase = None;
        let mut emit_progress = |progress: IdentityResolutionProgress| {
            let now = Instant::now();
            let phase_changed = last_phase != Some(progress.phase);
            let batch_ready = progress.files_considered.saturating_sub(last_files) >= 4;
            if phase_changed
                || batch_ready
                || now.duration_since(last_emit) >= Duration::from_millis(150)
            {
                let _ = app.emit(
                    "identity-resolution-progress",
                    identity_progress_dto(&progress),
                );
                last_emit = now;
                last_files = progress.files_considered;
                last_phase = Some(progress.phase);
            }
        };
        service.resolve_workspace_identities(
            parsed_workspace,
            "manual",
            force,
            &|| cancellation.load(Ordering::Relaxed),
            &mut emit_progress,
        )
    })
    .await
    .map_err(|_| "Le rapprochement des fichiers s’est interrompu. Réessayez.".to_owned());
    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?.map(identity_resolution_dto).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_identity_resolution(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<bool, String> {
    let registry_key = parse_workspace_id(&workspace_id)
        .map_err(command_error)?
        .to_string();
    let active = state
        .identity_cancellations
        .lock()
        .map_err(|_| "Le registre local des relations est indisponible.".to_owned())?;
    let Some(cancellation) = active.get(&registry_key) else {
        return Ok(false);
    };
    cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

#[tauri::command(rename_all = "camelCase")]
async fn list_identity_review_groups(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    status: String,
    limit: usize,
    offset: usize,
) -> Result<IdentityReviewPageDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .identity_review_groups(parse_workspace_id(&workspace_id)?, &status, limit, offset)
            .map(identity_review_page_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_identity_detail(
    state: State<'_, ManagedScanner>,
    identity_id: String,
) -> Result<IdentityDetailDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .identity_detail(&identity_id)
            .map(identity_detail_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn decide_identity_candidate(
    state: State<'_, ManagedScanner>,
    candidate_id: String,
    action: String,
    reason: Option<String>,
) -> Result<IdentityMutationDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let action = match action.as_str() {
            "confirm" => IdentityCandidateAction::Confirm,
            "reject" => IdentityCandidateAction::Reject,
            "keep_separate" => IdentityCandidateAction::KeepSeparate,
            _ => return Err(ApplicationError::InvalidIdentityDecision),
        };
        service
            .decide_identity_candidate(&candidate_id, action, reason.as_deref())
            .map(identity_mutation_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn merge_identities(
    state: State<'_, ManagedScanner>,
    primary_identity_id: String,
    secondary_identity_id: String,
    reason: Option<String>,
) -> Result<IdentityMutationDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .merge_identity_records(
                &primary_identity_id,
                &secondary_identity_id,
                reason.as_deref(),
            )
            .map(identity_mutation_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn unlink_identity_occurrence(
    state: State<'_, ManagedScanner>,
    identity_id: String,
    occurrence_id: String,
    reason: Option<String>,
) -> Result<IdentityMutationDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .unlink_identity_occurrence(&identity_id, &occurrence_id, reason.as_deref())
            .map(identity_mutation_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn generate_organization_proposal(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    root_id: Option<String>,
    recompute_current: bool,
    consumer_mode: Option<bool>,
) -> Result<OrganizationProposalDto, String> {
    let workspace_id = parse_workspace_id(&workspace_id).map_err(command_error)?;
    let root_id = root_id
        .as_deref()
        .map(parse_root_id)
        .transpose()
        .map_err(command_error)?;
    let registry_key = workspace_id.to_string();
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .proposal_cancellations
            .lock()
            .map_err(|_| "Le registre local des propositions est indisponible.".to_owned())?;
        if active.contains_key(&registry_key) {
            return Err(
                "Une proposition d’organisation est déjà en cours pour cet espace.".to_owned(),
            );
        }
        active.insert(registry_key.clone(), cancellation.clone());
    }
    let service = state.service.clone();
    let registry = state.proposal_cancellations.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut last_files = 0_u64;
        let mut last_phase = None;
        let mut emit_progress = |progress: ProposalBuildProgress| {
            let now = Instant::now();
            let phase_changed = last_phase != Some(progress.phase);
            let batch_ready = progress.files_evaluated.saturating_sub(last_files) >= 64;
            if phase_changed
                || batch_ready
                || now.duration_since(last_emit) >= Duration::from_millis(150)
            {
                let _ = app.emit(
                    "organization-proposal-progress",
                    organization_proposal_progress_dto(&progress),
                );
                last_emit = now;
                last_files = progress.files_evaluated;
                last_phase = Some(progress.phase);
            }
        };
        let consumer = consumer_mode.unwrap_or(false);
        let generated = if let Some(root_id) = root_id {
            if consumer {
                service.generate_consumer_organization_proposal_for_root(
                    workspace_id,
                    root_id,
                    recompute_current,
                    &|| cancellation.load(Ordering::Relaxed),
                    &mut emit_progress,
                )
            } else {
                service.generate_organization_proposal_for_root(
                    workspace_id,
                    root_id,
                    recompute_current,
                    &|| cancellation.load(Ordering::Relaxed),
                    &mut emit_progress,
                )
            }
        } else {
            service.generate_organization_proposal(
                workspace_id,
                recompute_current,
                &|| cancellation.load(Ordering::Relaxed),
                &mut emit_progress,
            )
        }?;
        // Drop the full in-memory proposal before IPC; UI receives a bounded projection.
        let _ = generated;
        service
            .latest_organization_proposal_for_ui(workspace_id, root_id, 500)
            .map(organization_proposal_dto)
    })
    .await
    .map_err(|_| "La préparation de l’organisation s’est interrompue. Réessayez.".to_owned());
    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?.map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_organization_proposal(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<bool, String> {
    let registry_key = parse_workspace_id(&workspace_id)
        .map_err(command_error)?
        .to_string();
    let active = state
        .proposal_cancellations
        .lock()
        .map_err(|_| "Le registre local des propositions est indisponible.".to_owned())?;
    let Some(cancellation) = active.get(&registry_key) else {
        return Ok(false);
    };
    cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

/// When `ui_bound` is true (desktop UI default), load folder nodes + bounded operations only.
#[tauri::command(rename_all = "camelCase")]
async fn get_latest_organization_proposal(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    root_id: Option<String>,
    ui_bound: Option<bool>,
    operation_limit: Option<usize>,
) -> Result<OrganizationProposalDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let root = root_id.as_deref().map(parse_root_id).transpose()?;
        if ui_bound.unwrap_or(true) {
            return service
                .latest_organization_proposal_for_ui(
                    workspace_id,
                    root,
                    operation_limit.unwrap_or(500),
                )
                .map(organization_proposal_dto);
        }
        if let Some(root_id) = root {
            service
                .latest_organization_proposal_for_root(workspace_id, root_id)
                .map(organization_proposal_dto)
        } else {
            service
                .latest_organization_proposal(workspace_id)
                .map(organization_proposal_dto)
        }
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_organization_proposal(
    state: State<'_, ManagedScanner>,
    proposal_id: String,
) -> Result<OrganizationProposalDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .organization_proposal(parse_proposal_id(&proposal_id)?)
            .map(organization_proposal_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
async fn set_organization_proposal_override(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    proposal_id: String,
    file_id: String,
    action: String,
    destination: Option<Vec<String>>,
    proposed_name: Option<String>,
    reason: Option<String>,
) -> Result<OrganizationProposalDto, String> {
    let proposal_id = parse_proposal_id(&proposal_id).map_err(command_error)?;
    let file_id = parse_file_id(&file_id).map_err(command_error)?;
    let action = proposal_override_action(&action).map_err(command_error)?;
    let registry_key = format!("proposal:{proposal_id}");
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .proposal_cancellations
            .lock()
            .map_err(|_| "Le registre local des propositions est indisponible.".to_owned())?;
        if active.contains_key(&registry_key) {
            return Err("Cette proposition est déjà en cours de modification.".to_owned());
        }
        active.insert(registry_key.clone(), cancellation.clone());
    }
    let service = state.service.clone();
    let registry = state.proposal_cancellations.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut emit_progress = |progress: ProposalBuildProgress| {
            let _ = app.emit(
                "organization-proposal-progress",
                organization_proposal_progress_dto(&progress),
            );
        };
        service.set_organization_proposal_override(
            proposal_id,
            file_id,
            action,
            destination,
            proposed_name,
            reason,
            &|| cancellation.load(Ordering::Relaxed),
            &mut emit_progress,
        )
    })
    .await
    .map_err(|_| "La modification virtuelle s’est interrompue.".to_owned());
    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?
        .map(organization_proposal_dto)
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
async fn set_organization_proposal_status(
    state: State<'_, ManagedScanner>,
    proposal_id: String,
    status: String,
) -> Result<OrganizationProposalDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let status = match status.as_str() {
            "reviewed" => OrganizationProposalStatus::Reviewed,
            "approved_for_future_apply" => OrganizationProposalStatus::ApprovedForFutureApply,
            "cancelled" => OrganizationProposalStatus::Cancelled,
            _ => return Err(ApplicationError::InvalidOrganizationProposal),
        };
        service
            .set_organization_proposal_status(parse_proposal_id(&proposal_id)?, status)
            .map(organization_proposal_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn refresh_organization_proposal_drift(
    state: State<'_, ManagedScanner>,
    proposal_id: String,
) -> Result<OrganizationProposalDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .refresh_organization_proposal_drift(parse_proposal_id(&proposal_id)?)
            .map(|(_, proposal)| organization_proposal_dto(proposal))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_rules_preferences(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<RulesPreferencesStateDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .rules_preferences_state(parse_workspace_id(&workspace_id)?)
            .map(rules_preferences_state_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn create_local_rule(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    input: LocalRuleInputDto,
) -> Result<LocalRuleDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .create_local_rule(parse_workspace_id(&workspace_id)?, &input.into())
            .map(Into::into)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn update_local_rule(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    rule_id: String,
    input: LocalRuleInputDto,
) -> Result<LocalRuleDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .update_local_rule(
                parse_workspace_id(&workspace_id)?,
                parse_rule_id(&rule_id)?,
                &input.into(),
            )
            .map(Into::into)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn set_local_rule_enabled(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    rule_id: String,
    enabled: bool,
) -> Result<LocalRuleDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .set_local_rule_enabled(
                parse_workspace_id(&workspace_id)?,
                parse_rule_id(&rule_id)?,
                enabled,
            )
            .map(Into::into)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn delete_local_rule(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    rule_id: String,
) -> Result<bool, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service.delete_local_rule(parse_workspace_id(&workspace_id)?, parse_rule_id(&rule_id)?)?;
        Ok(true)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn reorder_local_rules(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    ordered_ids: Vec<String>,
) -> Result<Vec<LocalRuleDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let ordered_ids = ordered_ids
            .iter()
            .map(|value| parse_rule_id(value))
            .collect::<Result<Vec<_>, _>>()?;
        service
            .reorder_local_rules(parse_workspace_id(&workspace_id)?, &ordered_ids)
            .map(|rules| rules.into_iter().map(Into::into).collect())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn store_local_organization_preferences(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    preferences: OrganizationPreferencesDto,
) -> Result<OrganizationPreferencesDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .store_local_organization_preferences(
                parse_workspace_id(&workspace_id)?,
                &preferences.into(),
            )
            .map(Into::into)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn accept_local_rule_suggestion(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    suggestion_id: String,
) -> Result<LocalRuleDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .accept_local_rule_suggestion(
                parse_workspace_id(&workspace_id)?,
                parse_rule_suggestion_id(&suggestion_id)?,
            )
            .map(Into::into)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn dismiss_local_rule_suggestion(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    suggestion_id: String,
) -> Result<RuleSuggestionDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .dismiss_local_rule_suggestion(
                parse_workspace_id(&workspace_id)?,
                parse_rule_suggestion_id(&suggestion_id)?,
            )
            .map(Into::into)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn recompute_rules_proposal(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<Option<OrganizationProposalDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .recompute_after_rule_change(
                parse_workspace_id(&workspace_id)?,
                &|| false,
                &mut |progress| {
                    let _ = app.emit(
                        "organization-proposal-progress",
                        organization_proposal_progress_dto(&progress),
                    );
                },
            )
            .map(|proposal| proposal.map(organization_proposal_dto))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn prepare_execution(
    state: State<'_, ManagedScanner>,
    proposal_id: String,
    revision: u32,
) -> Result<ExecutionDetailDto, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        service
            .prepare_execution(parse_proposal_id(&proposal_id)?, revision)
            .map(execution_detail_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn approve_execution(
    state: State<'_, ManagedScanner>,
    execution_id: String,
    confirmation_phrase: Option<String>,
) -> Result<ExecutionDetailDto, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        let challenge = service.create_execution_consent_challenge(
            parse_execution_id(&execution_id)?,
            confirmation_phrase.as_deref(),
        )?;
        if !show_native_execution_confirmation(challenge.summary()) {
            let _ = service.discard_execution_consent_challenge(challenge)?;
            return Err(ApplicationError::ExecutionConfirmationDeclined);
        }
        service
            .finalize_execution_consent(challenge)
            .map(execution_detail_dto)
    })
    .await
}

fn show_native_execution_confirmation(summary: &NativeExecutionConfirmation) -> bool {
    let description = format!(
        "Ce plan approuvé va maintenant modifier des fichiers.\n\n\
         Fichiers : {}\nDossiers à créer : {}\nDestination : {}\nCode du plan : {}\n\n\
         Aucun fichier existant ne sera remplacé. Continuer ?",
        summary.file_count,
        summary.folder_count,
        summary.destination_root_display,
        summary.plan_verification_code,
    );
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Appliquer cette organisation ?")
        .set_description(description)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

#[tauri::command(rename_all = "camelCase")]
async fn start_execution(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    execution_id: String,
) -> Result<ExecutionDetailDto, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        service
            .start_execution(parse_execution_id(&execution_id)?, &mut |progress| {
                let _ = app.emit(
                    "organization-execution-progress",
                    execution_progress_dto(progress),
                );
            })
            .map(execution_detail_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn pause_execution(
    state: State<'_, ManagedScanner>,
    execution_id: String,
) -> Result<bool, String> {
    let service = state.execution_service.clone();
    run_blocking(move || service.pause_execution(parse_execution_id(&execution_id)?)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn cancel_execution(
    state: State<'_, ManagedScanner>,
    execution_id: String,
) -> Result<bool, String> {
    let service = state.execution_service.clone();
    run_blocking(move || service.cancel_execution(parse_execution_id(&execution_id)?)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_execution_status(
    state: State<'_, ManagedScanner>,
    execution_id: String,
) -> Result<ExecutionDetailDto, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        service
            .execution_status(parse_execution_id(&execution_id)?)
            .map(execution_detail_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn list_execution_history(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    limit: usize,
) -> Result<Vec<ExecutionSessionDto>, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        service
            .execution_history(parse_workspace_id(&workspace_id)?, limit)
            .map(|history| history.into_iter().map(execution_session_dto).collect())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn rollback_execution(
    app: AppHandle,
    state: State<'_, ManagedScanner>,
    execution_id: String,
) -> Result<ExecutionDetailDto, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        service
            .rollback_execution(parse_execution_id(&execution_id)?, &mut |progress| {
                let _ = app.emit(
                    "organization-execution-progress",
                    execution_progress_dto(progress),
                );
            })
            .map(execution_detail_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn recover_execution(
    state: State<'_, ManagedScanner>,
    execution_id: String,
) -> Result<RecoveryAssessmentDto, String> {
    let service = state.execution_service.clone();
    run_blocking(move || {
        service
            .recover_execution(parse_execution_id(&execution_id)?)
            .map(recovery_assessment_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn list_content_results(
    state: State<'_, ManagedScanner>,
    batch_id: String,
    limit: usize,
    offset: usize,
) -> Result<Vec<ContentDetailDto>, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .content_analysis_results(&batch_id, limit, offset)
            .map(|results| results.into_iter().map(content_detail_dto).collect())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn search_local_files(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    query: LocalSearchQueryDto,
) -> Result<LocalSearchPageDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .search_files(parse_workspace_id(&workspace_id)?, search_query(query))
            .map(local_search_page_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_embedding_model_status(
    state: State<'_, ManagedScanner>,
) -> Result<EmbeddingModelStatusDto, String> {
    let provider = state.embedding_provider.clone();
    run_blocking_string(move || Ok(embedding_model_status_dto(provider.model_status()))).await
}

#[tauri::command(rename_all = "camelCase")]
async fn activate_local_embedding_model(
    state: State<'_, ManagedScanner>,
) -> Result<EmbeddingModelStatusDto, String> {
    let provider = state.embedding_provider.clone();
    let cancel = state.model_install_cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    run_blocking_string(move || {
        // 1) Already verified app-local assets
        if provider.verify_installed().is_ok() {
            return Ok(embedding_model_status_dto(provider.model_status()));
        }
        // 2) Offline/dev: copy from SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR when set
        if provider.activate_from_env().is_ok() {
            return Ok(embedding_model_status_dto(provider.model_status()));
        }
        // 3) Production: user-consented pinned HTTPS install (no arbitrary URL)
        provider
            .install_pinned_model(&cancel, &mut |_| {})
            .map_err(embedding_model_error)?;
        Ok(embedding_model_status_dto(provider.model_status()))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn cancel_local_embedding_model_install(
    state: State<'_, ManagedScanner>,
) -> Result<EmbeddingModelStatusDto, String> {
    state.model_install_cancel.store(true, Ordering::SeqCst);
    let provider = state.embedding_provider.clone();
    run_blocking_string(move || Ok(embedding_model_status_dto(provider.model_status()))).await
}

#[tauri::command(rename_all = "camelCase")]
async fn retry_local_embedding_model(
    state: State<'_, ManagedScanner>,
) -> Result<EmbeddingModelStatusDto, String> {
    let provider = state.embedding_provider.clone();
    let cancel = state.model_install_cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    run_blocking_string(move || {
        provider.unload();
        if provider.verify_installed().is_ok() {
            return Ok(embedding_model_status_dto(provider.model_status()));
        }
        if provider.activate_from_env().is_ok() {
            return Ok(embedding_model_status_dto(provider.model_status()));
        }
        provider
            .install_pinned_model(&cancel, &mut |_| {})
            .map_err(embedding_model_error)?;
        Ok(embedding_model_status_dto(provider.model_status()))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn remove_local_embedding_model(
    state: State<'_, ManagedScanner>,
) -> Result<EmbeddingModelStatusDto, String> {
    let provider = state.embedding_provider.clone();
    let service = state.service.clone();
    run_blocking_string(move || {
        provider.remove_model().map_err(embedding_model_error)?;
        // Stale ANN must not be treated as usable without a model.
        let _ = service.mark_all_ann_rebuild_required("embedding model removed");
        Ok(embedding_model_status_dto(provider.model_status()))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn rebuild_semantic_ann_index(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
) -> Result<String, String> {
    let service = state.service.clone();
    run_blocking_string(move || {
        let workspace_id = parse_workspace_id(&workspace_id).map_err(command_error)?;
        service
            .rebuild_semantic_ann_index(workspace_id, &|| false)
            .map(|status| status.as_str().to_owned())
            .map_err(command_error)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn list_review_items(
    state: State<'_, ManagedScanner>,
    workspace_id: String,
    status: String,
    reason: String,
    limit: usize,
    offset: usize,
) -> Result<FileReviewPageDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        service
            .review_items(
                parse_workspace_id(&workspace_id)?,
                review_status_filter(&status),
                review_reason_filter(&reason),
                limit,
                offset,
            )
            .map(file_review_page_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn update_review_item(
    state: State<'_, ManagedScanner>,
    review_id: String,
    action: String,
) -> Result<FileReviewItemDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let action = match action.as_str() {
            "resolve" => ReviewAction::Resolve,
            "ignore" => ReviewAction::Ignore,
            _ => return Err(ApplicationError::NotFound),
        };
        service
            .update_review_item(&review_id, action)
            .map(file_review_item_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn get_file_detail(
    state: State<'_, ManagedScanner>,
    file_id: String,
) -> Result<LocalFileDetailDto, String> {
    let service = state.service.clone();
    run_blocking(move || service.file_detail(&file_id).map(local_file_detail_dto)).await
}

#[tauri::command(rename_all = "camelCase")]
async fn store_semantic_correction(
    state: State<'_, ManagedScanner>,
    file_id: String,
    field_key: String,
    action: String,
    value: Option<String>,
) -> Result<SemanticCorrectionDto, String> {
    let service = state.service.clone();
    run_blocking(move || {
        let action = match action.as_str() {
            "confirm" => SemanticCorrectionAction::Confirm,
            "correct" => SemanticCorrectionAction::Correct,
            _ => return Err(ApplicationError::InvalidSemanticCorrection),
        };
        service
            .store_semantic_correction(&file_id, &field_key, action, value.as_deref())
            .map(semantic_correction_dto)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
async fn retry_extraction(
    state: State<'_, ManagedScanner>,
    review_id: String,
) -> Result<ExtractionRetryDto, String> {
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state.retry_cancellations.lock().map_err(|_| {
            "Le registre local des nouvelles extractions est indisponible.".to_owned()
        })?;
        if active.contains_key(&review_id) {
            return Err("Une nouvelle extraction est déjà en cours pour ce fichier.".to_owned());
        }
        active.insert(review_id.clone(), cancellation.clone());
    }
    let service = state.service.clone();
    let registry = state.retry_cancellations.clone();
    let registry_key = review_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        service.retry_review_extraction(&review_id, &|| cancellation.load(Ordering::Relaxed))
    })
    .await
    .map_err(|_| "La nouvelle extraction locale s’est interrompue.".to_owned());
    if let Ok(mut active) = registry.lock() {
        active.remove(&registry_key);
    }
    result?.map(extraction_retry_dto).map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::needless_pass_by_value)]
fn cancel_extraction_retry(
    state: State<'_, ManagedScanner>,
    review_id: String,
) -> Result<bool, String> {
    let active = state
        .retry_cancellations
        .lock()
        .map_err(|_| "Le registre local des nouvelles extractions est indisponible.".to_owned())?;
    let Some(cancellation) = active.get(&review_id) else {
        return Ok(false);
    };
    cancellation.store(true, Ordering::Relaxed);
    Ok(true)
}

async fn run_blocking<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApplicationError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| "Une opération interne s’est interrompue. Réessayez.".to_owned())?
        .map_err(command_error)
}

async fn run_blocking_string<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| "Une opération interne s’est interrompue. Réessayez.".to_owned())?
}

fn parse_workspace_id(value: &str) -> Result<WorkspaceId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::NotFound)
}

fn parse_root_id(value: &str) -> Result<RootId, ApplicationError> {
    value
        .parse()
        .map_err(|_| ApplicationError::InvalidMonitoringRequest)
}

fn parse_scan_id(value: &str) -> Result<ScanId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::NotFound)
}

fn parse_proposal_id(value: &str) -> Result<ProposalId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::NotFound)
}

fn parse_execution_id(value: &str) -> Result<ExecutionId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::NotFound)
}

fn parse_file_id(value: &str) -> Result<FileId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::NotFound)
}

fn parse_rule_id(value: &str) -> Result<RuleId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::InvalidRule)
}

fn parse_rule_suggestion_id(value: &str) -> Result<RuleSuggestionId, ApplicationError> {
    value.parse().map_err(|_| ApplicationError::InvalidRule)
}

fn proposal_override_action(value: &str) -> Result<ProposalOverrideAction, ApplicationError> {
    match value {
        "destination" => Ok(ProposalOverrideAction::Destination),
        "rename" => Ok(ProposalOverrideAction::Rename),
        "destination_and_rename" => Ok(ProposalOverrideAction::DestinationAndRename),
        "keep_in_place" => Ok(ProposalOverrideAction::KeepInPlace),
        "to_review" => Ok(ProposalOverrideAction::ToReview),
        "reject" => Ok(ProposalOverrideAction::Reject),
        _ => Err(ApplicationError::InvalidOrganizationProposal),
    }
}

fn embedding_model_status_dto(status: search::EmbeddingModelStatusView) -> EmbeddingModelStatusDto {
    EmbeddingModelStatusDto {
        model_id: status.model_id,
        version: status.version,
        dimensions: status.dimensions,
        status: status.status.as_str().to_owned(),
        approximate_disk_bytes: status.approximate_disk_bytes,
        license: status.license,
        local_only: status.local_only,
        download_implemented: status.download_implemented,
        last_error: status.last_error,
        install_root: status.install_root,
    }
}

fn embedding_model_error(error: search::EmbeddingError) -> String {
    match error {
        search::EmbeddingError::Unavailable => {
            "Recherche sémantique indisponible. La recherche classique reste active.".to_owned()
        }
        search::EmbeddingError::Corrupt => {
            "Le modèle de recherche sémantique n'a pas pu être vérifié.".to_owned()
        }
        search::EmbeddingError::Failed(message) if message.contains("insufficient disk space") => {
            "Espace disque insuffisant pour installer le modèle sémantique.".to_owned()
        }
        search::EmbeddingError::Failed(message) if message.contains("cancelled") => {
            "Installation du modèle sémantique annulée.".to_owned()
        }
        search::EmbeddingError::Failed(message) => {
            format!("Échec du modèle sémantique local : {message}")
        }
        search::EmbeddingError::InputLimit | search::EmbeddingError::InvalidVector => {
            "La requête d’embedding locale a été refusée.".to_owned()
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn command_error(error: ApplicationError) -> String {
    match &error {
        ApplicationError::InvalidWorkspaceName => {
            "Le nom de l’inventaire est vide ou dépasse 80 caractères.".to_owned()
        }
        ApplicationError::NotFound => "La ressource demandée n’existe plus.".to_owned(),
        ApplicationError::Persistence(inner) => {
            humanize_nested_failure("persistence", &inner.to_string())
        }
        ApplicationError::Platform(inner) => {
            humanize_nested_failure("platform", &inner.to_string())
        }
        ApplicationError::Catalog(inner) => humanize_nested_failure("catalog", &inner.to_string()),
        ApplicationError::Io(inner) => humanize_nested_failure("io", &inner.to_string()),
        ApplicationError::Knowledge(inner) => {
            humanize_nested_failure("semantic", &inner.to_string())
        }
        ApplicationError::ContentExtraction(_) => {
            "Certains documents n’ont pas pu être lus complètement. Le reste de l’analyse continue."
                .to_owned()
        }
        ApplicationError::InvalidOrganizationProposal => {
            "Cette modification proposée a été refusée pour des raisons de sécurité des chemins."
                .to_owned()
        }
        ApplicationError::InvalidMonitoringRequest => {
            "La demande de surveillance est invalide pour ce dossier.".to_owned()
        }
        ApplicationError::InvalidRule => {
            "La règle a été refusée : vérifiez ses conditions et l’emplacement proposé.".to_owned()
        }
        ApplicationError::ExecutionApprovalRequired => {
            "La proposition approuvée ou sa révision ne correspond plus.".to_owned()
        }
        ApplicationError::ExecutionAlreadyActive => {
            "Une exécution ou une récupération doit être terminée avant d’en préparer une autre."
                .to_owned()
        }
        ApplicationError::ExecutionRecoveryRequired | ApplicationError::JournalLocked => {
            "Une opération précédente nécessite votre attention avant de continuer.".to_owned()
        }
        ApplicationError::ExecutionPreflightBlocked => {
            "Aucun fichier approuvé ne peut être modifié en toute sécurité après revalidation."
                .to_owned()
        }
        ApplicationError::ExecutionConsentExpired => {
            "La confirmation a expiré. Vérifiez de nouveau le plan avant de continuer.".to_owned()
        }
        ApplicationError::ExecutionConfirmationDeclined => {
            "La confirmation a été annulée sans modifier de fichier.".to_owned()
        }
        ApplicationError::Operations(_) => {
            "Une opération précédente nécessite votre attention avant toute modification."
                .to_owned()
        }
        ApplicationError::InvalidExecution | ApplicationError::ExecutionSafety(_) => {
            "Le plan d’exécution a échoué à une vérification de sécurité.".to_owned()
        }
        _ => "Cette action n’a pas pu être terminée. Aucun fichier n’a été modifié.".to_owned(),
    }
}

fn humanize_nested_failure(kind: &str, detail: &str) -> String {
    let normalized = detail.to_ascii_lowercase();
    if normalized.contains("permission")
        || normalized.contains("denied")
        || normalized.contains("eacces")
        || normalized.contains("access")
    {
        return "L’accès à ce dossier n’est plus disponible.".to_owned();
    }
    if normalized.contains("not found") || normalized.contains("enoent") {
        return "Cet élément est introuvable. Il a peut‑être été déplacé ou renommé.".to_owned();
    }
    if normalized.contains("timeout") || normalized.contains("timed out") {
        return "L’opération a pris trop de temps. Réessayez dans un instant.".to_owned();
    }
    if kind == "semantic"
        || normalized.contains("embedding")
        || normalized.contains("onnx")
        || normalized.contains("model")
    {
        return "La recherche intelligente est temporairement indisponible. La recherche classique reste disponible.".to_owned();
    }
    if kind == "catalog" || normalized.contains("scan") {
        return "L’analyse n’a pas pu se terminer correctement. Réessayez.".to_owned();
    }
    if normalized.contains("watcher") || normalized.contains("monitor") {
        return "La surveillance de ce dossier s’est interrompue. L’analyse manuelle reste disponible.".to_owned();
    }
    if normalized.contains("corrupt")
        || normalized.contains("cipher")
        || normalized.contains("journal")
    {
        return "Une opération précédente nécessite votre attention avant de continuer.".to_owned();
    }
    "Cette action n’a pas pu être terminée pour le moment. Réessayez.".to_owned()
}

fn monitoring_dashboard_dto(dashboard: MonitoringDashboard) -> MonitoringDashboardDto {
    MonitoringDashboardDto {
        workspace_id: dashboard.state.workspace_id.to_string(),
        mode: dashboard.state.mode.database_name().to_owned(),
        paused: dashboard.state.paused,
        startup_reconciliation_pending: dashboard.state.startup_reconciliation_pending,
        automatic_execution_enabled: false,
        folders: dashboard
            .roots
            .into_iter()
            .map(|root| MonitoredFolderDto {
                root_id: root.root_id.to_string(),
                display_label: root.display_label,
                selected_path: root.selected_path,
                enabled: root.enabled,
                status: root.status.database_name().to_owned(),
                pending_jobs: root.pending_jobs,
                last_reconciled_at: root.last_reconciled_at,
                last_error: root.last_error,
            })
            .collect(),
        counts: MonitoringCountsDto {
            files_analyzed: dashboard.counts.files_analyzed,
            ready_to_organize: dashboard.counts.ready_to_organize,
            needs_review: dashboard.counts.needs_review,
            pending_proposals: dashboard.counts.pending_proposals,
            pending_jobs: dashboard.counts.pending_jobs,
        },
        recent_activity: dashboard
            .activity
            .into_iter()
            .map(|activity| MonitoringActivityDto {
                id: activity.id,
                summary: activity.summary,
                files_analyzed: activity.files_analyzed,
                ready_to_organize: activity.ready_to_organize,
                needs_review: activity.needs_review,
                failed: activity.failed,
                created_at: activity.created_at,
            })
            .collect(),
        exclusions: dashboard
            .exclusions
            .into_iter()
            .map(|exclusion| MonitoringExclusionDto {
                id: exclusion.id,
                root_id: exclusion.root_id.map(|root_id| root_id.to_string()),
                kind: exclusion.kind.database_name().to_owned(),
                value: exclusion.value,
                enabled: exclusion.enabled,
            })
            .collect(),
    }
}

fn restored_workspace_session_dto(
    session: application::RestoredWorkspaceSession,
) -> RestoredWorkspaceSessionDto {
    let root = session.root.map(|root| RegisteredRootDto {
        id: root.id.to_string(),
        display_label: root.display_label,
        selected_path: root.absolute_path,
    });
    RestoredWorkspaceSessionDto {
        workspace: WorkspaceDto {
            id: session.workspace.id.to_string(),
            name: session.workspace.name,
            root: root.clone(),
            created_at: Some(session.workspace.created_at),
        },
        root,
        scan: session.latest_scan.map(scan_result_dto),
        safe_read_only: session.safe_read_only,
        filesystem_execution_resumed: session.filesystem_execution_resumed,
    }
}

fn scan_result_dto(scan: ScanRecord) -> ScanResultDto {
    ScanResultDto {
        id: scan.id.to_string(),
        status: scan.status.to_ascii_uppercase(),
        started_at: scan.started_at,
        completed_at: scan.completed_at,
        files_discovered: scan.discovered_count,
        files_indexed: scan.indexed_count,
        directories_discovered: scan.directory_count,
        bytes_discovered: scan.byte_count,
        files_hashed: scan.hashed_count,
        duplicate_groups: scan.duplicate_group_count,
        errors: scan.error_count,
        skipped_items: scan.skipped_count,
        truncated: scan.truncated,
    }
}

fn progress_dto(progress: ScanProgress) -> ScanProgressDto {
    ScanProgressDto {
        scan_id: progress.scan_id.to_string(),
        phase: phase_name(progress.phase).to_owned(),
        files_discovered: progress.files_discovered,
        files_indexed: progress.files_indexed,
        directories_discovered: progress.directories_discovered,
        bytes_discovered: progress.bytes_discovered,
        files_hashed: progress.files_hashed,
        duplicate_groups: progress.duplicate_groups,
        errors: progress.errors,
        skipped_items: progress.skipped_items,
    }
}

fn content_analysis_dto(batch: ExtractionBatchRecord) -> ContentAnalysisDto {
    ContentAnalysisDto {
        id: batch.id,
        scan_id: batch.scan_id.to_string(),
        status: batch.status.to_ascii_uppercase(),
        files_queued: batch.files_queued,
        files_completed: batch.files_completed,
        successful: batch.successful_count,
        partial: batch.partial_count,
        unsupported: batch.unsupported_count,
        skipped: batch.skipped_count,
        failed: batch.failed_count,
        ocr_processed: batch.ocr_processed_count,
        started_at: batch.started_at,
        completed_at: batch.completed_at,
    }
}

fn content_progress_dto(progress: &ContentAnalysisProgress) -> ContentAnalysisProgressDto {
    ContentAnalysisProgressDto {
        batch_id: progress.batch_id.clone(),
        scan_id: progress.scan_id.to_string(),
        phase: content_phase_name(progress.phase).to_owned(),
        files_queued: progress.files_queued,
        files_completed: progress.files_completed,
        successful: progress.successful,
        partial: progress.partial,
        unsupported: progress.unsupported,
        skipped: progress.skipped,
        failed: progress.failed,
        ocr_processed: progress.ocr_processed,
    }
}

fn semantic_analysis_dto(batch: SemanticAnalysisBatchRecord) -> SemanticAnalysisDto {
    SemanticAnalysisDto {
        id: batch.id,
        scan_id: batch.scan_id.to_string(),
        status: batch.status.to_ascii_uppercase(),
        files_queued: batch.files_queued,
        files_completed: batch.files_completed,
        high_confidence: batch.high_confidence_count,
        needs_review: batch.needs_review_count,
        unknown: batch.unknown_count,
        partial: batch.partial_count,
        failed: batch.failed_count,
        started_at: batch.started_at,
        completed_at: batch.completed_at,
    }
}

fn semantic_progress_dto(progress: &SemanticAnalysisProgress) -> SemanticAnalysisProgressDto {
    SemanticAnalysisProgressDto {
        batch_id: progress.batch_id.clone(),
        scan_id: progress.scan_id.to_string(),
        phase: semantic_phase_name(progress.phase).to_owned(),
        files_queued: progress.files_queued,
        files_completed: progress.files_completed,
        high_confidence: progress.high_confidence,
        needs_review: progress.needs_review,
        unknown: progress.unknown,
        partial: progress.partial,
        failed: progress.failed,
    }
}

fn identity_resolution_dto(run: IdentityResolverRunRecord) -> IdentityResolutionDto {
    IdentityResolutionDto {
        run_id: run.run_id,
        workspace_id: run.workspace_id.to_string(),
        trigger_kind: run.trigger_kind.to_ascii_uppercase(),
        status: run.status.to_ascii_uppercase(),
        resolver_id: run.resolver_id,
        resolver_version: run.resolver_version,
        files_considered: run.files_considered,
        occurrences_processed: run.occurrences_processed,
        blocking_memberships: run.blocking_memberships,
        comparisons: run.comparisons,
        candidates_created: run.candidates_created,
        auto_links_created: run.auto_links_created,
        started_at: run.started_at,
        completed_at: run.completed_at,
    }
}

fn identity_progress_dto(progress: &IdentityResolutionProgress) -> IdentityResolutionProgressDto {
    IdentityResolutionProgressDto {
        run_id: progress.run_id.clone(),
        workspace_id: progress.workspace_id.to_string(),
        phase: identity_phase_name(progress.phase).to_owned(),
        files_considered: progress.files_considered,
        occurrences_processed: progress.occurrences_processed,
        blocking_memberships: progress.blocking_memberships,
        comparisons: progress.comparisons,
        candidates_created: progress.candidates_created,
        auto_links_created: progress.auto_links_created,
    }
}

fn content_detail_dto(result: ExtractionDetailRecord) -> ContentDetailDto {
    ContentDetailDto {
        file_version_id: result.file_version_id,
        filename: result.filename,
        relative_path: result.relative_path,
        extension: result.extension,
        status: result.status.to_ascii_uppercase(),
        extractor_type: result.extractor_type,
        extractor_version: result.extractor_version,
        detected_content_type: result.detected_content_type,
        type_mismatch: result.type_mismatch,
        text_preview: result.text_preview,
        character_count: result.character_count,
        page_count: result.page_count,
        sheet_count: result.sheet_count,
        slide_count: result.slide_count,
        image_width: result.image_width,
        image_height: result.image_height,
        requires_ocr: result.requires_ocr,
        ocr_used: result.ocr_used,
        ocr_confidence: result.ocr_confidence,
        language_hint: result.language_hint,
        extraction_duration_ms: result.extraction_duration_ms,
        truncated: result.truncated,
        structured_metadata: result.structured_metadata,
        error_category: result
            .error_category
            .map(|category| category.to_ascii_uppercase()),
        error_message: result.error_message,
        extracted_at: result.extracted_at,
    }
}

fn search_query(query: LocalSearchQueryDto) -> SearchQuery {
    SearchQuery {
        text: query.text,
        filters: SearchFilters {
            file_type: match query.filters.file_type.as_str() {
                "pdf" => FileTypeFilter::Pdf,
                "documents" => FileTypeFilter::Documents,
                "spreadsheets" => FileTypeFilter::Spreadsheets,
                "presentations" => FileTypeFilter::Presentations,
                "images" => FileTypeFilter::Images,
                "archives" => FileTypeFilter::Archives,
                "other" => FileTypeFilter::Other,
                _ => FileTypeFilter::All,
            },
            modified: match query.filters.modified.as_str() {
                "today" => ModifiedFilter::Today,
                "last_7_days" => ModifiedFilter::LastSevenDays,
                "last_30_days" => ModifiedFilter::LastThirtyDays,
                "this_year" => ModifiedFilter::ThisYear,
                _ => ModifiedFilter::Any,
            },
            extraction: match query.filters.extraction.as_str() {
                "success" => ExtractionFilter::Success,
                "partial" => ExtractionFilter::Partial,
                "failed" => ExtractionFilter::Failed,
                "unsupported" => ExtractionFilter::Unsupported,
                _ => ExtractionFilter::Any,
            },
            ocr: match query.filters.ocr.as_str() {
                "used" => OcrFilter::Used,
                "not_used" => OcrFilter::NotUsed,
                "unavailable" => OcrFilter::Unavailable,
                _ => OcrFilter::Any,
            },
            minimum_size: query.filters.minimum_size,
            maximum_size: query.filters.maximum_size,
            document_type: document_type_filter(&query.filters.document_type),
            context: match query.filters.context.as_str() {
                "personal" => ContextFilter::Personal,
                "business" => ContextFilter::Business,
                "mixed" => ContextFilter::Mixed,
                "unknown" => ContextFilter::Unknown,
                _ => ContextFilter::Any,
            },
            customer: query.filters.customer,
            supplier: query.filters.supplier,
            project: query.filters.project,
            year: query.filters.year,
            amount_minimum_minor: query.filters.amount_minimum_minor,
            amount_maximum_minor: query.filters.amount_maximum_minor,
            currency: query.filters.currency,
            semantic_status: match query.filters.semantic_status.as_str() {
                "success" => SemanticStatusFilter::Success,
                "partial" => SemanticStatusFilter::Partial,
                "unknown" => SemanticStatusFilter::Unknown,
                "failed" => SemanticStatusFilter::Failed,
                "pending" => SemanticStatusFilter::Pending,
                _ => SemanticStatusFilter::Any,
            },
            minimum_confidence_percent: query.filters.minimum_confidence_percent,
        },
        sort: match query.sort.as_str() {
            "newest" => SearchSort::Newest,
            "oldest" => SearchSort::Oldest,
            "filename" => SearchSort::Filename,
            "size" => SearchSort::Size,
            _ => SearchSort::Relevance,
        },
        page: query.page,
        page_size: query.page_size,
        semantic_search: query.semantic_search.unwrap_or(true),
        disabled_intents: query.disabled_intents,
    }
}

fn document_type_filter(value: &str) -> DocumentTypeFilter {
    match value {
        "invoice" => DocumentTypeFilter::Invoice,
        "quote" => DocumentTypeFilter::Quote,
        "contract" => DocumentTypeFilter::Contract,
        "purchase_order" => DocumentTypeFilter::PurchaseOrder,
        "delivery_note" => DocumentTypeFilter::DeliveryNote,
        "bank_statement" => DocumentTypeFilter::BankStatement,
        "tax_document" => DocumentTypeFilter::TaxDocument,
        "payslip" => DocumentTypeFilter::Payslip,
        "employment_contract" => DocumentTypeFilter::EmploymentContract,
        "insurance_document" => DocumentTypeFilter::InsuranceDocument,
        "legal_document" => DocumentTypeFilter::LegalDocument,
        "administrative_document" => DocumentTypeFilter::AdministrativeDocument,
        "receipt" => DocumentTypeFilter::Receipt,
        "report" => DocumentTypeFilter::Report,
        "letter" => DocumentTypeFilter::Letter,
        "cv" => DocumentTypeFilter::Cv,
        "photo" => DocumentTypeFilter::Photo,
        "video" => DocumentTypeFilter::Video,
        "spreadsheet" => DocumentTypeFilter::Spreadsheet,
        "presentation" => DocumentTypeFilter::Presentation,
        "archive" => DocumentTypeFilter::Archive,
        "other" => DocumentTypeFilter::Other,
        "unknown" => DocumentTypeFilter::Unknown,
        _ => DocumentTypeFilter::Any,
    }
}

fn local_search_page_dto(page: search::SearchPage) -> LocalSearchPageDto {
    LocalSearchPageDto {
        query: page.query,
        page: page.page,
        page_size: page.page_size,
        total: page.total,
        has_more: page.has_more,
        results: page
            .results
            .into_iter()
            .map(|result| LocalSearchResultDto {
                file_id: result.file_id,
                filename: result.filename,
                relative_path: result.relative_path,
                detected_type: result.detected_type,
                extension: result.extension,
                byte_size: result.byte_size,
                modified_at: result.modified_at,
                extraction_status: result.extraction_status,
                ocr_status: result.ocr_status,
                duplicate: result.duplicate,
                match_source: match result.match_source {
                    MatchSource::Filename => "filename",
                    MatchSource::Path => "path",
                    MatchSource::Content => "content",
                    MatchSource::Metadata => "metadata",
                    MatchSource::Structured => "structured",
                    MatchSource::Relationship => "relationship",
                    MatchSource::Semantic => "semantic",
                }
                .to_owned(),
                relevance: result.relevance,
                snippet: result.snippet,
                why_matched: result.why_matched,
            })
            .collect(),
        interpreted_query: page
            .interpreted_query
            .into_iter()
            .map(|chip| QueryChipDto {
                id: chip.id,
                kind: chip.kind,
                label: chip.label,
                value: chip.value,
            })
            .collect(),
        embeddings: EmbeddingSearchStatusDto {
            availability: match page.embeddings.availability {
                EmbeddingAvailability::AvailableDevelopment => "available_development",
                EmbeddingAvailability::AvailableProduction => "available_production",
                EmbeddingAvailability::Unavailable => "unavailable",
            }
            .to_owned(),
            provider_id: page.embeddings.provider_id,
            version: page.embeddings.version,
            production_ready: page.embeddings.production_ready,
            indexed_files: page.embeddings.indexed_files,
            ann_index_status: page.embeddings.ann_index_status,
        },
        timings: SearchTimingsDto {
            total_ms: page.timings.total_ms,
            lexical_and_structured_ms: page.timings.lexical_and_structured_ms,
            query_embed_ms: page.timings.query_embed_ms,
            ann_ms: page.timings.ann_ms,
            vector_ms: page.timings.vector_ms,
            fusion_ms: page.timings.fusion_ms,
        },
    }
}

fn review_status_filter(value: &str) -> ReviewStatusFilter {
    match value {
        "resolved" => ReviewStatusFilter::Resolved,
        "ignored" => ReviewStatusFilter::Ignored,
        "all" => ReviewStatusFilter::All,
        _ => ReviewStatusFilter::NeedsReview,
    }
}

fn review_reason_filter(value: &str) -> ReviewReasonFilter {
    match value {
        "ocr" => ReviewReasonFilter::Ocr,
        "unsupported" => ReviewReasonFilter::Unsupported,
        "permissions" => ReviewReasonFilter::Permissions,
        "partial" => ReviewReasonFilter::Partial,
        "corrupt" => ReviewReasonFilter::Corrupt,
        "semantic" => ReviewReasonFilter::Semantic,
        _ => ReviewReasonFilter::All,
    }
}

fn file_review_item_dto(item: ReviewItemRecord) -> FileReviewItemDto {
    FileReviewItemDto {
        review_id: item.review_id,
        file_id: item.file_id,
        filename: item.filename,
        relative_path: item.relative_path,
        reason: item.reason.to_ascii_uppercase(),
        source_subsystem: item.source_subsystem.to_ascii_uppercase(),
        severity: item.severity.to_ascii_uppercase(),
        explanation: item.explanation,
        technical_details: item.technical_details,
        status: item.status.to_ascii_uppercase(),
        retry_available: item.retry_available,
        retry_count: item.retry_count,
        extraction_status: item
            .extraction_status
            .map(|status| status.to_ascii_uppercase()),
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}

fn file_review_page_dto(page: persistence::ReviewPageRecord) -> FileReviewPageDto {
    FileReviewPageDto {
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        items: page.items.into_iter().map(file_review_item_dto).collect(),
    }
}

fn local_file_detail_dto(detail: FileDetailRecord) -> LocalFileDetailDto {
    LocalFileDetailDto {
        file_id: detail.file_id,
        file_version_id: detail.file_version_id,
        filename: detail.filename,
        relative_path: detail.relative_path,
        extension: detail.extension,
        detected_type: detail.detected_type,
        byte_size: detail.byte_size,
        created_at: detail.created_at,
        modified_at: detail.modified_at,
        hash: detail.hash,
        duplicate: detail.duplicate,
        extraction_status: detail
            .extraction_status
            .map(|status| status.to_ascii_uppercase()),
        extractor_type: detail.extractor_type,
        extractor_version: detail.extractor_version,
        ocr_status: detail.ocr_status.map(|status| status.to_ascii_uppercase()),
        text_preview: detail.text_preview,
        character_count: detail.character_count,
        review_items: detail
            .review_items
            .into_iter()
            .map(file_review_item_dto)
            .collect(),
        semantic_analysis: detail.semantic_analysis.map(semantic_detail_dto),
        relationships: detail
            .relationships
            .into_iter()
            .map(identity_relationship_dto)
            .collect(),
    }
}

fn identity_summary_dto(identity: IdentitySummaryRecord) -> IdentitySummaryDto {
    IdentitySummaryDto {
        identity_id: identity.identity_id,
        identity_type: identity.identity_type.to_ascii_uppercase(),
        display_name: identity.display_name,
        normalized_display_name: identity.normalized_display_name,
        resolution_status: identity.resolution_status.to_ascii_uppercase(),
        lifecycle_status: identity.lifecycle_status.to_ascii_uppercase(),
        confidence: identity.confidence,
        user_locked: identity.user_locked,
        occurrence_count: identity.occurrence_count,
        file_count: identity.file_count,
        aliases: identity.aliases,
        roles: identity
            .roles
            .into_iter()
            .map(|role| role.to_ascii_uppercase())
            .collect(),
    }
}

fn identity_match_evidence_dto(evidence: IdentityMatchEvidenceRecord) -> IdentityMatchEvidenceDto {
    IdentityMatchEvidenceDto {
        evidence_type: evidence.evidence_type.to_ascii_uppercase(),
        strength: evidence.strength.to_ascii_uppercase(),
        polarity: evidence.polarity.to_ascii_uppercase(),
        left_value: evidence.left_value,
        right_value: evidence.right_value,
        weight: evidence.weight,
        explanation: evidence.explanation,
    }
}

fn identity_candidate_dto(candidate: IdentityCandidateRecord) -> IdentityCandidateDto {
    IdentityCandidateDto {
        candidate_id: candidate.candidate_id,
        review_group_key: candidate.review_group_key,
        score: candidate.score,
        policy_decision: candidate.policy_decision.to_ascii_uppercase(),
        status: candidate.status.to_ascii_uppercase(),
        resolver_version: candidate.resolver_version,
        left: identity_summary_dto(candidate.left),
        right: identity_summary_dto(candidate.right),
        evidence: candidate
            .evidence
            .into_iter()
            .map(identity_match_evidence_dto)
            .collect(),
        created_at: candidate.created_at,
        updated_at: candidate.updated_at,
    }
}

fn identity_review_group_dto(group: IdentityReviewGroupRecord) -> IdentityReviewGroupDto {
    IdentityReviewGroupDto {
        review_group_id: group.review_group_id,
        review_reason: group.review_reason.to_ascii_uppercase(),
        group_key: group.group_key,
        title: group.title,
        explanation: group.explanation,
        max_score: group.max_score,
        candidate_count: group.candidate_count,
        occurrence_count: group.occurrence_count,
        file_count: group.file_count,
        status: group.status.to_ascii_uppercase(),
        resolver_version: group.resolver_version,
        candidates: group
            .candidates
            .into_iter()
            .map(identity_candidate_dto)
            .collect(),
        created_at: group.created_at,
        updated_at: group.updated_at,
    }
}

fn identity_review_page_dto(page: IdentityReviewPageRecord) -> IdentityReviewPageDto {
    IdentityReviewPageDto {
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        items: page
            .items
            .into_iter()
            .map(identity_review_group_dto)
            .collect(),
    }
}

fn identity_occurrence_dto(occurrence: IdentityOccurrenceRecord) -> IdentityOccurrenceDto {
    IdentityOccurrenceDto {
        occurrence_id: occurrence.occurrence_id,
        file_id: occurrence.file_id,
        filename: occurrence.filename,
        relative_path: occurrence.relative_path,
        original_value: occurrence.original_value,
        normalized_value: occurrence.normalized_value,
        confidence: occurrence.confidence,
        role: occurrence.role.map(|role| role.to_ascii_uppercase()),
        analyzer_version: occurrence.analyzer_version,
        active: occurrence.active,
    }
}

fn identity_relationship_dto(relationship: IdentityRelationshipRecord) -> IdentityRelationshipDto {
    IdentityRelationshipDto {
        relationship_id: relationship.relationship_id,
        relationship_type: relationship.relationship_type.to_ascii_uppercase(),
        identity_id: relationship.identity_id,
        display_name: relationship.display_name,
        identity_type: relationship.identity_type.to_ascii_uppercase(),
        confidence: relationship.confidence,
        status: relationship.status.to_ascii_uppercase(),
        user_confirmation_state: relationship
            .user_confirmation_state
            .map(|state| state.to_ascii_uppercase()),
        evidence: relationship.evidence,
    }
}

fn identity_identifier_dto(identifier: IdentityIdentifierRecord) -> IdentityIdentifierDto {
    IdentityIdentifierDto {
        kind: identifier.kind.to_ascii_uppercase(),
        value: identifier.value,
    }
}

fn identity_audit_event_dto(event: IdentityAuditEventRecord) -> IdentityAuditEventDto {
    IdentityAuditEventDto {
        event_type: event.event_type.to_ascii_uppercase(),
        decision_source: event.decision_source.to_ascii_uppercase(),
        related_identity_id: event.related_identity_id,
        reason: event.reason,
        created_at: event.created_at,
    }
}

fn identity_detail_dto(detail: IdentityDetailRecord) -> IdentityDetailDto {
    IdentityDetailDto {
        identity: identity_summary_dto(detail.identity),
        occurrences: detail
            .occurrences
            .into_iter()
            .map(identity_occurrence_dto)
            .collect(),
        occurrence_total: detail.occurrence_total,
        occurrences_truncated: detail.occurrences_truncated,
        identifiers: detail
            .identifiers
            .into_iter()
            .map(identity_identifier_dto)
            .collect(),
        relationships: detail
            .relationships
            .into_iter()
            .map(identity_relationship_dto)
            .collect(),
        projects: detail
            .projects
            .into_iter()
            .map(identity_summary_dto)
            .collect(),
        audit_events: detail
            .audit_events
            .into_iter()
            .map(identity_audit_event_dto)
            .collect(),
        resolver_version: detail.resolver_version,
        updated_at: detail.updated_at,
    }
}

fn identity_mutation_dto(mutation: IdentityMutationRecord) -> IdentityMutationDto {
    IdentityMutationDto {
        decision_id: mutation.decision_id,
        primary_identity_id: mutation.primary_identity_id,
        secondary_identity_id: mutation.secondary_identity_id,
        occurrence_id: mutation.occurrence_id,
        action: mutation.action.to_ascii_uppercase(),
        created_at: mutation.created_at,
    }
}

fn rules_preferences_state_dto(
    value: application::RulesPreferencesState,
) -> RulesPreferencesStateDto {
    RulesPreferencesStateDto {
        rules: value.rules.into_iter().map(Into::into).collect(),
        suggestions: value.suggestions.into_iter().map(Into::into).collect(),
        preferences: value.preferences.into(),
    }
}

fn organization_proposal_progress_dto(
    progress: &ProposalBuildProgress,
) -> OrganizationProposalProgressDto {
    OrganizationProposalProgressDto {
        proposal_id: progress.proposal_id.to_string(),
        phase: match progress.phase {
            ProposalBuildPhase::Evaluating => "EVALUATING",
            ProposalBuildPhase::ResolvingGroups => "RESOLVING_GROUPS",
            ProposalBuildPhase::DetectingConflicts => "DETECTING_CONFLICTS",
            ProposalBuildPhase::BuildingTree => "BUILDING_TREE",
            ProposalBuildPhase::Completed => "COMPLETED",
            ProposalBuildPhase::Cancelled => "CANCELLED",
        }
        .to_owned(),
        files_total: progress.files_total,
        files_evaluated: progress.files_evaluated,
        high_confidence: progress.high_confidence,
        needs_review: progress.needs_review,
        conflicts: progress.conflicts,
    }
}

fn organization_proposal_dto(proposal: OrganizationProposal) -> OrganizationProposalDto {
    OrganizationProposalDto {
        id: proposal.id.to_string(),
        revision_id: proposal.revision_id.to_string(),
        workspace_id: proposal.workspace_id.to_string(),
        root_id: proposal.root_id.to_string(),
        source_scan_id: proposal.source_scan_id.to_string(),
        revision: proposal.revision,
        status: proposal.status.database_name().to_ascii_uppercase(),
        engine_version: proposal.engine_version,
        policy_version: proposal.policy_version,
        source_semantic_version: proposal.source_semantic_version,
        source_relationship_version: proposal.source_relationship_version,
        created_at: proposal.created_at,
        updated_at: proposal.updated_at,
        summary: OrganizationProposalSummaryDto {
            files_analyzed: proposal.summary.files_analyzed,
            proposed_moves: proposal.summary.proposed_moves,
            proposed_renames: proposal.summary.proposed_renames,
            unchanged: proposal.summary.unchanged,
            needs_review: proposal.summary.needs_review,
            unresolved: proposal.summary.unresolved,
            conflicts: proposal.summary.conflicts,
            high_confidence: proposal.summary.high_confidence,
            medium_confidence: proposal.summary.medium_confidence,
            low_confidence: proposal.summary.low_confidence,
            duplicate_no_action: proposal.summary.duplicate_no_action,
            average_depth: proposal.summary.average_depth,
            maximum_depth: proposal.summary.maximum_depth,
        },
        change: OrganizationProposalChangeDto {
            destinations_changed: proposal.diff.destinations_changed,
            files_added: proposal.diff.files_added,
            conflicts_resolved: proposal.diff.conflicts_resolved,
            moved_to_review: proposal.diff.moved_to_review,
        },
        nodes: proposal
            .nodes
            .into_iter()
            .map(virtual_proposal_node_dto)
            .collect(),
        operations: proposal
            .operations
            .into_iter()
            .map(organization_operation_dto)
            .collect(),
    }
}

fn virtual_proposal_node_dto(node: VirtualProposalNode) -> VirtualProposalNodeDto {
    VirtualProposalNodeDto {
        id: node.id.to_string(),
        parent_id: node.parent_id.map(|id| id.to_string()),
        kind: match node.kind {
            domain::VirtualNodeKind::Root => "ROOT",
            domain::VirtualNodeKind::Folder => "FOLDER",
            domain::VirtualNodeKind::File => "FILE",
        }
        .to_owned(),
        name: node.name,
        virtual_path: node.virtual_path,
        operation_id: node.operation_id.map(|id| id.to_string()),
        child_count: node.child_count,
        needs_review_count: node.needs_review_count,
        conflict_count: node.conflict_count,
    }
}

fn execution_detail_dto(detail: ExecutionDetail) -> ExecutionDetailDto {
    ExecutionDetailDto {
        session: execution_session_dto(detail.session),
        operations: detail
            .operations
            .into_iter()
            .map(|operation| ExecutionOperationDto {
                id: operation.id.to_string(),
                proposal_operation_id: operation
                    .proposal_operation_id
                    .map(|value| value.to_string()),
                kind: operation.kind.database_name().to_ascii_uppercase(),
                source_relative_path: operation.source_relative_path,
                destination_relative_path: operation.destination_relative_path,
                sequence: operation.sequence,
                status: operation.status.database_name().to_ascii_uppercase(),
                reason: operation.reason,
                error_code: operation.error_code,
                error_message: operation.error_message,
            })
            .collect(),
    }
}

fn execution_session_dto(session: ExecutionSession) -> ExecutionSessionDto {
    ExecutionSessionDto {
        id: session.id.to_string(),
        plan_id: session.plan_id.to_string(),
        proposal_id: session.proposal_id.to_string(),
        proposal_revision: session.proposal_revision,
        workspace_id: session.workspace_id.to_string(),
        status: session.status.database_name().to_ascii_uppercase(),
        recovery_state: session.recovery_state.database_name().to_ascii_uppercase(),
        plan_digest: session.plan_digest_hex,
        approved_operation_count: session.approval.operation_count,
        consent_state: session.consent.state.database_name().to_ascii_uppercase(),
        consent_issued_at_unix_ms: session.consent.issued_at_unix_ms,
        consent_expires_at_unix_ms: session.consent.expires_at_unix_ms,
        consent_attested_at_unix_ms: session.consent.attested_at_unix_ms,
        consent_consumed_at_unix_ms: session.consent.consumed_at_unix_ms,
        consent_invalidated_at_unix_ms: session.consent.invalidated_at_unix_ms,
        summary: ExecutionSummaryDto {
            affected_files: session.summary.affected_files,
            folders_to_create: session.summary.folders_to_create,
            files_to_move: session.summary.files_to_move,
            files_to_rename: session.summary.files_to_rename,
            files_unchanged: session.summary.files_unchanged,
            conflicts: session.summary.conflicts,
            needs_review: session.summary.needs_review,
            preflight_ok: session.summary.preflight_ok,
            applied: session.summary.applied,
            blocked: session.summary.blocked,
            skipped: session.summary.skipped,
            failed: session.summary.failed,
            rolled_back: session.summary.rolled_back,
            rollback_blocked: session.summary.rollback_blocked,
            rollback_failed: session.summary.rollback_failed,
        },
        current_operation: session.current_operation,
        rollback_available: session.rollback_available,
        confirmation_phrase_required: session.confirmation_phrase_required,
        created_at: session.created_at,
        approved_at: session.approved_at,
        started_at: session.started_at,
        completed_at: session.completed_at,
        rolled_back_at: session.rolled_back_at,
        error: session.error,
    }
}

fn execution_progress_dto(progress: ExecutionProgress) -> ExecutionProgressDto {
    ExecutionProgressDto {
        execution_id: progress.execution_id.to_string(),
        status: progress.status.database_name().to_ascii_uppercase(),
        completed: progress.completed,
        total: progress.total,
        applied: progress.applied,
        blocked: progress.blocked,
        skipped: progress.skipped,
        failed: progress.failed,
        current: progress.current,
    }
}

fn recovery_assessment_dto(assessment: RecoveryAssessment) -> RecoveryAssessmentDto {
    RecoveryAssessmentDto {
        execution_id: assessment.execution_id.to_string(),
        state: assessment.state.database_name().to_ascii_uppercase(),
        affected_count: assessment.affected_count,
        not_started: assessment.not_started,
        applied: assessment.applied,
        ambiguous: assessment.ambiguous,
        verified_applied_items: assessment
            .verified_applied_items
            .into_iter()
            .map(recovery_item_dto)
            .collect(),
        verified_not_started_items: assessment
            .verified_not_started_items
            .into_iter()
            .map(recovery_item_dto)
            .collect(),
        ambiguous_items: assessment
            .ambiguous_items
            .into_iter()
            .map(recovery_item_dto)
            .collect(),
        rollback_available: assessment.rollback_available,
        executor_sessions: assessment
            .executor_sessions
            .into_iter()
            .map(|session| ExecutorSessionFactDto {
                session_id: session.session_id,
                execution_id: session.execution_id.to_string(),
                plan_id: session.plan_id.to_string(),
                purpose: session.purpose.database_name().to_ascii_uppercase(),
                coordinator_pid: session.coordinator_pid,
                child_pid: session.child_pid,
                opened_at_unix_ms: session.opened_at_unix_ms,
            })
            .collect(),
        executor_requests: assessment
            .executor_requests
            .into_iter()
            .map(|request| ExecutorRequestFactDto {
                request_id: request.request_id,
                session_id: request.session_id,
                operation_id: request.operation_id.to_string(),
                direction: request.direction.database_name().to_ascii_uppercase(),
                request_sequence: request.request_sequence,
                intent_event_sequence: request.intent_event_sequence,
                outcome_class: request.outcome_class,
                attempt_count: request.attempt_count,
                error_class: request.error_class,
                state: request.state.database_name().to_ascii_uppercase(),
            })
            .collect(),
        journal_diagnostics: JournalDiagnosticStateDto {
            locked: assessment.journal_diagnostics.locked,
            diagnostics: assessment
                .journal_diagnostics
                .diagnostics
                .into_iter()
                .map(journal_diagnostic_dto)
                .collect(),
        },
        message: assessment.message,
    }
}

fn recovery_item_dto(item: domain::RecoveryItem) -> RecoveryItemDto {
    RecoveryItemDto {
        operation_id: item.operation_id.to_string(),
        direction: item.direction.database_name().to_ascii_uppercase(),
        item: item.item,
        reason: item.reason,
    }
}

fn journal_diagnostic_dto(diagnostic: domain::JournalDiagnostic) -> JournalDiagnosticDto {
    JournalDiagnosticDto {
        scope: match diagnostic.scope {
            domain::JournalDiagnosticScope::Database => "database",
            domain::JournalDiagnosticScope::External => "external",
        }
        .to_owned(),
        execution_id: diagnostic.execution_id.map(|id| id.to_string()),
        code: diagnostic.code,
        message: diagnostic.message,
        detected_at_unix_ms: diagnostic.detected_at_unix_ms,
        recovery_available: diagnostic.recovery_available,
        rollback_available: diagnostic.rollback_available,
    }
}

fn organization_operation_dto(
    operation: OrganizationProposalOperation,
) -> OrganizationOperationDto {
    let proposed_relative_path = operation.proposed_relative_path();
    OrganizationOperationDto {
        id: operation.id.to_string(),
        file_id: operation.file_id.to_string(),
        file_version_id: operation.file_version_id.to_string(),
        source_relative_path: operation.source.relative_path,
        source_name: operation.source_name,
        source_hash: operation.source.content_hash,
        source_byte_size: operation.source.byte_size,
        source_modified_at: operation.source.modified_at,
        machine_destination: operation.machine_destination,
        machine_name: operation.machine_name,
        proposed_destination: operation.proposed_destination,
        proposed_name: operation.proposed_name,
        proposed_relative_path,
        operation_kind: operation
            .operation_kind
            .database_name()
            .to_ascii_uppercase(),
        confidence_score: operation.confidence_score,
        confidence_level: operation
            .confidence_level
            .database_name()
            .to_ascii_uppercase(),
        reasons: operation
            .reasons
            .into_iter()
            .map(|reason| OrganizationReasonDto {
                code: reason.code.to_ascii_uppercase(),
                explanation: reason.explanation,
                evidence_references: reason.evidence_references,
            })
            .collect(),
        conflict_state: operation
            .conflict_state
            .database_name()
            .to_ascii_uppercase(),
        needs_review: operation.needs_review,
        stale: operation.stale,
        user_override: operation.user_override,
        disruption_score: operation.disruption_score,
        proposed_path_length: operation.proposed_path_length,
        proposed_depth: operation.proposed_depth,
        semantic_context: operation.semantic_context.to_ascii_uppercase(),
        document_type: operation.document_type.to_ascii_uppercase(),
        customer_name: operation.customer_name,
        supplier_name: operation.supplier_name,
        project_name: operation.project_name,
        duplicate_group_id: operation.duplicate_group_id,
        duplicate_canonical: operation.duplicate_canonical,
    }
}

fn semantic_detail_dto(detail: SemanticAnalysisDetailRecord) -> SemanticAnalysisDetailDto {
    SemanticAnalysisDetailDto {
        analysis_id: detail.analysis_id,
        status: detail.status.to_ascii_uppercase(),
        analyzer_id: detail.analyzer_id,
        analyzer_version: detail.analyzer_version,
        provider_id: detail.provider_id,
        provider_version: detail.provider_version,
        schema_version: detail.schema_version,
        input_quality: detail.input_quality,
        input_quality_status: detail.input_quality_status.to_ascii_uppercase(),
        input_quality_reasons: detail
            .input_quality_reasons
            .into_iter()
            .map(|reason| reason.to_ascii_uppercase())
            .collect(),
        language: detail.language,
        analyzed_at: detail.analyzed_at,
        fields: detail.fields.into_iter().map(semantic_field_dto).collect(),
        entities: detail
            .entities
            .into_iter()
            .map(semantic_entity_dto)
            .collect(),
    }
}

fn semantic_field_dto(field: SemanticFieldRecord) -> SemanticFieldDto {
    SemanticFieldDto {
        field_id: field.field_id,
        field_key: field.field_key.to_ascii_uppercase(),
        value_kind: field.value_kind.map(|kind| kind.to_ascii_uppercase()),
        display_value: field.display_value,
        machine_display_value: field.machine_display_value,
        normalized_value: field.normalized_value,
        confidence: field.confidence,
        status: field.status.to_ascii_uppercase(),
        source_method: field.source_method.to_ascii_uppercase(),
        analyzer_version: field.analyzer_version,
        value_source: field.value_source.to_ascii_uppercase(),
        user_state: field.user_state.map(|state| state.to_ascii_uppercase()),
        evidence: field
            .evidence
            .into_iter()
            .map(semantic_evidence_dto)
            .collect(),
        candidates: field
            .candidates
            .into_iter()
            .map(semantic_candidate_dto)
            .collect(),
    }
}

fn semantic_candidate_dto(candidate: SemanticCandidateValueRecord) -> SemanticCandidateValueDto {
    SemanticCandidateValueDto {
        display_value: candidate.display_value,
        normalized_value: candidate.normalized_value,
        confidence: candidate.confidence,
        status: candidate.status.to_ascii_uppercase(),
        source_method: candidate.source_method.to_ascii_uppercase(),
        evidence: candidate
            .evidence
            .into_iter()
            .map(semantic_evidence_dto)
            .collect(),
    }
}

fn semantic_entity_dto(entity: SemanticEntityRecord) -> SemanticEntityDto {
    SemanticEntityDto {
        entity_id: entity.entity_id,
        candidate_key: entity.candidate_key,
        entity_type: entity.entity_type.to_ascii_uppercase(),
        original_value: entity.original_value,
        normalized_value: entity.normalized_value,
        confidence: entity.confidence,
        status: entity.status.to_ascii_uppercase(),
        source_method: entity.source_method.to_ascii_uppercase(),
        evidence: entity
            .evidence
            .into_iter()
            .map(semantic_evidence_dto)
            .collect(),
    }
}

fn semantic_evidence_dto(evidence: SemanticEvidenceRecord) -> SemanticEvidenceDto {
    SemanticEvidenceDto {
        evidence_type: evidence.evidence_type.to_ascii_uppercase(),
        exact_text: evidence.exact_text,
        start_offset: evidence.start_offset,
        end_offset: evidence.end_offset,
        page_number: evidence.page_number,
        sheet_name: evidence.sheet_name,
        slide_number: evidence.slide_number,
        source_label: evidence.source_label,
        explanation: evidence.explanation,
        extraction_method: evidence.extraction_method.to_ascii_uppercase(),
        analyzer_version: evidence.analyzer_version,
    }
}

fn semantic_correction_dto(correction: SemanticCorrectionRecord) -> SemanticCorrectionDto {
    SemanticCorrectionDto {
        correction_id: correction.correction_id,
        file_id: correction.file_id,
        field_key: correction.field_key.to_ascii_uppercase(),
        correction_state: correction.correction_state.to_ascii_uppercase(),
        value_kind: correction.value_kind.to_ascii_uppercase(),
        display_value: correction.display_value,
        normalized_value: correction.normalized_value,
        created_at: correction.created_at,
        updated_at: correction.updated_at,
    }
}

fn extraction_retry_dto(outcome: ExtractionRetryOutcome) -> ExtractionRetryDto {
    ExtractionRetryDto {
        review_id: outcome.review_id,
        batch_id: outcome.batch_id,
        file_id: outcome.file_id,
        status: match outcome.status {
            ExtractionRetryStatus::Succeeded => "SUCCEEDED",
            ExtractionRetryStatus::Partial => "PARTIAL",
            ExtractionRetryStatus::Failed => "FAILED",
            ExtractionRetryStatus::Unavailable => "UNAVAILABLE",
            ExtractionRetryStatus::Cancelled => "CANCELLED",
        }
        .to_owned(),
        extraction_status: outcome
            .extraction_status
            .map(|status| status.to_ascii_uppercase()),
        message: outcome.message,
    }
}

const fn content_phase_name(phase: ContentAnalysisPhase) -> &'static str {
    match phase {
        ContentAnalysisPhase::Running => "RUNNING",
        ContentAnalysisPhase::Completed => "COMPLETED",
        ContentAnalysisPhase::Cancelled => "CANCELLED",
        ContentAnalysisPhase::Failed => "FAILED",
    }
}

const fn identity_phase_name(phase: IdentityResolutionPhase) -> &'static str {
    match phase {
        IdentityResolutionPhase::Running => "RUNNING",
        IdentityResolutionPhase::Completed => "COMPLETED",
        IdentityResolutionPhase::Cancelled => "CANCELLED",
        IdentityResolutionPhase::Failed => "FAILED",
    }
}

const fn semantic_phase_name(phase: SemanticAnalysisPhase) -> &'static str {
    match phase {
        SemanticAnalysisPhase::Running => "RUNNING",
        SemanticAnalysisPhase::Completed => "COMPLETED",
        SemanticAnalysisPhase::Cancelled => "CANCELLED",
        SemanticAnalysisPhase::Failed => "FAILED",
    }
}

const fn phase_name(phase: ScanPhase) -> &'static str {
    match phase {
        ScanPhase::Discovering => "DISCOVERING",
        ScanPhase::Inspecting => "INSPECTING",
        ScanPhase::Hashing => "HASHING",
        ScanPhase::Persisting => "PERSISTING",
        ScanPhase::Completed => "COMPLETED",
        ScanPhase::Cancelled => "CANCELLED",
    }
}

struct InitializedApplication {
    scanner: Arc<ScannerApplicationService>,
    execution: Arc<ExecutionApplicationService>,
    embedding_provider: Arc<OnnxLocalEmbeddingProvider>,
}

#[allow(clippy::too_many_lines)]
fn initialize_application(app: &tauri::App) -> Result<InitializedApplication, io::Error> {
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| io::Error::other(error.to_string()))?;
    fs::create_dir_all(&data_dir)?;
    let secret_store = OsSecretStore::new(ROOT_AUTHORITY_SECRET_SERVICE);
    let key_name = "catalog-database-key-v1";
    let stored_key = tauri::async_runtime::block_on(secret_store.load(key_name))
        .map_err(|error| io::Error::other(error.to_string()))?;
    let key = if let Some(bytes) = stored_key {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| io::Error::other("invalid database key length"))?;
        DatabaseKey::from_bytes(bytes)
    } else {
        let key = DatabaseKey::generate();
        tauri::async_runtime::block_on(
            secret_store.store(key_name, &key.expose_for_secret_store()),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        key
    };
    let mut executor_root_authority = load_or_create_executor_root(&secret_store)?;
    let database_path = data_dir.join("catalog.db");
    let journal_path = data_dir.join("operation-recovery.jsonl.enc");
    let mut execution_key_material = key.expose_for_secret_store();
    let journal_key = JournalKey::derive(&execution_key_material);
    let consent_authority = ExecutionConsentAuthorityKey::derive(&executor_root_authority);
    execution_key_material.fill(0);
    let database = Arc::new(
        Database::open(&database_path, &key)
            .map_err(|error| io::Error::other(error.to_string()))?,
    );

    #[cfg(target_os = "windows")]
    let concrete_platform = Arc::new(platform_windows::WindowsPlatform);
    #[cfg(target_os = "macos")]
    let concrete_platform = Arc::new(platform_macos::MacOsPlatform);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    return Err(io::Error::other(
        "this development build supports Windows and macOS only",
    ));
    let os_platform: Arc<dyn ReadOnlyPlatform> = concrete_platform.clone();
    let apply_gate = ApplyGate::for_approved_execution_host();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let (executor_client, sidecar_path, apply_gate): (
        Arc<dyn ApprovedExecutorClient>,
        Option<std::path::PathBuf>,
        ApplyGate,
    ) = {
        let resource_dir = app
            .path()
            .resource_dir()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let current_executable = std::env::current_exe()?;
        match executor_client::resolve_packaged_sidecar(&resource_dir, &current_executable)
            .and_then(|sidecar| {
                executor_client::ProcessApprovedExecutorClient::new(
                    sidecar.clone(),
                    executor_root_authority,
                )
                .map(|client| (client, sidecar))
            }) {
            Ok((client, sidecar)) => (Arc::new(client), Some(sidecar), apply_gate),
            Err(error) => {
                let _ = error;
                (
                    Arc::new(UnavailableApprovedExecutorClient),
                    None,
                    ApplyGate {
                        enabled: false,
                        reason:
                            "L’application des fichiers n’est pas disponible dans cette session."
                                .to_owned(),
                    },
                )
            }
        }
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let (executor_client, sidecar_path, apply_gate): (
        Arc<dyn ApprovedExecutorClient>,
        Option<std::path::PathBuf>,
        ApplyGate,
    ) = (
        Arc::new(UnavailableApprovedExecutorClient),
        None,
        apply_gate,
    );
    executor_root_authority.fill(0);
    let detected_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    let journal = FileJournal::open_or_locked(&journal_path, journal_key, detected_at_unix_ms);
    let mut protected_paths = vec![data_dir.clone(), database_path, journal_path];
    if let Ok(executable) = std::env::current_exe() {
        protected_paths.push(executable);
    }
    if let Some(sidecar_path) = sidecar_path {
        protected_paths.push(sidecar_path);
    }
    let mut policy = ExecutionSafetyPolicy::default().with_protected_paths(protected_paths);
    #[cfg(target_os = "macos")]
    {
        policy.allow_qualified_case_only_rename = true;
    }
    let execution = Arc::new(
        ExecutionApplicationService::new(
            database.clone(),
            os_platform.clone(),
            executor_client,
            journal,
            apply_gate,
            policy,
            consent_authority,
        )
        .map_err(|error| io::Error::other(error.to_string()))?,
    );

    let model_root = data_dir.join("models").join("embeddings");
    fs::create_dir_all(&model_root)?;
    let embedding_provider = Arc::new(
        OnnxLocalEmbeddingProvider::new(&model_root)
            .map_err(|error| io::Error::other(error.to_string()))?,
    );
    // Best-effort verify of previously installed assets; never download on startup.
    let _ = embedding_provider.verify_installed();
    let ann_root = model_root.join("ann");
    fs::create_dir_all(&ann_root)?;
    let scanner = Arc::new(ScannerApplicationService::new_with_all_engines_and_ann(
        database,
        os_platform,
        Arc::new(LocalExtractionEngine::local_default()),
        Arc::new(DeterministicSemanticProvider::default()),
        embedding_provider.clone() as Arc<dyn LocalEmbeddingProvider>,
        Some(ann_root),
    ));

    Ok(InitializedApplication {
        scanner,
        execution,
        embedding_provider,
    })
}

fn load_or_create_executor_root(secret_store: &OsSecretStore) -> Result<[u8; 32], io::Error> {
    let root = if let Some(bytes) =
        tauri::async_runtime::block_on(secret_store.load(ROOT_AUTHORITY_SECRET_NAME))
            .map_err(|error| io::Error::other(error.to_string()))?
    {
        bytes
            .try_into()
            .map_err(|_| io::Error::other("invalid operation executor root secret length"))?
    } else {
        let mut generated = [0_u8; 32];
        getrandom::fill(&mut generated).map_err(|error| io::Error::other(error.to_string()))?;
        tauri::async_runtime::block_on(secret_store.store(ROOT_AUTHORITY_SECRET_NAME, &generated))
            .map_err(|error| io::Error::other(error.to_string()))?;
        generated
    };
    #[cfg(target_os = "macos")]
    privacy::persist_shared_executor_root(ROOT_AUTHORITY_SECRET_SERVICE, &root)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(root)
}

fn start_monitoring_loop(
    service: Arc<ScannerApplicationService>,
    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
) -> Result<(), io::Error> {
    if let Err(error) = service.restore_monitoring_runtime() {
        // Monitoring is proposal-only. A restore failure must not prevent launch.
        eprintln!("monitoring restore failed; continuing without watchers: {error}");
    }
    thread::Builder::new()
        .name("local-proposal-monitor".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(2));
                let Ok(workspace_ids) = service.monitoring_workspace_ids() else {
                    continue;
                };
                for workspace_id in workspace_ids {
                    let registry_key = workspace_id.to_string();
                    let cancellation = Arc::new(AtomicBool::new(false));
                    let registered = cancellations.lock().is_ok_and(|mut registry| {
                        if registry.contains_key(&registry_key) {
                            false
                        } else {
                            registry.insert(registry_key.clone(), cancellation.clone());
                            true
                        }
                    });
                    if !registered {
                        continue;
                    }
                    let _ = service.run_monitoring_cycle(workspace_id, &|| {
                        cancellation.load(Ordering::Relaxed)
                    });
                    if let Ok(mut registry) = cancellations.lock() {
                        registry.remove(&registry_key);
                    }
                }
            }
        })
        .map(|_| ())
}

#[cfg(debug_assertions)]
fn run_packaged_nsopenpanel_qualification(
    app: &tauri::App,
    scanner: &ScannerApplicationService,
) -> Result<(), io::Error> {
    let qualify_root = std::env::var("WORKING_NAME_QUALIFY_NSOPENPANEL").ok();
    let qualify_relaunch = std::env::var("WORKING_NAME_QUALIFY_RELAUNCH").is_ok();
    if qualify_root.is_none() && !qualify_relaunch {
        return Ok(());
    }
    let data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| io::Error::other(error.to_string()))?;
    fs::create_dir_all(&data_dir)?;
    if let Some(hint) = qualify_root {
        let dialog = rfd::FileDialog::new()
            .set_title("Sélectionner le dossier exact à analyser en lecture seule")
            .set_directory(&hint);
        let selected = dialog.pick_folder();
        let mut write_ok = false;
        let mut read_ok = false;
        let selected_path = selected.as_ref().map(|path| path.display().to_string());
        if let Some(path) = selected.as_ref() {
            let sandbox_ok = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("supremacy-m18-step2-sandbox-"));
            if sandbox_ok {
                let marker = path.join(".working-name-qualify-access");
                write_ok = fs::write(&marker, b"ok").is_ok();
                read_ok = fs::read(&marker).ok().as_deref() == Some(b"ok");
                let _ = fs::remove_file(&marker);
            }
        }
        let body = serde_json::json!({
            "panel": "NSOpenPanel",
            "title": "Sélectionner le dossier exact à analyser en lecture seule",
            "hint": hint,
            "selected": selected_path,
            "granted": selected.is_some(),
            "registered": selected.is_some(),
            "write_ok": write_ok,
            "read_ok": read_ok,
            "native_prompt": "not_recorded_by_hook",
        });
        fs::write(data_dir.join("qualify-nsopenpanel.json"), body.to_string())?;
    }
    if qualify_relaunch {
        let prior = fs::read_to_string(data_dir.join("qualify-nsopenpanel.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let selected = prior
            .as_ref()
            .and_then(|value| value.get("selected"))
            .and_then(|value| value.as_str())
            .map(PathBuf::from);
        let sandbox_ok = selected.as_ref().is_some_and(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("supremacy-m18-step2-sandbox-"))
        });
        let readable = sandbox_ok
            && selected
                .as_ref()
                .is_some_and(|path| fs::read_dir(path).is_ok());
        let writable = sandbox_ok
            && selected.as_ref().is_some_and(|path| {
                let marker = path.join(".working-name-qualify-relaunch");
                let wrote = fs::write(&marker, b"ok").is_ok();
                let _ = fs::remove_file(&marker);
                wrote
            });
        let session = scanner
            .restore_workspace_session()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let body = serde_json::json!({
            "selected_from_prior_panel": selected.as_ref().map(|path| path.display().to_string()),
            "sandbox_path_accepted": sandbox_ok,
            "readable": readable,
            "writable": writable,
            "workspace_restored": session.is_some(),
            "catalog_root_untouched": true,
        });
        fs::write(data_dir.join("qualify-relaunch.json"), body.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn run_packaged_folder_access_diagnose() -> bool {
    let requested = std::env::args().any(|arg| arg == "--diagnose-folder-access")
        || std::env::var_os("ZEMO_DIAGNOSE_FOLDER_ACCESS").is_some();
    if !requested {
        return false;
    }
    let mut folders = Vec::new();
    for kind in folder_access::UserContentKind::all()
        .into_iter()
        .filter(|kind| kind.recommended())
    {
        let probe = enrich_probe(folder_access::probe_kind(kind, None));
        folders.push(serde_json::json!({
            "logical_name": probe.logical_name,
            "resolved_path": probe.resolved_path,
            "canonical_path": probe.canonical_path,
            "exists": probe.exists,
            "is_dir": probe.is_dir,
            "readable": probe.readable,
            "writable": probe.writable,
            "raw_os_error": probe.raw_os_error,
            "error_kind": probe.error_kind,
            "platform_error": probe.platform_error,
            "failed_stage": probe.failed_stage,
            "inspect_result": probe.inspect_result,
            "access_state": probe.access_state,
            "human_status": probe.human_status,
            "technical_details": probe.technical_details,
        }));
    }
    let exe = std::env::current_exe().ok();
    let plist_path = exe.as_ref().and_then(|path| {
        path.parent()
            .and_then(std::path::Path::parent)
            .map(|contents| contents.join("Info.plist"))
    });
    let plist_text = plist_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    let report = serde_json::json!({
        "exe": exe.as_ref().map(|path| path.display().to_string()),
        "info_plist": plist_path.as_ref().map(|path| path.display().to_string()),
        "info_plist_keys": {
            "NSDesktopFolderUsageDescription": plist_text.contains("NSDesktopFolderUsageDescription"),
            "NSDocumentsFolderUsageDescription": plist_text.contains("NSDocumentsFolderUsageDescription"),
            "NSDownloadsFolderUsageDescription": plist_text.contains("NSDownloadsFolderUsageDescription"),
            "NSPicturesFolderUsageDescription": plist_text.contains("NSPicturesFolderUsageDescription"),
            "NSMoviesFolderUsageDescription": plist_text.contains("NSMoviesFolderUsageDescription"),
        },
        "folders": folders,
    });
    let encoded = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_owned());
    let tmp = std::env::temp_dir().join("zemo-packaged-folder-access.json");
    let _ = fs::write(&tmp, &encoded);
    if let Some(home) = dirs::home_dir() {
        let logs = home.join("Library/Logs");
        let _ = fs::create_dir_all(&logs);
        let _ = fs::write(logs.join("ZEMO-folder-access.json"), &encoded);
    }
    eprintln!("{encoded}");
    true
}

pub fn run() {
    if run_packaged_folder_access_diagnose() {
        return;
    }
    let result = tauri::Builder::default()
        .setup(|app| {
            let services = initialize_application(app)?;
            let monitoring_cancellations = Arc::new(Mutex::new(HashMap::new()));
            start_monitoring_loop(services.scanner.clone(), monitoring_cancellations.clone())?;
            #[cfg(debug_assertions)]
            run_packaged_nsopenpanel_qualification(app, &services.scanner)?;
            app.manage(ManagedScanner {
                service: services.scanner,
                execution_service: services.execution,
                embedding_provider: services.embedding_provider,
                model_install_cancel: Arc::new(AtomicBool::new(false)),
                cancellations: Arc::new(Mutex::new(HashMap::new())),
                content_cancellations: Arc::new(Mutex::new(HashMap::new())),
                semantic_cancellations: Arc::new(Mutex::new(HashMap::new())),
                identity_cancellations: Arc::new(Mutex::new(HashMap::new())),
                proposal_cancellations: Arc::new(Mutex::new(HashMap::new())),
                retry_cancellations: Arc::new(Mutex::new(HashMap::new())),
                monitoring_cancellations,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_system_status,
            restore_workspace_session,
            get_monitoring_dashboard,
            pause_monitoring,
            resume_monitoring,
            set_monitored_folder_enabled,
            add_monitoring_exclusion,
            remove_monitoring_exclusion,
            run_monitoring_cycle,
            cancel_monitoring,
            create_workspace,
            select_and_register_root,
            list_user_content_locations,
            probe_user_content_access,
            authorize_user_content_folder,
            register_user_content_root,
            scan_workspace,
            cancel_scan,
            list_scan_files,
            list_scan_duplicates,
            list_scan_errors,
            analyze_content,
            cancel_content_analysis,
            analyze_semantics,
            cancel_semantic_analysis,
            resolve_identities,
            cancel_identity_resolution,
            list_identity_review_groups,
            get_identity_detail,
            decide_identity_candidate,
            merge_identities,
            unlink_identity_occurrence,
            generate_organization_proposal,
            cancel_organization_proposal,
            get_latest_organization_proposal,
            get_organization_proposal,
            set_organization_proposal_override,
            set_organization_proposal_status,
            refresh_organization_proposal_drift,
            get_rules_preferences,
            create_local_rule,
            update_local_rule,
            set_local_rule_enabled,
            delete_local_rule,
            reorder_local_rules,
            store_local_organization_preferences,
            accept_local_rule_suggestion,
            dismiss_local_rule_suggestion,
            recompute_rules_proposal,
            prepare_execution,
            approve_execution,
            start_execution,
            pause_execution,
            cancel_execution,
            get_execution_status,
            list_execution_history,
            rollback_execution,
            recover_execution,
            list_content_results,
            search_local_files,
            get_embedding_model_status,
            activate_local_embedding_model,
            cancel_local_embedding_model_install,
            retry_local_embedding_model,
            remove_local_embedding_model,
            rebuild_semantic_ann_index,
            list_review_items,
            update_review_item,
            get_file_detail,
            store_semantic_correction,
            retry_extraction,
            cancel_extraction_retry,
        ])
        .run(tauri::generate_context!());
    if result.is_err() {
        eprintln!("failed to run the local safe scanner");
    }
}
