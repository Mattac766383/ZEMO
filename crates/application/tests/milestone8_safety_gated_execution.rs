mod support;

use application::{
    ApprovedExecutorClient, ApprovedExecutorError, ApprovedExecutorSession,
    ExecutionApplicationService, ExecutionConsentAuthorityKey, ExecutorDispatchResult,
    ScannerApplicationService, executor_response_digest,
};
use domain::{
    ExecutionConsentState, ExecutionId, ExecutionOperationKind, ExecutionOperationStatus,
    ExecutionRecoveryState, ExecutorRequestDirection, ExecutorRequestIdentity,
    ExecutorRequestState, ExecutorSessionIdentity, ExecutorSessionPurpose, JournalEventKind,
    OperationJournalEvent, OperationStepId, OrganizationExecutionStatus, OrganizationProposal,
    OrganizationProposalStatus, ProposalOperationKind, ProposalOverrideAction, WorkspaceId,
};
use ipc_contracts::executor_v2::{
    CommittedJournalEventBinding, ExecutorAttemptAudit, ExecutorOutcome,
    ImmutableExecutionEnvelope, OperationDirection, SessionAuthorization,
};
use operations::{
    ApplyGate, DurableJournal, ExecutionSafetyPolicy, FileJournal, JournalKey, LockedJournal,
    MemoryJournal, OperationsError,
};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};
use support::{MutationSandbox, SandboxApprovedExecutorClient, assert_is_test_sandbox};

#[cfg(target_os = "macos")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_macos::MacOsPlatform)
}

#[cfg(target_os = "windows")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_windows::WindowsPlatform)
}

fn approved_fixture(
    sandbox: &MutationSandbox,
) -> (
    Arc<Database>,
    Arc<dyn ReadOnlyPlatform>,
    Arc<ScannerApplicationService>,
    WorkspaceId,
    OrganizationProposal,
) {
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([108; 32]))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    approved_fixture_with_database(sandbox, database)
}

fn approved_fixture_with_database(
    sandbox: &MutationSandbox,
    database: Arc<Database>,
) -> (
    Arc<Database>,
    Arc<dyn ReadOnlyPlatform>,
    Arc<ScannerApplicationService>,
    WorkspaceId,
    OrganizationProposal,
) {
    let fixtures = [
        (
            "Downloads/invoice-dupont.txt",
            "FACTURE\nCustomer: Dupont SARL\nSupplier: Point P\nProject: Bordeaux\nProject reference: BDX-2026\nInvoice number: FP-39482\nDate: 2026-06-17\nTotal: 1437.82 EUR",
        ),
        (
            "Downloads/invoice-martin.txt",
            "INVOICE\nCustomer: Martin SAS\nSupplier: Office Local\nProject: Renovation\nProject reference: REN-2026\nInvoice number: INV-8821\nDate: 2026-07-01\nTotal: 452.10 EUR",
        ),
        (
            "Downloads/quote-dupont.txt",
            "DEVIS\nCustomer: Dupont SARL\nProject: Bordeaux\nProject reference: BDX-2026\nQuote number: Q-2026-14\nDate: 2026-06-01\nMontant: 900 EUR",
        ),
        (
            "Downloads/invoice-garcia.txt",
            "INVOICE\nCustomer: Garcia SARL\nSupplier: Local Supply\nProject: Kitchen\nProject reference: KIT-2026\nInvoice number: INV-9912\nDate: 2026-07-08\nTotal: 718.42 EUR",
        ),
    ];
    for (path, content) in fixtures {
        sandbox.write(path, content.as_bytes());
    }
    let platform = native_platform();
    let scanner = Arc::new(ScannerApplicationService::new(
        database.clone(),
        platform.clone(),
    ));
    let workspace = scanner
        .create_workspace("Milestone 8 mutation sandbox")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    scanner
        .register_root(workspace.id, sandbox.path())
        .unwrap_or_else(|error| panic!("sandbox root should register: {error}"));
    let scan = scanner
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should succeed: {error}"));
    scanner
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("content analysis should succeed: {error}"));
    scanner
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantic analysis should succeed: {error}"));
    scanner
        .resolve_workspace_identities(workspace.id, "manual", true, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("identity resolution should succeed: {error}"));
    let proposal = scanner
        .generate_organization_proposal(workspace.id, false, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("proposal should build: {error}"));
    let file_ids = proposal
        .operations
        .iter()
        .map(|operation| (operation.file_id, operation.source_name.clone()))
        .collect::<Vec<_>>();
    let mut proposal = proposal;
    for (index, (file_id, source_name)) in file_ids.into_iter().enumerate() {
        let (destination, proposed_name) = match index {
            0 => (
                vec![
                    "Organized".to_owned(),
                    "Approved".to_owned(),
                    "Dupont".to_owned(),
                ],
                source_name,
            ),
            1 => (
                vec!["Organized".to_owned(), "Approved-1".to_owned()],
                format!("renamed-{source_name}"),
            ),
            _ => (
                vec!["Downloads".to_owned()],
                format!("renamed-{source_name}"),
            ),
        };
        proposal = scanner
            .set_organization_proposal_override(
                proposal.id,
                file_id,
                ProposalOverrideAction::DestinationAndRename,
                Some(destination),
                Some(proposed_name),
                Some("Explicitly approved M8 sandbox destination".to_owned()),
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("sandbox override should persist: {error}"));
    }
    let proposal = scanner
        .set_organization_proposal_status(
            proposal.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("proposal should be explicitly approved: {error}"));
    (database, platform, scanner, workspace.id, proposal)
}

fn execution_service(
    database: Arc<Database>,
    platform: Arc<dyn ReadOnlyPlatform>,
    sandbox: &MutationSandbox,
) -> ExecutionApplicationService {
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    execution_service_with(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
    )
}

fn execution_service_with(
    database: Arc<Database>,
    platform: Arc<dyn ReadOnlyPlatform>,
    executor: Arc<dyn ApprovedExecutorClient>,
    journal: Arc<dyn DurableJournal>,
) -> ExecutionApplicationService {
    execution_service_with_security(
        database,
        platform,
        executor,
        journal,
        ExecutionSafetyPolicy::default(),
        [111; 32],
    )
}

fn execution_service_with_security(
    database: Arc<Database>,
    platform: Arc<dyn ReadOnlyPlatform>,
    executor: Arc<dyn ApprovedExecutorClient>,
    journal: Arc<dyn DurableJournal>,
    policy: ExecutionSafetyPolicy,
    consent_authority: [u8; 32],
) -> ExecutionApplicationService {
    ExecutionApplicationService::new(
        database,
        platform,
        executor,
        journal,
        ApplyGate {
            enabled: true,
            reason: "isolated mutation sandbox".to_owned(),
        },
        policy,
        ExecutionConsentAuthorityKey::from_bytes(consent_authority),
    )
    .unwrap_or_else(|error| panic!("execution service should initialize: {error}"))
}

fn qualified_case_only_policy() -> ExecutionSafetyPolicy {
    let mut policy = ExecutionSafetyPolicy::default();
    policy.allow_qualified_case_only_rename = true;
    policy
}

fn attest_execution(
    service: &ExecutionApplicationService,
    execution_id: ExecutionId,
) -> Result<domain::ExecutionDetail, application::ApplicationError> {
    let challenge = service.create_execution_consent_challenge(execution_id, None)?;
    service.finalize_execution_consent(challenge)
}

fn case_only_revision(
    scanner: &ScannerApplicationService,
    proposal: &OrganizationProposal,
    selected_count: usize,
    exclude_unselected: bool,
) -> (OrganizationProposal, Vec<(domain::FileId, String, String)>) {
    let selected = proposal
        .operations
        .iter()
        .take(selected_count)
        .map(|operation| {
            (
                operation.file_id,
                operation.source.relative_path.clone(),
                operation.source_name.to_ascii_uppercase(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), selected_count);
    let mut revision = proposal.clone();
    for (file_id, source, destination_name) in &selected {
        revision = scanner
            .set_organization_proposal_override(
                revision.id,
                *file_id,
                ProposalOverrideAction::DestinationAndRename,
                Some(parent_segments(source)),
                Some(destination_name.clone()),
                Some("Qualification-only case rename fixture".to_owned()),
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("case-only override should persist: {error}"));
    }
    if exclude_unselected {
        for operation in proposal.operations.iter().filter(|operation| {
            !selected
                .iter()
                .any(|selected| selected.0 == operation.file_id)
        }) {
            revision = scanner
                .set_organization_proposal_override(
                    revision.id,
                    operation.file_id,
                    ProposalOverrideAction::KeepInPlace,
                    None,
                    None,
                    Some("Excluded from case-only qualification fixture".to_owned()),
                    &|| false,
                    &mut |_| {},
                )
                .unwrap_or_else(|error| panic!("unselected override should persist: {error}"));
        }
    }
    revision = scanner
        .set_organization_proposal_status(
            revision.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("case-only revision should approve: {error}"));
    (revision, selected)
}

#[derive(Clone, Copy)]
enum CrashPoint {
    BeforeMutation,
    AfterMutation,
}

struct CrashExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    point: CrashPoint,
    mutations: Arc<AtomicUsize>,
}

impl CrashExecutorClient {
    fn new(
        sandbox: &MutationSandbox,
        reader: Arc<dyn ReadOnlyPlatform>,
        point: CrashPoint,
    ) -> Self {
        Self {
            inner: Arc::new(SandboxApprovedExecutorClient::new(sandbox.path(), reader)),
            point,
            mutations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ApprovedExecutorClient for CrashExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        Ok(Box::new(CrashExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            point: self.point,
            mutations: self.mutations.clone(),
        }))
    }
}

struct CrashExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    point: CrashPoint,
    mutations: Arc<AtomicUsize>,
}

impl ApprovedExecutorSession for CrashExecutorSession {
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
        let first = self.mutations.fetch_add(1, Ordering::SeqCst) == 0;
        if first && matches!(self.point, CrashPoint::BeforeMutation) {
            panic!("simulated process crash before native mutation");
        }
        let result = self.inner.dispatch_prepared(request, journal_intent);
        if first && result.is_ok() && matches!(self.point, CrashPoint::AfterMutation) {
            panic!("simulated process crash after native mutation");
        }
        result
    }
}

struct CrashAfterStageExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    crashed: Arc<AtomicBool>,
}

impl CrashAfterStageExecutorClient {
    fn new(sandbox: &MutationSandbox, reader: Arc<dyn ReadOnlyPlatform>) -> Self {
        Self {
            inner: Arc::new(SandboxApprovedExecutorClient::new(sandbox.path(), reader)),
            crashed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ApprovedExecutorClient for CrashAfterStageExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        let stage_id = envelope
            .operations
            .iter()
            .find(|operation| {
                matches!(
                    operation.primitive,
                    ipc_contracts::executor_v2::OperationPrimitiveManifest::InternalStage { .. }
                )
            })
            .map(|operation| operation.operation_id.parse())
            .transpose()
            .map_err(|_| ApprovedExecutorError::Unavailable("invalid stage id".to_owned()))?
            .ok_or_else(|| {
                ApprovedExecutorError::Unavailable("case-only stage is missing".to_owned())
            })?;
        Ok(Box::new(CrashAfterStageExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            stage_id,
            crashed: self.crashed.clone(),
        }))
    }
}

struct CrashAfterStageExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    stage_id: OperationStepId,
    crashed: Arc<AtomicBool>,
}

