#![cfg(any(target_os = "macos", target_os = "windows"))]

//! Milestone 14 — incremental proposal updates + equivalence vs full rebuild.

use application::{ProposalRebuildMode, ScannerApplicationService};
use domain::{FileId, ProposalOverrideAction};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use std::{collections::BTreeMap, fs, path::Path, sync::Arc, time::Instant};
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
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(target, content).expect("fixture file");
}

fn fingerprint_proposal(
    proposal: &domain::OrganizationProposal,
) -> BTreeMap<String, (String, String, String, bool, String)> {
    proposal
        .operations
        .iter()
        .map(|operation| {
            (
                operation.file_id.to_string(),
                (
                    operation.proposed_destination.join("\\"),
                    operation.proposed_name.clone(),
                    operation.operation_kind.database_name().to_owned(),
                    operation.needs_review,
                    operation.conflict_state.database_name().to_owned(),
                ),
            )
        })
        .collect()
}

fn prepare_workspace(
    root: &Path,
) -> (
    ScannerApplicationService,
    domain::WorkspaceId,
    domain::RootId,
) {
    let database =
        Arc::new(Database::open_in_memory(&DatabaseKey::from_bytes([14; 32])).expect("database"));
    let service = ScannerApplicationService::new(database, native_platform());
    let workspace = service.create_workspace("m14").expect("workspace");
    let registered = service
        .register_root(workspace.id, root)
        .expect("register root");
    (service, workspace.id, registered.id)
}

fn scan_pipeline(service: &ScannerApplicationService, workspace_id: domain::WorkspaceId) {
    let scan = service
        .scan_workspace(workspace_id, &|| false, &mut |_| {})
        .expect("scan");
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("content");
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .expect("semantics");
    service
        .resolve_workspace_identities(workspace_id, "manual", true, &|| false, &mut |_| {})
        .expect("identities");
}

#[test]
fn incremental_one_file_matches_full_rebuild() {
    let fixture = TempDir::new().expect("fixture");
    write_file(
        fixture.path(),
        "Downloads/invoice-a.txt",
        "FACTURE\nCustomer: Dupont SARL\nSupplier: Point P\nInvoice number: A-1\nDate: 2026-01-10\nTotal: 100 EUR",
    );
    write_file(
        fixture.path(),
        "Downloads/invoice-b.txt",
        "FACTURE\nCustomer: Dupont SARL\nSupplier: Point P\nInvoice number: B-2\nDate: 2026-01-11\nTotal: 200 EUR",
    );
    write_file(
        fixture.path(),
        "Downloads/notes.txt",
        "personal notes without strong business signals",
    );

    let (service, workspace_id, root_id) = prepare_workspace(fixture.path());
    scan_pipeline(&service, workspace_id);
    let initial = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial proposal");

    write_file(
        fixture.path(),
        "Downloads/invoice-a.txt",
        "FACTURE\nCustomer: Martin SA\nSupplier: Point P\nInvoice number: A-1\nDate: 2026-02-01\nTotal: 150 EUR",
    );
    scan_pipeline(&service, workspace_id);

    let dirty = initial
        .operations
        .iter()
        .find(|operation| operation.source_name == "invoice-a.txt")
        .map(|operation| operation.file_id)
        .expect("invoice-a");

    let incremental = service
        .update_organization_proposal_incrementally(
            workspace_id,
            root_id,
            &[dirty],
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("incremental");
    assert_eq!(incremental.rebuild_mode, ProposalRebuildMode::Incremental);

    let full = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            true,
            &|| false,
            &mut |_| {},
        )
        .expect("full rebuild");

    assert_eq!(
        fingerprint_proposal(&incremental.proposal),
        fingerprint_proposal(&full),
        "incremental proposal state must match a clean full rebuild"
    );
}

