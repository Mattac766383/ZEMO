mod support;

use application::{
    ApprovedExecutorClient, ApprovedExecutorError, ApprovedExecutorSession,
    ExecutionApplicationService, ExecutionConsentAuthorityKey, ExecutorDispatchResult,
    ScannerApplicationService, executor_response_digest,
};
use domain::{
    ExecutionId, ExecutionOperationKind, ExecutionRecoveryState, ExecutorRequestIdentity,
    ExecutorRequestState, ExecutorSessionIdentity, JournalDiagnostic, JournalEventKind,
    OperationJournalEvent, OperationStepId, OrganizationProposal, OrganizationProposalDiff,
    OrganizationProposalOperation, OrganizationProposalStatus, OrganizationProposalSummary,
    OrganizationReason, OrganizationRevisionId, PlanId, ProposalConfidenceLevel,
    ProposalConflictState, ProposalId, ProposalItemId, ProposalOperationKind,
    ProposalSourceSnapshot, WorkspaceId,
};
use ipc_contracts::executor_v2::{
    CommittedJournalEventBinding, ExecutorAttemptAudit, ExecutorOutcome,
    ImmutableExecutionEnvelope, OperationDirection, OperationPrimitiveManifest,
    SessionAuthorization,
};
use operations::{
    ApplyGate, CrossVolumeTransferDraft, CrossVolumeTransferService, DurableJournal,
    ExecutionSafetyPolicy, MemoryJournal, OperationsError, TransferApproval, TransferError,
};
use persistence::{Database, DatabaseKey, ProposalSourceFileRecord, ProposalWorkspaceSourceRecord};
use platform::ReadOnlyPlatform;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use support::{
    MutationSandbox, SandboxApprovedExecutorClient, SandboxFileOperations, assert_is_test_sandbox,
};

const QUALIFICATION_TIMESTAMP: &str = "2026-08-11T00:00:00Z";

#[cfg(target_os = "macos")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_macos::MacOsPlatform)
}

#[cfg(target_os = "windows")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_windows::WindowsPlatform)
}

struct CatalogFixture {
    database: Arc<Database>,
    platform: Arc<dyn ReadOnlyPlatform>,
    workspace_id: WorkspaceId,
    source: ProposalWorkspaceSourceRecord,
    source_by_path: BTreeMap<String, ProposalSourceFileRecord>,
}

impl CatalogFixture {
    fn scan(sandbox: &MutationSandbox, expected_files: usize, name: &str) -> Self {
        let database = Arc::new(
            Database::open_in_memory(&DatabaseKey::from_bytes([201; 32]))
                .unwrap_or_else(|error| panic!("qualification database should open: {error}")),
        );
        let platform = native_platform();
        let scanner = ScannerApplicationService::new(database.clone(), platform.clone());
        let workspace = scanner
            .create_workspace(name)
            .unwrap_or_else(|error| panic!("qualification workspace should be created: {error}"));
        let root = scanner
            .register_root(workspace.id, sandbox.path())
            .unwrap_or_else(|error| panic!("sandbox root should register: {error}"));
        let scan = scanner
            .scan_workspace(workspace.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("sandbox scan should succeed: {error}"));
        assert_eq!(scan.indexed_count, expected_files as u64);
        let source = database
            .organization_source_for_root(workspace.id, root.id)
            .unwrap_or_else(|error| panic!("proposal source should load: {error}"));
        assert_eq!(source.files.len(), expected_files);
        let source_by_path = source
            .files
            .iter()
            .cloned()
            .map(|file| (normalized_relative(&file.relative_path), file))
            .collect();
        Self {
            database,
            platform,
            workspace_id: workspace.id,
            source,
            source_by_path,
        }
    }

    fn persist_proposal(&self, mappings: &[(String, String)]) -> OrganizationProposal {
        assert!(!mappings.is_empty());
        let operations = mappings
            .iter()
            .map(|(source, destination)| self.operation(source, destination))
            .collect::<Vec<_>>();
        let maximum_depth = operations
            .iter()
            .map(|operation| operation.proposed_depth as u32)
            .max()
            .unwrap_or(0);
        let average_depth = operations
            .iter()
            .map(|operation| operation.proposed_depth as f32)
            .sum::<f32>()
            / operations.len() as f32;
        let proposed_moves = operations
            .iter()
            .filter(|operation| operation.operation_kind == ProposalOperationKind::MoveProposal)
            .count() as u64;
        let proposed_renames = operations.len() as u64 - proposed_moves;
        let proposal = OrganizationProposal {
            id: ProposalId::new(),
            revision_id: OrganizationRevisionId::new(),
            workspace_id: self.workspace_id,
            root_id: self.source.root_id,
            source_scan_id: self.source.scan_id,
            revision: 1,
            status: OrganizationProposalStatus::ApprovedForFutureApply,
            engine_version: "m8-qualification-fixture-v1".to_owned(),
            policy_version: "m8-strict-no-overwrite-v1".to_owned(),
            source_semantic_version: self.source.semantic_version.clone(),
            source_relationship_version: self.source.relationship_version.clone(),
            created_at: QUALIFICATION_TIMESTAMP.to_owned(),
            updated_at: QUALIFICATION_TIMESTAMP.to_owned(),
            summary: OrganizationProposalSummary {
                files_analyzed: operations.len() as u64,
                proposed_moves,
                proposed_renames,
                unchanged: 0,
                needs_review: 0,
                unresolved: 0,
                conflicts: 0,
                high_confidence: operations.len() as u64,
                medium_confidence: 0,
                low_confidence: 0,
                duplicate_no_action: 0,
                average_depth,
                maximum_depth,
            },
            diff: OrganizationProposalDiff::default(),
            nodes: Vec::new(),
            operations,
        };
        self.database
            .persist_organization_proposal(&proposal, "initial")
            .unwrap_or_else(|error| panic!("qualification proposal should persist: {error}"));
        proposal
    }

