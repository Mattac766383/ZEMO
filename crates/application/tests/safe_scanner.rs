#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::ScannerApplicationService;
use persistence::{Database, DatabaseKey, InventorySort};
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
        Database::open_in_memory(&DatabaseKey::from_bytes([42; 32]))
            .unwrap_or_else(|error| panic!("encrypted test database should open: {error}")),
    );
    let service = ScannerApplicationService::new(database, native_platform());
    let workspace = service
        .create_workspace("Safe scanner test")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, root)
        .unwrap_or_else(|error| panic!("temporary root should register: {error}"));
    (service, workspace.id)
}

fn assert_inside(root: &Path, target: &Path) {
    assert!(
        target.starts_with(root) && target != root,
        "test mutation escaped temporary root: {}",
        target.display()
    );
}

fn create_dir(root: &Path, relative: &str) -> PathBuf {
    let target = root.join(relative);
    assert_inside(root, &target);
    fs::create_dir_all(&target)
        .unwrap_or_else(|error| panic!("temporary directory should be created: {error}"));
    target
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let target = root.join(relative);
    assert_inside(root, &target);
    if let Some(parent) = target.parent() {
        assert!(parent.starts_with(root));
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("temporary parent should be created: {error}"));
    }
    fs::write(&target, bytes)
        .unwrap_or_else(|error| panic!("temporary file should be written: {error}"));
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
fn empty_directory_is_a_valid_scan() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    let (service, workspace_id) = scanner_for(temporary.path());

    let scan = run_scan(&service, workspace_id);

    assert_eq!(scan.status, "completed");
    assert_eq!(scan.discovered_count, 0);
    assert_eq!(scan.indexed_count, 0);
    assert_eq!(scan.directory_count, 1);
    assert_eq!(scan.error_count, 0);
}

#[test]
fn nested_unicode_and_zero_byte_files_are_inventoried() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    create_dir(temporary.path(), "nested/empty");
    write_file(
        temporary.path(),
        "nested/客户-фактура-é.txt",
        "bonjour".as_bytes(),
    );
    write_file(temporary.path(), "zero.bin", &[]);
    let (service, workspace_id) = scanner_for(temporary.path());

    let scan = run_scan(&service, workspace_id);
    let files = service
        .scan_files(scan.id, InventorySort::RelativePath, false, 100, 0)
        .expect("inventory should load");

    assert_eq!(scan.discovered_count, 2);
    assert_eq!(scan.indexed_count, 2);
    assert!(scan.directory_count >= 3);
    assert!(
        files
            .iter()
            .any(|file| file.filename == "客户-фактура-é.txt")
    );
    assert!(files.iter().any(|file| file.byte_size == 0));
}

#[test]
fn thousand_file_inventory_is_batched_and_query_is_bounded() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    for index in 0..1_000_u16 {
        let bytes = vec![b'x'; usize::from(index) + 1];
        write_file(
            temporary.path(),
            &format!("bulk/file-{index:04}.dat"),
            &bytes,
        );
    }
    let (service, workspace_id) = scanner_for(temporary.path());

    let scan = run_scan(&service, workspace_id);
    let first_page = service
        .scan_files(scan.id, InventorySort::Filename, false, 10_000, 0)
        .expect("inventory page should load");

    assert_eq!(scan.discovered_count, 1_000);
    assert_eq!(scan.indexed_count, 1_000);
    assert_eq!(scan.hashed_count, 0);
    assert_eq!(first_page.len(), 1_000);
}

#[test]
fn exact_duplicates_require_digest_and_same_size() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    write_file(temporary.path(), "invoice.pdf", b"same");
    write_file(temporary.path(), "copy/invoice-copy.pdf", b"same");
    write_file(temporary.path(), "same-size-not-duplicate.pdf", b"nope");
    let (service, workspace_id) = scanner_for(temporary.path());

    let scan = run_scan(&service, workspace_id);
    let groups = service
        .scan_duplicate_groups(scan.id)
        .expect("duplicate groups should load");

    assert_eq!(scan.hashed_count, 3);
    assert_eq!(scan.duplicate_group_count, 1);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].files.len(), 2);
    assert_eq!(groups[0].byte_size, 4);
}

