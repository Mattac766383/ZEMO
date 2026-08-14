#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::{ProposalBuildPhase, ScannerApplicationService};
use domain::{OrganizationProposalStatus, ProposalOperationKind, ProposalOverrideAction};
use organizer::{DEFAULT_MAX_FILENAME_UTF16, DEFAULT_MAX_SEGMENT_UTF16, validate_component};
use persistence::{Database, DatabaseKey};
use platform::{ChangeHint, ChangeScope, LocalEventKind, ReadOnlyPlatform};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

#[cfg(target_os = "macos")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_macos::MacOsPlatform)
}

#[cfg(target_os = "windows")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_windows::WindowsPlatform)
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    assert!(target.starts_with(root) && target != root);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
    }
    fs::write(target, content)
        .unwrap_or_else(|error| panic!("fixture file should be written: {error}"));
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, blake3::Hash)> {
    let mut output = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("fixture directory should be readable: {error}"))
        {
            let entry =
                entry.unwrap_or_else(|error| panic!("fixture entry should be readable: {error}"));
            let path = entry.path();
            let metadata = entry
                .metadata()
                .unwrap_or_else(|error| panic!("fixture metadata should be readable: {error}"));
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("fixture bytes should be readable: {error}"));
                output.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|error| panic!("path should remain scoped: {error}"))
                        .to_path_buf(),
                    (metadata.len(), blake3::hash(&bytes)),
                );
            }
        }
    }
    output
}