impl ApprovedExecutorSession for CrashAfterStageExecutorSession {
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
        let operation_id = request.operation_id;
        let forward = request.direction == ExecutorRequestDirection::Forward;
        let outcome = self.inner.dispatch_prepared(request, journal_intent);
        if operation_id == self.stage_id
            && forward
            && outcome.is_ok()
            && !self.crashed.swap(true, Ordering::SeqCst)
        {
            panic!("simulated crash after the internal staging transition");
        }
        outcome
    }
}

#[derive(Default)]
struct ExecutorAudit {
    sessions: Mutex<Vec<SessionAuthorization>>,
    calls: Mutex<Vec<(OperationDirection, CommittedJournalEventBinding)>>,
}

struct AuditedExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    journal: Arc<MemoryJournal>,
    audit: Arc<ExecutorAudit>,
}

impl AuditedExecutorClient {
    fn new(
        sandbox: &MutationSandbox,
        reader: Arc<dyn ReadOnlyPlatform>,
        journal: Arc<MemoryJournal>,
    ) -> (Self, Arc<ExecutorAudit>) {
        let audit = Arc::new(ExecutorAudit::default());
        (
            Self {
                inner: Arc::new(SandboxApprovedExecutorClient::new(sandbox.path(), reader)),
                journal,
                audit: audit.clone(),
            },
            audit,
        )
    }
}

impl ApprovedExecutorClient for AuditedExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        let execution_id = envelope
            .execution_id
            .parse()
            .map_err(|_| ApprovedExecutorError::Unavailable("invalid execution id".to_owned()))?;
        self.audit
            .sessions
            .lock()
            .unwrap_or_else(|error| panic!("executor audit should lock: {error}"))
            .push(authorization.clone());
        Ok(Box::new(AuditedExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            journal: self.journal.clone(),
            execution_id,
            audit: self.audit.clone(),
        }))
    }
}

struct AuditedExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    journal: Arc<MemoryJournal>,
    execution_id: ExecutionId,
    audit: Arc<ExecutorAudit>,
}

impl ApprovedExecutorSession for AuditedExecutorSession {
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
        let operation_id = request.operation_id;
        let direction = match request.direction {
            ExecutorRequestDirection::Forward => OperationDirection::Forward,
            ExecutorRequestDirection::Rollback => OperationDirection::Rollback,
        };
        let events = self
            .journal
            .events(self.execution_id)
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        let intent = events.last().ok_or_else(|| {
            ApprovedExecutorError::Unavailable(
                "executor was called before a durable intent event".to_owned(),
            )
        })?;
        let precondition = events.get(events.len().saturating_sub(2)).ok_or_else(|| {
            ApprovedExecutorError::Unavailable(
                "executor was called before durable preconditions".to_owned(),
            )
        })?;
        assert_eq!(intent.step_id, Some(operation_id));
        assert_eq!(intent.sequence, journal_intent.database_sequence);
        assert_eq!(
            intent.event_digest,
            *journal_intent.database_event_digest.as_bytes()
        );
        assert_eq!(
            journal_intent.database_sequence,
            journal_intent.external_sequence
        );
        assert_eq!(
            journal_intent.database_event_digest,
            journal_intent.external_event_digest
        );
        assert_eq!(precondition.step_id, Some(operation_id));
        assert_eq!(precondition.kind, JournalEventKind::PreconditionsValidated);
        assert_eq!(
            intent.kind,
            if direction == OperationDirection::Forward {
                JournalEventKind::IntentDurable
            } else {
                JournalEventKind::RollbackIntent
            }
        );
        self.audit
            .calls
            .lock()
            .unwrap_or_else(|error| panic!("executor audit should lock: {error}"))
            .push((direction.clone(), journal_intent.clone()));
        self.inner.dispatch_prepared(request, journal_intent)
    }
}

#[derive(Clone, Copy)]
enum ForcedExecutorOutcome {
    ProvenNotApplied,
    Ambiguous,
}

struct ForcedExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    outcome: ForcedExecutorOutcome,
}

impl ForcedExecutorClient {
    fn new(
        sandbox: &MutationSandbox,
        reader: Arc<dyn ReadOnlyPlatform>,
        outcome: ForcedExecutorOutcome,
    ) -> Self {
        Self {
            inner: Arc::new(SandboxApprovedExecutorClient::new(sandbox.path(), reader)),
            outcome,
        }
    }
}

impl ApprovedExecutorClient for ForcedExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        let target = envelope
            .operations
            .iter()
            .find(|operation| operation.proposal_operation_id.is_some())
            .map(|operation| operation.operation_id.clone())
            .ok_or_else(|| {
                ApprovedExecutorError::Unavailable(
                    "test envelope has no approved file operation".to_owned(),
                )
            })?;
        Ok(Box::new(ForcedExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            target,
            outcome: self.outcome,
        }))
    }
}

struct ForcedExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    target: String,
    outcome: ForcedExecutorOutcome,
}

impl ApprovedExecutorSession for ForcedExecutorSession {
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
            return match self.outcome {
                ForcedExecutorOutcome::ProvenNotApplied => {
                    let outcome = ExecutorOutcome::ProvenNotApplied {
                        code: "source_precondition_changed".to_owned(),
                        detail: "The executor proved that it did not mutate the source.".to_owned(),
                        audit: ExecutorAttemptAudit {
                            attempt_count: 1,
                            error_class: None,
                        },
                    };
                    Ok(ExecutorDispatchResult {
                        response_digest_hex: executor_response_digest(&request, &outcome)?,
                        outcome,
                    })
                }
                ForcedExecutorOutcome::Ambiguous => Err(ApprovedExecutorError::Ambiguous(
                    "mock authenticated response had an invalid MAC".to_owned(),
                )),
            };
        }
        self.inner.dispatch_prepared(request, journal_intent)
    }
}

struct AckThenDatabaseFailureExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    database_path: PathBuf,
    database_key: [u8; 32],
}

impl ApprovedExecutorClient for AckThenDatabaseFailureExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        Ok(Box::new(AckThenDatabaseFailureExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            database_path: self.database_path.clone(),
            database_key: self.database_key,
            inject_failure: true,
        }))
    }
}

struct AckThenDatabaseFailureExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    database_path: PathBuf,
    database_key: [u8; 32],
    inject_failure: bool,
}

impl ApprovedExecutorSession for AckThenDatabaseFailureExecutorSession {
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
        let acknowledged = self.inner.dispatch_prepared(request, journal_intent)?;
        if self.inject_failure {
            install_executor_response_failure(&self.database_path, &self.database_key);
            self.inject_failure = false;
        }
        Ok(acknowledged)
    }
}

fn with_raw_test_database(path: &Path, key: &[u8; 32], action: impl FnOnce(&rusqlite::Connection)) {
    let connection = rusqlite::Connection::open(path)
        .unwrap_or_else(|error| panic!("test database should reopen: {error}"));
    let key_hex = key
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    connection
        .execute_batch(&format!(
            "PRAGMA key = \"x'{key_hex}'\"; PRAGMA busy_timeout = 5000;"
        ))
        .unwrap_or_else(|error| panic!("test database key should apply: {error}"));
    action(&connection);
}

fn install_executor_response_failure(path: &Path, key: &[u8; 32]) {
    with_raw_test_database(path, key, |connection| {
        connection
            .execute_batch(
                "CREATE TRIGGER fail_executor_response_after_ack
                 BEFORE UPDATE OF response_digest ON local_executor_requests
                 WHEN NEW.response_digest IS NOT NULL
                 BEGIN
                    SELECT RAISE(ABORT, 'injected response commit failure');
                 END;",
            )
            .unwrap_or_else(|error| panic!("response failure trigger should install: {error}"));
    });
}

fn remove_executor_response_failure(path: &Path, key: &[u8; 32]) {
    with_raw_test_database(path, key, |connection| {
        connection
            .execute_batch("DROP TRIGGER fail_executor_response_after_ack;")
            .unwrap_or_else(|error| panic!("response failure trigger should be removed: {error}"));
    });
}

struct FailingFlushJournal {
    inner: MemoryJournal,
    fail_after_intent: AtomicUsize,
}

impl FailingFlushJournal {
    fn new() -> Self {
        Self {
            inner: MemoryJournal::default(),
            fail_after_intent: AtomicUsize::new(0),
        }
    }
}

impl DurableJournal for FailingFlushJournal {
    fn append(&self, event: OperationJournalEvent) -> Result<(), OperationsError> {
        if event.kind == JournalEventKind::IntentDurable {
            self.fail_after_intent.store(1, Ordering::SeqCst);
        }
        self.inner.append(event)
    }

    fn flush(&self) -> Result<(), OperationsError> {
        if self.fail_after_intent.swap(0, Ordering::SeqCst) == 1 {
            return Err(OperationsError::Journal(
                "injected durable flush failure".to_owned(),
            ));
        }
        self.inner.flush()
    }

    fn events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError> {
        self.inner.events(execution_id)
    }

    fn diagnostics(&self) -> Vec<domain::JournalDiagnostic> {
        Vec::new()
    }
}

#[derive(Default)]
struct BlockingState {
    entered: bool,
    released: bool,
}

struct BoundaryBlockingExecutorClient {
    inner: Arc<dyn ApprovedExecutorClient>,
    boundary: Arc<(Mutex<BlockingState>, Condvar)>,
    mutations: Arc<AtomicUsize>,
}

impl BoundaryBlockingExecutorClient {
    fn new(
        sandbox: &MutationSandbox,
        reader: Arc<dyn ReadOnlyPlatform>,
        boundary: Arc<(Mutex<BlockingState>, Condvar)>,
    ) -> Self {
        Self {
            inner: Arc::new(SandboxApprovedExecutorClient::new(sandbox.path(), reader)),
            boundary,
            mutations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

struct BoundaryBlockingExecutorSession {
    inner: Box<dyn ApprovedExecutorSession>,
    boundary: Arc<(Mutex<BlockingState>, Condvar)>,
    mutations: Arc<AtomicUsize>,
}

impl BoundaryBlockingExecutorSession {
    fn wait_at_first_boundary(&self) {
        if self.mutations.fetch_add(1, Ordering::SeqCst) != 0 {
            return;
        }
        let (lock, condition) = &*self.boundary;
        let mut state = lock
            .lock()
            .unwrap_or_else(|error| panic!("boundary state should lock: {error}"));
        state.entered = true;
        condition.notify_all();
        while !state.released {
            state = condition
                .wait(state)
                .unwrap_or_else(|error| panic!("boundary wait should resume: {error}"));
        }
    }
}

impl ApprovedExecutorClient for BoundaryBlockingExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        Ok(Box::new(BoundaryBlockingExecutorSession {
            inner: self.inner.open_session(envelope, authorization)?,
            boundary: self.boundary.clone(),
            mutations: self.mutations.clone(),
        }))
    }
}

impl ApprovedExecutorSession for BoundaryBlockingExecutorSession {
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
        self.wait_at_first_boundary();
        self.inner.dispatch_prepared(request, journal_intent)
    }
}