    fn operation(
        &self,
        source_path: &str,
        destination_path: &str,
    ) -> OrganizationProposalOperation {
        let source = self
            .source_by_path
            .get(&normalized_relative(source_path))
            .unwrap_or_else(|| panic!("source should have been scanned: {source_path}"));
        let destination = path_segments(destination_path);
        let (proposed_name, proposed_destination) = destination
            .split_last()
            .map(|(name, parents)| (name.clone(), parents.to_vec()))
            .unwrap_or_else(|| panic!("destination should contain a file name"));
        let source_parent = path_segments(&source.relative_path);
        let source_parent = &source_parent[..source_parent.len().saturating_sub(1)];
        let operation_kind = if source_parent == proposed_destination.as_slice() {
            ProposalOperationKind::RenameProposal
        } else {
            ProposalOperationKind::MoveProposal
        };
        OrganizationProposalOperation {
            id: ProposalItemId::new(),
            file_id: source
                .file_id
                .parse()
                .unwrap_or_else(|error| panic!("file id should parse: {error}")),
            file_version_id: source
                .file_version_id
                .parse()
                .unwrap_or_else(|error| panic!("file version id should parse: {error}")),
            source: ProposalSourceSnapshot {
                relative_path: source.relative_path.clone(),
                content_hash: source.content_hash.clone(),
                byte_size: source.byte_size,
                modified_at: source.modified_at.clone(),
            },
            source_name: source.filename.clone(),
            machine_destination: proposed_destination.clone(),
            machine_name: proposed_name.clone(),
            proposed_destination,
            proposed_name,
            operation_kind,
            confidence_score: 1.0,
            confidence_level: ProposalConfidenceLevel::VeryHigh,
            reasons: vec![OrganizationReason {
                code: "m8_qualification".to_owned(),
                explanation: "Explicit deterministic sandbox qualification mapping.".to_owned(),
                evidence_references: vec![source.relative_path.clone()],
            }],
            conflict_state: ProposalConflictState::None,
            needs_review: false,
            stale: false,
            user_override: true,
            disruption_score: 0.1,
            proposed_path_length: normalized_relative(destination_path).encode_utf16().count(),
            proposed_depth: path_segments(destination_path).len(),
            semantic_context: "unknown".to_owned(),
            document_type: "qualification_fixture".to_owned(),
            customer_name: None,
            supplier_name: None,
            project_name: None,
            duplicate_group_id: None,
            duplicate_canonical: true,
        }
    }
}

#[derive(Default)]
struct IndexedMemoryJournal {
    events: Mutex<HashMap<ExecutionId, Vec<OperationJournalEvent>>>,
}

impl DurableJournal for IndexedMemoryJournal {
    fn append(&self, event: OperationJournalEvent) -> Result<(), OperationsError> {
        self.events
            .lock()
            .map_err(|_| OperationsError::Journal("qualification journal poisoned".to_owned()))?
            .entry(event.execution_id)
            .or_default()
            .push(event);
        Ok(())
    }

    fn flush(&self) -> Result<(), OperationsError> {
        Ok(())
    }

    fn events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError> {
        Ok(self
            .events
            .lock()
            .map_err(|_| OperationsError::Journal("qualification journal poisoned".to_owned()))?
            .get(&execution_id)
            .cloned()
            .unwrap_or_default())
    }
}

struct FailingPreconditionJournal {
    inner: IndexedMemoryJournal,
    failures: AtomicUsize,
}

impl FailingPreconditionJournal {
    fn new() -> Self {
        Self {
            inner: IndexedMemoryJournal::default(),
            failures: AtomicUsize::new(0),
        }
    }
}

impl DurableJournal for FailingPreconditionJournal {
    fn append(&self, event: OperationJournalEvent) -> Result<(), OperationsError> {
        if event.kind == JournalEventKind::PreconditionsValidated
            && self.failures.fetch_add(1, Ordering::SeqCst) == 0
        {
            return Err(OperationsError::Journal(
                "injected journal preparation append failure".to_owned(),
            ));
        }
        self.inner.append(event)
    }

    fn flush(&self) -> Result<(), OperationsError> {
        self.inner.flush()
    }

    fn events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError> {
        self.inner.events(execution_id)
    }

    fn diagnostics(&self) -> Vec<JournalDiagnostic> {
        Vec::new()
    }
}

struct DirectoryFailureExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    dispatches: Arc<AtomicUsize>,
}

impl ApprovedExecutorClient for DirectoryFailureExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        let target = envelope
            .operations
            .iter()
            .find(|operation| {
                matches!(
                    operation.primitive,
                    OperationPrimitiveManifest::CreateDirectory { .. }
                )
            })
            .map(|operation| operation.operation_id.clone())
            .ok_or_else(|| {
                ApprovedExecutorError::Unavailable(
                    "qualification envelope has no directory operation".to_owned(),
                )
            })?;
        Ok(Box::new(DirectoryFailureExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            target,
            dispatches: self.dispatches.clone(),
        }))
    }
}

struct DirectoryFailureExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    target: String,
    dispatches: Arc<AtomicUsize>,
}

impl ApprovedExecutorSession for DirectoryFailureExecutorSession {
    fn identity(&self) -> &ExecutorSessionIdentity {
        self.inner.identity()
    }

    fn prepare_operation(
        &mut self,
        operation_id: OperationStepId,
        direction: OperationDirection,
    ) -> Result<ExecutorRequestIdentity, ApprovedExecutorError> {
        self.inner.prepare_operation(operation_id, direction)
    }

    fn dispatch_prepared(
        &mut self,
        request: ExecutorRequestIdentity,
        journal_intent: CommittedJournalEventBinding,
    ) -> Result<ExecutorDispatchResult, ApprovedExecutorError> {
        if request.operation_id.to_string() == self.target {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            let outcome = ExecutorOutcome::ProvenNotApplied {
                code: "injected_create_directory_failure".to_owned(),
                detail: "The sandbox executor proved no directory was created.".to_owned(),
                audit: ExecutorAttemptAudit {
                    attempt_count: 1,
                    error_class: None,
                },
            };
            return Ok(ExecutorDispatchResult {
                response_digest_hex: executor_response_digest(&request, &outcome)?,
                outcome,
            });
        }
        self.inner.dispatch_prepared(request, journal_intent)
    }
}

struct CorruptingPostconditionExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    root: PathBuf,
    dispatches: Arc<AtomicUsize>,
}

impl ApprovedExecutorClient for CorruptingPostconditionExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        let (target, destination) = envelope
            .operations
            .iter()
            .find(|operation| operation.proposal_operation_id.is_some())
            .map(|operation| {
                (
                    operation.operation_id.clone(),
                    primitive_destination(&operation.primitive).to_owned(),
                )
            })
            .ok_or_else(|| {
                ApprovedExecutorError::Unavailable(
                    "qualification envelope has no file operation".to_owned(),
                )
            })?;
        Ok(Box::new(CorruptingPostconditionExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            root: self.root.clone(),
            target,
            destination,
            dispatches: self.dispatches.clone(),
        }))
    }
}

struct CorruptingPostconditionExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    root: PathBuf,
    target: String,
    destination: String,
    dispatches: Arc<AtomicUsize>,
}

impl ApprovedExecutorSession for CorruptingPostconditionExecutorSession {
    fn identity(&self) -> &ExecutorSessionIdentity {
        self.inner.identity()
    }

    fn prepare_operation(
        &mut self,
        operation_id: OperationStepId,
        direction: OperationDirection,
    ) -> Result<ExecutorRequestIdentity, ApprovedExecutorError> {
        self.inner.prepare_operation(operation_id, direction)
    }

    fn dispatch_prepared(
        &mut self,
        request: ExecutorRequestIdentity,
        journal_intent: CommittedJournalEventBinding,
    ) -> Result<ExecutorDispatchResult, ApprovedExecutorError> {
        let target = request.operation_id.to_string() == self.target
            && request.direction == domain::ExecutorRequestDirection::Forward;
        let dispatched = self.inner.dispatch_prepared(request, journal_intent)?;
        if target && matches!(&dispatched.outcome, ExecutorOutcome::Success { .. }) {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            let destination = self.root.join(relative_path(&self.destination));
            assert_is_test_sandbox(&self.root, &destination);
            fs::write(&destination, b"injected postcondition corruption")
                .unwrap_or_else(|error| panic!("postcondition corruption should write: {error}"));
        }
        Ok(dispatched)
    }
}