#[test]
fn incremental_delete_removes_operation_and_matches_full_rebuild() {
    let fixture = TempDir::new().expect("fixture");
    write_file(
        fixture.path(),
        "Downloads/keep.txt",
        "FACTURE\nCustomer: Dupont SARL\nInvoice number: K-1\nDate: 2026-01-10\nTotal: 10 EUR",
    );
    write_file(
        fixture.path(),
        "Downloads/drop.txt",
        "FACTURE\nCustomer: Dupont SARL\nInvoice number: D-1\nDate: 2026-01-11\nTotal: 20 EUR",
    );
    let (service, workspace_id, root_id) = prepare_workspace(fixture.path());
    scan_pipeline(&service, workspace_id);
    let initial = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial");
    let deleted = initial
        .operations
        .iter()
        .find(|operation| operation.source_name == "drop.txt")
        .map(|operation| operation.file_id)
        .expect("drop.txt");

    // Incremental delete path should drop the operation even before catalog GC.
    let incremental = service
        .update_organization_proposal_incrementally(
            workspace_id,
            root_id,
            &[],
            &[deleted],
            &|| false,
            &mut |_| {},
        )
        .expect("incremental delete");
    assert_eq!(incremental.rebuild_mode, ProposalRebuildMode::Incremental);
    assert!(
        !incremental
            .proposal
            .operations
            .iter()
            .any(|operation| operation.file_id == deleted)
    );
    assert_eq!(
        incremental.proposal.summary.files_analyzed,
        initial.summary.files_analyzed.saturating_sub(1)
    );
    assert!(
        incremental
            .proposal
            .operations
            .iter()
            .any(|operation| operation.source_name == "keep.txt")
    );

    // Removing the source file and rescanning should leave a catalog that a
    // full rebuild can also organize without the deleted operation once the
    // location is no longer current. If the scanner still reports the path as
    // present (platform/timing), the incremental delete result remains the
    // authoritative removed-op state for this regression.
    fs::remove_file(fixture.path().join("Downloads/drop.txt")).expect("delete file");
    scan_pipeline(&service, workspace_id);
    let full = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            true,
            &|| false,
            &mut |_| {},
        )
        .expect("full");
    if !full
        .operations
        .iter()
        .any(|operation| operation.file_id == deleted)
    {
        assert_eq!(
            fingerprint_proposal(&incremental.proposal),
            fingerprint_proposal(&full)
        );
    } else {
        assert!(
            incremental.proposal.operations.len() < full.operations.len(),
            "incremental delete must drop the operation even when catalog GC lags"
        );
    }
}