#[test]
fn approved_plan_applies_with_write_ahead_journal_and_round_trip_rollback() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let initial = sandbox.snapshot();
    let journal = Arc::new(MemoryJournal::default());
    let (executor, audit) = AuditedExecutorClient::new(&sandbox, platform.clone(), journal.clone());
    let service = execution_service_with(database.clone(), platform, Arc::new(executor), journal);

    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("approved proposal should pass preflight: {error}"));
    assert_eq!(
        prepared.session.status,
        OrganizationExecutionStatus::AwaitingConfirmation
    );
    assert!(prepared.session.summary.preflight_ok > 0);
    assert!(!prepared.session.approval.user_confirmed);
    assert_eq!(
        prepared.session.approval.operation_count,
        prepared.session.summary.affected_files
    );
    let mut executable_proposal_ids = prepared
        .operations
        .iter()
        .filter(|operation| operation.status == ExecutionOperationStatus::PreflightOk)
        .filter_map(|operation| operation.proposal_operation_id)
        .collect::<Vec<_>>();
    executable_proposal_ids.sort_unstable();
    assert_eq!(
        prepared.session.approval.approved_operation_ids,
        executable_proposal_ids
    );
    assert_eq!(
        prepared.session.approval.operation_count,
        u64::try_from(executable_proposal_ids.len())
            .unwrap_or_else(|_| panic!("approved operation count should fit"))
    );
    assert!(
        prepared
            .operations
            .iter()
            .filter(|operation| operation.proposal_operation_id.is_some())
            .all(|operation| {
                matches!(
                    operation.kind,
                    ExecutionOperationKind::Move
                        | ExecutionOperationKind::Rename
                        | ExecutionOperationKind::MoveAndRename
                )
            })
    );
    let supported_kinds = prepared
        .operations
        .iter()
        .filter(|operation| operation.proposal_operation_id.is_some())
        .map(|operation| operation.kind)
        .collect::<Vec<_>>();
    assert!(supported_kinds.contains(&ExecutionOperationKind::Move));
    assert!(supported_kinds.contains(&ExecutionOperationKind::Rename));
    assert!(supported_kinds.contains(&ExecutionOperationKind::MoveAndRename));

    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("final confirmation should be recorded: {error}"));
    assert!(approved.session.approval.user_confirmed);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("safe execution should complete: {error}"));
    assert!(matches!(
        completed.session.status,
        OrganizationExecutionStatus::Completed | OrganizationExecutionStatus::Partial
    ));
    assert!(completed.session.summary.applied > 0);
    assert!(completed.session.rollback_available);
    assert!(
        database
            .validate_execution_journal(completed.session.id)
            .unwrap_or_else(|error| panic!("journal should validate: {error}"))
    );
    let forward_sessions = database
        .executor_session_facts(completed.session.id)
        .unwrap_or_else(|error| panic!("executor sessions should load: {error}"));
    assert_eq!(forward_sessions.len(), 1);
    assert_eq!(forward_sessions[0].purpose, ExecutorSessionPurpose::Forward);
    assert_eq!(forward_sessions[0].session_id.len(), 64);
    let forward_requests = database
        .executor_request_facts(completed.session.id)
        .unwrap_or_else(|error| panic!("executor requests should load: {error}"));
    assert!(!forward_requests.is_empty());
    assert!(forward_requests.iter().all(|request| {
        request.direction == ExecutorRequestDirection::Forward
            && matches!(
                request.state,
                ExecutorRequestState::ProvenApplied | ExecutorRequestState::ProvenNotStarted
            )
            && request.request_id.len() == 64
            && request.request_sequence > 0
    }));
    let completed_retention = database
        .execution_retention_metadata(completed.session.id)
        .unwrap_or_else(|error| panic!("retention metadata should load: {error}"));
    assert!(completed_retention.finalized_at.is_some());
    assert!(!completed_retention.active_recovery);
    assert!(completed_retention.rollback_eligible);
    assert!(completed_retention.cleanup_eligible_at.is_none());
    for operation in completed.operations.iter().filter(|operation| {
        operation.proposal_operation_id.is_some()
            && operation.status == ExecutionOperationStatus::Applied
    }) {
        let destination = sandbox.path().join(&operation.destination_relative_path);
        assert_is_test_sandbox(sandbox.path(), &destination);
        assert!(destination.is_file());
        let source = sandbox.path().join(
            operation
                .original_source_relative_path
                .as_deref()
                .unwrap_or_else(|| panic!("file operation should retain original source")),
        );
        assert!(!source.exists());
    }
    assert_ne!(initial, sandbox.snapshot());

    let rolled_back = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("rollback should complete: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(
        rolled_back.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    assert_eq!(initial, sandbox.snapshot());
    assert!(
        database
            .validate_execution_journal(rolled_back.session.id)
            .unwrap_or_else(|error| panic!("rollback journal should validate: {error}"))
    );
    let all_sessions = database
        .executor_session_facts(rolled_back.session.id)
        .unwrap_or_else(|error| panic!("executor sessions should reload: {error}"));
    assert_eq!(all_sessions.len(), 2);
    assert_ne!(all_sessions[0].session_id, all_sessions[1].session_id);
    assert_eq!(
        all_sessions[1].purpose,
        ExecutorSessionPurpose::Rollback,
        "rollback must use a fresh session identity"
    );
    let rollback_requests = database
        .executor_request_facts(rolled_back.session.id)
        .unwrap_or_else(|error| panic!("rollback request facts should load: {error}"))
        .into_iter()
        .filter(|request| request.direction == ExecutorRequestDirection::Rollback)
        .collect::<Vec<_>>();
    assert!(!rollback_requests.is_empty());
    assert!(
        rollback_requests
            .iter()
            .all(|request| request.state == ExecutorRequestState::ProvenApplied)
    );
    let rolled_back_retention = database
        .execution_retention_metadata(rolled_back.session.id)
        .unwrap_or_else(|error| panic!("rolled-back retention should load: {error}"));
    assert!(!rolled_back_retention.active_recovery);
    assert!(!rolled_back_retention.rollback_eligible);
    assert!(rolled_back_retention.minimum_retain_until.is_some());
    assert!(rolled_back_retention.cleanup_eligible_at.is_some());
    let sessions = audit
        .sessions
        .lock()
        .unwrap_or_else(|error| panic!("executor audit should lock: {error}"));
    assert!(matches!(
        sessions.first(),
        Some(SessionAuthorization::Forward)
    ));
    let Some(SessionAuthorization::Rollback {
        eligible_operations,
    }) = sessions.last()
    else {
        panic!("rollback must open a fresh exact rollback session");
    };
    assert!(!eligible_operations.is_empty());
    drop(sessions);
    let calls = audit
        .calls
        .lock()
        .unwrap_or_else(|error| panic!("executor audit should lock: {error}"));
    assert!(
        calls
            .iter()
            .any(|(direction, _)| *direction == OperationDirection::Forward)
    );
    assert!(
        calls
            .iter()
            .any(|(direction, _)| *direction == OperationDirection::Rollback)
    );
}

#[test]
fn executor_proven_not_applied_keeps_source_and_avoids_recovery() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let executor = Arc::new(ForcedExecutorClient::new(
        &sandbox,
        platform.clone(),
        ForcedExecutorOutcome::ProvenNotApplied,
    ));
    let service = execution_service_with(
        database.clone(),
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("proposal should prepare: {error}"));
    let source = prepared
        .operations
        .iter()
        .find(|operation| operation.proposal_operation_id.is_some())
        .and_then(|operation| operation.source_relative_path.as_deref())
        .map(|relative| sandbox.path().join(relative))
        .unwrap_or_else(|| panic!("approved operation should have a source"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("execution should attest: {error}"));

    let stopped = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("proven non-application is not ambiguous: {error}"));
    assert!(source.is_file());
    assert_ne!(
        stopped.session.status,
        OrganizationExecutionStatus::RecoveryRequired
    );
    assert_eq!(
        stopped.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    let failed_event = database
        .execution_journal_events(stopped.session.id)
        .unwrap_or_else(|error| panic!("journal should load: {error}"))
        .into_iter()
        .find(|event| event.kind == JournalEventKind::StepFailed)
        .unwrap_or_else(|| panic!("executor refusal should persist a failed step"));
    let payload: serde_json::Value = serde_json::from_slice(&failed_event.payload)
        .unwrap_or_else(|error| panic!("failed-step payload should decode: {error}"));
    assert_eq!(payload["executor_audit"]["attempt_count"], 1);
}