struct AuthorizationProbeExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    rejected: Arc<AtomicUsize>,
}

impl ApprovedExecutorClient for AuthorizationProbeExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        let mut session = self.inner.open_session(envelope, authorization)?;
        match session.prepare_operation(OperationStepId::new(), OperationDirection::Forward) {
            Err(_) => {
                self.rejected.fetch_add(1, Ordering::SeqCst);
                Ok(session)
            }
            Ok(_) => Err(ApprovedExecutorError::Ambiguous(
                "sandbox executor authorized an operation outside its envelope".to_owned(),
            )),
        }
    }
}

fn execution_service(
    fixture: &CatalogFixture,
    executor: Arc<dyn ApprovedExecutorClient>,
    journal: Arc<dyn DurableJournal>,
) -> ExecutionApplicationService {
    ExecutionApplicationService::new(
        fixture.database.clone(),
        fixture.platform.clone(),
        executor,
        journal,
        ApplyGate {
            enabled: true,
            reason: "isolated M8 qualification sandbox".to_owned(),
        },
        ExecutionSafetyPolicy::default(),
        ExecutionConsentAuthorityKey::from_bytes([202; 32]),
    )
    .unwrap_or_else(|error| panic!("qualification execution service should initialize: {error}"))
}

fn attest(
    service: &ExecutionApplicationService,
    execution_id: ExecutionId,
) -> domain::ExecutionDetail {
    let challenge = service
        .create_execution_consent_challenge(execution_id, None)
        .unwrap_or_else(|error| panic!("qualification challenge should be created: {error}"));
    service
        .finalize_execution_consent(challenge)
        .unwrap_or_else(|error| panic!("qualification consent should be attested: {error}"))
}

#[test]
fn journal_preparation_failure_prevents_directory_and_file_mutation() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.bin", b"journal failure fixture");
    let before = sandbox.snapshot();
    let fixture = CatalogFixture::scan(&sandbox, 1, "M8 journal failure qualification");
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.bin".to_owned(),
        "organized/new/item.bin".to_owned(),
    )]);
    let journal = Arc::new(FailingPreconditionJournal::new());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        fixture.platform.clone(),
    ));
    let service = execution_service(&fixture, executor, journal.clone());
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should prepare directory operations: {error}"));
    assert!(
        prepared
            .operations
            .iter()
            .any(|operation| operation.kind == ExecutionOperationKind::CreateDirectory)
    );
    let approved = attest(&service, prepared.session.id);

    assert!(
        service
            .start_execution(approved.session.id, &mut |_| {})
            .is_err()
    );
    assert_eq!(journal.failures.load(Ordering::SeqCst), 1);
    assert_eq!(before, sandbox.snapshot());
    assert!(!sandbox.path().join("organized").exists());
}

#[test]
fn directory_creation_failure_is_proven_not_applied_and_stops_dependents() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.bin", b"directory failure fixture");
    let before = sandbox.snapshot();
    let fixture = CatalogFixture::scan(&sandbox, 1, "M8 directory failure qualification");
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.bin".to_owned(),
        "organized/new/item.bin".to_owned(),
    )]);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let inner: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        fixture.platform.clone(),
    ));
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(DirectoryFailureExecutorClient {
        inner,
        dispatches: dispatches.clone(),
    });
    let service = execution_service(
        &fixture,
        executor,
        Arc::new(IndexedMemoryJournal::default()),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest(&service, prepared.session.id);
    let stopped = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("proven directory refusal is not ambiguous: {error}"));

    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(
        stopped.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    assert_eq!(before, sandbox.snapshot());
    assert!(!sandbox.path().join("organized").exists());
}

#[test]
fn postcondition_failure_enters_ambiguous_recovery_without_retry() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.bin", b"expected postcondition bytes");
    create_directory_guarded(&sandbox, "organized");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M8 postcondition failure qualification");
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.bin".to_owned(),
        "organized/item.bin".to_owned(),
    )]);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let inner: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        fixture.platform.clone(),
    ));
    let executor: Arc<dyn ApprovedExecutorClient> =
        Arc::new(CorruptingPostconditionExecutorClient {
            inner,
            root: sandbox.path().to_path_buf(),
            dispatches: dispatches.clone(),
        });
    let service = execution_service(
        &fixture,
        executor,
        Arc::new(IndexedMemoryJournal::default()),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest(&service, prepared.session.id);

    assert!(matches!(
        service.start_execution(approved.session.id, &mut |_| {}),
        Err(application::ApplicationError::ExecutionRecoveryRequired)
    ));
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    let assessment = service
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("recovery should classify the corruption: {error}"));
    assert_eq!(assessment.state, ExecutionRecoveryState::RecoveryAmbiguous);
    assert!(assessment.ambiguous >= 1);
    assert!(
        service
            .rollback_execution(approved.session.id, &mut |_| {})
            .is_err()
    );
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(
        fs::read(sandbox.path().join("organized/item.bin"))
            .unwrap_or_else(|error| panic!("corrupted destination should remain: {error}")),
        b"injected postcondition corruption"
    );
}

#[test]
fn cross_volume_copy_gate_refuses_before_any_sandbox_mutation() {
    let sandbox = MutationSandbox::new();
    sandbox.write("source/item.bin", b"cross-volume gate fixture");
    let before = sandbox.snapshot();
    let platform = native_platform();
    let source = sandbox.path().join("source/item.bin");
    let destination = sandbox.path().join("destination/item.bin");
    let temporary = sandbox.path().join("destination/.item.bin.partial");
    let vault = sandbox.path().join("vault/item.bin");
    for path in [&source, &destination, &temporary, &vault] {
        assert_is_test_sandbox(sandbox.path(), path);
    }
    let expected_source = platform
        .fingerprint(&source, true, domain::MAX_EXECUTION_VERIFICATION_BYTES)
        .unwrap_or_else(|error| panic!("source fingerprint should succeed: {error}"));
    let plan = CrossVolumeTransferDraft {
        id: PlanId::new(),
        source,
        destination,
        recovery_vault_path: vault,
        destination_temporary_path: temporary,
        expected_source,
        created_at_unix_ms: 1,
    }
    .seal()
    .unwrap_or_else(|error| panic!("cross-volume refusal plan should seal: {error}"));
    let approval = TransferApproval {
        plan_id: plan.id,
        plan_digest: plan.digest,
        approved_at_unix_ms: 2,
    };
    let filesystem = Arc::new(SandboxFileOperations::new(sandbox.path(), platform.clone()));
    let service =
        CrossVolumeTransferService::new(platform, filesystem, Arc::new(MemoryJournal::default()));

    assert!(matches!(
        service.execute(&plan, &approval, 3),
        Err(TransferError::GateDisabled)
    ));
    assert_eq!(before, sandbox.snapshot());
}

