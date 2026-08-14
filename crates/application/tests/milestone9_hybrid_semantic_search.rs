#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::{ScannerApplicationService, SemanticCorrectionAction};
use extraction::{ContentExtractionEngine, LocalExtractionEngine};
use knowledge::{DeterministicSemanticProvider, SemanticProvider};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use search::{
    ContextFilter, DeterministicTestEmbeddingProvider, DocumentTypeFilter, EmbeddingAvailability,
    EmbeddingError, EmbeddingInput, EmbeddingOutput, EmbeddingProviderDescriptor, FileTypeFilter,
    LocalEmbeddingProvider, SearchFilters, SearchQuery, SemanticStatusFilter,
    UnavailableEmbeddingProvider,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
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

fn test_embedding_service(database: Arc<Database>) -> ScannerApplicationService {
    ScannerApplicationService::new_with_all_engines(
        database,
        native_platform(),
        Arc::new(LocalExtractionEngine::local_default()) as Arc<dyn ContentExtractionEngine>,
        Arc::new(DeterministicSemanticProvider::default()) as Arc<dyn SemanticProvider>,
        Arc::new(DeterministicTestEmbeddingProvider::default()) as Arc<dyn LocalEmbeddingProvider>,
    )
}

#[derive(Clone)]
struct UnsafeNetworkDeclaredEmbeddingProvider {
    calls: Arc<AtomicUsize>,
}

impl LocalEmbeddingProvider for UnsafeNetworkDeclaredEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor {
            provider_id: "unsafe-network-provider".to_owned(),
            version: "1".to_owned(),
            dimensions: 192,
            local_only: false,
            production_ready: true,
            requires_download: true,
            model_size_bytes: 2 * 1024 * 1024 * 1024,
            max_model_size_bytes: 1024 * 1024 * 1024,
        }
    }

    fn availability(&self) -> EmbeddingAvailability {
        EmbeddingAvailability::AvailableProduction
    }

    fn embed_batch(
        &self,
        _inputs: &[EmbeddingInput],
    ) -> Result<Vec<EmbeddingOutput>, EmbeddingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(EmbeddingError::Unavailable)
    }
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    assert!(target.starts_with(root) && target != root);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("fixture parent should be created: {error}"));
    }
    fs::write(target, content).unwrap_or_else(|error| panic!("fixture should be written: {error}"));
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, blake3::Hash)> {
    let mut output = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("fixture should be readable: {error}"))
        {
            let entry =
                entry.unwrap_or_else(|error| panic!("fixture entry should be readable: {error}"));
            let path = entry.path();
            let metadata = entry
                .metadata()
                .unwrap_or_else(|error| panic!("fixture metadata should load: {error}"));
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("fixture bytes should load: {error}"));
                output.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|error| panic!("path should stay scoped: {error}"))
                        .to_path_buf(),
                    (metadata.len(), blake3::hash(&bytes)),
                );
            }
        }
    }
    output
}

fn run_pipeline(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
) -> persistence::ScanRecord {
    let scan = service
        .scan_workspace(workspace_id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should complete: {error}"));
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("content extraction should complete: {error}"));
    service
        .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("semantic indexing should complete: {error}"));
    scan
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        page_size: 100,
        ..SearchQuery::default()
    }
}

fn file_id(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
    filename: &str,
) -> String {
    service
        .search_files(workspace_id, query(filename))
        .unwrap_or_else(|error| panic!("filename lookup should succeed: {error}"))
        .results
        .into_iter()
        .find(|result| result.filename == filename)
        .unwrap_or_else(|| panic!("{filename} should be indexed"))
        .file_id
}

