//! Application use cases and orchestration.

mod approved_executor;
mod content;
mod execution;
mod identity;
#[path = "search.rs"]
mod local_search;
mod monitoring;
mod proposal;
mod review;
mod rules;
mod scanner;
mod semantic;

pub use approved_executor::{
    ApprovedExecutorClient, ApprovedExecutorError, ApprovedExecutorSession, ExecutorDispatchResult,
    UnavailableApprovedExecutorClient, executor_nonce_hash, executor_response_digest,
    fresh_request_nonce, prepare_executor_request_identity, synthetic_executor_session_identity,
};
pub use content::{
    ContentAnalysisPhase, ContentAnalysisProgress, ExtractionRetryOutcome, ExtractionRetryStatus,
};
pub use execution::{
    ExecutionApplicationService, ExecutionConsentAuthorityKey, ExecutionSystemStatus,
    ExecutionVerificationProgress, NativeExecutionConfirmation,
};
pub use identity::{IdentityResolutionPhase, IdentityResolutionProgress};
pub use monitoring::{MonitoringDashboard, RestoredWorkspaceSession};
pub use organizer::{
    OrganizationBuildOutcome, ProposalBuildPhase, ProposalBuildProgress, ProposalRebuildMode,
};
pub use rules::RulesPreferencesState;
pub use scanner::{ScannerApplicationService, ScannerSystemStatus};
pub use semantic::{SemanticAnalysisPhase, SemanticAnalysisProgress, SemanticCorrectionAction};

use catalog::{CatalogScanner, CatalogedFile, ScanOutput, ScanPolicy};
use domain::{
    ActorId, ApprovalReceipt, ArtifactId, DisplayLabel, EvidenceLocator, EvidenceRef, FileId,
    FileVersionId, PlanId, ProposalAction, ProposalId, ProposalItem, ProposalItemId,
    ProposalRevision, ProposalSimulation, ReviewReason, ReviewState, RootId, SealedPlan,
    SearchDocument, SearchHit, SearchResponse, WorkspaceId,
};
use extraction::{DeterministicExtractor, ExtractionEngine, ExtractionRequest};
use knowledge::{LocalSemanticAnalyzer, SemanticDocument};
use operations::{ApplyGate, OperationsError, compile_plan};
use organizer::{OrganizationEngine, OrganizationInput, OrganizationPolicy};
use parking_lot::Mutex;
use persistence::{
    AnalysisCandidate, Database, PersistenceError, RootRecord, ScanRecord, WorkspaceRecord,
};
use platform::{ChangeMonitor, PlatformError, PollingChangeMonitor, ReadOnlyPlatform};
use search::{HybridSearchEngine, SearchConfig, tokenize};
use simulation::{SimulationFile, SimulationOutcome, VirtualFileSystem};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_EXTRACTION_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXTRACTED_CHARS: usize = 2_000_000;

pub struct ApplicationService {
    database: Arc<Database>,
    read_only_platform: Arc<dyn ReadOnlyPlatform>,
    scanner: CatalogScanner,
    extractor: Arc<dyn ExtractionEngine>,
    semantic_analyzer: LocalSemanticAnalyzer,
    organizer: OrganizationEngine,
    monitor: Arc<dyn ChangeMonitor>,
    pending_recovery: AtomicUsize,
    state: Mutex<ApplicationState>,
}

impl std::fmt::Debug for ApplicationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApplicationService")
            .field("database", &self.database)
            .field("apply_gate", &ApplyGate::for_in_process_host())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct ApplicationState {
    scans: HashMap<WorkspaceId, RuntimeScan>,
    proposals: HashMap<ProposalId, RuntimeProposal>,
    plans: HashMap<PlanId, RuntimePlan>,
}

#[derive(Debug, Clone)]
struct RuntimeScan {
    record: ScanRecord,
    root: RootRecord,
    files: Vec<RuntimeFile>,
}

#[derive(Debug, Clone)]
struct RuntimeFile {
    cataloged: CatalogedFile,
    artifact_id: ArtifactId,
    extracted_text: String,
    semantics: SemanticDocument,
    anomalies: Vec<ReviewReason>,
}

#[derive(Debug, Clone)]
struct RuntimeProposal {
    revision: ProposalRevision,
    simulation: Option<SimulationOutcome>,
}

#[derive(Debug, Clone)]
pub struct PlanView {
    pub plan: SealedPlan,
    pub approved_at: Option<String>,
}

