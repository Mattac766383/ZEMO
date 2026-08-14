#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::{IdentityResolutionPhase, ScannerApplicationService, SemanticCorrectionAction};
use knowledge::{
    IdentityOccurrence, IdentityResolutionPolicy, IdentityType, SignalKind, assess_match,
    generate_candidates,
};
use persistence::{Database, DatabaseKey, IdentityCandidateAction};
use platform::ReadOnlyPlatform;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
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
                .unwrap_or_else(|error| panic!("metadata should be readable: {error}"));
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("fixture bytes should be readable: {error}"));
                output.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|error| {
                            panic!("fixture path should remain scoped: {error}")
                        })
                        .to_path_buf(),
                    (metadata.len(), blake3::hash(&bytes)),
                );
            }
        }
    }
    output
}

fn file_id(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
    filename: &str,
) -> String {
    service
        .search_files(
            workspace_id,
            search::SearchQuery {
                text: filename.to_owned(),
                sort: search::SearchSort::Filename,
                page_size: 100,
                ..search::SearchQuery::default()
            },
        )
        .unwrap_or_else(|error| panic!("local search should succeed: {error}"))
        .results
        .into_iter()
        .find(|result| result.filename == filename)
        .unwrap_or_else(|| panic!("{filename} should be indexed"))
        .file_id
}

fn relationship_identity(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
    filename: &str,
    relationship_type: &str,
) -> String {
    let detail = service
        .file_detail(&file_id(service, workspace_id, filename))
        .unwrap_or_else(|error| panic!("file detail should load: {error}"));
    detail
        .relationships
        .into_iter()
        .find(|relationship| relationship.relationship_type == relationship_type)
        .unwrap_or_else(|| panic!("{filename} should have {relationship_type}"))
        .identity_id
}

