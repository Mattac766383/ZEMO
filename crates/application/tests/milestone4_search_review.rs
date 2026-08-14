#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::{ExtractionRetryStatus, ScannerApplicationService};
use extraction::{
    ContentExtractionEngine, ContentKind, ErrorCategory, ExtractionLimits, ExtractionPlan,
    ExtractionResult, ExtractionStatus, ExtractorType, FileTypeDetection, ReadMode,
};
use persistence::{Database, DatabaseKey, ReviewAction, ReviewReasonFilter, ReviewStatusFilter};
use platform::ReadOnlyPlatform;
use search::{
    ExtractionFilter, FileTypeFilter, ModifiedFilter, OcrFilter, SearchFilters, SearchQuery,
    SearchSort,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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

fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let target = root.join(relative);
    assert!(target.starts_with(root) && target != root);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("fixture parent should exist: {error}"));
    }
    fs::write(&target, bytes)
        .unwrap_or_else(|error| panic!("fixture write should succeed: {error}"));
    target
}

fn service_for(
    root: &Path,
    database: Arc<Database>,
    engine: Option<Arc<dyn ContentExtractionEngine>>,
) -> (ScannerApplicationService, domain::WorkspaceId) {
    let service = engine.map_or_else(
        || ScannerApplicationService::new(database.clone(), native_platform()),
        |engine| {
            ScannerApplicationService::new_with_content_engine(
                database.clone(),
                native_platform(),
                engine,
                None,
            )
        },
    );
    let workspace = service
        .create_workspace("Milestone 4 test")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    service
        .register_root(workspace.id, root)
        .unwrap_or_else(|error| panic!("fixture root should register: {error}"));
    (service, workspace.id)
}

fn memory_database(seed: u8) -> Arc<Database> {
    Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([seed; 32]))
            .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
    )
}

fn run_scan(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
) -> persistence::ScanRecord {
    service
        .scan_workspace(workspace_id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should complete: {error}"))
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        ..SearchQuery::default()
    }
}