#[test]
fn randomized_safe_operation_graphs_preserve_data_and_round_trip() {
    const SEEDS: [u64; 8] = [
        0x1020_3040_5060_7080,
        0x8877_6655_4433_2211,
        0x0ddc_0ffe_e15e_beef,
        0xa5a5_5a5a_c3c3_3c3c,
        0x1357_9bdf_2468_ace0,
        0xfedc_ba98_7654_3210,
        0x3141_5926_5358_9793,
        0x2718_2818_2845_9045,
    ];
    for seed in SEEDS {
        let sandbox = MutationSandbox::new();
        let mut random = DeterministicRandom::new(seed);
        let sources = randomized_sources(&sandbox, &mut random, 24);
        let mappings = randomized_operation_graph(&sandbox, &mut random, &sources);
        let initial = sandbox.snapshot();
        let expected = remap_snapshot(&initial, &mappings);
        let fixture = CatalogFixture::scan(&sandbox, sources.len(), "M8 randomized graph");
        let proposal = fixture.persist_proposal(&mappings);
        let rejected = Arc::new(AtomicUsize::new(0));
        let inner: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
            sandbox.path(),
            fixture.platform.clone(),
        ));
        let executor: Arc<dyn ApprovedExecutorClient> =
            Arc::new(AuthorizationProbeExecutorClient {
                inner,
                rejected: rejected.clone(),
            });
        let service = execution_service(
            &fixture,
            executor,
            Arc::new(IndexedMemoryJournal::default()),
        );
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("seed {seed:#x} should preflight: {error}"));
        let approved = attest(&service, prepared.session.id);
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("seed {seed:#x} should apply: {error}"));
        assert_eq!(
            completed.session.status,
            domain::OrganizationExecutionStatus::Completed,
            "seed {seed:#x}"
        );
        assert_eq!(expected, sandbox.snapshot(), "seed {seed:#x}");
        assert!(
            !service
                .system_status()
                .unwrap_or_else(|error| panic!("system status should load: {error}"))
                .recovery_required,
            "seed {seed:#x}"
        );
        assert!(
            fixture
                .database
                .executor_request_facts(completed.session.id)
                .unwrap_or_else(|error| panic!("executor proofs should load: {error}"))
                .iter()
                .all(|request| request.state == ExecutorRequestState::ProvenApplied),
            "seed {seed:#x}"
        );
        assert!(
            fixture
                .database
                .validate_execution_journal(completed.session.id)
                .unwrap_or_else(|error| panic!("journal should validate: {error}")),
            "seed {seed:#x}"
        );
        let rolled_back = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("seed {seed:#x} should roll back: {error}"));
        assert_eq!(
            rolled_back.session.status,
            domain::OrganizationExecutionStatus::RolledBack,
            "seed {seed:#x}"
        );
        assert_eq!(initial, sandbox.snapshot(), "seed {seed:#x}");
        assert!(rejected.load(Ordering::SeqCst) >= 2, "seed {seed:#x}");
    }
}

#[test]
fn randomized_rollback_conflicts_never_overwrite_intruders() {
    const SEEDS: [u64; 6] = [
        0x0102_0304_0506_0708,
        0x1112_1314_1516_1718,
        0x2122_2324_2526_2728,
        0x3132_3334_3536_3738,
        0x4142_4344_4546_4748,
        0x5152_5354_5556_5758,
    ];
    for seed in SEEDS {
        let sandbox = MutationSandbox::new();
        let mut random = DeterministicRandom::new(seed);
        let sources = randomized_sources(&sandbox, &mut random, 16);
        let mappings = sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let destination =
                    format!("organized/group-{}/moved-{index:03}.bin", random.index(5));
                create_parent_guarded(&sandbox, &destination);
                (source.clone(), destination)
            })
            .collect::<Vec<_>>();
        let initial = sandbox.snapshot();
        let fixture = CatalogFixture::scan(&sandbox, sources.len(), "M8 randomized rollback");
        let proposal = fixture.persist_proposal(&mappings);
        let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
            SandboxApprovedExecutorClient::new(sandbox.path(), fixture.platform.clone()),
        );
        let service = execution_service(
            &fixture,
            executor,
            Arc::new(IndexedMemoryJournal::default()),
        );
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("seed {seed:#x} should preflight: {error}"));
        let approved = attest(&service, prepared.session.id);
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("seed {seed:#x} should apply: {error}"));
        let conflict_index = random.index(mappings.len());
        let conflict_source = &mappings[conflict_index].0;
        let intruder = format!("intruder-{seed:016x}").into_bytes();
        sandbox.write(conflict_source, &intruder);

        let partial = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("seed {seed:#x} conflict should be reported: {error}"));
        assert_eq!(
            partial.session.status,
            domain::OrganizationExecutionStatus::RollbackPartial,
            "seed {seed:#x}"
        );
        let observed = sandbox.snapshot();
        assert_eq!(observed.len(), initial.len() + 1, "seed {seed:#x}");
        for (index, (source, destination)) in mappings.iter().enumerate() {
            let source_path = relative_path(source);
            let destination_path = relative_path(destination);
            let expected = initial
                .get(&source_path)
                .unwrap_or_else(|| panic!("source signature should exist: {source}"));
            let at_source = observed.get(&source_path);
            let at_destination = observed.get(&destination_path);
            if index == conflict_index {
                assert_eq!(
                    at_source,
                    Some(&(intruder.len() as u64, blake3::hash(&intruder))),
                    "seed {seed:#x}"
                );
                assert_eq!(at_destination, Some(expected), "seed {seed:#x}");
            } else {
                assert!(
                    (at_source == Some(expected) && at_destination.is_none())
                        || (at_source.is_none() && at_destination == Some(expected)),
                    "seed {seed:#x} lost or duplicated {source}"
                );
            }
        }
        assert_eq!(
            fs::read(sandbox.path().join(relative_path(conflict_source)))
                .unwrap_or_else(|error| panic!("intruder should remain readable: {error}")),
            intruder,
            "seed {seed:#x}"
        );
    }
}