#[test]
fn hybrid_search_understands_facts_relationships_fallback_and_updates() {
    let fixture = TempDir::new().expect("fixture directory should exist");
    write_file(
        fixture.path(),
        "imports/00482.txt",
        "FACTURE N° PP-00482\nSupplier: Point P\nCustomer: Martin Client SAS\nProject: Martin\nProject reference: MARTIN-26\nInvoice date: 17/06/2026\nMontant TTC: 1 400,00 EUR",
    );
    write_file(
        fixture.path(),
        "clear-but-wrong-name-invoice.txt",
        "FACTURE N° OTHER-77\nSupplier: Other Materials\nCustomer: Martin Client SAS\nProject: Martin\nProject reference: MARTIN-26\nInvoice date: 18/06/2026\nMontant TTC: 1 400,00 EUR",
    );
    write_file(
        fixture.path(),
        "dupont-contract.txt",
        "CONTRAT\nCustomer: Dupont SAS\nContract title: Maintenance\nSignature date: 2026-01-12",
    );
    write_file(
        fixture.path(),
        "personal-admin.txt",
        "DOCUMENT ADMINISTRATIF PERSONNEL\nAttestation personnelle\nDate: 2025-02-03",
    );
    write_file(
        fixture.path(),
        "project-photo-note.txt",
        "PHOTO\nProject: Dupont\nProject reference: DUPONT-PHOTO-26\nImage du chantier Dupont",
    );

    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([119; 32]))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    );
    let service = test_embedding_service(database.clone());
    let workspace = service
        .create_workspace("Milestone 9 hybrid retrieval")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("root should register: {error}"));
    run_pipeline(&service, workspace.id);
    let source_snapshot = snapshot(fixture.path());

    let natural = service
        .search_files(
            workspace.id,
            query("Retrouve la facture Point P d'environ 1 400 € du chantier Martin"),
        )
        .unwrap_or_else(|error| panic!("natural query should succeed: {error}"));
    assert_eq!(natural.results[0].filename, "00482.txt");
    let natural_detail = database
        .file_detail(&natural.results[0].file_id)
        .unwrap_or_else(|error| panic!("natural result detail should load: {error}"));
    assert!(
        natural.results[0]
            .why_matched
            .iter()
            .any(|reason| reason.contains("Point P"))
    );
    assert!(
        natural.results[0]
            .why_matched
            .iter()
            .any(|reason| reason.contains("montant")),
        "natural-query reasons: {:?}; semantic detail: {:?}",
        natural.results[0].why_matched,
        natural_detail.semantic_analysis
    );
    assert!(
        natural
            .interpreted_query
            .iter()
            .any(|chip| chip.kind == "amount" && chip.label.contains("1400"))
    );
    assert_eq!(
        natural.embeddings.availability,
        EmbeddingAvailability::AvailableDevelopment
    );
    assert!(!natural.embeddings.production_ready);

    let amount = service
        .search_files(workspace.id, query("facture autour de 1400 euros"))
        .unwrap_or_else(|error| panic!("amount query should succeed: {error}"));
    assert!(
        amount
            .results
            .iter()
            .any(|result| result.filename == "00482.txt")
    );
    let date = service
        .search_files(workspace.id, query("facture fournisseur de juin 2026"))
        .unwrap_or_else(|error| panic!("date query should succeed: {error}"));
    assert!(
        date.results
            .iter()
            .any(|result| result.filename == "00482.txt")
    );
    let supplier = service
        .search_files(workspace.id, query("facture Point P"))
        .unwrap_or_else(|error| panic!("supplier query should succeed: {error}"));
    assert_eq!(supplier.results[0].filename, "00482.txt");
    let project = service
        .search_files(workspace.id, query("documents du projet Martin"))
        .unwrap_or_else(|error| panic!("project query should succeed: {error}"));
    assert!(
        project
            .results
            .iter()
            .any(|result| result.filename == "00482.txt")
    );
    let contract = service
        .search_files(workspace.id, query("contrat Dupont 2026"))
        .unwrap_or_else(|error| panic!("contract query should succeed: {error}"));
    assert_eq!(contract.results[0].filename, "dupont-contract.txt");
    let photo = service
        .search_files(workspace.id, query("photos liées au chantier Dupont"))
        .unwrap_or_else(|error| panic!("photo/project query should succeed: {error}"));
    assert!(
        photo
            .results
            .iter()
            .any(|result| result.filename == "project-photo-note.txt")
    );
    let personal = service
        .search_files(workspace.id, query("documents administratifs personnels"))
        .unwrap_or_else(|error| panic!("personal context query should succeed: {error}"));
    let personal_detail = database
        .file_detail(&file_id(&service, workspace.id, "personal-admin.txt"))
        .unwrap_or_else(|error| panic!("personal detail should load: {error}"));
    assert!(
        personal
            .results
            .iter()
            .any(|result| result.filename == "personal-admin.txt"),
        "personal results: {:?}; semantic detail: {:?}",
        personal.results,
        personal_detail.semantic_analysis
    );

    let filtered = service
        .search_files(
            workspace.id,
            SearchQuery {
                filters: SearchFilters {
                    file_type: FileTypeFilter::Documents,
                    document_type: DocumentTypeFilter::Invoice,
                    context: ContextFilter::Business,
                    supplier: Some("Point P".to_owned()),
                    year: Some(2026),
                    amount_minimum_minor: Some(130_000),
                    amount_maximum_minor: Some(150_000),
                    currency: Some("EUR".to_owned()),
                    semantic_status: SemanticStatusFilter::Partial,
                    minimum_confidence_percent: Some(65),
                    ..SearchFilters::default()
                },
                ..SearchQuery::default()
            },
        )
        .unwrap_or_else(|error| panic!("structured filters should succeed: {error}"));
    assert_eq!(
        filtered.results.len(),
        1,
        "target semantic detail: {:?}",
        natural_detail.semantic_analysis
    );
    assert_eq!(filtered.results[0].filename, "00482.txt");

    let target_id = file_id(&service, workspace.id, "00482.txt");
    service
        .store_semantic_correction(
            &target_id,
            "supplier_candidate",
            SemanticCorrectionAction::Confirm,
            None,
        )
        .unwrap_or_else(|error| panic!("supplier confirmation should persist: {error}"));
    let confirmed = service
        .search_files(workspace.id, query("facture Point P"))
        .unwrap_or_else(|error| panic!("confirmed query should succeed: {error}"));
    assert_eq!(confirmed.results[0].filename, "00482.txt");
    assert!(
        confirmed.results[0]
            .why_matched
            .iter()
            .any(|reason| reason.contains("confirmé"))
    );

    let fallback = ScannerApplicationService::new_with_all_engines(
        database.clone(),
        native_platform(),
        Arc::new(LocalExtractionEngine::local_default()) as Arc<dyn ContentExtractionEngine>,
        Arc::new(DeterministicSemanticProvider::default()) as Arc<dyn SemanticProvider>,
        Arc::new(UnavailableEmbeddingProvider) as Arc<dyn LocalEmbeddingProvider>,
    );
    let lexical_structured = fallback
        .search_files(
            workspace.id,
            query("facture Point P environ 1400 euros chantier Martin"),
        )
        .unwrap_or_else(|error| panic!("fallback query should succeed: {error}"));
    assert_eq!(lexical_structured.results[0].filename, "00482.txt");
    assert_eq!(
        lexical_structured.embeddings.availability,
        EmbeddingAvailability::Unavailable
    );
    assert!(fallback.system_status().network_disabled);

    let unsafe_provider_calls = Arc::new(AtomicUsize::new(0));
    let unsafe_provider = ScannerApplicationService::new_with_all_engines(
        database.clone(),
        native_platform(),
        Arc::new(LocalExtractionEngine::local_default()) as Arc<dyn ContentExtractionEngine>,
        Arc::new(DeterministicSemanticProvider::default()) as Arc<dyn SemanticProvider>,
        Arc::new(UnsafeNetworkDeclaredEmbeddingProvider {
            calls: unsafe_provider_calls.clone(),
        }) as Arc<dyn LocalEmbeddingProvider>,
    );
    let guarded = unsafe_provider
        .search_files(workspace.id, query("facture Point P chantier Martin"))
        .unwrap_or_else(|error| {
            panic!("unsafe provider should fail closed to local search: {error}")
        });
    assert_eq!(guarded.results[0].filename, "00482.txt");
    assert_eq!(
        guarded.embeddings.availability,
        EmbeddingAvailability::Unavailable
    );
    assert_eq!(unsafe_provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(snapshot(fixture.path()), source_snapshot);

    write_file(
        fixture.path(),
        "imports/00482.txt",
        "FACTURE\nSupplier: Point P\nCustomer: Martin Client SAS\nProject: Martin\nProject reference: MARTIN-26\nInvoice date: 17/06/2026\nTotal: 1 600,00 EUR",
    );
    run_pipeline(&service, workspace.id);
    let updated = service
        .search_files(
            workspace.id,
            query("facture Point P environ 1600 euros chantier Martin"),
        )
        .unwrap_or_else(|error| panic!("updated query should succeed: {error}"));
    assert_eq!(updated.results[0].filename, "00482.txt");
    let descriptor = DeterministicTestEmbeddingProvider::default().descriptor();
    let stats = database
        .local_embedding_index_stats(workspace.id, &descriptor)
        .unwrap_or_else(|error| panic!("embedding statistics should load: {error}"));
    assert_eq!(stats.file_count, 5);
    assert!(stats.vector_count >= stats.file_count);
    assert!(stats.vector_bytes > 0);
}

