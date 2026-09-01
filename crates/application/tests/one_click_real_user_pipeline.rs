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
fn one_click_real_dirty_desktop_is_analyzed_moved_and_exactly_undoable() {
    let sandbox = MutationSandbox::new();
    sandbox.write(
        "Desktop/Clients/Martin/Chantier Bordeaux/notes.txt",
        b"Compte rendu chantier Bordeaux pour le client Martin. Travaux et devis a verifier.",
    );
    sandbox.write(
        "Desktop/Divers/facture_2026.txt",
        b"Facture 2026 client Martin montant 1400 EUR. Chantier Bordeaux.",
    );
    sandbox.write(
        "Desktop/Ancien dossier/Sous dossier/photo.jpg",
        b"fake-jpeg-private-beta-fixture",
    );
    sandbox.write(
        "Desktop/portfolio/package.json",
        br#"{"name":"portfolio","scripts":{"dev":"vite"}}"#,
    );
    sandbox.write(
        "Desktop/portfolio/src/index.js",
        b"console.log('portfolio');",
    );
    sandbox.write("Desktop/lodash/package.json", br#"{"name":"lodash-local"}"#);
    sandbox.write("Desktop/lodash/fp/map.js", b"export const map = () => {};");
    sandbox.write(
        "Desktop/maquette-experience-esport/index.html",
        b"<html><body>maquette esport</body></html>",
    );
    sandbox.write(
        "Desktop/maquette-experience-esport/assets/app.css",
        b"body { margin: 0; }",
    );

    let initial = sandbox.snapshot();
    let mutation_root = sandbox.path();
    let desktop = mutation_root.join("Desktop");
    assert_is_test_sandbox(mutation_root, &desktop);
    assert!(desktop.is_dir());

    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([231; 32]))
            .unwrap_or_else(|error| panic!("real-user database should open: {error}")),
    );
    let platform = native_platform();
    let scanner = ScannerApplicationService::new(database.clone(), platform.clone());
    let workspace = scanner
        .create_workspace("One-Click real user acceptance")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));

    // The execution sandbox itself stays the registered root so every physical mutation
    // remains protected by the same fail-closed guard used by the qualification suite.
    // `Desktop/` is intentionally a real nested dirty tree to prove recursive discovery.
    let root = scanner
        .register_root(workspace.id, mutation_root)
        .unwrap_or_else(|error| panic!("guarded sandbox root should register: {error}"));

    let scan = scanner
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("recursive dirty-tree scan should succeed: {error}"));
    assert_eq!(
        scan.indexed_count, 9,
        "all nested fixture files must be indexed"
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

    // The real Ranger path must use the semantic pipeline, not only extensions/path names.
    assert!(
        proposal.source_semantic_version.is_some(),
        "One-Click generated a proposal without first running semantic analysis"
    );
    assert_eq!(proposal.summary.files_analyzed, 9);
    assert!(
        proposal.summary.proposed_moves >= 9,
        "dirty nested files produced no real moves: {:#?}",
        proposal.summary
    );
    assert!(proposal.operations.iter().any(|operation| {
        operation.source.relative_path.replace('\\', "/")
            == "Desktop/Clients/Martin/Chantier Bordeaux/notes.txt"
            && operation.operation_kind == ProposalOperationKind::MoveProposal
            && operation.proposed_destination
                == [
                    "Documents",
                    "Travail",
                    "Clients",
                    "Martin",
                    "Chantier Bordeaux",
                ]
    }));
    assert!(proposal.operations.iter().any(|operation| {
        operation.source.relative_path.replace('\\', "/") == "Desktop/Divers/facture_2026.txt"
            && operation.operation_kind == ProposalOperationKind::MoveProposal
            && operation.proposed_destination == ["Documents", "Administratif", "Factures"]
    }));
    assert!(proposal.operations.iter().any(|operation| {
        operation.source.relative_path.replace('\\', "/")
            == "Desktop/Ancien dossier/Sous dossier/photo.jpg"
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
        mutation_root,
        platform.clone(),
    ));
    let execution = ExecutionApplicationService::new(
        database,
        platform,
        executor,
        Arc::new(MemoryJournal::default()),
        ApplyGate {
            enabled: true,
            reason: "isolated real-user One-Click acceptance sandbox".to_owned(),
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
    assert!(completed.session.summary.applied >= 3);

    let notes_destination = mutation_root
        .join("Documents")
        .join("Travail")
        .join("Clients")
        .join("Martin")
        .join("Chantier Bordeaux")
        .join("notes.txt");
    let invoice_destination = mutation_root
        .join("Documents")
        .join("Administratif")
        .join("Factures")
        .join("facture_2026.txt");
    let photo_destination = mutation_root
        .join("Images")
        .join("Photos")
        .join("photo.jpg");
    for path in [&notes_destination, &invoice_destination, &photo_destination] {
        assert_is_test_sandbox(mutation_root, path);
        assert!(
            path.is_file(),
            "expected physical destination missing: {}",
            path.display()
        );
    }
    assert!(
        !desktop
            .join("Clients/Martin/Chantier Bordeaux/notes.txt")
            .exists()
    );
    assert!(!desktop.join("Divers/facture_2026.txt").exists());
    assert!(
        !desktop
            .join("Ancien dossier/Sous dossier/photo.jpg")
            .exists()
    );

    let portfolio_destination = desktop
        .join("Développement")
        .join("Projets")
        .join("portfolio");
    let lodash_destination = desktop.join("Développement").join("Projets").join("lodash");
    let maquette_destination = desktop
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
        !desktop.join("portfolio").exists(),
        "the old top-level project folder must be removed once empty"
    );
    assert!(
        !desktop.join("lodash").exists(),
        "package-like clutter must no longer remain on the Desktop"
    );
    assert!(
        !desktop.join("maquette-experience-esport").exists(),
        "work/project folder must move as one preserved tree"
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
        "Undo must restore the exact original tree and bytes"
    );
}
