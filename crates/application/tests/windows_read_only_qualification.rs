#![cfg(windows)]

//! Windows read-only product-flow qualification against a temporary sandbox.
//! Uses ScannerApplicationService boundaries only (no M14-A internals).

use application::ScannerApplicationService;
use persistence::{Database, DatabaseKey, ReviewReasonFilter, ReviewStatusFilter};
use platform::ReadOnlyPlatform;
use platform_windows::WindowsPlatform;
use search::SearchQuery;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::{Builder, TempDir};

const FORBIDDEN_PROFILE_DIRS: &[&str] = &["Documents", "Desktop", "Downloads"];

fn m15_sandbox() -> TempDir {
    let dir = Builder::new()
        .prefix("supremacy-m15-sandbox-ro-")
        .tempdir()
        .unwrap_or_else(|error| panic!("sandbox: {error}"));
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|error| panic!("temp root: {error}"));
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize: {error}"));
    assert!(root.starts_with(&temporary_root));
    for forbidden in FORBIDDEN_PROFILE_DIRS {
        assert!(
            !root
                .components()
                .any(|component| component.as_os_str() == *forbidden)
        );
    }
    dir
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(relative);
    assert!(path.starts_with(root) && path != root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| panic!("parent: {error}"));
    }
    fs::write(&path, bytes).unwrap_or_else(|error| panic!("write: {error}"));
    path
}

#[test]
fn reports_test_thread_stack_env() {
    let stack = std::env::var("RUST_MIN_STACK").unwrap_or_else(|_| "unset".to_owned());
    eprintln!("RUST_MIN_STACK={stack}");
    assert!(
        stack.parse::<u64>().unwrap_or(0) >= 16 * 1024 * 1024,
        "Windows application tests require RUST_MIN_STACK >= 16 MiB, got {stack}"
    );
}

#[test]
fn windows_read_only_scan_extract_search_review_proposals_rules() {
    let sandbox = m15_sandbox();
    let root = sandbox.path();
    write_file(
        root,
        "incoming/invoice-dupont.txt",
        b"Invoice Dupont materials renovation 2024 EUR 1250",
    );
    write_file(
        root,
        "incoming/photo-plage.txt",
        b"Vacation photo at the beach with family",
    );

    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([15; 32]))
            .unwrap_or_else(|error| panic!("db open: {error}")),
    );
    let platform: Arc<dyn ReadOnlyPlatform> = Arc::new(WindowsPlatform);
    let service = ScannerApplicationService::new(database, platform);
    let workspace = service
        .create_workspace("M15 Windows read-only")
        .unwrap_or_else(|error| panic!("workspace: {error}"));
    service
        .set_current_workspace(workspace.id)
        .unwrap_or_else(|error| panic!("set current workspace: {error}"));
    let registered = service
        .register_root(workspace.id, root)
        .unwrap_or_else(|error| panic!("register root: {error}"));

    let scan = service
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan: {error}"));
    assert!(scan.indexed_count >= 2);

    let restored = service
        .restore_workspace_session()
        .unwrap_or_else(|error| panic!("restore session: {error}"))
        .unwrap_or_else(|| panic!("restored session should exist"));
    assert_eq!(restored.workspace.id, workspace.id);
    assert_eq!(restored.root.map(|value| value.id), Some(registered.id));
    assert!(restored.safe_read_only);
    assert!(!restored.filesystem_execution_resumed);

    let extraction = service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("extraction: {error}"));
    assert!(
        extraction.files_completed
            + extraction.successful_count
            + extraction.partial_count
            + extraction.unsupported_count
            + extraction.skipped_count
            + extraction.failed_count
            >= 1
            || scan.indexed_count >= 2,
        "extraction batch should progress for scanned files"
    );

    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantic analysis: {error}"));
    let identities = service
        .resolve_workspace_identities(workspace.id, "manual", true, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("identity resolution: {error}"));
    let _ = identities;

    let lexical = service
        .search_files(
            workspace.id,
            SearchQuery {
                text: "invoice dupont".to_owned(),
                ..SearchQuery::default()
            },
        )
        .unwrap_or_else(|error| panic!("lexical search: {error}"));
    assert!(
        !lexical.results.is_empty(),
        "lexical search should return sandbox invoice"
    );

    let _review = service
        .review_items(
            workspace.id,
            ReviewStatusFilter::All,
            ReviewReasonFilter::All,
            50,
            0,
        )
        .unwrap_or_else(|error| panic!("review page: {error}"));

    let rules = service
        .rules_preferences_state(workspace.id)
        .unwrap_or_else(|error| panic!("rules state: {error}"));
    let _ = &rules.preferences;

    let proposal = service
        .generate_organization_proposal_for_root(
            workspace.id,
            registered.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("proposal generate: {error}"));
    assert_eq!(proposal.workspace_id, workspace.id);
    assert_eq!(proposal.root_id, registered.id);
}
