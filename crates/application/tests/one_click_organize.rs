#![cfg(any(target_os = "macos", target_os = "windows"))]

mod support;

use application::{
    ApprovedExecutorClient, ExecutionApplicationService, ExecutionConsentAuthorityKey,
    ScannerApplicationService,
};
use domain::{
    ExecutionId, OrganizationExecutionStatus, OrganizationProposalStatus, ProposalOperationKind,
};
use operations::{ApplyGate, DurableJournal, ExecutionSafetyPolicy, MemoryJournal};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use std::sync::Arc;
use support::{MutationSandbox, SandboxApprovedExecutorClient};

#[cfg(target_os = "macos")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_macos::MacOsPlatform)
}

#[cfg(target_os = "windows")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_windows::WindowsPlatform)
}

fn write_corpus(sandbox: &MutationSandbox) {
    let files = [
        ("invoice.pdf", b"%PDF-1.4 invoice" as &[u8]),
        ("screenshot.png", b"\x89PNG screenshot"),
        ("holiday.jpg", b"\xFF\xD8\xFF holiday"),
        ("school.docx", b"PK school"),
        ("notes.txt", b"personal notes"),
        ("video.mp4", b"ftypmp42"),
        ("archive.zip", b"PK zip"),
        ("setup.exe", b"MZ installer"),
        ("App.lnk", b"L\0\0\0 shortcut"),
        ("unknown.xyz", b"???"),
        ("chrome.exe", b"MZ chrome"),
        ("library.dll", b"MZ dll"),
    ];
    for (name, bytes) in files {
        sandbox.write(name, bytes);
    }
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
    ExecutionApplicationService::new(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()) as Arc<dyn DurableJournal>,
        ApplyGate {
            enabled: true,
            reason: "one-click sandbox".to_owned(),
        },
        ExecutionSafetyPolicy::default(),
        ExecutionConsentAuthorityKey::from_bytes([42; 32]),
    )
    .unwrap_or_else(|error| panic!("execution service should initialize: {error}"))
}

fn attest(
    service: &ExecutionApplicationService,
    execution_id: ExecutionId,
) -> domain::ExecutionDetail {
    let challenge = service
        .create_execution_consent_challenge(execution_id, None)
        .unwrap_or_else(|error| panic!("consent challenge should exist: {error}"));
    service
        .finalize_execution_consent(challenge)
        .unwrap_or_else(|error| panic!("consent should finalize: {error}"))
}

#[test]
fn one_click_consumer_proposal_moves_personal_files_and_undo_restores() {
    let sandbox = MutationSandbox::new();
    write_corpus(&sandbox);
    let before = sandbox.snapshot();
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([19; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}")),
    );
    let platform = native_platform();
    let scanner = ScannerApplicationService::new(database.clone(), platform.clone());
    let workspace = scanner
        .create_workspace("One-click corpus")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    let root = scanner
        .register_root(workspace.id, sandbox.path())
        .unwrap_or_else(|error| panic!("root should register: {error}"));
    let scan = scanner
        .scan_workspace_consumer(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("metadata-only consumer scan should succeed: {error}"));
    assert_eq!(
        scan.indexed_count, 12,
        "all top-level fixture files should be indexed"
    );
    assert_eq!(
        scan.hashed_count, 0,
        "one-click discovery must never hash content"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "scan and preview preparation must not mutate source files",
    );

    let proposal = scanner
        .generate_consumer_organization_proposal_for_root(
            workspace.id,
            root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("consumer proposal should build: {error}"));
    assert_eq!(
        sandbox.snapshot(),
        before,
        "proposal generation must remain read-only",
    );

    let by_name = proposal
        .operations
        .iter()
        .map(|operation| (operation.source_name.as_str(), operation))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        by_name["invoice.pdf"].operation_kind,
        ProposalOperationKind::MoveProposal
    );
    assert_eq!(
        by_name["invoice.pdf"]
            .proposed_destination
            .first()
            .map(String::as_str),
        Some("Documents")
    );
    assert_eq!(
        by_name["setup.exe"]
            .proposed_destination
            .first()
            .map(String::as_str),
        Some("Installateurs")
    );
    assert_eq!(
        by_name["chrome.exe"].operation_kind,
        ProposalOperationKind::KeepInPlace
    );
    assert_eq!(
        by_name["library.dll"].operation_kind,
        ProposalOperationKind::KeepInPlace
    );
    assert_eq!(
        by_name["App.lnk"].operation_kind,
        ProposalOperationKind::KeepInPlace
    );
    assert_eq!(by_name["unknown.xyz"].proposed_destination, ["À vérifier"]);
    assert!(proposal.summary.maximum_depth <= 3);

    let approved = scanner
        .set_organization_proposal_status(
            proposal.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("proposal should approve: {error}"));
    let service = execution_service(database, platform, &sandbox);
    let prepared = service
        .prepare_execution(approved.id, approved.revision)
        .unwrap_or_else(|error| panic!("prepare should pass: {error}"));
    let attested = attest(&service, prepared.session.id);
    let completed = service
        .start_execution(attested.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("apply should complete: {error}"));
    assert!(matches!(
        completed.session.status,
        OrganizationExecutionStatus::Completed | OrganizationExecutionStatus::Partial
    ));
    let root_path = sandbox.path();
    assert!(root_path.join("Documents").is_dir());
    assert!(root_path.join("Installateurs").is_dir());
    assert!(!root_path.join("invoice.pdf").exists());
    assert!(root_path.join("chrome.exe").is_file());
    assert!(root_path.join("library.dll").is_file());
    assert!(root_path.join("App.lnk").is_file());
    assert!(!root_path.join("unknown.xyz").exists());
    assert!(root_path.join("À vérifier").join("unknown.xyz").is_file());

    let rolled_back = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("undo should complete: {error}"));
    assert!(matches!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack | OrganizationExecutionStatus::RollbackPartial
    ));
    assert_eq!(sandbox.snapshot(), before);
}

#[test]
fn consumer_scan_has_no_arbitrary_file_count_cap() {
    let sandbox = MutationSandbox::new();
    const FILE_COUNT: usize = 5_257;
    for index in 0..FILE_COUNT {
        sandbox.write(&format!("loose-{index:05}.txt"), b"x");
    }
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([31; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}")),
    );
    let platform = native_platform();
    let scanner = ScannerApplicationService::new(database, platform);
    let workspace = scanner
        .create_workspace("Unbounded one-click corpus")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    scanner
        .register_root(workspace.id, sandbox.path())
        .unwrap_or_else(|error| panic!("root should register: {error}"));
    let scan = scanner
        .scan_workspace_consumer(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("unbounded metadata scan should succeed: {error}"));
    assert_eq!(scan.indexed_count as usize, FILE_COUNT);
    assert!(
        !scan.truncated,
        "one-click must not silently truncate a large folder"
    );
}
