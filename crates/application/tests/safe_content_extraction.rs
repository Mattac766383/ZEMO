#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::ScannerApplicationService;
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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

fn scanner_for(root: &Path) -> (ScannerApplicationService, domain::WorkspaceId) {
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([73; 32]))
            .unwrap_or_else(|error| panic!("encrypted test database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database, native_platform());
    let workspace = service
        .create_workspace("Content extraction test")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, root)
        .unwrap_or_else(|error| panic!("temporary root should register: {error}"));
    (service, workspace.id)
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let target = root.join(relative);
    assert!(target.starts_with(root) && target != root);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("fixture parent should be created: {error}"));
    }
    fs::write(&target, bytes).unwrap_or_else(|error| panic!("fixture should be written: {error}"));
    target
}

fn run_scan(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
) -> persistence::ScanRecord {
    service
        .scan_workspace(workspace_id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("safe scan should complete: {error}"))
}

#[test]
fn batch_persists_success_failure_unsupported_and_recovers_per_file() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    write_file(temporary.path(), "good.txt", b"Invoice 2026");
    write_file(temporary.path(), "invalid.txt", &[0xff, 0xfe, 0xfd]);
    write_file(temporary.path(), "unknown.bin", &[0, 1, 2, 3]);
    write_file(temporary.path(), "broken.pdf", b"%PDF-1.7\nbroken");
    let (service, workspace_id) = scanner_for(temporary.path());
    let scan = run_scan(&service, workspace_id);

    let batch = service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("one parser failure must not abort the batch");
    let results = service
        .content_analysis_results(&batch.id, 100, 0)
        .expect("persisted extraction results should load");

    assert_eq!(batch.status, "completed");
    assert_eq!(batch.files_queued, 4);
    assert_eq!(batch.files_completed, 4);
    assert_eq!(batch.successful_count, 1);
    assert_eq!(batch.unsupported_count, 1);
    assert_eq!(batch.failed_count, 2);
    assert_eq!(results.len(), 4);
    assert!(results.iter().any(|result| {
        result.filename == "good.txt"
            && result.status == "success"
            && result.text_preview == "Invoice 2026"
    }));
    assert!(results.iter().any(|result| {
        result.filename == "invalid.txt"
            && result.error_category.as_deref() == Some("invalid_encoding")
    }));
    assert!(
        results
            .iter()
            .any(|result| { result.filename == "unknown.bin" && result.status == "unsupported" })
    );
}

#[test]
fn extension_type_mismatch_is_persisted_without_parser_invocation() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    write_file(temporary.path(), "disguised.pdf", b"MZ\x90\x00");
    let (service, workspace_id) = scanner_for(temporary.path());
    let scan = run_scan(&service, workspace_id);

    let batch = service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("mismatch should be a per-file result");
    let result = service
        .content_analysis_results(&batch.id, 10, 0)
        .expect("mismatch result should persist")
        .into_iter()
        .next()
        .expect("one result should exist");

    assert_eq!(result.status, "failed");
    assert!(result.type_mismatch);
    assert_eq!(result.error_category.as_deref(), Some("type_mismatch"));
    assert!(result.extractor_type.is_none());
}

#[test]
fn cancellation_stops_scheduling_and_preserves_completed_results() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    for index in 0..200_u16 {
        write_file(
            temporary.path(),
            &format!("bulk/file-{index:03}.txt"),
            format!("local content {index}").as_bytes(),
        );
    }
    let (service, workspace_id) = scanner_for(temporary.path());
    let scan = run_scan(&service, workspace_id);
    let cancellation = AtomicBool::new(false);

    let batch = service
        .analyze_scan_content(
            scan.id,
            &|| cancellation.load(Ordering::Relaxed),
            &mut |progress| {
                if progress.files_completed >= 1 {
                    cancellation.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect("cancelled extraction should finish transactionally");

    assert_eq!(batch.status, "cancelled");
    assert_eq!(batch.files_completed, batch.files_queued);
    assert!(batch.successful_count >= 1);
    assert!(batch.successful_count < batch.files_queued);
    assert!(batch.skipped_count > 0);
}

#[test]
fn disappearing_source_fails_one_result_and_batch_continues() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    write_file(temporary.path(), "present.txt", b"still here");
    let disappearing = write_file(temporary.path(), "gone.txt", b"gone");
    let (service, workspace_id) = scanner_for(temporary.path());
    let scan = run_scan(&service, workspace_id);
    fs::remove_file(&disappearing).expect("fixture should disappear after scan");

    let batch = service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("missing source should not abort other files");
    let results = service
        .content_analysis_results(&batch.id, 10, 0)
        .expect("results should load");

    assert_eq!(batch.successful_count, 1);
    assert_eq!(batch.failed_count, 1);
    assert!(results.iter().any(|result| {
        result.filename == "gone.txt" && result.error_category.as_deref() == Some("source_changed")
    }));
}

#[test]
fn complete_pipeline_is_non_destructive_for_synthetic_fixture_tree() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    let paths = [
        write_file(temporary.path(), "notes/readme.md", b"# Local notes"),
        write_file(
            temporary.path(),
            "data/report.json",
            br#"{"customer":"ACME","amount":42}"#,
        ),
        write_file(temporary.path(), "binary/unknown.bin", &[0, 1, 2, 3, 4]),
    ];
    let before = paths
        .iter()
        .map(|path| {
            let bytes = fs::read(path).expect("fixture should be readable");
            (
                path.clone(),
                fs::metadata(path)
                    .expect("fixture metadata should exist")
                    .len(),
                blake3::hash(&bytes),
            )
        })
        .collect::<Vec<_>>();
    let (service, workspace_id) = scanner_for(temporary.path());
    let scan = run_scan(&service, workspace_id);

    let batch = service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("complete local extraction should succeed");
    assert_eq!(batch.files_completed, 3);

    for (path, expected_size, expected_hash) in before {
        assert!(path.exists(), "source path must remain present");
        assert_eq!(
            fs::metadata(&path)
                .expect("source metadata should remain readable")
                .len(),
            expected_size
        );
        let bytes = fs::read(&path).expect("source should remain readable");
        assert_eq!(blake3::hash(&bytes), expected_hash);
    }
}