#[test]
fn lexical_search_is_safe_ranked_filtered_paginated_and_synchronized() {
    let fixture = TempDir::new().expect("fixture directory should exist");
    write_file(
        fixture.path(),
        "Facture_été_2026.txt",
        "Facture Point P pour l’école Dupont — montant 1 437 euros".as_bytes(),
    );
    write_file(fixture.path(), "invoice.txt", b"ordinary local document");
    write_file(
        fixture.path(),
        "notes/content-only.txt",
        b"the invoice keyword appears only in extracted content",
    );
    write_file(
        fixture.path(),
        "Clients/Dupont/scan-00482.txt",
        b"poor filename but useful Bordeaux project text",
    );
    write_file(fixture.path(), "data/budget.csv", b"year,total\n2026,1437");
    write_file(fixture.path(), "update.txt", b"zebraword original content");
    let (service, workspace_id) = service_for(fixture.path(), memory_database(81), None);
    let first_scan = run_scan(&service, workspace_id);

    let filename_before_extraction = service
        .search_files(workspace_id, query("Facture"))
        .expect("filename index should be available after scan");
    assert_eq!(
        filename_before_extraction.results[0].filename,
        "Facture_été_2026.txt"
    );

    service
        .analyze_scan_content(first_scan.id, &|| false, &mut |_| {})
        .expect("content extraction should complete");

    let content = service
        .search_files(workspace_id, query("Point P"))
        .expect("content search should succeed");
    assert_eq!(content.total, 1);
    assert_eq!(
        content.results[0].match_source,
        search::MatchSource::Content
    );
    assert!(content.results[0].snippet.contains("Point P"));
    assert!(content.results[0].snippet.chars().count() <= 500);

    for safe_query in [
        "été",
        "ete",
        "l’école",
        "l'ecole",
        "\"Point P\"",
        "Facture: Point-P!",
        "客户",
        "🧾",
        "\" OR () ***",
    ] {
        service
            .search_files(workspace_id, query(safe_query))
            .unwrap_or_else(|error| panic!("query {safe_query:?} must be safe: {error}"));
    }
    service
        .search_files(workspace_id, query(&"facture ".repeat(2_000)))
        .expect("very long input must be bounded");

    let path_result = service
        .search_files(workspace_id, query("Dupont"))
        .expect("path search should succeed");
    assert!(path_result.results.iter().any(|item| {
        item.relative_path.contains("Clients/Dupont")
            && item.match_source == search::MatchSource::Path
    }));

    let relevance = service
        .search_files(workspace_id, query("invoice"))
        .expect("relevance search should succeed");
    assert_eq!(relevance.results[0].filename, "invoice.txt");

    let all = service
        .search_files(workspace_id, SearchQuery::default())
        .expect("empty query should browse the bounded catalog");
    let whitespace = service
        .search_files(workspace_id, query(" \t\n "))
        .expect("whitespace should be handled");
    assert_eq!(all.total, 6);
    assert_eq!(whitespace.total, all.total);
    assert_eq!(
        service
            .search_files(workspace_id, query("not-present-anywhere"))
            .expect("no-result query should succeed")
            .total,
        0
    );

    let first_page = service
        .search_files(
            workspace_id,
            SearchQuery {
                page_size: 2,
                ..SearchQuery::default()
            },
        )
        .expect("first page should load");
    let second_page = service
        .search_files(
            workspace_id,
            SearchQuery {
                page: 1,
                page_size: 2,
                ..SearchQuery::default()
            },
        )
        .expect("second page should load");
    assert_eq!(first_page.results.len(), 2);
    assert!(first_page.has_more);
    assert_ne!(
        first_page.results[0].file_id,
        second_page.results[0].file_id
    );

    for sort in [
        SearchSort::Relevance,
        SearchSort::Newest,
        SearchSort::Oldest,
        SearchSort::Filename,
        SearchSort::Size,
    ] {
        assert!(
            !service
                .search_files(
                    workspace_id,
                    SearchQuery {
                        sort,
                        ..SearchQuery::default()
                    }
                )
                .expect("each trusted sort should work")
                .results
                .is_empty()
        );
    }

    let filtered = service
        .search_files(
            workspace_id,
            SearchQuery {
                filters: SearchFilters {
                    file_type: FileTypeFilter::Documents,
                    modified: ModifiedFilter::Today,
                    extraction: ExtractionFilter::Success,
                    ocr: OcrFilter::NotUsed,
                    ..SearchFilters::default()
                },
                ..SearchQuery::default()
            },
        )
        .expect("combined filters should work");
    assert!(filtered.total >= 5);
    assert!(
        filtered
            .results
            .iter()
            .all(|item| item.extension.as_deref() == Some("txt"))
    );

    write_file(
        fixture.path(),
        "update.txt",
        b"newquasar replacement content is longer",
    );
    let second_scan = run_scan(&service, workspace_id);
    service
        .analyze_scan_content(second_scan.id, &|| false, &mut |_| {})
        .expect("changed content should be re-extracted");
    assert_eq!(
        service
            .search_files(workspace_id, query("zebraword"))
            .expect("old index lookup should succeed")
            .total,
        0
    );
    assert_eq!(
        service
            .search_files(workspace_id, query("newquasar"))
            .expect("updated index lookup should succeed")
            .total,
        1
    );
}

#[derive(Debug)]
struct ScenarioEngine {
    limits: ExtractionLimits,
    attempts: Mutex<HashMap<String, usize>>,
}

impl ScenarioEngine {
    fn new() -> Self {
        Self {
            limits: ExtractionLimits::default(),
            attempts: Mutex::new(HashMap::new()),
        }
    }
}

impl ContentExtractionEngine for ScenarioEngine {
    fn limits(&self) -> &ExtractionLimits {
        &self.limits
    }

    fn prepare(
        &self,
        extension: Option<&str>,
        _declared_media_type: Option<&str>,
        _prefix: &[u8],
        file_size: u64,
    ) -> ExtractionPlan {
        let extension = extension.unwrap_or_default().to_ascii_lowercase();
        ExtractionPlan {
            detection: FileTypeDetection {
                extension: Some(extension.clone()),
                content_kind: if extension == "mp4" {
                    ContentKind::Video
                } else {
                    ContentKind::Text
                },
                detected_content_type: if extension == "mp4" {
                    "video/mp4".to_owned()
                } else {
                    "text/plain".to_owned()
                },
                magic_confirmed: false,
                mismatch: false,
            },
            read_mode: ReadMode::Whole(file_size),
            preflight_status: None,
            preflight_error: None,
            preflight_message: None,
        }
    }