#[test]
fn proposal_pipeline_is_persistent_reviewable_and_never_mutates_sources() {
    let fixture = TempDir::new().expect("fixture should exist");
    let fixtures = [
        (
            "Downloads/scan_38492.txt",
            "FACTURE\nCustomer: Dupont SARL\nSupplier: Point P\nProject: Project Bordeaux\nProject reference: BDX-2026\nInvoice number: FP-39482\nDate: 2026-06-17\nTotal: 1437.82 EUR",
        ),
        (
            "Downloads/devis-final.txt",
            "DEVIS\nCustomer: Dupont SARL\nProject: Project Bordeaux\nProject reference: BDX-2026\nQuote number: D-2026-04\nDate: 2026-05-10\nMontant: 900 EUR",
        ),
        (
            "Business/Clients/Dupont SARL/Invoices/Invoice-2025.txt",
            "FACTURE\nCustomer: Dupont SARL\nInvoice number: 2025-10\nDate: 2025-11-02\nTotal: 200 EUR",
        ),
        (
            "Downloads/unknown.bin",
            "unlabeled bytes without reliable meaning",
        ),
        (
            "Downloads/duplicate-a.txt",
            "FACTURE\nSupplier: Exact Copy Ltd\nInvoice number: COPY-1\nTotal: 10 EUR",
        ),
        (
            "Downloads/duplicate-b.txt",
            "FACTURE\nSupplier: Exact Copy Ltd\nInvoice number: COPY-1\nTotal: 10 EUR",
        ),
    ];
    for (path, content) in fixtures {
        write_file(fixture.path(), path, content);
    }
    let before = snapshot(fixture.path());
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([107; 32]))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database.clone(), native_platform());
    let workspace = service
        .create_workspace("Milestone 7 organization fixtures")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("fixture root should register: {error}"));
    let scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should succeed: {error}"));
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("extraction should succeed: {error}"));
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantic analysis should succeed: {error}"));
    service
        .resolve_workspace_identities(workspace.id, "manual", true, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("relationship resolution should succeed: {error}"));

    let mut phases = Vec::new();
    let proposal = service
        .generate_organization_proposal(workspace.id, false, &|| false, &mut |progress| {
            phases.push(progress.phase)
        })
        .unwrap_or_else(|error| panic!("proposal generation should succeed: {error}"));
    assert_eq!(proposal.status, OrganizationProposalStatus::ReadyForReview);
    assert_eq!(proposal.summary.files_analyzed, fixtures.len() as u64);
    assert_eq!(proposal.operations.len(), fixtures.len());
    assert!(phases.contains(&ProposalBuildPhase::Evaluating));
    assert!(phases.contains(&ProposalBuildPhase::BuildingTree));
    assert!(phases.contains(&ProposalBuildPhase::Completed));
    assert!(
        proposal
            .nodes
            .iter()
            .any(|node| node.kind == domain::VirtualNodeKind::Root)
    );
    assert!(
        proposal
            .operations
            .iter()
            .any(|operation| operation.operation_kind == ProposalOperationKind::ToReview)
    );
    assert!(proposal.operations.iter().all(|operation| {
        operation
            .proposed_destination
            .iter()
            .all(|segment| validate_component(segment, DEFAULT_MAX_SEGMENT_UTF16).is_ok())
            && validate_component(&operation.proposed_name, DEFAULT_MAX_FILENAME_UTF16).is_ok()
            && operation.proposed_depth <= 6
            && operation.proposed_path_length <= 240
    }));
    assert_eq!(before, snapshot(fixture.path()));

    let editable = proposal
        .operations
        .iter()
        .find(|operation| operation.source_name == "scan_38492.txt")
        .unwrap_or_else(|| panic!("generic invoice should be present"));
    let edited = service
        .set_organization_proposal_override(
            proposal.id,
            editable.file_id,
            ProposalOverrideAction::DestinationAndRename,
            Some(vec!["Business".into(), "Chosen by user".into()]),
            Some("2026-06-17_Invoice_FP-39482.txt".into()),
            Some("authoritative test override".into()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("virtual edit should succeed: {error}"));
    let edited_operation = edited
        .operations
        .iter()
        .find(|operation| operation.file_id == editable.file_id)
        .unwrap_or_else(|| panic!("edited operation should remain present"));
    assert_eq!(
        edited_operation.proposed_destination,
        ["Business", "Chosen by user"]
    );
    assert_eq!(
        edited_operation.proposed_name,
        "2026-06-17_Invoice_FP-39482.txt"
    );
    assert!(edited_operation.user_override);
    assert_eq!(edited.revision, 2);

    let recomputed = service
        .generate_organization_proposal(workspace.id, true, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("safe recomputation should succeed: {error}"));
    let recomputed_operation = recomputed
        .operations
        .iter()
        .find(|operation| operation.file_id == editable.file_id)
        .unwrap_or_else(|| panic!("recomputed operation should remain present"));
    assert_eq!(
        recomputed_operation.proposed_destination,
        ["Business", "Chosen by user"]
    );
    assert!(recomputed_operation.user_override);
    assert_eq!(recomputed.revision, 3);

    let rejected_file_id = recomputed
        .operations
        .iter()
        .find(|operation| operation.source_name == "unknown.bin")
        .map(|operation| operation.file_id)
        .unwrap_or_else(|| panic!("review item should remain available"));
    let rejected = service
        .set_organization_proposal_override(
            proposal.id,
            rejected_file_id,
            ProposalOverrideAction::Reject,
            None,
            None,
            Some("keep this file where it is".into()),
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("virtual rejection should succeed: {error}"));
    let rejected_operation = rejected
        .operations
        .iter()
        .find(|operation| operation.file_id == rejected_file_id)
        .unwrap_or_else(|| panic!("rejected operation should remain present"));
    assert_eq!(
        rejected_operation.operation_kind,
        ProposalOperationKind::KeepInPlace
    );
    assert_eq!(
        rejected_operation.proposed_relative_path(),
        "Downloads\\unknown.bin"
    );
    assert_eq!(rejected.revision, 4);

    let approved = service
        .set_organization_proposal_status(
            proposal.id,
            OrganizationProposalStatus::ApprovedForFutureApply,
        )
        .unwrap_or_else(|error| panic!("future-only approval should persist: {error}"));
    assert_eq!(
        approved.status,
        OrganizationProposalStatus::ApprovedForFutureApply
    );
    assert_eq!(before, snapshot(fixture.path()));

    let cancelled = service
        .generate_organization_proposal(workspace.id, false, &|| true, &mut |_| {})
        .unwrap_or_else(|error| panic!("cancellation should be consistent: {error}"));
    assert_eq!(cancelled.status, OrganizationProposalStatus::Cancelled);
    assert!(cancelled.operations.is_empty());
    let still_current = service
        .latest_organization_proposal(workspace.id)
        .unwrap_or_else(|error| panic!("prior valid proposal should remain current: {error}"));
    assert_eq!(still_current.id, proposal.id);
    assert_eq!(before, snapshot(fixture.path()));
}