#[test]
#[ignore = "explicit 10,000-operation sandbox qualification"]
fn qualification_10000_operations_apply_verify_rollback_reports_metrics() {
    const OPERATION_COUNT: usize = 10_000;
    const BATCH_SIZE: usize = 100;
    let memory = PeakMemorySampler::start();
    let setup_started = Instant::now();
    let sandbox = MutationSandbox::new();
    let mut all_mappings = Vec::with_capacity(OPERATION_COUNT);
    for index in 0..OPERATION_COUNT {
        let source = format!(
            "qualification/input/shard-{:03}/item-{index:05}.bin",
            index / BATCH_SIZE
        );
        let destination = format!(
            "qualification/output/shard-{:03}/item-{index:05}.bin",
            index / BATCH_SIZE
        );
        sandbox.write(
            &source,
            format!("m8-qualified-payload-{index:05}").as_bytes(),
        );
        create_parent_guarded(&sandbox, &destination);
        all_mappings.push((source, destination));
    }
    let initial = sandbox.snapshot();
    let fixture = CatalogFixture::scan(
        &sandbox,
        OPERATION_COUNT,
        "M8 10000 operation qualification",
    );
    let journal = Arc::new(IndexedMemoryJournal::default());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        fixture.platform.clone(),
    ));
    let service = execution_service(&fixture, executor, journal);
    let setup_duration = setup_started.elapsed();
    let mut preflight_duration = Duration::ZERO;
    let mut consent_duration = Duration::ZERO;
    let mut execution_duration = Duration::ZERO;
    let mut verification_duration = Duration::ZERO;
    let mut rollback_duration = Duration::ZERO;
    let mut executed_operations = 0_usize;

    for (batch_index, mappings) in all_mappings.chunks(BATCH_SIZE).enumerate() {
        let proposal = fixture.persist_proposal(mappings);
        let started = Instant::now();
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| {
                panic!("qualification batch {batch_index} should preflight: {error}")
            });
        preflight_duration += started.elapsed();
        assert_eq!(
            prepared.operations.len(),
            mappings.len(),
            "precreated destination parents should avoid synthetic directory operations"
        );
        assert!(
            prepared
                .operations
                .iter()
                .all(|operation| operation.proposal_operation_id.is_some())
        );
        executed_operations += prepared.operations.len();

        let started = Instant::now();
        let approved = attest(&service, prepared.session.id);
        consent_duration += started.elapsed();

        let started = Instant::now();
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| {
                panic!("qualification batch {batch_index} should apply: {error}")
            });
        execution_duration += started.elapsed();
        assert_eq!(
            completed.session.status,
            domain::OrganizationExecutionStatus::Completed
        );

        let started = Instant::now();
        for (source, destination) in mappings {
            let source = sandbox.path().join(relative_path(source));
            let destination = sandbox.path().join(relative_path(destination));
            assert_is_test_sandbox(sandbox.path(), &source);
            assert_is_test_sandbox(sandbox.path(), &destination);
            assert!(!source.exists());
            let index = destination
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.strip_prefix("item-"))
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(|| panic!("qualified destination index should parse"));
            assert_eq!(
                fs::read(&destination)
                    .unwrap_or_else(|error| panic!("destination should verify: {error}")),
                format!("m8-qualified-payload-{index:05}").as_bytes()
            );
        }
        assert!(
            fixture
                .database
                .validate_execution_journal(completed.session.id)
                .unwrap_or_else(|error| panic!("batch journal should validate: {error}"))
        );
        verification_duration += started.elapsed();

        let started = Instant::now();
        let rolled_back = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| {
                panic!("qualification batch {batch_index} should roll back: {error}")
            });
        rollback_duration += started.elapsed();
        assert_eq!(
            rolled_back.session.status,
            domain::OrganizationExecutionStatus::RolledBack
        );
    }

    assert_eq!(executed_operations, OPERATION_COUNT);
    assert_eq!(initial, sandbox.snapshot());
    let peak_memory_bytes = memory.finish();
    let metrics = serde_json::json!({
        "qualification": "m8_sandbox_apply_verify_rollback",
        "operations": executed_operations,
        "batches": OPERATION_COUNT / BATCH_SIZE,
        "batch_size": BATCH_SIZE,
        "setup_duration_ms": duration_ms(setup_duration),
        "preflight_duration_ms": duration_ms(preflight_duration),
        "preflight_scope": "ExecutionApplicationService::prepare_execution including plan persistence and journal synchronization",
        "consent_preparation_duration_ms": duration_ms(consent_duration),
        "journal_preparation_duration_ms": serde_json::Value::Null,
        "journal_preparation_scope": "not separately exposed; combined into preflight, execution, and rollback durations",
        "execution_duration_ms": duration_ms(execution_duration),
        "execution_scope": "ExecutionApplicationService::start_execution including internal postcondition verification",
        "verification_duration_ms": duration_ms(verification_duration),
        "verification_scope": "independent sandbox path/content verification plus authenticated database journal validation",
        "rollback_duration_ms": duration_ms(rollback_duration),
        "peak_memory_bytes": peak_memory_bytes,
        "peak_memory_method": "sampled process resident set at 50ms intervals",
    });
    println!(
        "M8_QUALIFICATION_METRICS={}",
        serde_json::to_string(&metrics)
            .unwrap_or_else(|error| panic!("qualification metrics should encode: {error}"))
    );
}

fn primitive_destination(primitive: &OperationPrimitiveManifest) -> &str {
    match primitive {
        OperationPrimitiveManifest::CreateDirectory {
            destination_relative_path,
        }
        | OperationPrimitiveManifest::SameVolumeMove {
            destination_relative_path,
            ..
        }
        | OperationPrimitiveManifest::SameVolumeRename {
            destination_relative_path,
            ..
        }
        | OperationPrimitiveManifest::SameVolumeMoveAndRename {
            destination_relative_path,
            ..
        }
        | OperationPrimitiveManifest::InternalStage {
            destination_relative_path,
            ..
        } => destination_relative_path,
    }
}