#[test]
fn ambiguous_executor_response_forces_recovery_without_retry() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let executor = Arc::new(ForcedExecutorClient::new(
        &sandbox,
        platform.clone(),
        ForcedExecutorOutcome::Ambiguous,
    ));
    let service = execution_service_with(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("proposal should prepare: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("execution should attest: {error}"));

    assert!(matches!(
        service.start_execution(approved.session.id, &mut |_| {}),
        Err(application::ApplicationError::ExecutionRecoveryRequired)
    ));
    let detail = service
        .execution_status(approved.session.id)
        .unwrap_or_else(|error| panic!("recovery status should persist: {error}"));
    assert_eq!(
        detail.session.status,
        OrganizationExecutionStatus::RecoveryRequired
    );
}

#[test]
fn swap_cycle_uses_internal_staging_without_overwrite_and_rolls_back() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let initial = sandbox.snapshot();
    let selected = proposal
        .operations
        .iter()
        .take(2)
        .map(|operation| {
            (
                operation.file_id,
                operation.source.relative_path.clone(),
                operation.source_name.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 2);
    let first_bytes = fs::read(sandbox.path().join(normalized_path(&selected[0].1)))
        .unwrap_or_else(|error| panic!("first source should be readable: {error}"));
    let second_bytes = fs::read(sandbox.path().join(normalized_path(&selected[1].1)))
        .unwrap_or_else(|error| panic!("second source should be readable: {error}"));

    let mut cycle = scanner
        .set_organization_proposal_override(
            proposal.id,
            selected[0].0,
            ProposalOverrideAction::DestinationAndRename,
            Some(parent_segments(&selected[1].1)),
            Some(selected[1].2.clone()),
            Some("Explicit swap destination A to B".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("first swap override should persist: {error}"));
    cycle = scanner
        .set_organization_proposal_override(
            cycle.id,
            selected[1].0,
            ProposalOverrideAction::DestinationAndRename,
            Some(parent_segments(&selected[0].1)),
            Some(selected[0].2.clone()),
            Some("Explicit swap destination B to A".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("second swap override should persist: {error}"));
    for operation in cycle
        .operations
        .clone()
        .into_iter()
        .filter(|operation| !selected.iter().any(|item| item.0 == operation.file_id))
    {
        cycle = scanner
            .set_organization_proposal_override(
                cycle.id,
                operation.file_id,
                ProposalOverrideAction::KeepInPlace,
                None,
                None,
                Some("Excluded from the explicit swap test".to_owned()),
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("unselected operation should be excluded: {error}"));
    }
    cycle = scanner
        .set_organization_proposal_status(
            cycle.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("swap revision should be approved: {error}"));
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(cycle.id, cycle.revision)
        .unwrap_or_else(|error| panic!("swap should produce a safe staged plan: {error}"));
    assert_eq!(
        prepared
            .operations
            .iter()
            .filter(|operation| operation.kind == ExecutionOperationKind::InternalStage)
            .count(),
        2
    );
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("swap confirmation should persist: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("staged swap should complete: {error}"));
    assert_eq!(
        completed.session.status,
        OrganizationExecutionStatus::Completed
    );
    assert_eq!(
        fs::read(sandbox.path().join(normalized_path(&selected[0].1)))
            .unwrap_or_else(|error| panic!("first swapped path should be readable: {error}")),
        second_bytes
    );
    assert_eq!(
        fs::read(sandbox.path().join(normalized_path(&selected[1].1)))
            .unwrap_or_else(|error| panic!("second swapped path should be readable: {error}")),
        first_bytes
    );
    let rolled_back = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("staged swap rollback should complete: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(
        rolled_back.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn case_only_rename_is_blocked_by_the_default_production_policy() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let selected = proposal
        .operations
        .first()
        .unwrap_or_else(|| panic!("fixture should contain a proposal operation"));
    let case_name = selected.source_name.to_ascii_uppercase();
    assert_ne!(case_name, selected.source_name);
    let revision = scanner
        .set_organization_proposal_override(
            proposal.id,
            selected.file_id,
            ProposalOverrideAction::DestinationAndRename,
            Some(parent_segments(&selected.source.relative_path)),
            Some(case_name),
            Some("Default case-only refusal fixture".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("case-only override should persist: {error}"));
    let revision = scanner
        .set_organization_proposal_status(
            revision.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("revision should approve: {error}"));
    let service = execution_service(database, platform, &sandbox);

    let prepared = service
        .prepare_execution(revision.id, revision.revision)
        .unwrap_or_else(|error| panic!("independent safe subset should prepare: {error}"));
    let blocked = prepared
        .operations
        .iter()
        .find(|operation| {
            operation.source_relative_path.as_deref()
                == Some(selected.source.relative_path.as_str())
        })
        .unwrap_or_else(|| panic!("case-only operation should remain auditable"));

    assert_eq!(blocked.status, ExecutionOperationStatus::Blocked);
    assert_eq!(
        blocked.error_code.as_deref(),
        Some("case_only_rename_unqualified")
    );
    assert!(
        prepared
            .operations
            .iter()
            .all(|operation| operation.kind != ExecutionOperationKind::InternalStage)
    );
}

#[test]
fn qualified_case_only_rename_uses_authenticated_two_step_staging_and_rolls_back() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let (revision, selected) = case_only_revision(&scanner, &proposal, 1, true);
    let initial = sandbox.snapshot();
    let original_bytes = fs::read(sandbox.path().join(normalized_path(&selected[0].1)))
        .unwrap_or_else(|error| panic!("case-only source should be readable: {error}"));
    let journal = Arc::new(MemoryJournal::default());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let service = execution_service_with_security(
        database,
        platform,
        executor,
        journal.clone(),
        qualified_case_only_policy(),
        [111; 32],
    );

    let prepared = service
        .prepare_execution(revision.id, revision.revision)
        .unwrap_or_else(|error| panic!("qualified case rename should prepare: {error}"));
    assert!(
        prepared
            .session
            .approval
            .safety_policy
            .allow_qualified_case_only_rename
    );
    let stage = prepared
        .operations
        .iter()
        .find(|operation| operation.kind == ExecutionOperationKind::InternalStage)
        .unwrap_or_else(|| panic!("qualified plan should contain a stage"));
    let final_operation = prepared
        .operations
        .iter()
        .find(|operation| operation.proposal_operation_id.is_some())
        .unwrap_or_else(|| panic!("qualified plan should contain the final operation"));
    assert_eq!(
        final_operation.source_relative_path.as_deref(),
        Some(stage.destination_relative_path.as_str())
    );
    assert!(final_operation.dependencies.contains(&stage.id));
    assert!(
        stage
            .destination_relative_path
            .starts_with(&format!(".supremacy-staging/{}/", prepared.session.id))
    );
    let mut verification_progress = Vec::new();
    let verified = service
        .verify_approved_source_streaming(
            prepared.session.id,
            final_operation.id,
            &|| false,
            &mut |progress| verification_progress.push(progress),
        )
        .unwrap_or_else(|error| {
            panic!("coordinator streaming verification should succeed: {error}")
        });
    assert_eq!(
        verification_progress
            .last()
            .map(|progress| progress.bytes_hashed),
        Some(verified.byte_size)
    );
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("qualified plan should attest: {error}"));
    let envelope = ImmutableExecutionEnvelope::try_from_execution_detail(&approved)
        .unwrap_or_else(|error| panic!("staged transitions should enter the envelope: {error}"));
    assert!(envelope.operation(&stage.id.to_string()).is_some());
    assert!(
        envelope
            .operation(&final_operation.id.to_string())
            .is_some()
    );

    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("qualified case rename should complete: {error}"));
    assert_eq!(
        fs::read(
            sandbox.path().join(
                parent_segments(&selected[0].1)
                    .into_iter()
                    .chain(std::iter::once(selected[0].2.clone()))
                    .collect::<Vec<_>>()
                    .join("/")
            )
        )
        .unwrap_or_else(|error| panic!("case-renamed file should be readable: {error}")),
        original_bytes
    );
    let events = journal
        .events(completed.session.id)
        .unwrap_or_else(|error| panic!("journal should be readable: {error}"));
    for operation_id in [stage.id, final_operation.id] {
        let applied = events
            .iter()
            .find(|event| {
                event.step_id == Some(operation_id)
                    && event.kind == JournalEventKind::AppliedObserved
            })
            .unwrap_or_else(|| panic!("each transition should have an applied event"));
        let payload: serde_json::Value = serde_json::from_slice(&applied.payload)
            .unwrap_or_else(|error| panic!("applied payload should decode: {error}"));
        assert_eq!(payload["executor_audit"]["attempt_count"], 1);
    }
    let rolled_back = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("qualified case rename should roll back: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn qualified_case_only_staging_names_are_collision_resistant_within_the_plan() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let (revision, _selected) = case_only_revision(&scanner, &proposal, 2, true);
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let service = execution_service_with_security(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
        qualified_case_only_policy(),
        [111; 32],
    );

    let prepared = service
        .prepare_execution(revision.id, revision.revision)
        .unwrap_or_else(|error| panic!("two qualified case renames should prepare: {error}"));
    let staging_paths = prepared
        .operations
        .iter()
        .filter(|operation| operation.kind == ExecutionOperationKind::InternalStage)
        .map(|operation| operation.destination_relative_path.clone())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(staging_paths.len(), 2);
    assert!(staging_paths.iter().all(|path| {
        path.starts_with(&format!(".supremacy-staging/{}/", prepared.session.id))
            && !sandbox.path().join(path).exists()
    }));
}

#[test]
fn crash_at_case_only_temporary_transition_reconciles_and_reverse_rolls_back() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let (revision, _selected) = case_only_revision(&scanner, &proposal, 1, true);
    let initial = sandbox.snapshot();
    let journal = Arc::new(MemoryJournal::default());
    let crashing_executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
        CrashAfterStageExecutorClient::new(&sandbox, platform.clone()),
    );
    let service = execution_service_with_security(
        database.clone(),
        platform.clone(),
        crashing_executor,
        journal.clone(),
        qualified_case_only_policy(),
        [111; 32],
    );
    let prepared = service
        .prepare_execution(revision.id, revision.revision)
        .unwrap_or_else(|error| panic!("qualified case rename should prepare: {error}"));
    let stage = prepared
        .operations
        .iter()
        .find(|operation| operation.kind == ExecutionOperationKind::InternalStage)
        .cloned()
        .unwrap_or_else(|| panic!("qualified plan should contain a stage"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("qualified plan should attest: {error}"));

    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = service.start_execution(approved.session.id, &mut |_| {});
    }));
    assert!(crash.is_err());
    assert!(
        sandbox
            .path()
            .join(&stage.destination_relative_path)
            .is_file()
    );
    let interrupted = database
        .execution_detail(approved.session.id)
        .unwrap_or_else(|error| panic!("interrupted execution should load: {error}"));
    assert_eq!(
        interrupted
            .operations
            .iter()
            .find(|operation| operation.id == stage.id)
            .map(|operation| operation.status),
        Some(ExecutionOperationStatus::Running)
    );

    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let restarted = execution_service_with_security(
        database.clone(),
        platform,
        executor,
        journal,
        qualified_case_only_policy(),
        [111; 32],
    );
    let assessment = restarted
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("temporary transition should reconcile: {error}"));
    assert_eq!(
        assessment.state,
        ExecutionRecoveryState::RecoveryAvailable,
        "{assessment:#?}"
    );
    assert!(assessment.applied >= 1);
    let rolled_back = restarted
        .rollback_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("reconciled stage should reverse roll back: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(initial, sandbox.snapshot());
    assert!(
        database
            .validate_execution_journal(approved.session.id)
            .unwrap_or_else(|error| panic!("recovered journal should validate: {error}"))
    );
}

#[test]
fn rollback_never_overwrites_a_new_file_at_the_original_path() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));
    let operation = completed
        .operations
        .iter()
        .filter(|operation| {
            operation.status == ExecutionOperationStatus::Applied
                && operation.proposal_operation_id.is_some()
        })
        .max_by_key(|operation| operation.sequence)
        .unwrap_or_else(|| panic!("an applied file operation should exist"));
    let original = sandbox.path().join(
        operation
            .source_relative_path
            .as_deref()
            .unwrap_or_else(|| panic!("file operation should have an original path")),
    );
    let applied = sandbox.path().join(&operation.destination_relative_path);
    assert_is_test_sandbox(sandbox.path(), &original);
    assert_is_test_sandbox(sandbox.path(), &applied);
    fs::write(&original, b"external file that must not be overwritten")
        .unwrap_or_else(|error| panic!("external collision should be written: {error}"));
    let applied_bytes =
        fs::read(&applied).unwrap_or_else(|error| panic!("applied file should be read: {error}"));

    let rollback = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("rollback conflict should be reported: {error}"));
    assert_eq!(
        rollback.session.status,
        OrganizationExecutionStatus::RollbackPartial
    );
    assert!(!rollback.session.rollback_available);
    assert_eq!(
        fs::read(&original)
            .unwrap_or_else(|error| panic!("external collision should remain: {error}")),
        b"external file that must not be overwritten"
    );
    assert_eq!(
        fs::read(&applied).unwrap_or_else(|error| panic!("applied file should remain: {error}")),
        applied_bytes
    );
    assert!(rollback.session.summary.rollback_blocked >= 1);
}

#[test]
fn rollback_blocks_when_an_applied_file_changed_after_execution() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));
    let operation = completed
        .operations
        .iter()
        .filter(|operation| {
            operation.status == ExecutionOperationStatus::Applied
                && operation.proposal_operation_id.is_some()
        })
        .max_by_key(|operation| operation.sequence)
        .unwrap_or_else(|| panic!("an applied file operation should exist"));
    let applied = sandbox.path().join(&operation.destination_relative_path);
    assert_is_test_sandbox(sandbox.path(), &applied);
    fs::write(&applied, b"externally modified after organization")
        .unwrap_or_else(|error| panic!("applied file should be modified: {error}"));

    let rollback = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("modified rollback should be reported: {error}"));
    assert_eq!(
        rollback.session.status,
        OrganizationExecutionStatus::RollbackPartial
    );
    assert_eq!(
        fs::read(&applied).unwrap_or_else(|error| panic!("modified file should remain: {error}")),
        b"externally modified after organization"
    );
    assert!(rollback.session.summary.rollback_blocked >= 1);
}

#[test]
fn rollback_blocks_same_content_replacement_with_a_different_native_identity() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform.clone(), &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));
    let operation = completed
        .operations
        .iter()
        .filter(|operation| {
            operation.status == ExecutionOperationStatus::Applied
                && operation.proposal_operation_id.is_some()
        })
        .max_by_key(|operation| operation.sequence)
        .unwrap_or_else(|| panic!("an applied file operation should exist"));
    let applied = sandbox.path().join(&operation.destination_relative_path);
    assert_is_test_sandbox(sandbox.path(), &applied);
    let original_fingerprint = operation
        .post_fingerprint
        .as_ref()
        .unwrap_or_else(|| panic!("applied operation should retain its exact post fingerprint"));
    let bytes =
        fs::read(&applied).unwrap_or_else(|error| panic!("applied file should be read: {error}"));
    let replacement = applied.with_extension("supremacy-replacement");
    assert_is_test_sandbox(sandbox.path(), &replacement);
    fs::write(&replacement, &bytes)
        .unwrap_or_else(|error| panic!("same-content replacement should be written: {error}"));
    fs::remove_file(&applied)
        .unwrap_or_else(|error| panic!("applied object should be removed: {error}"));
    fs::rename(&replacement, &applied)
        .unwrap_or_else(|error| panic!("replacement should enter the applied path: {error}"));
    let replacement_fingerprint = platform
        .fingerprint(&applied, true, domain::MAX_EXECUTION_VERIFICATION_BYTES)
        .unwrap_or_else(|error| panic!("replacement should fingerprint: {error}"));
    assert_ne!(
        replacement_fingerprint.native_identity,
        original_fingerprint.native_identity
    );
    assert_eq!(
        replacement_fingerprint.content_digest,
        original_fingerprint.content_digest
    );

    let rollback = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("identity replacement should be reported: {error}"));
    assert_eq!(
        rollback.session.status,
        OrganizationExecutionStatus::RollbackPartial
    );
    assert_eq!(
        fs::read(&applied).unwrap_or_else(|error| panic!("replacement should remain: {error}")),
        bytes
    );
    assert!(rollback.session.summary.rollback_blocked >= 1);
}