#[test]
fn proposals_and_duplicate_sets_remain_strictly_scoped_per_root() {
    let fixture = TempDir::new().expect("fixture should exist");
    let root_a_path = fixture.path().join("root-a");
    let root_b_path = fixture.path().join("root-b");
    fs::create_dir_all(&root_a_path).expect("root A should exist");
    fs::create_dir_all(&root_b_path).expect("root B should exist");
    let duplicate = "FACTURE\nSupplier: Root Scoped Ltd\nInvoice: DUP-10\nTotal: 10 EUR";
    write_file(&root_a_path, "file1.txt", duplicate);
    write_file(&root_a_path, "file1-copy.txt", duplicate);
    write_file(&root_b_path, "file1.txt", duplicate);

    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([117; 32]))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database, native_platform());
    let workspace = service
        .create_workspace("Per-root monitoring isolation")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));

    let root_a = service
        .register_root(workspace.id, &root_a_path)
        .unwrap_or_else(|error| panic!("root A should register: {error}"));
    let scan_a = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("root A should scan: {error}"));
    service
        .analyze_scan_content(scan_a.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("root A extraction should succeed: {error}"));
    service
        .analyze_scan_semantics(scan_a.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("root A semantics should succeed: {error}"));
    let proposal_a = service
        .generate_organization_proposal_for_root(
            workspace.id,
            root_a.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("root A proposal should generate: {error}"));
    assert_eq!(proposal_a.root_id, root_a.id);
    let duplicate_groups_a = service
        .scan_duplicate_groups(scan_a.id)
        .unwrap_or_else(|error| panic!("root A duplicates should load: {error}"));
    assert_eq!(duplicate_groups_a.len(), 1);
    assert_eq!(duplicate_groups_a[0].files.len(), 2);

    let root_b = service
        .register_root(workspace.id, &root_b_path)
        .unwrap_or_else(|error| panic!("root B should register: {error}"));
    let scan_b = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("root B should scan: {error}"));
    service
        .analyze_scan_content(scan_b.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("root B extraction should succeed: {error}"));
    service
        .analyze_scan_semantics(scan_b.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("root B semantics should succeed: {error}"));
    let proposal_b = service
        .generate_organization_proposal_for_root(
            workspace.id,
            root_b.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("root B proposal should generate: {error}"));
    assert_eq!(proposal_b.root_id, root_b.id);
    assert_ne!(proposal_a.id, proposal_b.id);
    assert_eq!(proposal_a.operations.len(), 2);
    assert_eq!(proposal_b.operations.len(), 1);
    assert!(
        service
            .scan_duplicate_groups(scan_b.id)
            .unwrap_or_else(|error| panic!("root B duplicates should load: {error}"))
            .is_empty()
    );
    assert_eq!(
        service
            .latest_organization_proposal_for_root(workspace.id, root_a.id)
            .unwrap_or_else(|error| panic!("root A proposal should remain current: {error}"))
            .id,
        proposal_a.id
    );
    assert_eq!(
        service
            .scan_duplicate_groups(scan_a.id)
            .unwrap_or_else(|error| panic!("root A duplicates should remain: {error}"))
            .len(),
        1
    );
    assert!(
        service
            .generate_organization_proposal(workspace.id, false, &|| false, &mut |_| {})
            .is_err(),
        "workspace-wide generation must reject ambiguous multi-root scope"
    );
}

#[test]
fn monitoring_proposal_is_reviewable_after_database_restart_without_rescanning() {
    let fixture = TempDir::new().expect("fixture should exist");
    let database_dir = TempDir::new().expect("database directory should exist");
    write_file(
        fixture.path(),
        "persisted-invoice.txt",
        "FACTURE\nSupplier: Persisted Ltd\nInvoice: RESTART-1\nTotal: 25 EUR",
    );
    let database_path = database_dir.path().join("catalog.db");
    let key_bytes = [118_u8; 32];
    let database = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(key_bytes))
            .unwrap_or_else(|error| panic!("database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database.clone(), native_platform());
    let workspace = service
        .create_workspace("Restart proposal")
        .unwrap_or_else(|error| panic!("workspace should exist: {error}"));
    let root = service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("root should register: {error}"));
    let scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should complete: {error}"));
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("content should complete: {error}"));
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantics should complete: {error}"));
    let proposal = service
        .generate_organization_proposal_for_root(
            workspace.id,
            root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("proposal should generate: {error}"));
    drop(service);
    drop(database);

    let reopened = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(key_bytes))
            .unwrap_or_else(|error| panic!("database should reopen: {error}")),
    );
    let restored_service = ScannerApplicationService::new(reopened, native_platform());
    let restored = restored_service
        .restore_workspace_session()
        .unwrap_or_else(|error| panic!("workspace should restore: {error}"))
        .unwrap_or_else(|| panic!("workspace session should exist"));
    assert_eq!(restored.workspace.id, workspace.id);
    assert_eq!(restored.root.map(|value| value.id), Some(root.id));
    let persisted = restored_service
        .latest_organization_proposal_for_root(workspace.id, root.id)
        .unwrap_or_else(|error| panic!("proposal should load without rescanning: {error}"));
    assert_eq!(persisted.id, proposal.id);
    assert_eq!(persisted.operations.len(), 1);
    assert_eq!(persisted.operations[0].source_name, "persisted-invoice.txt");
}