#[test]
fn cross_file_resolution_is_reviewable_reversible_and_non_destructive() {
    let fixture = TempDir::new().expect("fixture directory should exist");
    let fixtures = [
        (
            "a-point-p-invoice.txt",
            "FACTURE\nSupplier: Point P\nSIRET: 732 829 320 00074\nEmail: billing@point-p.example\nTotal: 42,00 EUR",
        ),
        (
            "a-point-p-quote.txt",
            "DEVIS\nSupplier: POINT.P\nSIRET: 732 829 320 00074\nEmail: devis@point-p.example\nMontant: 75,00 EUR",
        ),
        (
            "b-martin-first.txt",
            "FACTURE\nSupplier: Martin SARL\nSIRET: 552 100 554 00013\nTotal: 10,00 EUR",
        ),
        (
            "b-martin-second.txt",
            "DEVIS\nSupplier: Martin SARL\nSIRET: 123 456 789 00007\nMontant: 11,00 EUR",
        ),
        (
            "c-dupont-construction.txt",
            "FACTURE\nSupplier: Dupont Construction\nTotal: 100,00 EUR",
        ),
        (
            "c-dupont-electricite.txt",
            "DEVIS\nSupplier: Dupont Électricité\nMontant: 101,00 EUR",
        ),
        (
            "d-contoso-invoice.txt",
            "FACTURE\nCustomer: Contoso SAS\nSIRET: 987 654 321 00007\nTotal: 200,00 EUR",
        ),
        (
            "d-contoso-contract.txt",
            "CONTRAT\nCustomer: CONTOSO S.A.S.\nSIRET: 987 654 321 00007\nDate de signature: 2026-01-01",
        ),
        (
            "e-jean-martin.txt",
            "LETTRE\nContact: Jean Martin\nDate: 2026-01-01",
        ),
        (
            "e-jean-pierre-martin.txt",
            "LETTRE\nContact: Jean-Pierre Martin\nDate: 2026-01-02",
        ),
        (
            "f-project-quote.txt",
            "DEVIS\nCustomer: Martin Client SAS\nProject: Martin Bordeaux\nProject reference: MARTIN-BDX-26\nAddress: 10 rue Exemple Bordeaux\nMontant: 500,00 EUR",
        ),
        (
            "f-project-contract.txt",
            "CONTRAT\nCustomer: Martin Client SAS\nProject: Projet Martin\nProject reference: MARTIN-BDX-26\nAddress: 10 rue Exemple Bordeaux\nDate de signature: 2026-02-01",
        ),
        (
            "g-project-lyon.txt",
            "DEVIS\nCustomer: Martin Client SAS\nProject: Martin Lyon\nProject reference: MARTIN-LYO-26\nAddress: 2 rue Exemple Lyon\nMontant: 300,00 EUR",
        ),
        (
            "g-project-nice.txt",
            "FACTURE\nCustomer: Martin Client SAS\nProject: Martin Nice\nProject reference: MARTIN-NCE-26\nAddress: 3 rue Exemple Nice\nTotal: 301,00 EUR",
        ),
        (
            "h-review-one.txt",
            "FACTURE\nSupplier: Review Co\nTotal: 20,00 EUR",
        ),
        (
            "h-review-two.txt",
            "DEVIS\nSupplier: REVIEW.CO\nMontant: 21,00 EUR",
        ),
        (
            "i-merge-one.txt",
            "FACTURE\nSupplier: Merge Foundation\nTotal: 30,00 EUR",
        ),
        (
            "i-merge-two.txt",
            "DEVIS\nSupplier: MERGE FOUNDATION\nMontant: 31,00 EUR",
        ),
        (
            "j-stale-one.txt",
            "FACTURE\nSupplier: Stale Candidate\nTotal: 40,00 EUR",
        ),
        (
            "j-stale-two.txt",
            "DEVIS\nSupplier: STALE CANDIDATE\nMontant: 41,00 EUR",
        ),
        (
            "k-adversarial.txt",
            "FACTURE\nSupplier: <script>alert(1)</script>'; DROP TABLE identity_candidates;--\nTotal: 9,00 EUR",
        ),
        (
            "l-correction-merge-one.txt",
            "FACTURE\nSupplier: Auto Correction\nSIRET: 732 829 320 00074\nTotal: 50,00 EUR",
        ),
        (
            "l-correction-merge-two.txt",
            "DEVIS\nSupplier: AUTO CORRECTION\nSIRET: 732 829 320 00074\nMontant: 51,00 EUR",
        ),
    ];
    for (name, content) in fixtures {
        write_file(fixture.path(), name, content);
    }
    let before = snapshot(fixture.path());
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([106; 32]))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database.clone(), native_platform());
    let workspace = service
        .create_workspace("Milestone 6 cross-file fixtures")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("fixture root should register: {error}"));
    let scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should succeed: {error}"));
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("content extraction should succeed: {error}"));
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantic and identity analysis should succeed: {error}"));

    let point_invoice = relationship_identity(
        &service,
        workspace.id,
        "a-point-p-invoice.txt",
        "file_supplier",
    );
    let point_quote = relationship_identity(
        &service,
        workspace.id,
        "a-point-p-quote.txt",
        "file_supplier",
    );
    assert_eq!(point_invoice, point_quote);
    let point = service
        .identity_detail(&point_invoice)
        .unwrap_or_else(|error| panic!("identity detail should load: {error}"));
    assert_eq!(point.identity.identity_type, "organization");
    assert_eq!(point.identity.occurrence_count, 2);
    assert!(point.identity.roles.iter().any(|role| role == "supplier"));
    assert!(point.identity.aliases.len() >= 2);
    assert!(point.audit_events.iter().any(|event| {
        event
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("same validated company identifier"))
    }));

    let martin_first = relationship_identity(
        &service,
        workspace.id,
        "b-martin-first.txt",
        "file_supplier",
    );
    let martin_second = relationship_identity(
        &service,
        workspace.id,
        "b-martin-second.txt",
        "file_supplier",
    );
    assert_ne!(martin_first, martin_second);

    let dupont_construction = relationship_identity(
        &service,
        workspace.id,
        "c-dupont-construction.txt",
        "file_supplier",
    );
    let dupont_electricite = relationship_identity(
        &service,
        workspace.id,
        "c-dupont-electricite.txt",
        "file_supplier",
    );
    assert_ne!(dupont_construction, dupont_electricite);

    let contoso_invoice = relationship_identity(
        &service,
        workspace.id,
        "d-contoso-invoice.txt",
        "file_customer",
    );
    let contoso_contract = relationship_identity(
        &service,
        workspace.id,
        "d-contoso-contract.txt",
        "file_customer",
    );
    assert_eq!(contoso_invoice, contoso_contract);

    let project_quote = relationship_identity(
        &service,
        workspace.id,
        "f-project-quote.txt",
        "file_project",
    );
    let project_contract = relationship_identity(
        &service,
        workspace.id,
        "f-project-contract.txt",
        "file_project",
    );
    assert_eq!(project_quote, project_contract);
    let project = service
        .identity_detail(&project_quote)
        .unwrap_or_else(|error| panic!("project detail should load: {error}"));
    assert!(
        project
            .relationships
            .iter()
            .any(|relationship| relationship.relationship_type == "project_customer")
    );

    let lyon = relationship_identity(&service, workspace.id, "g-project-lyon.txt", "file_project");
    let nice = relationship_identity(&service, workspace.id, "g-project-nice.txt", "file_project");
    assert_ne!(lyon, nice);

    let open_review = service
        .identity_review_groups(workspace.id, "needs_review", 50, 0)
        .unwrap_or_else(|error| panic!("identity review groups should load: {error}"));
    assert!(open_review.items.iter().any(|item| {
        item.review_reason == "conflicting_identity_evidence"
            && item.title.to_lowercase().contains("martin")
    }));
    let review_candidate = open_review
        .items
        .iter()
        .flat_map(|group| group.candidates.iter())
        .find(|candidate| {
            candidate
                .left
                .display_name
                .to_lowercase()
                .contains("review")
                && candidate
                    .right
                    .display_name
                    .to_lowercase()
                    .contains("review")
        })
        .unwrap_or_else(|| panic!("name-only supplier match should require review"))
        .candidate_id
        .clone();
    let rejected_pair = open_review
        .items
        .iter()
        .flat_map(|group| group.candidates.iter())
        .find(|candidate| candidate.candidate_id == review_candidate)
        .map(|candidate| {
            (
                candidate.left.identity_id.clone(),
                candidate.right.identity_id.clone(),
            )
        })
        .unwrap_or_else(|| panic!("rejected candidate identities should remain available"));
    let stale_candidate = open_review
        .items
        .iter()
        .flat_map(|group| group.candidates.iter())
        .find(|candidate| {
            candidate
                .left
                .display_name
                .to_lowercase()
                .contains("stale candidate")
                && candidate
                    .right
                    .display_name
                    .to_lowercase()
                    .contains("stale candidate")
        })
        .unwrap_or_else(|| panic!("stale-candidate fixture should require review"))
        .candidate_id
        .clone();
    service
        .decide_identity_candidate(
            &review_candidate,
            IdentityCandidateAction::Reject,
            Some("synthetic false-positive rejection"),
        )
        .unwrap_or_else(|error| panic!("candidate rejection should persist: {error}"));

    let merge_candidate_record = open_review
        .items
        .iter()
        .flat_map(|group| group.candidates.iter())
        .find(|candidate| {
            candidate
                .left
                .display_name
                .to_lowercase()
                .contains("merge foundation")
                && candidate
                    .right
                    .display_name
                    .to_lowercase()
                    .contains("merge foundation")
        })
        .unwrap_or_else(|| panic!("manual merge fixture should require review"));
    let merge_pair = (
        merge_candidate_record.left.identity_id.clone(),
        merge_candidate_record.right.identity_id.clone(),
    );
    let merge_candidate = merge_candidate_record.candidate_id.clone();
    service
        .decide_identity_candidate(
            &merge_candidate,
            IdentityCandidateAction::Confirm,
            Some("synthetic user confirmation"),
        )
        .unwrap_or_else(|error| panic!("candidate confirmation should merge semantics: {error}"));

    service
        .store_semantic_correction(
            &file_id(&service, workspace.id, "j-stale-two.txt"),
            "supplier_candidate",
            SemanticCorrectionAction::Correct,
            Some("Corrected Supplier"),
        )
        .unwrap_or_else(|error| {
            panic!("semantic correction should trigger re-resolution: {error}")
        });
    let after_correction = service
        .identity_review_groups(workspace.id, "needs_review", 50, 0)
        .unwrap_or_else(|error| panic!("identity review should refresh after correction: {error}"));
    assert!(
        !after_correction
            .items
            .iter()
            .flat_map(|group| group.candidates.iter())
            .any(|candidate| candidate.candidate_id == stale_candidate)
    );
    let corrected_identity =
        relationship_identity(&service, workspace.id, "j-stale-two.txt", "file_supplier");
    assert_eq!(
        service
            .identity_detail(&corrected_identity)
            .unwrap_or_else(|error| panic!("corrected identity should load: {error}"))
            .identity
            .display_name,
        "Corrected Supplier"
    );
    let adversarial_identity =
        relationship_identity(&service, workspace.id, "k-adversarial.txt", "file_supplier");
    let adversarial_detail = service
        .identity_detail(&adversarial_identity)
        .unwrap_or_else(|error| panic!("adversarial identity should remain inert data: {error}"));
    assert!(!adversarial_detail.identity.display_name.is_empty());
    assert!(adversarial_detail.identity.display_name.chars().count() <= 512);
    let correction_merge_before = relationship_identity(
        &service,
        workspace.id,
        "l-correction-merge-one.txt",
        "file_supplier",
    );
    assert_eq!(
        correction_merge_before,
        relationship_identity(
            &service,
            workspace.id,
            "l-correction-merge-two.txt",
            "file_supplier"
        )
    );
    let correction_merge_detail_before = service
        .identity_detail(&correction_merge_before)
        .unwrap_or_else(|error| panic!("automatic identity should load: {error}"));
    assert!(!correction_merge_detail_before.identity.user_locked);
    assert_eq!(correction_merge_detail_before.identity.occurrence_count, 2);
    service
        .store_semantic_correction(
            &file_id(&service, workspace.id, "l-correction-merge-two.txt"),
            "supplier_candidate",
            SemanticCorrectionAction::Correct,
            Some("Different Corrected Supplier"),
        )
        .unwrap_or_else(|error| panic!("correction should detach a prior automatic link: {error}"));
    let correction_merge_after = relationship_identity(
        &service,
        workspace.id,
        "l-correction-merge-two.txt",
        "file_supplier",
    );
    let correction_merge_detail_after = service
        .identity_detail(&correction_merge_after)
        .unwrap_or_else(|error| panic!("detached identity should load: {error}"));
    assert_ne!(
        correction_merge_before, correction_merge_after,
        "corrected identity state: {correction_merge_detail_after:?}"
    );
    assert!(
        correction_merge_detail_after
            .audit_events
            .iter()
            .any(|event| event.event_type == "identity_split")
    );
    let merged_identity =
        relationship_identity(&service, workspace.id, "i-merge-one.txt", "file_supplier");
    assert_eq!(
        merged_identity,
        relationship_identity(&service, workspace.id, "i-merge-two.txt", "file_supplier")
    );
    let merged_secondary = if merge_pair.0 == merged_identity {
        &merge_pair.1
    } else {
        &merge_pair.0
    };
    assert_eq!(
        service
            .identity_detail(merged_secondary)
            .unwrap_or_else(|error| panic!("merged identity ID should canonicalize: {error}"))
            .identity
            .identity_id,
        merged_identity
    );

    let mut phases = Vec::new();
    let database_resolution_started = Instant::now();
    let database_resolution = service
        .resolve_workspace_identities(workspace.id, "manual", true, &|| false, &mut |progress| {
            phases.push(progress.phase)
        })
        .unwrap_or_else(|error| panic!("forced re-resolution should succeed: {error}"));
    let database_resolution_elapsed = database_resolution_started.elapsed();
    println!(
        "M6_DB_RESOLUTION files={} occurrences={} comparisons={} candidates={} auto_links={} elapsed_ms={}",
        database_resolution.files_considered,
        database_resolution.occurrences_processed,
        database_resolution.comparisons,
        database_resolution.candidates_created,
        database_resolution.auto_links_created,
        database_resolution_elapsed.as_millis(),
    );
    assert_eq!(phases.first(), Some(&IdentityResolutionPhase::Running));
    assert_eq!(phases.last(), Some(&IdentityResolutionPhase::Completed));
    assert_eq!(
        merged_identity,
        relationship_identity(&service, workspace.id, "i-merge-two.txt", "file_supplier")
    );
    let after_rejection = service
        .identity_review_groups(workspace.id, "needs_review", 50, 0)
        .unwrap_or_else(|error| panic!("identity review should reload: {error}"));
    assert!(
        !after_rejection
            .items
            .iter()
            .flat_map(|group| group.candidates.iter())
            .any(|candidate| candidate.candidate_id == review_candidate)
    );

    let merged_detail = service
        .identity_detail(&merged_identity)
        .unwrap_or_else(|error| panic!("merged identity should load: {error}"));
    let occurrence_to_unlink = merged_detail
        .occurrences
        .iter()
        .find(|occurrence| occurrence.filename == "i-merge-two.txt")
        .unwrap_or_else(|| panic!("merged occurrence should remain traceable"))
        .occurrence_id
        .clone();
    service
        .unlink_identity_occurrence(
            &merged_identity,
            &occurrence_to_unlink,
            Some("synthetic split correction"),
        )
        .unwrap_or_else(|error| panic!("occurrence unlink should be reversible: {error}"));
    let original_after_unlink = service
        .identity_detail(&merged_identity)
        .unwrap_or_else(|error| panic!("original identity should remain inspectable: {error}"));
    assert!(
        !original_after_unlink
            .identity
            .aliases
            .iter()
            .any(|alias| alias == "MERGE FOUNDATION")
    );
    let split_left =
        relationship_identity(&service, workspace.id, "i-merge-one.txt", "file_supplier");
    let split_right =
        relationship_identity(&service, workspace.id, "i-merge-two.txt", "file_supplier");
    assert_ne!(split_left, split_right);
    service
        .resolve_workspace_identities(workspace.id, "manual", true, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("post-split resolution should succeed: {error}"));
    assert_ne!(
        relationship_identity(&service, workspace.id, "i-merge-one.txt", "file_supplier"),
        relationship_identity(&service, workspace.id, "i-merge-two.txt", "file_supplier")
    );

    service
        .merge_identity_records(
            &rejected_pair.0,
            &rejected_pair.1,
            Some("explicitly reverse the prior keep-separate decision"),
        )
        .unwrap_or_else(|error| {
            panic!("explicit manual merge should supersede rejection: {error}")
        });
    assert_eq!(
        relationship_identity(&service, workspace.id, "h-review-one.txt", "file_supplier"),
        relationship_identity(&service, workspace.id, "h-review-two.txt", "file_supplier")
    );

    let cancelled = service
        .resolve_workspace_identities(workspace.id, "manual", true, &|| true, &mut |_| {})
        .unwrap_or_else(|error| panic!("cancelled resolution should stay consistent: {error}"));
    assert_eq!(cancelled.status, "cancelled");
    let single_flight = database
        .begin_identity_resolver_run(workspace.id, "manual")
        .unwrap_or_else(|error| panic!("single-flight resolver run should start: {error}"));
    assert!(
        database
            .begin_identity_resolver_run(workspace.id, "manual")
            .is_err()
    );
    database
        .finish_identity_resolver_run(&single_flight.run_id, "cancelled", 0, 0, 0, 0, 0, 0, None)
        .unwrap_or_else(|error| panic!("single-flight resolver run should close: {error}"));
    assert_eq!(
        database
            .foreign_key_violation_count()
            .unwrap_or_else(|error| panic!("foreign key check should succeed: {error}")),
        0
    );
    assert!(service.system_status().network_disabled);
    assert_eq!(snapshot(fixture.path()), before);
}