    fn extract(
        &self,
        plan: &ExtractionPlan,
        bytes: &[u8],
        _file_size: u64,
        is_cancelled: &dyn Fn() -> bool,
    ) -> ExtractionResult {
        let scenario = String::from_utf8_lossy(bytes).into_owned();
        if is_cancelled() {
            return scenario_result(
                plan,
                ExtractionStatus::Skipped,
                "",
                Some(ErrorCategory::Cancelled),
                false,
            );
        }
        if scenario == "retry-case" {
            let mut attempts = self
                .attempts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let count = attempts.entry(scenario.clone()).or_default();
            *count += 1;
            if *count > 1 {
                return scenario_result(
                    plan,
                    ExtractionStatus::Success,
                    "retry recovered",
                    None,
                    false,
                );
            }
            return scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::ParserFailure),
                false,
            );
        }
        match scenario.as_str() {
            "unreadable" => scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::Unreadable),
                false,
            ),
            "encrypted" => scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::EncryptedDocument),
                false,
            ),
            "unsupported" | "video" => scenario_result(
                plan,
                ExtractionStatus::Unsupported,
                "",
                Some(ErrorCategory::Unsupported),
                false,
            ),
            "corrupt" => scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::Corrupt),
                false,
            ),
            "too-large" => scenario_result(
                plan,
                ExtractionStatus::Skipped,
                "",
                Some(ErrorCategory::TooLarge),
                false,
            ),
            "ocr-failed" => scenario_result(
                plan,
                ExtractionStatus::Partial,
                "",
                Some(ErrorCategory::OcrFailed),
                false,
            ),
            "ocr-unavailable" => scenario_result(
                plan,
                ExtractionStatus::Partial,
                "partial OCR searchable text",
                Some(ErrorCategory::OcrUnavailable),
                false,
            ),
            "type-mismatch" => scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::TypeMismatch),
                true,
            ),
            "permission" => scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::PermissionDenied),
                false,
            ),
            "partial" => scenario_result(
                plan,
                ExtractionStatus::Partial,
                "valid partial searchable content",
                None,
                false,
            ),
            "failed" => scenario_result(
                plan,
                ExtractionStatus::Failed,
                "",
                Some(ErrorCategory::ParserFailure),
                false,
            ),
            _ => scenario_result(plan, ExtractionStatus::Success, &scenario, None, false),
        }
    }
}

fn scenario_result(
    plan: &ExtractionPlan,
    status: ExtractionStatus,
    text: &str,
    error: Option<ErrorCategory>,
    type_mismatch: bool,
) -> ExtractionResult {
    ExtractionResult {
        status,
        extractor: matches!(
            status,
            ExtractionStatus::Success | ExtractionStatus::Partial
        )
        .then_some(ExtractorType::PlainText),
        extractor_version: Some("test".to_owned()),
        detected_content_type: plan.detection.detected_content_type.clone(),
        type_mismatch,
        text: text.to_owned(),
        character_count: u64::try_from(text.chars().count()).unwrap_or(u64::MAX),
        page_count: None,
        sheet_count: None,
        slide_count: None,
        image_width: None,
        image_height: None,
        requires_ocr: matches!(
            error,
            Some(ErrorCategory::OcrFailed | ErrorCategory::OcrUnavailable)
        ),
        ocr_used: false,
        ocr_confidence: None,
        language_hint: Some("fr".to_owned()),
        duration_ms: 1,
        truncated: status == ExtractionStatus::Partial,
        metadata: serde_json::json!({"network": false}),
        error_category: error,
        error_message: error
            .map(|category| format!("test condition: {}", category.database_name())),
    }
}