#[test]
fn catalog_drift_marks_preview_stale_without_touching_the_file() {
    let fixture = TempDir::new().expect("fixture should exist");
    write_file(
        fixture.path(),
        "scan.txt",
        "FACTURE\nInvoice number: DRIFT-1\nDate: 2026-01-01\nTotal: 10 EUR",
    );
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([108; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database, native_platform());
    let workspace = service
        .create_workspace("Milestone 7 drift fixture")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("root should register: {error}"));
    let scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should succeed: {error}"));
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("extraction should succeed: {error}"));
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantics should succeed: {error}"));
    let proposal = service
        .generate_organization_proposal(workspace.id, false, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("proposal should build: {error}"));

    let changed_content = "FACTURE\nInvoice number: DRIFT-2\nDate: 2026-01-02\nTotal: 20 EUR";
    write_file(fixture.path(), "scan.txt", changed_content);
    service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("rescan should succeed: {error}"));
    let (changed, refreshed) = service
        .refresh_organization_proposal_drift(proposal.id)
        .unwrap_or_else(|error| panic!("drift refresh should succeed: {error}"));
    assert_eq!(changed, 1);
    assert!(refreshed.operations[0].stale);
    assert_eq!(
        refreshed.operations[0].conflict_state,
        domain::ProposalConflictState::StaleSource
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join("scan.txt"))
            .unwrap_or_else(|error| panic!("source should remain readable: {error}")),
        changed_content
    );
}