#[cfg(unix)]
#[test]
fn rollback_blocks_a_symbolic_link_in_the_original_path_ancestry() {
    use std::os::unix::fs::symlink;

    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));
    let operation = completed
        .operations
        .iter()
        .filter(|operation| {
            operation.status == ExecutionOperationStatus::Applied
                && operation.proposal_operation_id.is_some()
                && operation
                    .source_relative_path
                    .as_deref()
                    .map(Path::new)
                    .and_then(Path::parent)
                    != Path::new(&operation.destination_relative_path).parent()
        })
        .max_by_key(|operation| operation.sequence)
        .unwrap_or_else(|| panic!("fixture should contain an applied cross-directory move"));
    let original = sandbox.path().join(
        operation
            .source_relative_path
            .as_deref()
            .unwrap_or_else(|| panic!("file operation should retain its source")),
    );
    let original_parent = original
        .parent()
        .unwrap_or_else(|| panic!("original path should have a parent"));
    let backup = sandbox.path().join(".rollback-ancestry-backup");
    assert_is_test_sandbox(sandbox.path(), original_parent);
    assert_is_test_sandbox(sandbox.path(), &backup);
    fs::rename(original_parent, &backup)
        .unwrap_or_else(|error| panic!("original parent should move for test setup: {error}"));
    symlink(&backup, original_parent)
        .unwrap_or_else(|error| panic!("symbolic-link ancestry should be created: {error}"));
    let applied = sandbox.path().join(&operation.destination_relative_path);
    let applied_bytes =
        fs::read(&applied).unwrap_or_else(|error| panic!("applied file should be read: {error}"));

    let rollback = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("unsafe ancestry should be reported: {error}"));
    assert_eq!(
        rollback.session.status,
        OrganizationExecutionStatus::RollbackPartial
    );
    assert_eq!(
        fs::read(&applied).unwrap_or_else(|error| panic!("applied file should remain: {error}")),
        applied_bytes
    );
    assert!(rollback.session.summary.rollback_blocked >= 1);
}

#[test]
fn rollback_preserves_nonempty_directories_created_by_execution() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));
    let directory = completed
        .operations
        .iter()
        .filter(|operation| {
            operation.kind == ExecutionOperationKind::CreateDirectory
                && !operation
                    .destination_relative_path
                    .starts_with(".supremacy-staging")
        })
        .max_by_key(|operation| operation.sequence)
        .unwrap_or_else(|| panic!("a created destination directory should exist"));
    let sentinel = sandbox
        .path()
        .join(&directory.destination_relative_path)
        .join("external-after-apply.txt");
    assert_is_test_sandbox(sandbox.path(), &sentinel);
    fs::write(&sentinel, b"preserve me")
        .unwrap_or_else(|error| panic!("external directory content should be written: {error}"));

    let rollback = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("nonempty directory should be reported: {error}"));
    assert_eq!(
        rollback.session.status,
        OrganizationExecutionStatus::RollbackPartial
    );
    assert_eq!(
        fs::read(&sentinel)
            .unwrap_or_else(|error| panic!("external directory content should remain: {error}")),
        b"preserve me"
    );
    assert!(rollback.session.summary.rollback_blocked >= 1);
}

#[test]
fn destination_collision_and_source_drift_are_blocked_without_overwrite() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let safe = proposal
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.operation_kind,
                ProposalOperationKind::MoveProposal | ProposalOperationKind::RenameProposal
            ) && operation.user_override
                && !operation.stale
        })
        .collect::<Vec<_>>();
    assert!(
        safe.len() >= 4,
        "fixture should retain independent safe operations"
    );
    let collision = safe[0];
    let collision_path = sandbox.path().join(
        collision
            .proposed_destination
            .iter()
            .fold(std::path::PathBuf::new(), |path, segment| {
                path.join(segment)
            })
            .join(&collision.proposed_name),
    );
    assert_is_test_sandbox(sandbox.path(), &collision_path);
    fs::create_dir_all(
        collision_path
            .parent()
            .unwrap_or_else(|| panic!("collision should have a parent")),
    )
    .unwrap_or_else(|error| panic!("collision parent should be created: {error}"));
    fs::write(&collision_path, b"do not overwrite")
        .unwrap_or_else(|error| panic!("collision fixture should be written: {error}"));

    let drifted = safe[1];
    let drifted_path = sandbox
        .path()
        .join(normalized_path(&drifted.source.relative_path));
    assert_is_test_sandbox(sandbox.path(), &drifted_path);
    fs::write(&drifted_path, b"changed after approval")
        .unwrap_or_else(|error| panic!("drift fixture should be written: {error}"));
    let missing = safe[2];
    let missing_path = sandbox
        .path()
        .join(normalized_path(&missing.source.relative_path));
    assert_is_test_sandbox(sandbox.path(), &missing_path);
    fs::remove_file(&missing_path)
        .unwrap_or_else(|error| panic!("missing-source fixture should be removed: {error}"));
    let before = sandbox.snapshot();
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("independent safe subset should remain: {error}"));

    assert!(prepared.session.summary.blocked >= 3);
    assert!(prepared.operations.iter().any(|operation| {
        operation.proposal_operation_id == Some(collision.id)
            && operation.status == ExecutionOperationStatus::Blocked
            && operation.error_code.as_deref() == Some("destination_exists")
    }));
    assert!(prepared.operations.iter().any(|operation| {
        operation.proposal_operation_id == Some(drifted.id)
            && operation.status == ExecutionOperationStatus::Stale
    }));
    assert!(prepared.operations.iter().any(|operation| {
        operation.proposal_operation_id == Some(missing.id)
            && operation.status == ExecutionOperationStatus::Stale
            && operation.error_code.as_deref() == Some("source_unavailable")
    }));
    assert_eq!(
        fs::read(&collision_path)
            .unwrap_or_else(|error| panic!("collision bytes should remain: {error}")),
        b"do not overwrite"
    );
    assert_eq!(before, sandbox.snapshot());
}

