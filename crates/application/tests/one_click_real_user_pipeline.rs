mod support;

use application::{
    ApprovedExecutorClient, ExecutionApplicationService, ExecutionConsentAuthorityKey,
    ScannerApplicationService,
};
use domain::{OrganizationExecutionStatus, OrganizationProposalStatus, ProposalOperationKind};
use operations::{ApplyGate, ExecutionSafetyPolicy, MemoryJournal};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use std::{fs, sync::Arc};
use support::{MutationSandbox, SandboxApprovedExecutorClient, assert_is_test_sandbox};

#[cfg(target_os = "macos")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_macos::MacOsPlatform)
}

#[cfg(target_os = "windows")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_windows::WindowsPlatform)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn one_click_user_selected_folder_is_analyzed_moved_monitored_and_exactly_undoable() {
    let sandbox = MutationSandbox::new();

    // The sandbox itself represents the exact folder selected by the user.
    // Loose files prove normal classification; nested project folders prove that
    // coherent subtrees can be moved as blocks without escaping the selected scope.
    sandbox.write(
        "notes.txt",
        b"Compte rendu chantier Bordeaux pour le client Martin. Travaux et devis a verifier.",
    );
    sandbox.write(
        "facture_2026.txt",
        b"Facture 2026 client Martin montant 1400 EUR. Chantier Bordeaux.",
    );
    sandbox.write("photo.jpg", b"fake-jpeg-private-beta-fixture");
    sandbox.write(
        "portfolio/package.json",
        br#"{"name":"portfolio","scripts":{"dev":"vite"}}"#,
    );
    sandbox.write("portfolio/src/index.js", b"console.log('portfolio');");
    sandbox.write("lodash/package.json", br#"{"name":"lodash-local"}"#);
    sandbox.write("lodash/fp/map.js", b"export const map = () => {};");
    sandbox.write(
        "maquette-experience-esport/index.html",
        b"<html><body>maquette esport</body></html>",
    );
    sandbox.write(
        "maquette-experience-esport/assets/app.css",
        b"body { margin: 0; }",
    );

    let initial = sandbox.snapshot();
    let selected_root = sandbox.path();
    assert_is_test_sandbox(selected_root, selected_root);

    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([231; 32]))
            .unwrap_or_else(|error| panic!("real-user database should open: {error}")),
    );
    let platform = native_platform();
    let scanner = ScannerApplicationService::new(database.clone(), platform.clone());
    let workspace = scanner
        .create_workspace("One-Click selected-folder acceptance")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));

    let root = scanner
        .register_root(workspace.id, selected_root)
        .unwrap_or_else(|error| panic!("the exact user-selected folder should register: {error}"));

    // Selecting a folder once must make it an enabled monitored root so future
    // files can be detected without selecting/importing the folder again.
    let monitoring = scanner
        .monitoring_dashboard(workspace.id)
        .unwrap_or_else(|error| panic!("monitoring dashboard should load: {error}"));
    assert!(
        monitoring
            .roots
            .iter()
            .any(|candidate| candidate.root_id == root.id && candidate.enabled),
        "the selected folder was not enabled for future-file monitoring"
    );

    let scan = scanner
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("recursive selected-folder scan should succeed: {error}"));
    assert_eq!(
        scan.indexed_count, 9,
        "all files inside the selected folder, including nested files, must be indexed"
    );

    let proposal = scanner
        .generate_consumer_organization_proposal_for_root(
            workspace.id,
            root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("One-Click proposal should generate: {error}"));

    assert!(
        proposal.source_semantic_version.is_some(),
        "Ranger generated a proposal without running local semantic analysis"
    );
    assert_eq!(proposal.summary.files_analyzed, 9);
    assert!(
        proposal.summary.proposed_moves >= 9,
        "selected dirty folder produced no real moves: {:#?}",
        proposal.summary
    );

    // Regression guard for the strict persistence schema: internal placement
    // markers must never leak into the bounded semantic_context field.
    assert!(proposal.operations.iter().all(|operation| {
        matches!(
            operation.semantic_context.as_str(),
            "personal" | "business" | "mixed" | "unknown"
        )
    }));

    assert!(proposal.operations.iter().any(|operation| {
        operation.source.relative_path.replace('\\', "/") == "notes.txt"
            && operation.operation_kind == ProposalOperationKind::MoveProposal
            && operation.proposed_destination == ["Documents", "Travail"]
    }));
    assert!(proposal.operations.iter().any(|operation| {
        operation.source.relative_path.replace('\\', "/") == "facture_2026.txt"
            && operation.operation_kind == ProposalOperationKind::MoveProposal
            && operation.proposed_destination == ["Documents", "Administratif", "Factures"]
    }));
    assert!(proposal.operations.iter().any(|operation| {
        operation.source.relative_path.replace('\\', "/") == "photo.jpg"
            && operation.operation_kind == ProposalOperationKind::MoveProposal
            && operation.proposed_destination == ["Images", "Photos"]
    }));

    let proposal = scanner
        .set_organization_proposal_status(
            proposal.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("proposal should be approved: {error}"));

    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(SandboxApprovedExecutorClient::new(
        selected_root,
        platform.clone(),
    ));
    let execution = ExecutionApplicationService::new(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
        ApplyGate {
            enabled: true,
            reason: "isolated selected-folder acceptance sandbox".to_owned(),
        },
        ExecutionSafetyPolicy::default(),
        ExecutionConsentAuthorityKey::from_bytes([232; 32]),
    )
    .unwrap_or_else(|error| panic!("execution service should initialize: {error}"));

    let prepared = execution
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("generated proposal should preflight: {error}"));
    let challenge = execution
        .create_execution_consent_challenge(prepared.session.id, None)
        .unwrap_or_else(|error| panic!("consent challenge should be created: {error}"));
    let approved = execution
        .finalize_execution_consent(challenge)
        .unwrap_or_else(|error| panic!("consent should be finalized: {error}"));
    let completed = execution
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("physical One-Click Apply should succeed: {error}"));
    assert_eq!(
        completed.session.status,
        OrganizationExecutionStatus::Completed
    );
    assert_eq!(completed.session.summary.failed, 0);
    assert_eq!(completed.session.summary.blocked, 0);
    assert_eq!(completed.session.summary.skipped, 0);
    assert!(completed.session.summary.applied >= 9);

    let notes_destination = selected_root
        .join("Documents")
        .join("Travail")
        .join("notes.txt");
    let invoice_destination = selected_root
        .join("Documents")
        .join("Administratif")
        .join("Factures")
        .join("facture_2026.txt");
    let photo_destination = selected_root.join("Images").join("Photos").join("photo.jpg");
    for path in [&notes_destination, &invoice_destination, &photo_destination] {
        assert_is_test_sandbox(selected_root, path);
        assert!(
            path.is_file(),
            "expected physical destination missing: {}",
            path.display()
        );
    }
    assert!(!selected_root.join("notes.txt").exists());
    assert!(!selected_root.join("facture_2026.txt").exists());
    assert!(!selected_root.join("photo.jpg").exists());

    let portfolio_destination = selected_root
        .join("Développement")
        .join("Projets")
        .join("portfolio");
    let lodash_destination = selected_root
        .join("Développement")
        .join("Projets")
        .join("lodash");
    let maquette_destination = selected_root
        .join("Développement")
        .join("Projets")
        .join("maquette-experience-esport");
    assert!(portfolio_destination.join("package.json").is_file());
    assert!(portfolio_destination.join("src/index.js").is_file());
    assert!(lodash_destination.join("package.json").is_file());
    assert!(lodash_destination.join("fp/map.js").is_file());
    assert!(maquette_destination.join("index.html").is_file());
    assert!(maquette_destination.join("assets/app.css").is_file());
    assert!(
        !selected_root.join("portfolio").exists(),
        "old project folder must disappear after its preserved tree is moved"
    );
    assert!(
        !selected_root.join("lodash").exists(),
        "package-like clutter must no longer remain at selected-folder root"
    );
    assert!(
        !selected_root.join("maquette-experience-esport").exists(),
        "project folder must move as one preserved tree"
    );

    assert_eq!(
        fs::read(&invoice_destination)
            .unwrap_or_else(|error| panic!("moved invoice should remain readable: {error}")),
        b"Facture 2026 client Martin montant 1400 EUR. Chantier Bordeaux."
    );

    let rolled_back = execution
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("One-Click Undo should succeed: {error}"));
    assert_eq!(
        rolled_back.session.status,
        OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(
        initial,
        sandbox.snapshot(),
        "Undo must restore the exact original selected-folder tree and bytes"
    );
}