#[test]
fn ten_thousand_file_proposal_and_database_round_trip_stays_bounded() {
    const FILE_COUNT: usize = 10_000;
    let fixture = TempDir::new().expect("scale fixture should exist");
    let source_directory = fixture.path().join("Scale");
    fs::create_dir(&source_directory)
        .unwrap_or_else(|error| panic!("scale directory should be created: {error}"));
    for index in 0..FILE_COUNT {
        fs::write(
            source_directory.join(format!("unclassified_{index:05}.dat")),
            format!("opaque local fixture {index:05}"),
        )
        .unwrap_or_else(|error| panic!("scale fixture should be written: {error}"));
    }

    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([109; 32]))
            .unwrap_or_else(|error| panic!("scale database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database.clone(), native_platform());
    let workspace = service
        .create_workspace("Milestone 7 scale fixture")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    let root = service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("scale root should register: {error}"));
    let scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scale scan should succeed: {error}"));
    assert_eq!(scan.indexed_count, FILE_COUNT as u64);

    let started = Instant::now();
    let mut evaluating_at = None;
    let mut building_tree_at = None;
    let mut engine_completed_at = None;
    let proposal = service
        .generate_organization_proposal(workspace.id, false, &|| false, &mut |progress| {
            match progress.phase {
                ProposalBuildPhase::Evaluating => {
                    evaluating_at.get_or_insert_with(Instant::now);
                }
                ProposalBuildPhase::BuildingTree => {
                    building_tree_at.get_or_insert_with(Instant::now);
                }
                ProposalBuildPhase::Completed => engine_completed_at = Some(Instant::now()),
                _ => {}
            }
        })
        .unwrap_or_else(|error| panic!("scale proposal should build and persist: {error}"));
    let finished = Instant::now();
    let evaluating_at =
        evaluating_at.unwrap_or_else(|| panic!("evaluation timing should be captured"));
    let building_tree_at =
        building_tree_at.unwrap_or_else(|| panic!("tree timing should be captured"));
    let engine_completed_at =
        engine_completed_at.unwrap_or_else(|| panic!("engine timing should be captured"));
    let source_db_time = evaluating_at.duration_since(started);
    let engine_time = engine_completed_at.duration_since(evaluating_at);
    let tree_time = engine_completed_at.duration_since(building_tree_at);
    let persistence_db_time = finished.duration_since(engine_completed_at);
    let total_time = finished.duration_since(started);
    let retained_bytes = serde_json::to_vec(&proposal)
        .unwrap_or_else(|error| panic!("proposal should be measurable: {error}"))
        .len();

    assert_eq!(proposal.summary.files_analyzed, FILE_COUNT as u64);
    assert_eq!(proposal.operations.len(), FILE_COUNT);
    assert_eq!(proposal.summary.conflicts, 0);
    assert_eq!(proposal.summary.needs_review, FILE_COUNT as u64);
    assert!(proposal.summary.maximum_depth <= 6);
    assert!(total_time.as_secs() < 60);
    assert!(retained_bytes < 100 * 1024 * 1024);
    println!(
        "M7_DB_SCALE files={FILE_COUNT} total_ms={} engine_ms={} tree_ms={} source_db_ms={} persistence_db_ms={} retained_bytes={} conflicts={} review={} average_depth={:.2} maximum_depth={}",
        total_time.as_millis(),
        engine_time.as_millis(),
        tree_time.as_millis(),
        source_db_time.as_millis(),
        persistence_db_time.as_millis(),
        retained_bytes,
        proposal.summary.conflicts,
        proposal.summary.needs_review,
        proposal.summary.average_depth,
        proposal.summary.maximum_depth,
    );

    let second_fixture = TempDir::new().expect("second scale root should exist");
    write_file(
        second_fixture.path(),
        "independent.txt",
        "independent monitored root",
    );
    service
        .register_root(workspace.id, second_fixture.path())
        .unwrap_or_else(|error| panic!("second scale root should register: {error}"));
    let second_scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("second scale root should scan: {error}"));
    assert_eq!(second_scan.indexed_count, 1);
    database
        .mark_startup_reconciliation_completed(workspace.id)
        .unwrap_or_else(|error| panic!("scale startup reconciliation should clear: {error}"));

    let mut burst = (0..1_000)
        .map(|_| ChangeHint {
            root_token: "scale-root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("Scale/unclassified_00000.dat")),
            path_before: None,
            kind: LocalEventKind::Modified,
            scope: ChangeScope::File,
        })
        .collect::<Vec<_>>();
    burst.extend([
        ChangeHint {
            root_token: "scale-root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("Scale/new.dat")),
            path_before: None,
            kind: LocalEventKind::Created,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "scale-root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("Scale/unclassified_00001.dat")),
            path_before: None,
            kind: LocalEventKind::Modified,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "scale-root".to_owned(),
            native_key: None,
            path_after: None,
            path_before: Some(PathBuf::from("Scale/unclassified_00002.dat")),
            kind: LocalEventKind::Removed,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "scale-root".to_owned(),
            native_key: Some(vec![7; 8]),
            path_after: Some(PathBuf::from("Scale/renamed.dat")),
            path_before: Some(PathBuf::from("Scale/unclassified_00003.dat")),
            kind: LocalEventKind::Moved,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "scale-root".to_owned(),
            native_key: Some(vec![8; 8]),
            path_after: Some(PathBuf::from("Scale-renamed")),
            path_before: Some(PathBuf::from("Scale")),
            kind: LocalEventKind::Moved,
            scope: ChangeScope::Directory,
        },
    ]);
    let event_started = Instant::now();
    let persisted_events = service
        .record_monitoring_hints(workspace.id, root.id, &burst)
        .unwrap_or_else(|error| panic!("scale event burst should persist: {error}"));
    let queued = service
        .monitoring_dashboard(workspace.id)
        .unwrap_or_else(|error| panic!("scale dashboard should load: {error}"))
        .counts
        .pending_jobs;
    thread::sleep(Duration::from_millis(850));
    let cancelled = service
        .run_monitoring_cycle(workspace.id, &|| true)
        .unwrap_or_else(|error| panic!("scale cancellation should remain durable: {error}"));
    let rss_kib = current_rss_kib();
    assert_eq!(persisted_events, burst.len() as u64);
    assert!(
        queued <= 6,
        "event burst should coalesce to a bounded queue"
    );
    assert_eq!(cancelled.counts.pending_jobs, queued);
    assert_eq!(
        fs::read(source_directory.join("unclassified_00000.dat"))
            .unwrap_or_else(|error| panic!("monitored fixture should remain readable: {error}")),
        b"opaque local fixture 00000"
    );
    println!(
        "M10_1_MONITORING_SCALE catalog_files={} roots=2 raw_events={} queued_jobs={} event_and_cancel_ms={} rss_kib={}",
        FILE_COUNT + 1,
        persisted_events,
        queued,
        event_started.elapsed().as_millis(),
        rss_kib
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unavailable".to_owned())
    );
}

#[cfg(target_os = "macos")]
fn current_rss_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

#[cfg(not(target_os = "macos"))]
fn current_rss_kib() -> Option<u64> {
    None
}