#[test]
fn case_insensitive_duplicate_plan_destinations_are_blocked() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let selected = proposal.operations.iter().take(2).collect::<Vec<_>>();
    assert_eq!(selected.len(), 2);
    let mut revised = scanner
        .set_organization_proposal_override(
            proposal.id,
            selected[0].file_id,
            ProposalOverrideAction::DestinationAndRename,
            Some(vec!["Collision".to_owned()]),
            Some("Same.txt".to_owned()),
            Some("Case-insensitive collision fixture".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("first collision override should persist: {error}"));
    revised = scanner
        .set_organization_proposal_override(
            revised.id,
            selected[1].file_id,
            ProposalOverrideAction::DestinationAndRename,
            Some(vec!["collision".to_owned()]),
            Some("same.TXT".to_owned()),
            Some("Case-insensitive collision fixture".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("second collision override should persist: {error}"));
    revised = scanner
        .set_organization_proposal_status(
            revised.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("collision revision should be approved: {error}"));
    let collision_operation_ids = revised
        .operations
        .iter()
        .filter(|operation| {
            selected
                .iter()
                .any(|selected| selected.file_id == operation.file_id)
        })
        .map(|operation| operation.id)
        .collect::<Vec<_>>();
    assert_eq!(collision_operation_ids.len(), 2);
    assert!(revised.summary.conflicts >= 1 || revised.summary.needs_review >= 2);
    let before = sandbox.snapshot();
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(revised.id, revised.revision)
        .unwrap_or_else(|error| panic!("independent safe subset should remain: {error}"));
    let executable_collision_operations = prepared
        .operations
        .iter()
        .filter(|operation| {
            operation
                .proposal_operation_id
                .is_some_and(|id| collision_operation_ids.contains(&id))
                && operation.status == ExecutionOperationStatus::PreflightOk
        })
        .count();
    assert!(
        executable_collision_operations <= 1,
        "at most one case-insensitive collision target may remain executable"
    );
    assert_eq!(before, sandbox.snapshot());
}

#[test]
fn proposal_revision_change_after_preflight_invalidates_final_approval() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let initial = sandbox.snapshot();
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("initial preflight should succeed: {error}"));
    let operation = proposal
        .operations
        .first()
        .unwrap_or_else(|| panic!("proposal should contain an operation"));
    let revised = scanner
        .set_organization_proposal_override(
            proposal.id,
            operation.file_id,
            ProposalOverrideAction::DestinationAndRename,
            Some(vec!["Revised".to_owned()]),
            Some(operation.proposed_name.clone()),
            Some("Changed after M8 preflight".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("proposal revision should change: {error}"));
    assert!(revised.revision > proposal.revision);
    assert!(
        service
            .create_execution_consent_challenge(prepared.session.id, None)
            .is_err()
    );
    let invalidated = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("invalidated consent should remain inspectable: {error}"));
    assert_eq!(
        invalidated.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(
        invalidated.session.consent.invalidation_reason.as_deref(),
        Some("proposal_revision_changed")
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn proposal_revision_change_after_confirmation_blocks_start() {
    let sandbox = MutationSandbox::new();
    let (database, platform, scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let initial = sandbox.snapshot();
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("initial preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("confirmation should initially succeed: {error}"));
    let operation = proposal
        .operations
        .first()
        .unwrap_or_else(|| panic!("proposal should contain an operation"));
    scanner
        .set_organization_proposal_override(
            proposal.id,
            operation.file_id,
            ProposalOverrideAction::DestinationAndRename,
            Some(vec!["Changed-after-confirmation".to_owned()]),
            Some(operation.proposed_name.clone()),
            Some("Changed before native mutation".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("proposal revision should change: {error}"));
    assert!(
        service
            .start_execution(approved.session.id, &mut |_| {})
            .is_err()
    );
    let invalidated = service
        .execution_status(approved.session.id)
        .unwrap_or_else(|error| panic!("invalidated consent should remain inspectable: {error}"));
    assert_eq!(
        invalidated.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn source_drift_invalidates_consent_before_native_confirmation() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let source = prepared
        .operations
        .iter()
        .find(|operation| {
            operation.proposal_operation_id.is_some()
                && operation.status == ExecutionOperationStatus::PreflightOk
        })
        .and_then(|operation| operation.original_source_relative_path.as_deref())
        .unwrap_or_else(|| panic!("an approved source should exist"));
    let source = sandbox.path().join(source);
    assert_is_test_sandbox(sandbox.path(), &source);
    fs::write(&source, b"changed after the approved plan was frozen")
        .unwrap_or_else(|error| panic!("source drift should be written: {error}"));

    assert!(
        service
            .create_execution_consent_challenge(prepared.session.id, None)
            .is_err()
    );
    let invalidated = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("invalidated execution should load: {error}"));
    assert_eq!(
        invalidated.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(
        invalidated.session.consent.invalidation_reason.as_deref(),
        Some("source_fingerprint_changed")
    );
}

#[test]
fn destination_root_change_invalidates_consent_before_confirmation() {
    let sandbox = MutationSandbox::new();
    let alternate_root = MutationSandbox::new();
    let (database, platform, scanner, workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let replacement = scanner
        .register_root(workspace_id, alternate_root.path())
        .unwrap_or_else(|error| panic!("replacement root should register: {error}"));
    assert_ne!(replacement.id, prepared.session.root_id);

    assert!(
        service
            .create_execution_consent_challenge(prepared.session.id, None)
            .is_err()
    );
    let invalidated = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("invalidated execution should load: {error}"));
    assert_eq!(
        invalidated.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(
        invalidated.session.consent.invalidation_reason.as_deref(),
        Some("destination_root_changed")
    );
}

#[test]
fn safety_policy_change_invalidates_consent_before_confirmation() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database.clone(), platform.clone(), &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    drop(service);

    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let mut changed_policy = ExecutionSafetyPolicy::default();
    changed_policy.maximum_rehash_bytes = changed_policy.maximum_rehash_bytes.saturating_sub(1);
    let changed_service = execution_service_with_security(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
        changed_policy,
        [111; 32],
    );
    assert!(
        changed_service
            .create_execution_consent_challenge(prepared.session.id, None)
            .is_err()
    );
    let invalidated = changed_service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("invalidated execution should load: {error}"));
    assert_eq!(
        invalidated.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(
        invalidated.session.consent.invalidation_reason.as_deref(),
        Some("safety_policy_changed")
    );
}

#[test]
fn challenge_from_another_authority_cannot_be_finalized() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database.clone(), platform.clone(), &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let issued_at = 1_000_000;
    let challenge = service
        .create_execution_consent_challenge_at(prepared.session.id, None, issued_at)
        .unwrap_or_else(|error| panic!("challenge should be issued: {error}"));
    assert_eq!(
        challenge.summary().file_count,
        prepared.session.approval.operation_count
    );
    assert_eq!(
        challenge.summary().folder_count,
        prepared.session.summary.folders_to_create
    );
    assert_eq!(
        challenge.summary().destination_root_display,
        prepared.session.approval.destination_root.display_path
    );
    assert_eq!(
        challenge.summary().plan_verification_code.replace('-', ""),
        prepared.session.plan_digest_hex[..8].to_ascii_uppercase()
    );
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let foreign_authority = execution_service_with_security(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
        ExecutionSafetyPolicy::default(),
        [112; 32],
    );

    assert!(
        foreign_authority
            .finalize_execution_consent_at(challenge, issued_at + 1)
            .is_err()
    );
    let pending = foreign_authority
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("pending execution should load: {error}"));
    assert_eq!(
        pending.session.consent.state,
        ExecutionConsentState::Pending
    );
    assert!(!pending.session.approval.user_confirmed);
}

#[test]
fn tampered_persisted_attestation_is_invalidated_before_start() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database.clone(), platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let issued_at = 2_000_000;
    let challenge = service
        .create_execution_consent_challenge_at(prepared.session.id, None, issued_at)
        .unwrap_or_else(|error| panic!("challenge should be issued: {error}"));
    drop(challenge);
    let pending = database
        .execution_detail(prepared.session.id)
        .unwrap_or_else(|error| panic!("pending challenge should load: {error}"));
    let nonce = pending
        .session
        .consent
        .nonce
        .unwrap_or_else(|| panic!("nonce should be persisted"));
    let expires_at = pending
        .session
        .consent
        .expires_at_unix_ms
        .unwrap_or_else(|| panic!("expiry should be persisted"));
    database
        .attest_execution_consent(
            prepared.session.id,
            nonce,
            issued_at,
            expires_at,
            [0; 32],
            issued_at + 1,
        )
        .unwrap_or_else(|error| panic!("tampered persistence fixture should be written: {error}"));

    assert!(
        service
            .start_execution_at(prepared.session.id, issued_at + 2, &mut |_| {})
            .is_err()
    );
    let invalidated = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("invalidated execution should load: {error}"));
    assert_eq!(
        invalidated.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(
        invalidated.session.consent.invalidation_reason.as_deref(),
        Some("attestation_authentication_failed")
    );
}

#[test]
fn consent_expires_at_the_ten_minute_boundary() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let issued_at = 3_000_000;
    let challenge = service
        .create_execution_consent_challenge_at(prepared.session.id, None, issued_at)
        .unwrap_or_else(|error| panic!("challenge should be issued: {error}"));
    let pending = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("pending challenge should load: {error}"));
    let expires_at = pending
        .session
        .consent
        .expires_at_unix_ms
        .unwrap_or_else(|| panic!("expiry should be persisted"));
    assert_eq!(expires_at - issued_at, 10 * 60 * 1_000);

    assert!(
        service
            .finalize_execution_consent_at(challenge, expires_at)
            .is_err()
    );
    let expired = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("expired execution should load: {error}"));
    assert_eq!(
        expired.session.consent.state,
        ExecutionConsentState::Expired
    );
    assert!(!expired.session.approval.user_confirmed);

    let reissued_at = expires_at + 1;
    let replacement = service
        .create_execution_consent_challenge_at(prepared.session.id, None, reissued_at)
        .unwrap_or_else(|error| panic!("expired consent should allow a new challenge: {error}"));
    let approved = service
        .finalize_execution_consent_at(replacement, reissued_at + 1)
        .unwrap_or_else(|error| panic!("replacement challenge should attest: {error}"));
    let replacement_expiry = approved
        .session
        .consent
        .expires_at_unix_ms
        .unwrap_or_else(|| panic!("replacement expiry should persist"));
    assert!(
        service
            .start_execution_at(prepared.session.id, replacement_expiry, &mut |_| {})
            .is_err()
    );
    let expired_before_start = service
        .execution_status(prepared.session.id)
        .unwrap_or_else(|error| panic!("start expiry should remain inspectable: {error}"));
    assert_eq!(
        expired_before_start.session.consent.state,
        ExecutionConsentState::Expired
    );
}

#[test]
fn attested_consent_is_consumed_exactly_once_at_start() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let service = execution_service(database.clone(), platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let issued_at = 4_000_000;
    let challenge = service
        .create_execution_consent_challenge_at(prepared.session.id, None, issued_at)
        .unwrap_or_else(|error| panic!("challenge should be issued: {error}"));
    let approved = service
        .finalize_execution_consent_at(challenge, issued_at + 1)
        .unwrap_or_else(|error| panic!("challenge should be attested: {error}"));
    let completed = service
        .start_execution_at(approved.session.id, issued_at + 2, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should start: {error}"));
    assert_eq!(
        completed.session.consent.state,
        ExecutionConsentState::Consumed
    );
    let consent = &completed.session.consent;
    assert!(
        database
            .consume_execution_consent(
                completed.session.id,
                consent
                    .nonce
                    .unwrap_or_else(|| panic!("consumed nonce should remain auditable")),
                consent
                    .issued_at_unix_ms
                    .unwrap_or_else(|| panic!("issued timestamp should remain auditable")),
                consent
                    .expires_at_unix_ms
                    .unwrap_or_else(|| panic!("expiry should remain auditable")),
                consent
                    .attestation_mac
                    .unwrap_or_else(|| panic!("attestation should remain auditable")),
                issued_at + 3,
            )
            .is_err()
    );
}

#[test]
fn journal_flush_failure_blocks_the_filesystem_mutation() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    precreate_destination_parents(&sandbox, &proposal);
    let before = sandbox.snapshot();
    let journal = Arc::new(FailingFlushJournal::new());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let service = execution_service_with(database.clone(), platform, executor, journal.clone());
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should be durable: {error}"));

    let result = service.start_execution(approved.session.id, &mut |_| {});
    assert!(result.is_err());
    assert_eq!(before, sandbox.snapshot());
    let interrupted = database
        .execution_detail(approved.session.id)
        .unwrap_or_else(|error| panic!("interrupted detail should load: {error}"));
    let running = interrupted
        .operations
        .iter()
        .find(|operation| operation.status == ExecutionOperationStatus::Running)
        .unwrap_or_else(|| panic!("durable intent should remain recoverable"));
    let events = journal
        .events(approved.session.id)
        .unwrap_or_else(|error| panic!("injected journal should remain inspectable: {error}"));
    assert!(events.iter().any(|event| {
        event.step_id == Some(running.id) && event.kind == JournalEventKind::IntentDurable
    }));
    assert!(!events.iter().any(|event| {
        event.step_id == Some(running.id) && event.kind == JournalEventKind::AppliedObserved
    }));
    let durable_requests = database
        .executor_request_facts(approved.session.id)
        .unwrap_or_else(|error| panic!("durable request identity should load: {error}"));
    assert_eq!(durable_requests.len(), 1);
    assert_eq!(
        durable_requests[0].state,
        ExecutorRequestState::IntentDurable
    );
    assert_eq!(durable_requests[0].operation_id, running.id);
    assert_eq!(
        database
            .executor_session_facts(approved.session.id)
            .unwrap_or_else(|error| panic!("session identity should load: {error}"))
            .len(),
        1
    );
}

#[test]
fn locked_authenticated_journal_starts_read_only_without_repair() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let before = sandbox.snapshot();
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let locked: Arc<dyn DurableJournal> = Arc::new(LockedJournal::from_open_error(
        OperationsError::Journal("journal authentication failed".to_owned()),
        1_786_445_000_000,
    ));
    let service = execution_service_with(database, platform, executor, locked);

    let status = service
        .system_status()
        .unwrap_or_else(|error| panic!("locked status should remain inspectable: {error}"));
    assert!(status.journal_locked);
    assert!(!status.apply_gate.enabled);
    assert!(status.recovery_required);
    assert_eq!(status.journal_diagnostics.len(), 1);
    assert_eq!(
        status.journal_diagnostics[0].code,
        "external_journal_authentication_failed"
    );
    assert!(!status.journal_diagnostics[0].recovery_available);
    assert!(!status.journal_diagnostics[0].rollback_available);
    assert!(matches!(
        service.prepare_execution(proposal.id, proposal.revision),
        Err(application::ApplicationError::JournalLocked)
    ));
    assert_eq!(before, sandbox.snapshot());
}

#[test]
fn incomplete_external_journal_locks_startup_without_backfill_or_repair() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let before = sandbox.snapshot();
    let original_journal = Arc::new(MemoryJournal::default());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let service = execution_service_with(
        database.clone(),
        platform.clone(),
        executor,
        original_journal,
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should persist: {error}"));
    drop(service);

    let incomplete_journal = Arc::new(MemoryJournal::default());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let restarted =
        execution_service_with(database, platform, executor, incomplete_journal.clone());
    let status = restarted
        .system_status()
        .unwrap_or_else(|error| panic!("diagnostic status should load: {error}"));
    assert!(status.journal_locked);
    assert!(!status.apply_gate.enabled);
    assert!(
        status
            .journal_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "external_journal_incomplete")
    );
    assert!(
        incomplete_journal
            .events(approved.session.id)
            .unwrap_or_else(|error| panic!("incomplete journal should remain inspectable: {error}"))
            .is_empty(),
        "startup diagnostics must not backfill the encrypted journal"
    );
    assert!(matches!(
        restarted.start_execution(approved.session.id, &mut |_| {}),
        Err(application::ApplicationError::JournalLocked)
    ));
    assert_eq!(before, sandbox.snapshot());
}

