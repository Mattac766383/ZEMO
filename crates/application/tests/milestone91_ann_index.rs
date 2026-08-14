#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::ScannerApplicationService;
use extraction::{ContentExtractionEngine, LocalExtractionEngine};
use knowledge::{DeterministicSemanticProvider, SemanticProvider};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use search::{
    AnnIndexStatus, DeterministicTestEmbeddingProvider, LocalEmbeddingProvider, SearchQuery,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
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

fn service_with_ann(database: Arc<Database>, ann_root: PathBuf) -> ScannerApplicationService {
    ScannerApplicationService::new_with_all_engines_and_ann(
        database,
        native_platform(),
        Arc::new(LocalExtractionEngine::local_default()) as Arc<dyn ContentExtractionEngine>,
        Arc::new(DeterministicSemanticProvider::default()) as Arc<dyn SemanticProvider>,
        Arc::new(DeterministicTestEmbeddingProvider::new(384)) as Arc<dyn LocalEmbeddingProvider>,
        Some(ann_root),
    )
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("dirs");
    }
    fs::write(&target, content).expect("write");
}

fn run_pipeline(service: &ScannerApplicationService, workspace_id: domain::WorkspaceId) {
    let scan = service
        .scan_workspace(workspace_id, &|| false, &mut |_| {})
        .expect("scan");
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("extract");
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .expect("semantics");
}

fn corrupt_ann_artifacts(ann_root: &Path, workspace_id: &str) {
    let safe = workspace_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    let index_path = ann_root.join(format!("{safe}.usearch"));
    assert!(index_path.is_file(), "expected persisted ANN snapshot");
    fs::write(&index_path, b"not-a-valid-usearch-index").expect("corrupt index");
}

#[test]
fn ann_incremental_insert_update_and_corruption_fallback() {
    let temp = TempDir::new().expect("temp");
    let key = DatabaseKey::from_bytes([9_u8; 32]);
    let database = Arc::new(Database::open_in_memory(&key).expect("db"));
    write_file(
        temp.path(),
        "toiture.txt",
        "FACTURE N° TOIT-1\nSupplier: Point P\nCustomer: Dupont SARL\nProject: Couverture\nTravaux de réfection de toiture\nMontant TTC: 1 400,00 EUR",
    );
    write_file(
        temp.path(),
        "cv.txt",
        "Jean Dupont — CV développeur logiciel expérience React Rust",
    );

    let ann_root = temp.path().join("ann");
    let service = service_with_ann(database.clone(), ann_root.clone());
    let workspace = service.create_workspace("ann-step2").expect("workspace");
    service
        .register_root(workspace.id, temp.path())
        .expect("register root");
    run_pipeline(&service, workspace.id);

    let page = service
        .search_files(
            workspace.id,
            SearchQuery {
                text: "toiture Dupont".to_owned(),
                semantic_search: true,
                ..SearchQuery::default()
            },
        )
        .expect("search");
    assert!(
        !page.results.is_empty(),
        "expected search hits, got none (ann={:?}, embeddings={:?})",
        page.embeddings.ann_index_status,
        page.embeddings
    );
    // ANN may be ready after first semantic index; if not yet ready, lexical still works.
    assert!(matches!(
        page.embeddings.ann_index_status.as_deref(),
        Some("ready") | Some("not_available") | None
    ));

    write_file(
        temp.path(),
        "toiture.txt",
        "FACTURE N° TOIT-1\nSupplier: Point P\nCustomer: Dupont SARL\nProject: Couverture\nTravaux de réfection de toiture zinc\nMontant TTC: 1 400,00 EUR",
    );
    run_pipeline(&service, workspace.id);
    let page2 = service
        .search_files(
            workspace.id,
            SearchQuery {
                text: "zinc toiture".to_owned(),
                semantic_search: true,
                ..SearchQuery::default()
            },
        )
        .expect("search2");
    assert!(!page2.results.is_empty());
    assert_eq!(
        page2.embeddings.ann_index_status.as_deref(),
        Some("ready"),
        "ANN should be ready after semantic indexing with ann_root configured"
    );

    corrupt_ann_artifacts(&ann_root, &workspace.id.to_string());
    // New service instance forces ANN reload from corrupted artifacts.
    let service2 = service_with_ann(database, ann_root);
    let page3 = service2
        .search_files(
            workspace.id,
            SearchQuery {
                text: "toiture Dupont".to_owned(),
                semantic_search: true,
                ..SearchQuery::default()
            },
        )
        .expect("fallback search");
    assert!(!page3.results.is_empty(), "lexical fallback must work");
    assert_eq!(
        page3.embeddings.ann_index_status.as_deref(),
        Some(AnnIndexStatus::RebuildRequired.as_str())
    );

    let rebuilt = service2
        .rebuild_semantic_ann_index(workspace.id, &|| false)
        .expect("rebuild");
    assert_eq!(rebuilt, AnnIndexStatus::Ready);
}