#[derive(Debug, Clone)]
struct RuntimePlan {
    view: PlanView,
    approval: Option<ApprovalReceipt>,
}

#[derive(Debug, Clone)]
pub struct SystemStatus {
    pub local_first: bool,
    pub read_only_scan: bool,
    pub network_disabled: bool,
    pub apply_gate: ApplyGate,
    pub version: String,
    pub recovery_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitoringSummary {
    pub change_hints: usize,
    pub reconciliation_required: bool,
    pub auto_apply: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("invalid workspace name")]
    InvalidWorkspaceName,
    #[error("record was not found")]
    NotFound,
    #[error("the proposal still contains unreviewed or blocked mutations")]
    ReviewIncomplete,
    #[error("the simulation contains blockers")]
    SimulationBlocked,
    #[error("plan digest is invalid")]
    InvalidDigest,
    #[error("persistence failed: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("platform failed: {0}")]
    Platform(#[from] PlatformError),
    #[error("local filesystem inspection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("catalog failed: {0}")]
    Catalog(#[from] catalog::CatalogError),
    #[error("operation failed: {0}")]
    Operations(#[from] OperationsError),
    #[error("semantic analysis failed: {0}")]
    Knowledge(#[from] knowledge::KnowledgeError),
    #[error("safe local content extraction failed: {0}")]
    ContentExtraction(String),
    #[error("semantic correction is invalid")]
    InvalidSemanticCorrection,
    #[error("semantic worker returned an invalid result")]
    InvalidSemanticResult,
    #[error("identity decision input is invalid")]
    InvalidIdentityDecision,
    #[error("organization proposal input is invalid or unsafe")]
    InvalidOrganizationProposal,
    #[error("monitoring request is invalid or outside its registered root")]
    InvalidMonitoringRequest,
    #[error("local rule input is invalid or unsafe")]
    InvalidRule,
    #[error("filesystem execution requires an explicitly approved current proposal")]
    ExecutionApprovalRequired,
    #[error("another filesystem execution or recovery blocks this request")]
    ExecutionAlreadyActive,
    #[error("filesystem execution state or plan integrity is invalid")]
    InvalidExecution,
    #[error("filesystem execution recovery must be resolved first")]
    ExecutionRecoveryRequired,
    #[error("the authenticated execution journal is locked for diagnostics")]
    JournalLocked,
    #[error("no approved operation passed the fail-closed preflight")]
    ExecutionPreflightBlocked,
    #[error("the native execution consent expired before start authorization")]
    ExecutionConsentExpired,
    #[error("the user declined the native execution confirmation")]
    ExecutionConfirmationDeclined,
    #[error("execution safety policy refused the request: {0}")]
    ExecutionSafety(#[from] operations::SafetyPolicyError),
    #[error(transparent)]
    ApprovedExecutor(#[from] ApprovedExecutorError),
}

impl ApplicationService {
    #[must_use]
    pub fn new(database: Arc<Database>, read_only_platform: Arc<dyn ReadOnlyPlatform>) -> Self {
        Self::new_with_extractor(
            database,
            read_only_platform,
            Arc::new(DeterministicExtractor),
        )
    }

    #[must_use]
    pub fn new_with_extractor(
        database: Arc<Database>,
        read_only_platform: Arc<dyn ReadOnlyPlatform>,
        extractor: Arc<dyn ExtractionEngine>,
    ) -> Self {
        let scanner = CatalogScanner::new(read_only_platform.clone());
        Self {
            database,
            read_only_platform,
            scanner,
            extractor,
            semantic_analyzer: LocalSemanticAnalyzer,
            organizer: OrganizationEngine,
            monitor: Arc::new(PollingChangeMonitor::default()),
            pending_recovery: AtomicUsize::new(0),
            state: Mutex::new(ApplicationState::default()),
        }
    }

    #[must_use]
    pub fn system_status(&self) -> SystemStatus {
        SystemStatus {
            local_first: true,
            read_only_scan: true,
            network_disabled: true,
            apply_gate: ApplyGate::for_in_process_host(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            recovery_required: self.pending_recovery.load(Ordering::SeqCst) > 0,
        }
    }

    pub fn set_pending_recovery_count(&self, count: usize) {
        self.pending_recovery.store(count, Ordering::SeqCst);
    }

    pub fn create_workspace(&self, name: &str) -> Result<WorkspaceRecord, ApplicationError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(ApplicationError::InvalidWorkspaceName);
        }
        self.database
            .create_workspace(name)
            .map_err(ApplicationError::Persistence)
    }

    pub fn register_root(
        &self,
        workspace_id: WorkspaceId,
        absolute_path: &Path,
    ) -> Result<RootRecord, ApplicationError> {
        self.database.workspace(workspace_id)?;
        let volume = self.read_only_platform.inspect_volume(absolute_path)?;
        let display_value = absolute_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Racine sélectionnée".to_owned());
        let display_label =
            DisplayLabel::new(display_value).map_err(|_| ApplicationError::InvalidWorkspaceName)?;
        let root = self
            .database
            .register_root(
                workspace_id,
                RootId::new(),
                absolute_path,
                display_label.as_str(),
                &volume,
            )
            .map_err(ApplicationError::Persistence)?;
        Ok(root)
    }

    pub fn scan_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<ScanRecord, ApplicationError> {
        let root = self.database.active_root(workspace_id)?;
        let mut output = self.scanner.scan(
            workspace_id,
            root.id,
            &root.absolute_path_native,
            ScanPolicy::default(),
        )?;
        let observations = output
            .files
            .iter()
            .map(|file| file.observation.clone())
            .collect::<Vec<_>>();
        let persisted = self.database.persist_scan_detailed(
            workspace_id,
            root.id,
            output.scan_id,
            &observations,
            output.issues.len(),
        )?;
        reconcile_persisted_ids(&mut output, &persisted.files)?;
        let runtime_files = self.analyze_files(workspace_id, &root, output.files)?;
        let record = persisted.scan;

        self.state.lock().scans.insert(
            workspace_id,
            RuntimeScan {
                record: record.clone(),
                root,
                files: runtime_files,
            },
        );
        let _ = self.monitor.start(Path::new(
            &self.database.active_root(workspace_id)?.absolute_path,
        ));
        Ok(record)
    }

    pub fn poll_monitoring(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MonitoringSummary, ApplicationError> {
        let hints = self.monitor.drain_hints()?;
        if !hints.is_empty() {
            let mut state = self.state.lock();
            for proposal in state
                .proposals
                .values_mut()
                .filter(|proposal| proposal.revision.workspace_id == workspace_id)
            {
                for item in &mut proposal.revision.items {
                    if item.action.mutates_filesystem()
                        && !matches!(item.review_state, ReviewState::Rejected)
                    {
                        item.review_state = ReviewState::Stale;
                        if !item
                            .uncertainty_reasons
                            .contains(&ReviewReason::SourceChanged)
                        {
                            item.uncertainty_reasons.push(ReviewReason::SourceChanged);
                        }
                    }
                }
                proposal.simulation = None;
            }
        }
        Ok(MonitoringSummary {
            change_hints: hints.len(),
            reconciliation_required: !hints.is_empty(),
            auto_apply: false,
        })
    }

    fn analyze_files(
        &self,
        workspace_id: WorkspaceId,
        root: &RootRecord,
        files: Vec<CatalogedFile>,
    ) -> Result<Vec<RuntimeFile>, ApplicationError> {
        let mut output = Vec::with_capacity(files.len());
        for cataloged in files {
            let observation = &cataloged.observation;
            let artifact_id = ArtifactId::new();
            let extension = cataloged
                .absolute_path
                .extension()
                .map(|value| value.to_string_lossy().into_owned());
            let bytes = self
                .read_only_platform
                .read_bounded(&cataloged.absolute_path, MAX_EXTRACTION_BYTES);
            let extraction = bytes.ok().and_then(|bytes| {
                self.extractor
                    .extract(&ExtractionRequest {
                        request_id: observation.version_id.to_string(),
                        media_type: observation.detected_mime.clone(),
                        extension,
                        bytes,
                        max_output_chars: MAX_EXTRACTED_CHARS,
                    })
                    .ok()
            });
            let mut anomalies = anomalies_for(observation);
            if extraction.is_none() {
                anomalies.push(if observation.encrypted {
                    ReviewReason::EncryptedContent
                } else {
                    ReviewReason::UnsupportedFormat
                });
            }
            let extracted_text = extraction
                .as_ref()
                .map(|value| value.text.as_str())
                .unwrap_or_default();
            let semantics = self.semantic_analyzer.analyze(
                observation.version_id,
                observation.display_label.as_str(),
                observation.detected_mime.as_deref(),
                extracted_text,
            )?;

            if let Some(extracted) = &extraction {
                let candidate = AnalysisCandidate {
                    file_id: observation.file_id.to_string(),
                    file_version_id: observation.version_id.to_string(),
                    root_path: root.absolute_path.clone(),
                    relative_path: native_path_for_display(&observation.relative_path),
                    display_label: observation.display_label.as_str().to_owned(),
                    media_type: observation.detected_mime.clone(),
                    byte_size: i64::try_from(observation.fingerprint.byte_size).unwrap_or(i64::MAX),
                    content_digest: observation
                        .fingerprint
                        .content_digest
                        .map(|value| value.to_vec()),
                };
                self.database.store_extraction(
                    workspace_id,
                    &candidate,
                    extracted
                        .title
                        .as_deref()
                        .unwrap_or(observation.display_label.as_str()),
                    &extracted.text,
                    extracted.language.as_deref(),
                    extracted.method.database_name(),
                )?;
            }

            output.push(RuntimeFile {
                cataloged,
                artifact_id,
                extracted_text: extracted_text.to_owned(),
                semantics,
                anomalies,
            });
        }
        Ok(output)
    }

    pub fn search_workspace(
        &self,
        workspace_id: WorkspaceId,
        query: &str,
    ) -> Result<SearchResponse, ApplicationError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(SearchResponse {
                query: String::new(),
                hits: Vec::new(),
            });
        }
        if let Some(scan) = self.state.lock().scans.get(&workspace_id).cloned() {
            let documents = scan
                .files
                .iter()
                .filter(|file| !file.extracted_text.is_empty())
                .map(|file| {
                    let observation = &file.cataloged.observation;
                    let excerpt = file.extracted_text.chars().take(500).collect::<String>();
                    SearchDocument {
                        file_id: observation.file_id,
                        file_version_id: observation.version_id,
                        display_label: observation.display_label.clone(),
                        title: observation.display_label.as_str().to_owned(),
                        body: file.extracted_text.clone(),
                        detected_mime: observation.detected_mime.clone(),
                        language: None,
                        lexical_tokens: tokenize(&file.extracted_text),
                        embedding: None,
                        evidence: vec![EvidenceRef {
                            artifact_id: file.artifact_id,
                            file_version_id: observation.version_id,
                            display_label: observation.display_label.clone(),
                            locator: EvidenceLocator::Text {
                                start: 0,
                                end: excerpt.len(),
                                line_start: Some(1),
                                line_end: None,
                            },
                            excerpt: excerpt.clone(),
                            excerpt_digest: *blake3::hash(excerpt.as_bytes()).as_bytes(),
                            explanation: Some(
                                "Passage indexé localement, fusion BM25 et vecteur déterministe."
                                    .to_owned(),
                            ),
                        }],
                    }
                })
                .collect::<Vec<_>>();
            if !documents.is_empty() {
                return Ok(HybridSearchEngine::new(documents).search(
                    query,
                    SearchConfig {
                        limit: 30,
                        ..SearchConfig::default()
                    },
                ));
            }
        }
        let rows = self.database.search(workspace_id, query, 30)?;
        let hits = rows
            .into_iter()
            .filter_map(|row| {
                let file_id = row.file_id.parse::<FileId>().ok()?;
                let file_version_id = row.file_version_id.parse::<FileVersionId>().ok()?;
                let display_label = DisplayLabel::new(row.display_label).ok()?;
                let excerpt_digest = *blake3::hash(row.excerpt.as_bytes()).as_bytes();
                Some(SearchHit {
                    file_id,
                    file_version_id,
                    display_label: display_label.clone(),
                    summary: row.excerpt.clone(),
                    score: row.score as f32,
                    lexical_rank: None,
                    semantic_rank: None,
                    evidence: vec![EvidenceRef {
                        artifact_id: ArtifactId::new(),
                        file_version_id,
                        display_label,
                        locator: EvidenceLocator::Text {
                            start: 0,
                            end: row.excerpt.len(),
                            line_start: None,
                            line_end: None,
                        },
                        excerpt: row.excerpt,
                        excerpt_digest,
                        explanation: Some("Passage retourné par l’index FTS5 local.".to_owned()),
                    }],
                })
            })
            .collect();
        Ok(SearchResponse {
            query: query.to_owned(),
            hits,
        })
    }

    pub fn generate_proposal(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<ProposalRevision, ApplicationError> {
        let mut state = self.state.lock();
        let scan = state
            .scans
            .get(&workspace_id)
            .cloned()
            .ok_or(ApplicationError::NotFound)?;
        let inputs = scan
            .files
            .iter()
            .map(|file| OrganizationInput {
                file_id: file.cataloged.observation.file_id,
                file_version_id: file.cataloged.observation.version_id,
                display_label: file.cataloged.observation.display_label.clone(),
                artifact_id: file.artifact_id,
                semantics: file.semantics.clone(),
                anomalies: file.anomalies.clone(),
            })
            .collect::<Vec<_>>();
        let proposal = self.organizer.propose(
            workspace_id,
            scan.root.id,
            scan.record.id,
            &inputs,
            OrganizationPolicy::default(),
            now_unix_ms(),
        );
        state.proposals.insert(
            proposal.id,
            RuntimeProposal {
                revision: proposal.clone(),
                simulation: None,
            },
        );
        Ok(proposal)
    }

    pub fn simulate_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalSimulation, ApplicationError> {
        let mut state = self.state.lock();
        let proposal = state
            .proposals
            .get(&proposal_id)
            .cloned()
            .ok_or(ApplicationError::NotFound)?;
        let scan = state
            .scans
            .get(&proposal.revision.workspace_id)
            .cloned()
            .ok_or(ApplicationError::NotFound)?;
        let item_by_file = proposal
            .revision
            .items
            .iter()
            .map(|item| (item.file_id, item.id))
            .collect::<HashMap<_, _>>();
        let snapshot = scan
            .files
            .iter()
            .filter_map(|file| {
                item_by_file
                    .get(&file.cataloged.observation.file_id)
                    .copied()
                    .map(|item_id| SimulationFile {
                        item_id,
                        root_id: scan.root.id,
                        source_path: file.cataloged.observation.relative_path.clone(),
                        source_display: file
                            .cataloged
                            .observation
                            .display_label
                            .as_str()
                            .to_owned(),
                        fingerprint: file.cataloged.observation.fingerprint.clone(),
                    })
            })
            .collect::<Vec<_>>();
        let occupied = scan.files.iter().map(|file| {
            (
                scan.root.id,
                native_path_for_display(&file.cataloged.observation.relative_path),
            )
        });
        let existing_directories = existing_destination_directories(&scan.root, &proposal.revision);
        let outcome = VirtualFileSystem::with_snapshot(occupied, existing_directories)
            .simulate(&proposal.revision, &snapshot, now_unix_ms())
            .map_err(|_| ApplicationError::NotFound)?;
        let simulation = outcome.simulation.clone();
        state
            .proposals
            .get_mut(&proposal_id)
            .ok_or(ApplicationError::NotFound)?
            .simulation = Some(outcome);
        Ok(simulation)
    }

    pub fn review_proposal_item(
        &self,
        item_id: ProposalItemId,
        accept: bool,
    ) -> Result<ProposalItem, ApplicationError> {
        let mut state = self.state.lock();
        for proposal in state.proposals.values_mut() {
            if let Some(item) = proposal
                .revision
                .items
                .iter_mut()
                .find(|item| item.id == item_id)
            {
                item.decide(accept)
                    .map_err(|_| ApplicationError::ReviewIncomplete)?;
                proposal.revision.revision = proposal.revision.revision.saturating_add(1);
                proposal.simulation = None;
                return Ok(item.clone());
            }
        }
        Err(ApplicationError::NotFound)
    }

    pub fn proposal(&self, proposal_id: ProposalId) -> Result<ProposalRevision, ApplicationError> {
        self.state
            .lock()
            .proposals
            .get(&proposal_id)
            .map(|value| value.revision.clone())
            .ok_or(ApplicationError::NotFound)
    }

    pub fn seal_plan(&self, proposal_id: ProposalId) -> Result<PlanView, ApplicationError> {
        let mut state = self.state.lock();
        let runtime = state
            .proposals
            .get(&proposal_id)
            .cloned()
            .ok_or(ApplicationError::NotFound)?;
        if !runtime.revision.can_be_sealed() {
            return Err(ApplicationError::ReviewIncomplete);
        }
        let simulation = runtime
            .simulation
            .ok_or(ApplicationError::ReviewIncomplete)?;
        if simulation.simulation.has_blockers() {
            return Err(ApplicationError::SimulationBlocked);
        }
        let plan = compile_plan(&runtime.revision, &simulation)?;
        let view = PlanView {
            plan: plan.clone(),
            approved_at: None,
        };
        state.plans.insert(
            plan.id,
            RuntimePlan {
                view: view.clone(),
                approval: None,
            },
        );
        Ok(view)
    }

    pub fn approve_plan(
        &self,
        plan_id: PlanId,
        digest_hex: &str,
    ) -> Result<PlanView, ApplicationError> {
        let digest = decode_digest(digest_hex)?;
        let mut state = self.state.lock();
        let runtime = state
            .plans
            .get_mut(&plan_id)
            .ok_or(ApplicationError::NotFound)?;
        let approval = runtime
            .view
            .plan
            .approve(ActorId::new(), digest, now_unix_ms())
            .map_err(|_| ApplicationError::InvalidDigest)?;
        let approved_at = now_iso();
        runtime.approval = Some(approval);
        runtime.view.approved_at = Some(approved_at);
        Ok(runtime.view.clone())
    }

    pub fn plan(&self, plan_id: PlanId) -> Result<PlanView, ApplicationError> {
        self.state
            .lock()
            .plans
            .get(&plan_id)
            .map(|value| value.view.clone())
            .ok_or(ApplicationError::NotFound)
    }
}

fn reconcile_persisted_ids(
    output: &mut ScanOutput,
    persisted: &[persistence::PersistedFile],
) -> Result<(), ApplicationError> {
    let by_version = persisted
        .iter()
        .map(|file| (file.file_version_id.as_str(), file))
        .collect::<HashMap<_, _>>();
    for file in &mut output.files {
        let version = file.observation.version_id.to_string();
        let persisted = by_version
            .get(version.as_str())
            .ok_or(ApplicationError::NotFound)?;
        file.observation.file_id = persisted
            .file_id
            .parse()
            .map_err(|_| ApplicationError::NotFound)?;
    }
    Ok(())
}

fn anomalies_for(observation: &domain::FileObservation) -> Vec<ReviewReason> {
    let mut output = Vec::new();
    if observation
        .fingerprint
        .native_identity
        .reparse_tag
        .is_some()
    {
        output.push(ReviewReason::ReparsePoint);
    }
    if observation.fingerprint.native_identity.link_count != 1 {
        output.push(ReviewReason::HardLink);
    }
    if observation.cloud_placeholder {
        output.push(ReviewReason::CloudPlaceholder);
    }
    if observation.encrypted {
        output.push(ReviewReason::EncryptedContent);
    }
    if !observation.fingerprint.native_identity.volume.local {
        output.push(ReviewReason::NonLocalVolume);
    }
    output
}

fn native_path_for_display(path: &domain::NativePath) -> String {
    match path.encoding {
        domain::PathEncoding::UnixBytes => String::from_utf8_lossy(&path.bytes).into_owned(),
        domain::PathEncoding::WindowsUtf16Le => {
            let units = path
                .bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        }
    }
}

fn existing_destination_directories(
    root: &RootRecord,
    proposal: &ProposalRevision,
) -> Vec<(RootId, String)> {
    let root_path = root.absolute_path_native.as_path();
    let mut output = Vec::new();
    for item in &proposal.items {
        let destination = match &item.action {
            ProposalAction::Keep => continue,
            ProposalAction::Move { destination }
            | ProposalAction::PlaceInReview { destination } => destination,
        };
        for index in 1..=destination.folder_components.len() {
            let components = &destination.folder_components[..index];
            if components
                .iter()
                .any(|component| simulation::validate_component(component).is_some())
            {
                continue;
            }
            let display = components.join("\\");
            let path = components
                .iter()
                .fold(root_path.to_path_buf(), |current, component| {
                    current.join(component)
                });
            if fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            {
                output.push((destination.root_id, display));
            }
        }
    }
    output
}

fn decode_digest(value: &str) -> Result<[u8; 32], ApplicationError> {
    if value.len() != 64 {
        return Err(ApplicationError::InvalidDigest);
    }
    let mut output = [0_u8; 32];
    for (index, slot) in output.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| ApplicationError::InvalidDigest)?;
    }
    Ok(output)
}

fn now_unix_ms() -> i64 {
    OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .saturating_div(1_000_000) as i64
}

fn now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_decoder_rejects_partial_or_non_hex_values() {
        assert!(matches!(
            decode_digest("abc"),
            Err(ApplicationError::InvalidDigest)
        ));
        assert!(matches!(
            decode_digest(&"z".repeat(64)),
            Err(ApplicationError::InvalidDigest)
        ));
    }

    #[test]
    fn paths_are_for_internal_display_only() {
        let path = domain::NativePath {
            encoding: domain::PathEncoding::WindowsUtf16Le,
            bytes: "Clients\\ACME"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        };
        assert_eq!(native_path_for_display(&path), "Clients\\ACME");
    }
}