#[test]
fn execution_history_and_authenticated_journal_survive_reopen() {
    let sandbox = MutationSandbox::new();
    let state = tempfile::Builder::new()
        .prefix("supremacy-m8-durable-state-")
        .tempdir()
        .unwrap_or_else(|error| panic!("durable state sandbox should be created: {error}"));
    let database_path = state.path().join("catalog.db");
    let journal_path = state.path().join("operation-recovery.jsonl.enc");
    let key = DatabaseKey::from_bytes([109; 32]);
    let database = Arc::new(
        Database::open(&database_path, &key)
            .unwrap_or_else(|error| panic!("file database should open: {error}")),
    );
    let (database, platform, scanner, workspace_id, proposal) =
        approved_fixture_with_database(&sandbox, database);
    let journal = Arc::new(
        FileJournal::open(&journal_path, JournalKey::from_bytes([110; 32]))
            .unwrap_or_else(|error| panic!("authenticated journal should open: {error}")),
    );
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let service = execution_service_with(database.clone(), platform, executor, journal.clone());
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));
    let execution_id = completed.session.id;
    assert!(
        database
            .validate_execution_journal(execution_id)
            .unwrap_or_else(|error| panic!("database journal should validate: {error}"))
    );
    drop(service);
    drop(journal);
    drop(scanner);
    drop(database);

    let reopened = Database::open(&database_path, &key)
        .unwrap_or_else(|error| panic!("database should reopen: {error}"));
    let history = reopened
        .execution_history(workspace_id, 20)
        .unwrap_or_else(|error| panic!("execution history should reopen: {error}"));
    assert_eq!(
        history.first().map(|session| session.id),
        Some(execution_id)
    );
    assert_eq!(
        history.first().map(|session| session.status),
        Some(OrganizationExecutionStatus::Completed)
    );
    assert!(
        reopened
            .validate_execution_journal(execution_id)
            .unwrap_or_else(|error| panic!("reopened database journal should validate: {error}"))
    );
    let reopened_journal = FileJournal::open(&journal_path, JournalKey::from_bytes([110; 32]))
        .unwrap_or_else(|error| panic!("authenticated journal should reopen: {error}"));
    let events = reopened_journal
        .events(execution_id)
        .unwrap_or_else(|error| panic!("reopened journal events should decrypt: {error}"));
    assert_eq!(
        events.last().map(|event| event.kind),
        Some(JournalEventKind::ExecutionFinished)
    );
}

#[test]
fn pause_request_stops_only_between_recoverable_operation_units() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    precreate_destination_parents(&sandbox, &proposal);
    let initial = sandbox.snapshot();
    let boundary = Arc::new((Mutex::new(BlockingState::default()), Condvar::new()));
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(BoundaryBlockingExecutorClient::new(
        &sandbox,
        platform.clone(),
        boundary.clone(),
    ));
    let service = Arc::new(execution_service_with(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
    ));
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let issued_at = 5_000_000;
    let challenge = service
        .create_execution_consent_challenge_at(prepared.session.id, None, issued_at)
        .unwrap_or_else(|error| panic!("challenge should be issued: {error}"));
    let approved = service
        .finalize_execution_consent_at(challenge, issued_at + 1)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let execution_id = approved.session.id;
    let start_service = service.clone();
    let worker = thread::spawn(move || {
        start_service.start_execution_at(execution_id, issued_at + 2, &mut |_| {})
    });

    let (lock, condition) = &*boundary;
    let mut state = lock
        .lock()
        .unwrap_or_else(|error| panic!("boundary state should lock: {error}"));
    while !state.entered {
        state = condition
            .wait(state)
            .unwrap_or_else(|error| panic!("boundary wait should wake: {error}"));
    }
    assert!(
        service
            .pause_execution(execution_id)
            .unwrap_or_else(|error| panic!("pause request should persist: {error}"))
    );
    state.released = true;
    condition.notify_all();
    drop(state);

    let paused = worker
        .join()
        .unwrap_or_else(|_| panic!("execution worker should not panic"))
        .unwrap_or_else(|error| panic!("execution should pause safely: {error}"));
    assert_eq!(paused.session.status, OrganizationExecutionStatus::Paused);
    assert_eq!(paused.session.summary.applied, 1);
    assert!(paused.session.rollback_available);
    assert_eq!(
        paused.session.consent.state,
        ExecutionConsentState::Consumed
    );
    assert!(matches!(
        service.start_execution_at(execution_id, issued_at + 3, &mut |_| {}),
        Err(application::ApplicationError::InvalidExecution)
    ));
    let rolled_back = service
        .rollback_execution(execution_id, &mut |_| {})
        .unwrap_or_else(|error| panic!("paused execution should roll back: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(
        rolled_back.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn cancelling_a_confirmed_but_unstarted_execution_never_mutates_files() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let initial = sandbox.snapshot();
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    assert!(
        service
            .cancel_execution(approved.session.id)
            .unwrap_or_else(|error| panic!("cancel should complete: {error}"))
    );
    let cancelled = service
        .execution_status(approved.session.id)
        .unwrap_or_else(|error| panic!("cancelled execution should load: {error}"));
    assert_eq!(
        cancelled.session.status,
        OrganizationExecutionStatus::Cancelled
    );
    assert_eq!(
        cancelled.session.consent.state,
        ExecutionConsentState::Invalidated
    );
    assert_eq!(
        cancelled.session.consent.invalidation_reason.as_deref(),
        Some("execution_cancelled")
    );
    assert!(!cancelled.session.rollback_available);
    assert!(
        service
            .start_execution(approved.session.id, &mut |_| {})
            .is_err()
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn crash_before_move_is_recovered_as_not_started_without_data_loss() {
    assert_crash_recovery(CrashPoint::BeforeMutation, false);
}

#[test]
fn crash_after_move_before_commit_is_recovered_as_applied_and_rolls_back() {
    assert_crash_recovery(CrashPoint::AfterMutation, true);
}

#[test]
fn worker_ack_before_database_failure_recovers_from_durable_intent_without_replay() {
    let sandbox = MutationSandbox::new();
    let database_directory =
        tempfile::tempdir().unwrap_or_else(|error| panic!("database tempdir should open: {error}"));
    let database_path = database_directory.path().join("execution.db");
    let database_key = [117; 32];
    let database = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(database_key))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    let (database, platform, scanner, _workspace_id, proposal) =
        approved_fixture_with_database(&sandbox, database);
    let initial = sandbox.snapshot();
    let journal_path = database_directory.path().join("execution.journal");
    let journal_key = [118; 32];
    let journal = Arc::new(
        FileJournal::open(&journal_path, JournalKey::from_bytes(journal_key))
            .unwrap_or_else(|error| panic!("external journal should open: {error}")),
    );
    let inner: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let executor: Arc<dyn ApprovedExecutorClient> =
        Arc::new(AckThenDatabaseFailureExecutorClient {
            inner,
            database_path: database_path.clone(),
            database_key,
        });
    let service = execution_service_with(
        database.clone(),
        platform.clone(),
        executor,
        journal.clone(),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));

    assert!(matches!(
        service.start_execution(approved.session.id, &mut |_| {}),
        Err(application::ApplicationError::Persistence(_))
    ));
    let interrupted = database
        .execution_detail(approved.session.id)
        .unwrap_or_else(|error| panic!("interrupted execution should load: {error}"));
    let running = interrupted
        .operations
        .iter()
        .find(|operation| operation.status == ExecutionOperationStatus::Running)
        .unwrap_or_else(|| panic!("acknowledged mutation should retain its durable intent"));
    let acknowledged_path = sandbox.path().join(&running.destination_relative_path);
    assert!(
        acknowledged_path.exists(),
        "the worker mutation must precede the injected response commit failure"
    );
    let requests = database
        .executor_request_facts(approved.session.id)
        .unwrap_or_else(|error| panic!("durable request should load: {error}"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].state, ExecutorRequestState::IntentDurable);
    assert!(requests[0].response_digest_hex.is_none());
    assert_eq!(
        database
            .executor_session_facts(approved.session.id)
            .unwrap_or_else(|error| panic!("executor session should load: {error}"))
            .len(),
        1
    );

    remove_executor_response_failure(&database_path, &database_key);
    drop(service);
    drop(scanner);
    drop(database);
    drop(journal);
    let reopened = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(database_key))
            .unwrap_or_else(|error| panic!("database should reopen: {error}")),
    );
    let reopened_journal = Arc::new(
        FileJournal::open(&journal_path, JournalKey::from_bytes(journal_key))
            .unwrap_or_else(|error| panic!("external journal should reopen: {error}")),
    );
    let restarted_executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
        SandboxApprovedExecutorClient::new(sandbox.path(), platform.clone()),
    );
    let restarted = execution_service_with(
        reopened.clone(),
        platform,
        restarted_executor,
        reopened_journal,
    );
    let assessment = restarted
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("durable intent should reconcile: {error}"));
    assert_eq!(assessment.applied, 1);
    assert_eq!(assessment.ambiguous, 0);
    assert!(assessment.rollback_available);
    let recovered_requests = reopened
        .executor_request_facts(approved.session.id)
        .unwrap_or_else(|error| panic!("recovered request should load: {error}"));
    assert_eq!(
        recovered_requests[0].state,
        ExecutorRequestState::ProvenApplied
    );
    assert_eq!(
        reopened
            .executor_session_facts(approved.session.id)
            .unwrap_or_else(|error| panic!("sessions should load: {error}"))
            .len(),
        1,
        "recovery must not open or replay a forward executor session"
    );
    let rolled_back = restarted
        .rollback_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("verified applied operation should roll back: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert!(!acknowledged_path.exists());
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn rollback_ack_before_database_failure_reconciles_then_resumes_in_reverse_order() {
    let sandbox = MutationSandbox::new();
    let database_directory =
        tempfile::tempdir().unwrap_or_else(|error| panic!("database tempdir should open: {error}"));
    let database_path = database_directory.path().join("execution.db");
    let database_key = [119; 32];
    let database = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(database_key))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    let (database, platform, scanner, _workspace_id, proposal) =
        approved_fixture_with_database(&sandbox, database);
    let initial = sandbox.snapshot();
    let journal_path = database_directory.path().join("execution.journal");
    let journal_key = [120; 32];
    let journal = Arc::new(
        FileJournal::open(&journal_path, JournalKey::from_bytes(journal_key))
            .unwrap_or_else(|error| panic!("external journal should open: {error}")),
    );
    let normal_executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
        SandboxApprovedExecutorClient::new(sandbox.path(), platform.clone()),
    );
    let normal = execution_service_with(
        database.clone(),
        platform.clone(),
        normal_executor,
        journal.clone(),
    );
    let prepared = normal
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&normal, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = normal
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("forward execution should complete: {error}"));
    drop(normal);

    let inner: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let failing_executor: Arc<dyn ApprovedExecutorClient> =
        Arc::new(AckThenDatabaseFailureExecutorClient {
            inner,
            database_path: database_path.clone(),
            database_key,
        });
    let failing = execution_service_with(
        database.clone(),
        platform.clone(),
        failing_executor,
        journal.clone(),
    );
    assert!(matches!(
        failing.rollback_execution(completed.session.id, &mut |_| {}),
        Err(application::ApplicationError::Persistence(_))
    ));
    let interrupted_requests = database
        .executor_request_facts(completed.session.id)
        .unwrap_or_else(|error| panic!("interrupted rollback requests should load: {error}"));
    assert!(interrupted_requests.iter().any(|request| {
        request.direction == ExecutorRequestDirection::Rollback
            && request.state == ExecutorRequestState::IntentDurable
            && request.response_digest_hex.is_none()
    }));
    let sessions_before_restart = database
        .executor_session_facts(completed.session.id)
        .unwrap_or_else(|error| panic!("executor sessions should load: {error}"));
    assert_eq!(sessions_before_restart.len(), 2);

    remove_executor_response_failure(&database_path, &database_key);
    drop(failing);
    drop(scanner);
    drop(database);
    drop(journal);
    let reopened = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(database_key))
            .unwrap_or_else(|error| panic!("database should reopen: {error}")),
    );
    let reopened_journal = Arc::new(
        FileJournal::open(&journal_path, JournalKey::from_bytes(journal_key))
            .unwrap_or_else(|error| panic!("external journal should reopen: {error}")),
    );
    let restarted_executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
        SandboxApprovedExecutorClient::new(sandbox.path(), platform.clone()),
    );
    let restarted = execution_service_with(
        reopened.clone(),
        platform,
        restarted_executor,
        reopened_journal,
    );
    let assessment = restarted
        .recover_execution(completed.session.id)
        .unwrap_or_else(|error| panic!("rollback acknowledgement should reconcile: {error}"));
    assert_eq!(assessment.ambiguous, 0);
    assert!(assessment.applied >= 1);
    assert_eq!(
        reopened
            .executor_session_facts(completed.session.id)
            .unwrap_or_else(|error| panic!("recovery sessions should load: {error}"))
            .len(),
        2,
        "recovery must not dispatch or reopen a worker session"
    );
    let rolled_back = restarted
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("remaining rollback should resume: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    let final_sessions = reopened
        .executor_session_facts(completed.session.id)
        .unwrap_or_else(|error| panic!("final sessions should load: {error}"));
    assert_eq!(final_sessions.len(), 3);
    assert_ne!(
        final_sessions[1].session_id, final_sessions[2].session_id,
        "restart must authorize rollback through a fresh session identity"
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn crash_after_rollback_move_is_reconciled_before_rollback_resumes() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    let initial = sandbox.snapshot();
    let journal = Arc::new(MemoryJournal::default());
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let service = execution_service_with(
        database.clone(),
        platform.clone(),
        executor,
        journal.clone(),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("execution should complete: {error}"));

    let crashing_executor: Arc<dyn ApprovedExecutorClient> = Arc::new(CrashExecutorClient::new(
        &sandbox,
        platform.clone(),
        CrashPoint::AfterMutation,
    ));
    let crashing_rollback = execution_service_with(
        database.clone(),
        platform.clone(),
        crashing_executor,
        journal.clone(),
    );
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = crashing_rollback.rollback_execution(completed.session.id, &mut |_| {});
    }));
    assert!(crash.is_err());
    let interrupted = database
        .execution_detail(completed.session.id)
        .unwrap_or_else(|error| panic!("interrupted rollback should load: {error}"));
    assert_eq!(
        interrupted.session.status,
        OrganizationExecutionStatus::RollingBack
    );
    assert!(
        interrupted
            .operations
            .iter()
            .any(|operation| { operation.status == ExecutionOperationStatus::RollingBack })
    );

    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let restarted = execution_service_with(database, platform, executor, journal);
    let assessment = restarted
        .recover_execution(completed.session.id)
        .unwrap_or_else(|error| panic!("rollback recovery should reconcile: {error}"));
    assert_eq!(
        assessment.state,
        ExecutionRecoveryState::RecoveryAvailable,
        "{assessment:#?}"
    );
    assert_eq!(assessment.applied, 1);
    let rolled_back = restarted
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("reconciled rollback should resume: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(
        rolled_back.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    assert_eq!(initial, sandbox.snapshot());
}