#[test]
fn review_lifecycle_retry_and_file_detail_are_persistent_and_non_destructive() {
    let fixture = TempDir::new().expect("fixture directory should exist");
    let database_dir = TempDir::new().expect("database directory should exist");
    let cases = [
        ("unreadable.txt", "unreadable"),
        ("encrypted.pdf", "encrypted"),
        ("legacy.doc", "unsupported"),
        ("corrupt.pdf", "corrupt"),
        ("huge.txt", "too-large"),
        ("ocr-failed.png", "ocr-failed"),
        ("ocr-missing.png", "ocr-unavailable"),
        ("mismatch.pdf", "type-mismatch"),
        ("permission.txt", "permission"),
        ("partial.txt", "partial"),
        ("failed.txt", "failed"),
        ("retry.txt", "retry-case"),
        ("movie.mp4", "video"),
    ];
    for (name, content) in cases {
        write_file(fixture.path(), name, content.as_bytes());
    }
    let before = fixture_snapshot(fixture.path());
    let database_path = database_dir.path().join("catalog.db");
    let key = [93; 32];
    let database = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(key))
            .expect("encrypted database should open"),
    );
    let (service, workspace_id) = service_for(
        fixture.path(),
        database.clone(),
        Some(Arc::new(ScenarioEngine::new())),
    );
    let scan = run_scan(&service, workspace_id);
    service
        .analyze_scan_content(scan.id, &|| false, &mut |_| {})
        .expect("scenario extraction should complete");

    let review_page = service
        .review_items(
            workspace_id,
            ReviewStatusFilter::NeedsReview,
            ReviewReasonFilter::All,
            100,
            0,
        )
        .expect("review items should load");
    let reasons = review_page
        .items
        .iter()
        .map(|item| item.reason.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "unreadable",
        "encrypted",
        "unsupported_format",
        "corrupt",
        "too_large",
        "ocr_failed",
        "ocr_provider_unavailable",
        "type_mismatch",
        "permission_denied",
        "partial_extraction",
        "extraction_failed",
    ] {
        assert!(
            reasons.contains(&expected),
            "missing review reason {expected}"
        );
    }
    assert!(
        review_page
            .items
            .iter()
            .all(|item| item.filename != "movie.mp4"),
        "unsupported video must not create unactionable review spam"
    );

    let partial_search = service
        .search_files(workspace_id, query("valid partial searchable"))
        .expect("valid partial text should be indexed");
    assert_eq!(partial_search.total, 1);
    assert_eq!(
        partial_search.results[0].extraction_status.as_deref(),
        Some("partial")
    );
    let ocr_filter = service
        .search_files(
            workspace_id,
            SearchQuery {
                filters: SearchFilters {
                    ocr: OcrFilter::Unavailable,
                    ..SearchFilters::default()
                },
                ..SearchQuery::default()
            },
        )
        .expect("OCR unavailable filter should work");
    assert_eq!(ocr_filter.total, 1);

    let retry_item = review_page
        .items
        .iter()
        .find(|item| item.filename == "retry.txt")
        .expect("retry failure should be reviewable");
    let retry = service
        .retry_review_extraction(&retry_item.review_id, &|| false)
        .expect("retry should execute through the extraction pipeline");
    assert_eq!(retry.status, ExtractionRetryStatus::Succeeded);
    let detail = service
        .file_detail(&retry_item.file_id)
        .expect("file detail should load by file ID");
    assert_eq!(detail.extraction_status.as_deref(), Some("success"));
    assert_eq!(detail.text_preview, "retry recovered");

    let encrypted = review_page
        .items
        .iter()
        .find(|item| item.reason == "encrypted")
        .expect("encrypted review should exist");
    let unavailable = service
        .retry_review_extraction(&encrypted.review_id, &|| false)
        .expect("inapplicable retry should be a structured result");
    assert_eq!(unavailable.status, ExtractionRetryStatus::Unavailable);
    let cancellable = review_page
        .items
        .iter()
        .find(|item| item.filename == "failed.txt")
        .expect("failed extraction should be retryable");
    let cancelled = service
        .retry_review_extraction(&cancellable.review_id, &|| true)
        .expect("retry cancellation should be structured");
    assert_eq!(cancelled.status, ExtractionRetryStatus::Cancelled);

    let resolve = review_page
        .items
        .iter()
        .find(|item| item.reason == "corrupt")
        .expect("corrupt review should exist");
    let ignore = review_page
        .items
        .iter()
        .find(|item| item.reason == "unsupported_format")
        .expect("unsupported review should exist");
    assert_eq!(
        service
            .update_review_item(&resolve.review_id, ReviewAction::Resolve)
            .expect("resolve should persist")
            .status,
        "resolved"
    );
    assert_eq!(
        service
            .update_review_item(&ignore.review_id, ReviewAction::Ignore)
            .expect("ignore should persist")
            .status,
        "ignored"
    );

    let ocr_item = review_page
        .items
        .iter()
        .find(|item| item.reason == "ocr_provider_unavailable")
        .expect("OCR unavailable review should exist");
    for _ in 0..2 {
        service
            .retry_review_extraction(&ocr_item.review_id, &|| false)
            .expect("explicit retry should finish without looping");
    }
    let all_items = service
        .review_items(
            workspace_id,
            ReviewStatusFilter::All,
            ReviewReasonFilter::All,
            100,
            0,
        )
        .expect("all review states should load");
    assert_eq!(
        all_items
            .items
            .iter()
            .filter(|item| {
                item.file_id == ocr_item.file_id && item.reason == "ocr_provider_unavailable"
            })
            .count(),
        1,
        "retries must not duplicate review records"
    );
    assert_eq!(fixture_snapshot(fixture.path()), before);

    drop(service);
    drop(database);
    let reopened = Database::open(&database_path, &DatabaseKey::from_bytes(key))
        .expect("encrypted database should reopen");
    assert!(
        reopened
            .review_items(
                workspace_id,
                ReviewStatusFilter::Resolved,
                ReviewReasonFilter::All,
                100,
                0,
            )
            .expect("resolved state should survive reopen")
            .items
            .iter()
            .any(|item| item.review_id == resolve.review_id)
    );
    assert!(
        reopened
            .review_items(
                workspace_id,
                ReviewStatusFilter::Ignored,
                ReviewReasonFilter::All,
                100,
                0,
            )
            .expect("ignored state should survive reopen")
            .items
            .iter()
            .any(|item| item.review_id == ignore.review_id)
    );
    reopened
        .local_search_integrity_check()
        .expect("FTS integrity should survive reopen");
    assert_eq!(
        reopened
            .foreign_key_violation_count()
            .expect("foreign key check should run"),
        0
    );
}