#[test]
fn hybrid_search_scales_to_several_thousand_semantic_records() {
    const RECORD_COUNT: usize = 3_000;
    let fixture = TempDir::new().expect("fixture directory should exist");
    let database_dir = TempDir::new().expect("database directory should exist");
    let database_path = database_dir.path().join("milestone9-scale.db");
    for index in 0..RECORD_COUNT {
        write_file(
            fixture.path(),
            &format!("batch/{index:05}.txt"),
            &format!(
                "FACTURE\nSupplier: Supplier {index:05}\nInvoice number: INV-{index:05}\nInvoice date: 2026-06-{:02}\nTotal: {},00 EUR",
                index % 28 + 1,
                index + 1_000
            ),
        );
    }
    let database = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes([120; 32]))
            .unwrap_or_else(|error| panic!("scale database should open: {error}")),
    );
    let service = test_embedding_service(database.clone());
    let workspace = service
        .create_workspace("Milestone 9 scale")
        .unwrap_or_else(|error| panic!("scale workspace should be created: {error}"));
    service
        .register_root(workspace.id, fixture.path())
        .unwrap_or_else(|error| panic!("scale root should register: {error}"));

    let indexing_started = Instant::now();
    run_pipeline(&service, workspace.id);
    let indexing_elapsed = indexing_started.elapsed();
    assert!(
        indexing_elapsed < Duration::from_secs(120),
        "3,000-record indexing took {indexing_elapsed:?}"
    );

    let query_started = Instant::now();
    let result = service
        .search_files(
            workspace.id,
            query("facture Supplier 02999 autour de 3999 euros en juin 2026"),
        )
        .unwrap_or_else(|error| panic!("scale query should succeed: {error}"));
    let query_elapsed = query_started.elapsed();
    assert_eq!(result.results[0].filename, "02999.txt");
    assert!(
        query_elapsed < Duration::from_secs(5),
        "3,000-record query took {query_elapsed:?}"
    );

    let descriptor = DeterministicTestEmbeddingProvider::default().descriptor();
    let stats = database
        .local_embedding_index_stats(workspace.id, &descriptor)
        .unwrap_or_else(|error| panic!("scale index statistics should load: {error}"));
    assert_eq!(stats.file_count, RECORD_COUNT as u64);
    assert!(stats.vector_count >= RECORD_COUNT as u64);
    assert!(stats.vector_bytes >= RECORD_COUNT as u64 * descriptor.dimensions as u64);
    assert!(result.timings.vector_ms <= result.timings.total_ms);
    assert!(result.timings.fusion_ms <= result.timings.total_ms);
    eprintln!(
        "M9_SCALE records={RECORD_COUNT} indexing_ms={} query_ms={} lexical_structured_ms={} vector_ms={} fusion_ms={} vectors={} vector_bytes={}",
        indexing_elapsed.as_millis(),
        query_elapsed.as_millis(),
        result.timings.lexical_and_structured_ms,
        result.timings.vector_ms,
        result.timings.fusion_ms,
        stats.vector_count,
        stats.vector_bytes
    );
}