#[test]
fn recovery_fails_closed_when_both_source_and_destination_exist() {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    precreate_destination_parents(&sandbox, &proposal);
    let journal = Arc::new(MemoryJournal::default());
    let crashing_executor: Arc<dyn ApprovedExecutorClient> = Arc::new(CrashExecutorClient::new(
        &sandbox,
        platform.clone(),
        CrashPoint::BeforeMutation,
    ));
    let service = execution_service_with(
        database.clone(),
        platform.clone(),
        crashing_executor,
        journal.clone(),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = service.start_execution(approved.session.id, &mut |_| {});
    }));
    assert!(crash.is_err());
    let interrupted = database
        .execution_detail(approved.session.id)
        .unwrap_or_else(|error| panic!("interrupted detail should load: {error}"));
    let running = interrupted
        .operations
        .iter()
        .find(|operation| operation.status == ExecutionOperationStatus::Running)
        .unwrap_or_else(|| panic!("crashed operation should remain running"));
    let source = sandbox.path().join(
        running
            .source_relative_path
            .as_deref()
            .unwrap_or_else(|| panic!("file operation should have a source")),
    );
    let destination = sandbox.path().join(&running.destination_relative_path);
    assert_is_test_sandbox(sandbox.path(), &source);
    assert_is_test_sandbox(sandbox.path(), &destination);
    fs::copy(&source, &destination)
        .unwrap_or_else(|error| panic!("external ambiguous copy should succeed: {error}"));

    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let restarted = execution_service_with(database, platform, executor, journal);
    let assessment = restarted
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("recovery assessment should complete: {error}"));
    assert_eq!(assessment.state, ExecutionRecoveryState::RecoveryAmbiguous);
    assert!(assessment.ambiguous >= 1);
    assert!(
        restarted
            .rollback_execution(approved.session.id, &mut |_| {})
            .is_err()
    );
}

fn normalized_path(value: &str) -> std::path::PathBuf {
    value
        .replace('\\', "/")
        .split('/')
        .fold(std::path::PathBuf::new(), |path, part| path.join(part))
}

fn parent_segments(value: &str) -> Vec<String> {
    let normalized = value.replace('\\', "/");
    let mut segments = normalized.split('/').map(str::to_owned).collect::<Vec<_>>();
    let _ = segments.pop();
    segments
}

fn precreate_destination_parents(sandbox: &MutationSandbox, proposal: &OrganizationProposal) {
    for operation in &proposal.operations {
        if !matches!(
            operation.operation_kind,
            ProposalOperationKind::MoveProposal | ProposalOperationKind::RenameProposal
        ) {
            continue;
        }
        let destination = operation
            .proposed_destination
            .iter()
            .fold(std::path::PathBuf::new(), |path, segment| {
                path.join(segment)
            })
            .join(&operation.proposed_name);
        let absolute = sandbox.path().join(destination);
        assert_is_test_sandbox(sandbox.path(), &absolute);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("destination parent should be created: {error}"));
        }
    }
}

fn assert_crash_recovery(point: CrashPoint, expect_applied: bool) {
    let sandbox = MutationSandbox::new();
    let (database, platform, _scanner, _workspace_id, proposal) = approved_fixture(&sandbox);
    precreate_destination_parents(&sandbox, &proposal);
    let initial = sandbox.snapshot();
    let journal = Arc::new(MemoryJournal::default());
    let crashing_executor: Arc<dyn ApprovedExecutorClient> =
        Arc::new(CrashExecutorClient::new(&sandbox, platform.clone(), point));
    let service = execution_service_with(
        database.clone(),
        platform.clone(),
        crashing_executor,
        journal.clone(),
    );
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight should succeed: {error}"));
    assert!(
        prepared
            .operations
            .iter()
            .all(|operation| operation.kind != ExecutionOperationKind::CreateDirectory)
    );
    let approved = attest_execution(&service, prepared.session.id)
        .unwrap_or_else(|error| panic!("approval should succeed: {error}"));

    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = service.start_execution(approved.session.id, &mut |_| {});
    }));
    assert!(crash.is_err());
    let interrupted = database
        .execution_detail(approved.session.id)
        .unwrap_or_else(|error| panic!("interrupted execution should load: {error}"));
    assert_eq!(
        interrupted.session.status,
        OrganizationExecutionStatus::Running
    );
    let running = interrupted
        .operations
        .iter()
        .find(|operation| operation.status == ExecutionOperationStatus::Running)
        .unwrap_or_else(|| panic!("one operation should retain durable intent"));
    let events = journal
        .events(approved.session.id)
        .unwrap_or_else(|error| panic!("journal should be readable: {error}"));
    assert!(events.iter().any(|event| {
        event.step_id == Some(running.id) && event.kind == JournalEventKind::IntentDurable
    }));
    assert!(!events.iter().any(|event| {
        event.step_id == Some(running.id) && event.kind == JournalEventKind::AppliedObserved
    }));
    let durable_requests = database
        .executor_request_facts(approved.session.id)
        .unwrap_or_else(|error| panic!("durable request identity should load: {error}"));
    assert_eq!(durable_requests.len(), 1);
    assert_eq!(
        durable_requests[0].state,
        ExecutorRequestState::IntentDurable
    );
    assert_eq!(durable_requests[0].operation_id, running.id);

    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        sandbox.path(),
        platform.clone(),
    ));
    let restarted = execution_service_with(database.clone(), platform, executor, journal.clone());
    let startup = database
        .execution_detail(approved.session.id)
        .unwrap_or_else(|error| panic!("startup recovery state should load: {error}"));
    assert_eq!(
        startup.session.recovery_state,
        ExecutionRecoveryState::RecoveryAvailable
    );
    let assessment = restarted
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("recovery should reconcile the crash: {error}"));
    assert_eq!(assessment.state, ExecutionRecoveryState::RecoveryAvailable);
    assert_eq!(
        assessment.affected_count,
        assessment.not_started + assessment.applied + assessment.ambiguous
    );
    assert_eq!(assessment.ambiguous, 0);
    assert_eq!(
        database
            .executor_session_facts(approved.session.id)
            .unwrap_or_else(|error| panic!("startup must not open a forward session: {error}"))
            .len(),
        1,
        "recovery must reconcile without opening a forward executor session"
    );
    let reconciled_request = database
        .executor_request_facts(approved.session.id)
        .unwrap_or_else(|error| panic!("reconciled request should load: {error}"))
        .into_iter()
        .find(|request| request.request_id == durable_requests[0].request_id)
        .unwrap_or_else(|| panic!("exact durable request should remain"));
    if expect_applied {
        assert_eq!(assessment.applied, 1);
        assert_eq!(
            reconciled_request.state,
            ExecutorRequestState::ProvenApplied
        );
        assert!(
            assessment
                .verified_applied_items
                .iter()
                .any(|item| item.operation_id == running.id)
        );
    } else {
        assert_eq!(assessment.applied, 0);
        assert_eq!(
            reconciled_request.state,
            ExecutorRequestState::ProvenNotStarted
        );
        assert!(
            assessment
                .verified_not_started_items
                .iter()
                .any(|item| item.operation_id == running.id)
        );
    }
    let rolled_back = restarted
        .rollback_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("reconciled execution should roll back: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(
        rolled_back.session.recovery_state,
        ExecutionRecoveryState::RecoveryNotRequired
    );
    assert_eq!(initial, sandbox.snapshot());
    let recovered_sessions = database
        .executor_session_facts(approved.session.id)
        .unwrap_or_else(|error| panic!("session facts should remain queryable: {error}"));
    assert_eq!(
        recovered_sessions
            .iter()
            .filter(|session| session.purpose == ExecutorSessionPurpose::Forward)
            .count(),
        1
    );
    assert_eq!(
        recovered_sessions
            .iter()
            .filter(|session| session.purpose == ExecutorSessionPurpose::Rollback)
            .count(),
        usize::from(expect_applied)
    );
    assert!(
        database
            .validate_execution_journal(approved.session.id)
            .unwrap_or_else(|error| panic!("recovered journal should validate: {error}"))
    );
}

#[test]
fn mutation_test_guard_rejects_paths_outside_the_sandbox() {
    let sandbox = MutationSandbox::new();
    let outside = Path::new("/tmp/not-the-m8-sandbox");
    let result = std::panic::catch_unwind(|| assert_is_test_sandbox(sandbox.path(), outside));
    assert!(result.is_err());
}