#[test]
fn ten_thousand_occurrence_blocking_stays_bounded() {
    let fixture_started = Instant::now();
    let occurrences = (0..10_000)
        .map(|index| {
            let (name, domain) = if index < 1_000 {
                let pair = index / 2;
                (
                    format!("Paired Supplier {pair}"),
                    format!("paired-{pair}.example"),
                )
            } else {
                (
                    format!("Synthetic Supplier {index}"),
                    format!("supplier-{index}.example"),
                )
            };
            IdentityOccurrence::new(
                &format!("occurrence-{index}"),
                &format!("file-{index}"),
                Some(format!("entity-{index}")),
                None,
                IdentityType::Organization,
                None,
                &name,
                0.9,
                "5.0.0",
                [(SignalKind::Domain, domain)],
            )
            .unwrap_or_else(|error| panic!("scale occurrence should be valid: {error}"))
        })
        .collect::<Vec<_>>();
    let fixture_elapsed = fixture_started.elapsed();
    let generation_started = Instant::now();
    let generation = generate_candidates(&occurrences, IdentityResolutionPolicy::default());
    let generation_elapsed = generation_started.elapsed();
    let resolution_started = Instant::now();
    let resolved_candidates = generation
        .candidates
        .iter()
        .filter(|candidate| {
            assess_match(
                &occurrences[candidate.left_index],
                &occurrences[candidate.right_index],
                IdentityResolutionPolicy::default(),
            )
            .decision
                != knowledge::ResolutionDecision::Unknown
        })
        .count();
    let resolution_elapsed = resolution_started.elapsed();
    assert_eq!(generation.stats.occurrences, 10_000);
    assert_eq!(generation.stats.candidates, 500);
    assert_eq!(resolved_candidates, 500);
    assert!(generation.stats.comparisons < 100_000);
    assert!(generation.stats.comparisons < (10_000_usize * 9_999 / 2) / 100);
    println!(
        "M6_SCALE occurrences={} candidates={} comparisons={} blocking_memberships={} fixture_ms={} candidate_generation_ms={} resolution_ms={}",
        generation.stats.occurrences,
        generation.stats.candidates,
        generation.stats.comparisons,
        generation.stats.blocking_memberships,
        fixture_elapsed.as_millis(),
        generation_elapsed.as_millis(),
        resolution_elapsed.as_millis(),
    );
}