#[test]
fn large_duplicate_files_are_hashed_with_the_streaming_path() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    let mut bytes = vec![0_u8; 12 * 1024 * 1024];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::try_from(index % 251).unwrap_or(0);
    }
    write_file(temporary.path(), "large-a.bin", &bytes);
    write_file(temporary.path(), "nested/large-b.bin", &bytes);
    let expected = blake3::hash(&bytes).to_hex().to_string();
    drop(bytes);
    let (service, workspace_id) = scanner_for(temporary.path());

    let scan = run_scan(&service, workspace_id);
    let groups = service
        .scan_duplicate_groups(scan.id)
        .expect("large duplicate group should load");

    assert_eq!(scan.hashed_count, 2);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].digest_hex, expected);
    assert_eq!(groups[0].byte_size, 12 * 1024 * 1024);
}

#[test]
fn cancellation_persists_already_indexed_files() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    for index in 0..512_u16 {
        let bytes = vec![b'c'; usize::from(index) + 1];
        write_file(
            temporary.path(),
            &format!("cancel/file-{index:04}.dat"),
            &bytes,
        );
    }
    let (service, workspace_id) = scanner_for(temporary.path());
    let cancellation = AtomicBool::new(false);

    let scan = service
        .scan_workspace(
            workspace_id,
            &|| cancellation.load(Ordering::Relaxed),
            &mut |progress| {
                if progress.phase == catalog::ScanPhase::Inspecting && progress.files_indexed >= 128
                {
                    cancellation.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect("cancelled scan should persist safely");

    assert_eq!(scan.status, "cancelled");
    assert!(scan.indexed_count >= 128);
    assert!(scan.indexed_count < scan.discovered_count);
    assert!(scan.skipped_count > 0);
    let persisted = service
        .scan_files(scan.id, InventorySort::Filename, false, 1_000, 0)
        .expect("partial inventory should remain queryable");
    assert_eq!(
        persisted.len(),
        usize::try_from(scan.indexed_count).unwrap_or(usize::MAX)
    );
}

#[test]
fn scan_is_non_destructive_across_a_synthetic_tree() {
    let temporary = TempDir::new().expect("temporary directory should exist");
    let paths = [
        write_file(temporary.path(), "root.txt", b"root-data"),
        write_file(temporary.path(), "nested/one.bin", b"\x00\x01\x02\x03"),
        write_file(
            temporary.path(),
            "nested/deeper/évidence.json",
            br#"{"safe":true}"#,
        ),
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

    assert_eq!(scan.indexed_count, 3);
    for (path, expected_size, expected_digest) in before {
        assert!(
            path.exists(),
            "scanner must not move or delete {}",
            path.display()
        );
        let after = fs::read(&path).expect("fixture should remain readable");
        assert_eq!(
            fs::metadata(&path)
                .expect("fixture metadata should remain")
                .len(),
            expected_size
        );
        assert_eq!(blake3::hash(&after), expected_digest);
    }
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_recorded_without_aborting_when_permissions_are_enforced() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDir::new().expect("temporary directory should exist");
    let path = write_file(temporary.path(), "unreadable.txt", b"private");
    assert_inside(temporary.path(), &path);
    let original = fs::metadata(&path)
        .expect("fixture metadata should exist")
        .permissions();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0))
        .expect("fixture permissions should change");
    if native_platform().read_prefix(&path, 16).is_ok() {
        fs::set_permissions(&path, original).expect("fixture permissions should be restored");
        return;
    }
    let (service, workspace_id) = scanner_for(temporary.path());

    let result = service.scan_workspace(workspace_id, &|| false, &mut |_| {});
    fs::set_permissions(&path, original).expect("fixture permissions should be restored");
    let scan = result.expect("unreadable file must not abort the scan");
    let issues = service
        .scan_issues(scan.id)
        .expect("permission issue should load");

    assert_eq!(scan.indexed_count, 1);
    assert!(scan.error_count >= 1);
    assert!(
        issues
            .iter()
            .any(|issue| issue.category == "permission_denied")
    );
}

#[cfg(unix)]
#[test]
fn symbolic_link_loop_is_skipped_without_scope_escape() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().expect("temporary directory should exist");
    let nested = create_dir(temporary.path(), "nested");
    write_file(temporary.path(), "nested/inside.txt", b"inside");
    let link = nested.join("loop");
    assert_inside(temporary.path(), &link);
    symlink(temporary.path(), &link).expect("test symlink should be created");
    let (service, workspace_id) = scanner_for(temporary.path());

    let scan = run_scan(&service, workspace_id);
    let issues = service
        .scan_issues(scan.id)
        .expect("symlink issue should load");

    assert_eq!(scan.indexed_count, 1);
    assert!(scan.skipped_count >= 1);
    assert!(issues.iter().any(|issue| issue.category == "reparse_point"));
}