fn fixture_snapshot(root: &Path) -> Vec<(PathBuf, u64, blake3::Hash)> {
    fn visit(root: &Path, directory: &Path, output: &mut Vec<(PathBuf, u64, blake3::Hash)>) {
        let mut entries = fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("fixture directory should be readable: {error}"))
            .map(|entry| entry.unwrap_or_else(|error| panic!("fixture entry should load: {error}")))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("fixture metadata should load: {error}"));
            if metadata.is_dir() {
                visit(root, &path, output);
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or_else(|error| panic!("fixture must remain in root: {error}"))
                    .to_path_buf();
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("fixture file should be readable: {error}"));
                output.push((relative, metadata.len(), blake3::hash(&bytes)));
            }
        }
    }
    let mut output = Vec::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn several_thousand_catalog_records_remain_bounded_and_responsive() {
    let fixture = TempDir::new().expect("fixture directory should exist");
    for index in 0..3_000_u16 {
        write_file(
            fixture.path(),
            &format!("bulk/file-{index:04}.dat"),
            format!("record-{index}").as_bytes(),
        );
    }
    let (service, workspace_id) = service_for(fixture.path(), memory_database(105), None);
    let scan = run_scan(&service, workspace_id);
    assert_eq!(scan.indexed_count, 3_000);

    let started = Instant::now();
    let result = service
        .search_files(
            workspace_id,
            SearchQuery {
                text: "file-2999".to_owned(),
                page_size: 50,
                ..SearchQuery::default()
            },
        )
        .expect("large local catalog search should succeed");
    let elapsed = started.elapsed();
    let warm_started = Instant::now();
    for _ in 0..5 {
        service
            .search_files(
                workspace_id,
                SearchQuery {
                    text: "file-2999".to_owned(),
                    page_size: 50,
                    ..SearchQuery::default()
                },
            )
            .expect("repeated local search should succeed");
    }
    let warm_average = warm_started.elapsed() / 5;
    eprintln!(
        "milestone4_search_3000_records_cold_ms={} warm_average_ms={}",
        elapsed.as_secs_f64() * 1_000.0,
        warm_average.as_secs_f64() * 1_000.0,
    );
    assert_eq!(result.total, 1);
    assert_eq!(result.results.len(), 1);
    assert!(result.results.len() <= 50);
    assert!(
        elapsed < Duration::from_secs(1),
        "3,000-record search took {elapsed:?}"
    );
    assert!(
        warm_average < Duration::from_millis(250),
        "repeated 3,000-record search took {warm_average:?}"
    );
}