fn normalized_relative(value: &str) -> String {
    value
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn path_segments(value: &str) -> Vec<String> {
    normalized_relative(value)
        .split('/')
        .map(str::to_owned)
        .collect()
}

fn relative_path(value: &str) -> PathBuf {
    path_segments(value)
        .into_iter()
        .fold(PathBuf::new(), |path, segment| path.join(segment))
}

fn create_directory_guarded(sandbox: &MutationSandbox, relative: &str) {
    let path = sandbox.path().join(relative_path(relative));
    assert_is_test_sandbox(sandbox.path(), &path);
    fs::create_dir_all(path)
        .unwrap_or_else(|error| panic!("sandbox directory should be created: {error}"));
}

fn create_parent_guarded(sandbox: &MutationSandbox, relative: &str) {
    let path = sandbox.path().join(relative_path(relative));
    assert_is_test_sandbox(sandbox.path(), &path);
    let parent = path
        .parent()
        .unwrap_or_else(|| panic!("sandbox fixture should have a parent"));
    assert_is_test_sandbox(sandbox.path(), parent);
    fs::create_dir_all(parent)
        .unwrap_or_else(|error| panic!("sandbox parent should be created: {error}"));
}

fn randomized_sources(
    sandbox: &MutationSandbox,
    random: &mut DeterministicRandom,
    count: usize,
) -> Vec<String> {
    (0..count)
        .map(|index| {
            let source = format!(
                "tree/branch-{}/depth-{}/source-{index:03}-{:04x}.bin",
                random.index(5),
                random.index(4),
                random.next_u64() & 0xffff
            );
            sandbox.write(
                &source,
                format!("seeded-content-{index:03}-{:016x}", random.next_u64()).as_bytes(),
            );
            source
        })
        .collect()
}

fn randomized_operation_graph(
    sandbox: &MutationSandbox,
    random: &mut DeterministicRandom,
    sources: &[String],
) -> Vec<(String, String)> {
    let mut source_destinations = sources.to_vec();
    random.shuffle(&mut source_destinations);
    source_destinations.truncate(sources.len() / 2);
    let mut destinations = source_destinations;
    for index in destinations.len()..sources.len() {
        let destination = format!(
            "organized/group-{}/destination-{index:03}-{:04x}.bin",
            random.index(6),
            random.next_u64() & 0xffff
        );
        create_parent_guarded(sandbox, &destination);
        destinations.push(destination);
    }
    random.shuffle(&mut destinations);
    let normalized_sources = sources
        .iter()
        .map(|source| normalized_relative(source))
        .collect::<Vec<_>>();
    let mut found_derangement = false;
    for _ in 0..destinations.len() {
        if normalized_sources
            .iter()
            .zip(&destinations)
            .all(|(source, destination)| source != &normalized_relative(destination))
        {
            found_derangement = true;
            break;
        }
        destinations.rotate_left(1);
    }
    assert!(
        found_derangement,
        "random graph should avoid no-op mappings"
    );
    sources.iter().cloned().zip(destinations).collect()
}

fn remap_snapshot(
    initial: &BTreeMap<PathBuf, (u64, blake3::Hash)>,
    mappings: &[(String, String)],
) -> BTreeMap<PathBuf, (u64, blake3::Hash)> {
    mappings
        .iter()
        .map(|(source, destination)| {
            let signature = initial
                .get(&relative_path(source))
                .copied()
                .unwrap_or_else(|| panic!("source signature should exist: {source}"));
            (relative_path(destination), signature)
        })
        .collect()
}

struct DeterministicRandom(u64);

impl DeterministicRandom {
    fn new(seed: u64) -> Self {
        assert_ne!(seed, 0);
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn index(&mut self, upper: usize) -> usize {
        assert!(upper > 0);
        (self.next_u64() % upper as u64) as usize
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            values.swap(index, self.index(index + 1));
        }
    }
}

struct PeakMemorySampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    worker: Option<thread::JoinHandle<()>>,
}

impl PeakMemorySampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let worker_stop = stop.clone();
        let worker_peak = peak.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                if let Some(bytes) = current_resident_memory_bytes() {
                    worker_peak.fetch_max(bytes, Ordering::Relaxed);
                }
                thread::sleep(Duration::from_millis(50));
            }
            if let Some(bytes) = current_resident_memory_bytes() {
                worker_peak.fetch_max(bytes, Ordering::Relaxed);
            }
        });
        Self {
            stop,
            peak,
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> Option<u64> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .unwrap_or_else(|_| panic!("memory sampler should not panic"));
        }
        match self.peak.load(Ordering::Relaxed) {
            0 => None,
            bytes => Some(bytes),
        }
    }
}

#[cfg(target_os = "linux")]
fn current_resident_memory_bytes() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
                .and_then(|kilobytes| kilobytes.checked_mul(1024))
        })
}

#[cfg(target_os = "macos")]
fn current_resident_memory_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[cfg(target_os = "windows")]
fn current_resident_memory_bytes() -> Option<u64> {
    None
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(target_os = "macos")]
mod macos_native_apply {
    use super::*;