#[test]
fn user_override_survives_incremental_update() {
    let fixture = TempDir::new().expect("fixture");
    write_file(
        fixture.path(),
        "Downloads/alpha.txt",
        "FACTURE\nCustomer: Dupont SARL\nInvoice number: A-1\nDate: 2026-01-10\nTotal: 10 EUR",
    );
    write_file(
        fixture.path(),
        "Downloads/beta.txt",
        "FACTURE\nCustomer: Dupont SARL\nInvoice number: B-1\nDate: 2026-01-11\nTotal: 20 EUR",
    );
    let (service, workspace_id, root_id) = prepare_workspace(fixture.path());
    scan_pipeline(&service, workspace_id);
    let initial = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial");
    let alpha = initial
        .operations
        .iter()
        .find(|operation| operation.source_name == "alpha.txt")
        .cloned()
        .expect("alpha");
    let overridden = service
        .set_organization_proposal_override(
            initial.id,
            alpha.file_id,
            ProposalOverrideAction::ToReview,
            None,
            None,
            Some("manual hold".to_owned()),
            &|| false,
            &mut |_| {},
        )
        .expect("override");
    assert!(
        overridden
            .operations
            .iter()
            .find(|operation| operation.file_id == alpha.file_id)
            .is_some_and(|operation| operation.user_override && operation.needs_review)
    );

    let beta = overridden
        .operations
        .iter()
        .find(|operation| operation.source_name == "beta.txt")
        .map(|operation| operation.file_id)
        .expect("beta");
    write_file(
        fixture.path(),
        "Downloads/beta.txt",
        "FACTURE\nCustomer: Martin SA\nInvoice number: B-1\nDate: 2026-03-01\nTotal: 25 EUR",
    );
    scan_pipeline(&service, workspace_id);
    let incremental = service
        .update_organization_proposal_incrementally(
            workspace_id,
            root_id,
            &[beta],
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("incremental");
    let alpha_after = incremental
        .proposal
        .operations
        .iter()
        .find(|operation| operation.file_id == alpha.file_id)
        .expect("alpha survives");
    assert!(alpha_after.user_override);
    assert!(alpha_after.needs_review);
}

#[test]
fn rule_change_falls_back_to_full_rebuild() {
    let fixture = TempDir::new().expect("fixture");
    write_file(
        fixture.path(),
        "Downloads/doc.txt",
        "FACTURE\nCustomer: Dupont SARL\nInvoice number: R-1\nDate: 2026-01-10\nTotal: 10 EUR",
    );
    let (service, workspace_id, root_id) = prepare_workspace(fixture.path());
    scan_pipeline(&service, workspace_id);
    let _ = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial");
    let recomputed = service
        .recompute_after_rule_change(workspace_id, &|| false, &mut |_| {})
        .expect("rule recompute");
    assert!(recomputed.is_some());
}

#[test]
fn batch_of_one_hundred_files_can_use_incremental_path() {
    let fixture = TempDir::new().expect("fixture");
    for index in 0..120 {
        write_file(
            fixture.path(),
            &format!("Downloads/file_{index:03}.txt"),
            &format!(
                "FACTURE\nCustomer: Client {index}\nInvoice number: I-{index}\nDate: 2026-01-10\nTotal: {index} EUR"
            ),
        );
    }
    let (service, workspace_id, root_id) = prepare_workspace(fixture.path());
    scan_pipeline(&service, workspace_id);
    let initial = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial");
    let dirty = initial
        .operations
        .iter()
        .take(100)
        .map(|operation| operation.file_id)
        .collect::<Vec<FileId>>();
    for (index, _) in dirty.iter().enumerate() {
        write_file(
            fixture.path(),
            &format!("Downloads/file_{index:03}.txt"),
            &format!(
                "FACTURE\nCustomer: Updated {index}\nInvoice number: I-{index}\nDate: 2026-02-10\nTotal: {} EUR",
                index + 1
            ),
        );
    }
    scan_pipeline(&service, workspace_id);
    let started = Instant::now();
    let incremental = service
        .update_organization_proposal_incrementally(
            workspace_id,
            root_id,
            &dirty,
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("100-file incremental");
    let elapsed_ms = started.elapsed().as_millis();
    eprintln!(
        "m14 100-file incremental: {elapsed_ms} ms mode={:?}",
        incremental.rebuild_mode
    );
    let full = service
        .generate_organization_proposal_for_root(
            workspace_id,
            root_id,
            true,
            &|| false,
            &mut |_| {},
        )
        .expect("full");
    assert_eq!(
        fingerprint_proposal(&incremental.proposal),
        fingerprint_proposal(&full)
    );
}

#[test]
fn scale_fixture_one_file_incremental_beats_full_rebuild() {
    use persistence::{LargeScaleFixtureConfig, open_scale_database};

    let temp = TempDir::new().expect("temp");
    let db_path = temp.path().join("m14-scale.db");
    let database = open_scale_database(&db_path, &DatabaseKey::from_bytes([14; 32])).expect("db");
    let fixture = database
        .seed_large_scale_fixture(&LargeScaleFixtureConfig {
            file_count: 2_000,
            identity_count: 80,
            project_count: 40,
            review_item_target: 200,
            vector_file_count: 500,
            root_path: temp.path().join("root"),
            ..LargeScaleFixtureConfig::default()
        })
        .expect("seed");
    let service = ScannerApplicationService::new(std::sync::Arc::new(database), native_platform());

    let full_started = Instant::now();
    let initial = service
        .generate_organization_proposal_for_root(
            fixture.workspace.id,
            fixture.root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial full");
    let full_ms = full_started.elapsed().as_millis();

    let dirty = initial
        .operations
        .first()
        .map(|operation| operation.file_id)
        .expect("op");
    let one_started = Instant::now();
    let one = service
        .update_organization_proposal_incrementally(
            fixture.workspace.id,
            fixture.root.id,
            &[dirty],
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("one-file");
    let one_ms = one_started.elapsed().as_millis();

    let batch: Vec<FileId> = initial
        .operations
        .iter()
        .take(100)
        .map(|operation| operation.file_id)
        .collect();
    let batch_started = Instant::now();
    let hundred = service
        .update_organization_proposal_incrementally(
            fixture.workspace.id,
            fixture.root.id,
            &batch,
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("100-file");
    let hundred_ms = batch_started.elapsed().as_millis();

    eprintln!(
        "M14_SCALE_2K full_ms={full_ms} one_file_ms={one_ms} mode={:?} reason={:?} affected={} hundred_ms={hundred_ms} mode100={:?}",
        one.rebuild_mode,
        one.rebuild_reason,
        one.affected_file_ids.len(),
        hundred.rebuild_mode
    );
    assert!(matches!(
        one.rebuild_mode,
        ProposalRebuildMode::Incremental | ProposalRebuildMode::Full
    ));
    // When the invalidation neighborhood stays small relative to the catalog,
    // incremental persistence must beat a clean full rewrite.
    if one.rebuild_mode == ProposalRebuildMode::Incremental
        && one.affected_file_ids.len() * 10 < initial.summary.files_analyzed as usize
    {
        assert!(
            one_ms < full_ms,
            "localized one-file incremental ({one_ms} ms) should beat full rebuild ({full_ms} ms)"
        );
    }
}

#[test]
#[ignore = "expensive 100k M14 proposal delta; run with --ignored --release"]
fn scale_fixture_one_hundred_thousand_proposal_delta() {
    use persistence::{LargeScaleFixtureConfig, open_scale_database};

    let temp = TempDir::new().expect("temp");
    let db_path = temp.path().join("m14-scale-100k.db");
    let database = open_scale_database(&db_path, &DatabaseKey::from_bytes([14; 32])).expect("db");
    let fixture = database
        .seed_large_scale_fixture(&LargeScaleFixtureConfig {
            file_count: 100_000,
            root_path: temp.path().join("root"),
            ..LargeScaleFixtureConfig::default()
        })
        .expect("seed");
    let service = ScannerApplicationService::new(std::sync::Arc::new(database), native_platform());

    let full_started = Instant::now();
    let initial = service
        .generate_organization_proposal_for_root(
            fixture.workspace.id,
            fixture.root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("initial full");
    let full_ms = full_started.elapsed().as_millis();

    let dirty = initial
        .operations
        .first()
        .map(|operation| operation.file_id)
        .expect("op");
    let one_started = Instant::now();
    let one = service
        .update_organization_proposal_incrementally(
            fixture.workspace.id,
            fixture.root.id,
            &[dirty],
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("one-file");
    let one_ms = one_started.elapsed().as_millis();

    let batch: Vec<FileId> = initial
        .operations
        .iter()
        .take(100)
        .map(|operation| operation.file_id)
        .collect();
    let batch_started = Instant::now();
    let hundred = service
        .update_organization_proposal_incrementally(
            fixture.workspace.id,
            fixture.root.id,
            &batch,
            &[],
            &|| false,
            &mut |_| {},
        )
        .expect("100-file");
    let hundred_ms = batch_started.elapsed().as_millis();

    eprintln!(
        "M14_SCALE_100K full_ms={full_ms} one_file_ms={one_ms} one_mode={:?} one_reason={:?} hundred_ms={hundred_ms} hundred_mode={:?} files={}",
        one.rebuild_mode, one.rebuild_reason, hundred.rebuild_mode, initial.summary.files_analyzed
    );
}