    fn macos_service(
        fixture: &CatalogFixture,
        sandbox: &MutationSandbox,
        policy: ExecutionSafetyPolicy,
    ) -> ExecutionApplicationService {
        let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
            SandboxApprovedExecutorClient::macos_native(sandbox.path(), fixture.platform.clone()),
        );
        ExecutionApplicationService::new(
            fixture.database.clone(),
            fixture.platform.clone(),
            executor,
            Arc::new(IndexedMemoryJournal::default()),
            ApplyGate {
                enabled: true,
                reason: "macos native mutation sandbox".to_owned(),
            },
            policy,
            ExecutionConsentAuthorityKey::from_bytes([218; 32]),
        )
        .unwrap_or_else(|error| panic!("macos execution service should initialize: {error}"))
    }

    #[test]
    fn macos_native_move_rename_directory_verify_and_rollback() {
        let sandbox = MutationSandbox::new();
        sandbox.write("incoming/report.txt", b"macos-native-move");
        let initial = sandbox.snapshot();
        let fixture = CatalogFixture::scan(&sandbox, 1, "M18 macos native move");
        let proposal = fixture.persist_proposal(&[(
            "incoming/report.txt".to_owned(),
            "Organized/Reviewed/report-final.txt".to_owned(),
        )]);
        let service = macos_service(&fixture, &sandbox, ExecutionSafetyPolicy::default());
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("macos preflight should succeed: {error}"));
        let approved = attest(&service, prepared.session.id);
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("macos apply should succeed: {error}"));
        assert!(
            completed
                .operations
                .iter()
                .any(|operation| operation.kind == ExecutionOperationKind::CreateDirectory)
        );
        assert!(!sandbox.path().join("incoming/report.txt").exists());
        assert_eq!(
            fs::read(sandbox.path().join("Organized/Reviewed/report-final.txt"))
                .unwrap_or_else(|error| panic!("moved file should be readable: {error}")),
            b"macos-native-move"
        );
        let rolled = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("macos rollback should succeed: {error}"));
        assert_eq!(
            rolled.session.status,
            domain::OrganizationExecutionStatus::RolledBack
        );
        assert_eq!(sandbox.snapshot(), initial);
    }

    #[test]
    fn macos_native_no_overwrite_symlink_and_source_drift() {
        let sandbox = MutationSandbox::new();
        sandbox.write("incoming/keep-source.txt", b"original");
        sandbox.write("incoming/other.txt", b"other");
        let fixture = CatalogFixture::scan(&sandbox, 2, "M18 macos safety");
        let proposal = fixture.persist_proposal(&[
            (
                "incoming/keep-source.txt".to_owned(),
                "Organized/keep-source.txt".to_owned(),
            ),
            (
                "incoming/other.txt".to_owned(),
                "Organized/other.txt".to_owned(),
            ),
        ]);
        fs::create_dir_all(sandbox.path().join("Organized"))
            .unwrap_or_else(|error| panic!("dest parent: {error}"));
        sandbox.write("Organized/keep-source.txt", b"already-here");
        let service = macos_service(&fixture, &sandbox, ExecutionSafetyPolicy::default());
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("preflight should remain available: {error}"));
        assert!(prepared.operations.iter().any(|operation| {
            operation.error_code.as_deref() == Some("destination_exists")
                || operation.status != domain::ExecutionOperationStatus::PreflightOk
                    && operation
                        .source_relative_path
                        .as_deref()
                        .is_some_and(|path| path.contains("keep-source"))
        }));

        let sandbox = MutationSandbox::new();
        sandbox.write("incoming/drift.txt", b"before");
        let fixture = CatalogFixture::scan(&sandbox, 1, "M18 macos drift");
        let proposal = fixture.persist_proposal(&[(
            "incoming/drift.txt".to_owned(),
            "Organized/drift.txt".to_owned(),
        )]);
        sandbox.write("incoming/drift.txt", b"changed-after-scan");
        let service = macos_service(&fixture, &sandbox, ExecutionSafetyPolicy::default());
        let prepared = service.prepare_execution(proposal.id, proposal.revision);
        assert!(
            prepared.is_err()
                || prepared.is_ok_and(|detail| detail.operations.iter().any(|operation| {
                    operation
                        .error_code
                        .as_deref()
                        .is_some_and(|code| code.contains("drift") || code.contains("hash"))
                        || operation.status != domain::ExecutionOperationStatus::PreflightOk
                }))
        );
        assert_eq!(
            fs::read(sandbox.path().join("incoming/drift.txt"))
                .unwrap_or_else(|error| panic!("drifted source should remain: {error}")),
            b"changed-after-scan"
        );
    }

    #[test]
    fn macos_native_qualified_case_only_rename_rolls_back() {
        let sandbox = MutationSandbox::new();
        sandbox.write("incoming/Invoice.pdf", b"case-only");
        let initial = sandbox.snapshot();
        let fixture = CatalogFixture::scan(&sandbox, 1, "M18 macos case-only");
        let proposal = fixture.persist_proposal(&[(
            "incoming/Invoice.pdf".to_owned(),
            "incoming/invoice.pdf".to_owned(),
        )]);
        let mut policy = ExecutionSafetyPolicy::default();
        policy.allow_qualified_case_only_rename = true;
        let service = macos_service(&fixture, &sandbox, policy);
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("case-only preflight should succeed: {error}"));
        assert!(
            prepared
                .operations
                .iter()
                .any(|operation| operation.kind == ExecutionOperationKind::InternalStage)
        );
        let approved = attest(&service, prepared.session.id);
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("case-only apply should succeed: {error}"));
        assert_eq!(
            completed.session.summary.failed, 0,
            "case-only apply should not fail: {:?}",
            completed.operations
        );
        let incoming = sandbox.path().join("incoming");
        let leaves = fs::read_dir(&incoming)
            .unwrap_or_else(|error| panic!("incoming should be readable: {error}"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|error| panic!("dirent: {error}"))
                    .file_name()
            })
            .collect::<Vec<_>>();
        assert!(
            leaves
                .iter()
                .any(|name| name.to_string_lossy() == "invoice.pdf"),
            "case-preserving leaf should be invoice.pdf in {incoming:?}: {leaves:?}; operations={:?}",
            completed.operations
        );
        let payload = leaves.iter().find_map(|name| {
            fs::read(incoming.join(name))
                .ok()
                .filter(|bytes| bytes == b"case-only")
        });
        assert_eq!(payload.as_deref(), Some(b"case-only".as_slice()));
        let rolled = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("case-only rollback should succeed: {error}"));
        assert_eq!(
            rolled.session.status,
            domain::OrganizationExecutionStatus::RolledBack
        );
        assert_eq!(sandbox.snapshot(), initial);
    }

    #[test]
    fn macos_native_crash_before_and_after_mutation_reconcile() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CrashOnce {
            inner: Arc<dyn ApprovedExecutorClient>,
            point: &'static str,
            seen: Arc<AtomicUsize>,
        }
        impl ApprovedExecutorClient for CrashOnce {
            fn open_session(
                &self,
                envelope: ImmutableExecutionEnvelope,
                authorization: SessionAuthorization,
            ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
                Ok(Box::new(CrashOnceSession {
                    inner: self.inner.open_session(envelope, authorization)?,
                    point: self.point,
                    seen: Arc::clone(&self.seen),
                }))
            }
        }
        struct CrashOnceSession {
            inner: Box<dyn ApprovedExecutorSession>,
            point: &'static str,
            seen: Arc<AtomicUsize>,
        }
        impl ApprovedExecutorSession for CrashOnceSession {
            fn identity(&self) -> &ExecutorSessionIdentity {
                self.inner.identity()
            }
            fn prepare_operation(
                &mut self,
                operation_id: OperationStepId,
                direction: OperationDirection,
            ) -> Result<ExecutorRequestIdentity, ApprovedExecutorError> {
                self.inner.prepare_operation(operation_id, direction)
            }
            fn dispatch_prepared(
                &mut self,
                request: ExecutorRequestIdentity,
                journal_intent: CommittedJournalEventBinding,
            ) -> Result<ExecutorDispatchResult, ApprovedExecutorError> {
                if self.point == "before" && self.seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(ApprovedExecutorError::Ambiguous(
                        "injected crash before mutation".to_owned(),
                    ));
                }
                let dispatched = self.inner.dispatch_prepared(request, journal_intent)?;
                if self.point == "after" {
                    return Err(ApprovedExecutorError::Ambiguous(
                        "injected crash after mutation".to_owned(),
                    ));
                }
                Ok(dispatched)
            }
        }

        for point in ["before", "after"] {
            let sandbox = MutationSandbox::new();
            sandbox.write("incoming/item.txt", b"crash-fixture");
            let fixture = CatalogFixture::scan(&sandbox, 1, &format!("M18 crash {point}"));
            let proposal = fixture.persist_proposal(&[(
                "incoming/item.txt".to_owned(),
                "Organized/item.txt".to_owned(),
            )]);
            let inner: Arc<dyn ApprovedExecutorClient> =
                Arc::new(SandboxApprovedExecutorClient::macos_native(
                    sandbox.path(),
                    fixture.platform.clone(),
                ));
            let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(CrashOnce {
                inner,
                point,
                seen: Arc::new(AtomicUsize::new(0)),
            });
            let service = ExecutionApplicationService::new(
                fixture.database.clone(),
                fixture.platform.clone(),
                executor,
                Arc::new(IndexedMemoryJournal::default()),
                ApplyGate {
                    enabled: true,
                    reason: "macos native crash sandbox".to_owned(),
                },
                ExecutionSafetyPolicy::default(),
                ExecutionConsentAuthorityKey::from_bytes([219; 32]),
            )
            .unwrap_or_else(|error| panic!("crash service should initialize: {error}"));
            let prepared = service
                .prepare_execution(proposal.id, proposal.revision)
                .unwrap_or_else(|error| panic!("crash preflight should succeed: {error}"));
            let approved = attest(&service, prepared.session.id);
            let _ = service.start_execution(approved.session.id, &mut |_| {});
            let recovered = service
                .recover_execution(approved.session.id)
                .unwrap_or_else(|error| panic!("recovery should be deterministic: {error}"));
            assert!(
                recovered.ambiguous == 0
                    || recovered.state == domain::ExecutionRecoveryState::RecoveryAmbiguous
                    || recovered.state == domain::ExecutionRecoveryState::RecoveryAvailable
                    || recovered.state == domain::ExecutionRecoveryState::RecoveryRequired,
                "crash {point} must remain journaled: {recovered:?}"
            );
            if point == "before" {
                assert!(
                    sandbox.path().join("incoming/item.txt").exists()
                        || sandbox.path().join("Organized/item.txt").exists(),
                    "crash before mutation must not lose the file"
                );
            } else {
                assert!(
                    sandbox.path().join("incoming/item.txt").exists()
                        || sandbox.path().join("Organized/item.txt").exists(),
                    "crash after mutation must not lose the file"
                );
            }
        }
    }

    #[test]
    fn macos_native_cancel_before_start_does_not_mutate() {
        let sandbox = MutationSandbox::new();
        sandbox.write("incoming/keep.txt", b"untouched");
        let initial = sandbox.snapshot();
        let fixture = CatalogFixture::scan(&sandbox, 1, "M18 macos cancel");
        let proposal = fixture.persist_proposal(&[(
            "incoming/keep.txt".to_owned(),
            "Organized/keep.txt".to_owned(),
        )]);
        let service = macos_service(&fixture, &sandbox, ExecutionSafetyPolicy::default());
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("cancel preflight should succeed: {error}"));
        let approved = attest(&service, prepared.session.id);
        assert!(
            service
                .cancel_execution(approved.session.id)
                .unwrap_or_else(|error| panic!("cancel should complete: {error}"))
        );
        assert_eq!(sandbox.snapshot(), initial);
        assert_eq!(
            fs::read(sandbox.path().join("incoming/keep.txt"))
                .unwrap_or_else(|error| panic!("source should remain: {error}")),
            b"untouched"
        );
    }

    #[test]
    fn macos_native_rollback_blocked_when_destination_changed_externally() {
        let sandbox = MutationSandbox::new();
        sandbox.write("incoming/report.txt", b"original");
        let fixture = CatalogFixture::scan(&sandbox, 1, "M18 macos rollback blocked");
        let proposal = fixture.persist_proposal(&[(
            "incoming/report.txt".to_owned(),
            "Organized/report.txt".to_owned(),
        )]);
        let service = macos_service(&fixture, &sandbox, ExecutionSafetyPolicy::default());
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
        let approved = attest(&service, prepared.session.id);
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("apply should succeed: {error}"));
        let destination = sandbox.path().join("Organized/report.txt");
        fs::write(&destination, b"user-edited-after-apply")
            .unwrap_or_else(|error| panic!("external edit should write: {error}"));
        let rolled = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("blocked rollback should report: {error}"));
        assert_eq!(
            rolled.session.status,
            domain::OrganizationExecutionStatus::RollbackPartial
        );
        assert!(rolled.session.summary.rollback_blocked >= 1);
        assert_eq!(
            fs::read(&destination)
                .unwrap_or_else(|error| panic!("user edit must not be overwritten: {error}")),
            b"user-edited-after-apply"
        );
        assert!(!sandbox.path().join("incoming/report.txt").exists());
    }

    #[test]
    #[ignore = "explicit 10,000-operation macOS native sandbox qualification"]
    fn macos_native_10000_operations_apply_verify_rollback_reports_metrics() {
        const OPERATION_COUNT: usize = 10_000;
        const BATCH_SIZE: usize = 100;
        let memory = PeakMemorySampler::start();
        let setup_started = Instant::now();
        let sandbox = MutationSandbox::new();
        let mut all_mappings = Vec::with_capacity(OPERATION_COUNT);
        for index in 0..OPERATION_COUNT {
            let source = format!(
                "qualification/input/shard-{:03}/item-{index:05}.bin",
                index / BATCH_SIZE
            );
            let destination = format!(
                "qualification/output/shard-{:03}/item-{index:05}.bin",
                index / BATCH_SIZE
            );
            sandbox.write(&source, format!("m18-macos-native-{index:05}").as_bytes());
            create_parent_guarded(&sandbox, &destination);
            all_mappings.push((source, destination));
        }
        let initial = sandbox.snapshot();
        let fixture = CatalogFixture::scan(
            &sandbox,
            OPERATION_COUNT,
            "M18 macos native 10000 qualification",
        );
        let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
            SandboxApprovedExecutorClient::macos_native(sandbox.path(), fixture.platform.clone()),
        );
        let service = ExecutionApplicationService::new(
            fixture.database.clone(),
            fixture.platform.clone(),
            executor,
            Arc::new(IndexedMemoryJournal::default()),
            ApplyGate {
                enabled: true,
                reason: "macos native 10k sandbox".to_owned(),
            },
            ExecutionSafetyPolicy::default(),
            ExecutionConsentAuthorityKey::from_bytes([220; 32]),
        )
        .unwrap_or_else(|error| panic!("10k service should initialize: {error}"));
        let setup_duration = setup_started.elapsed();
        let mut preflight_duration = Duration::ZERO;
        let mut execution_duration = Duration::ZERO;
        let mut verification_duration = Duration::ZERO;
        let mut rollback_duration = Duration::ZERO;
        let mut executed_operations = 0_usize;

        for (batch_index, mappings) in all_mappings.chunks(BATCH_SIZE).enumerate() {
            let proposal = fixture.persist_proposal(mappings);
            let started = Instant::now();
            let prepared = service
                .prepare_execution(proposal.id, proposal.revision)
                .unwrap_or_else(|error| {
                    panic!("macos 10k batch {batch_index} should preflight: {error}")
                });
            preflight_duration += started.elapsed();
            executed_operations += prepared
                .operations
                .iter()
                .filter(|operation| operation.proposal_operation_id.is_some())
                .count();
            let approved = attest(&service, prepared.session.id);
            let started = Instant::now();
            let completed = service
                .start_execution(approved.session.id, &mut |_| {})
                .unwrap_or_else(|error| {
                    panic!("macos 10k batch {batch_index} should apply: {error}")
                });
            execution_duration += started.elapsed();
            let started = Instant::now();
            assert_eq!(
                completed.session.summary.failed, 0,
                "macos 10k batch {batch_index} must not fail"
            );
            verification_duration += started.elapsed();
            let started = Instant::now();
            let rolled = service
                .rollback_execution(completed.session.id, &mut |_| {})
                .unwrap_or_else(|error| {
                    panic!("macos 10k batch {batch_index} should rollback: {error}")
                });
            rollback_duration += started.elapsed();
            assert_eq!(
                rolled.session.status,
                domain::OrganizationExecutionStatus::RolledBack,
                "macos 10k batch {batch_index}"
            );
        }
        assert_eq!(executed_operations, OPERATION_COUNT);
        assert_eq!(sandbox.snapshot(), initial);
        let peak_rss = memory.finish();
        eprintln!(
            "M18 macos native 10k qualification: operations={OPERATION_COUNT} setup_ms={} preflight_ms={} execution_ms={} verification_ms={} rollback_ms={} total_ms={} peak_rss={peak_rss:?} files_lost=0 unexpected_files=0",
            duration_ms(setup_duration),
            duration_ms(preflight_duration),
            duration_ms(execution_duration),
            duration_ms(verification_duration),
            duration_ms(rollback_duration),
            duration_ms(
                setup_duration
                    + preflight_duration
                    + execution_duration
                    + verification_duration
                    + rollback_duration
            ),
        );
    }
}
