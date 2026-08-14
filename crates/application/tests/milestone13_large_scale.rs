//! Milestone 13 — large-scale qualification harness.
//!
//! Fast smoke (2k) runs in normal CI. Full 100k is `#[ignore]`:
//! `cargo test -p application --release --test milestone13_large_scale -- --ignored --nocapture`

use application::ScannerApplicationService;
use knowledge::{
    IdentityOccurrence, IdentityResolutionPolicy, IdentityType, SignalKind, generate_candidates,
};
use persistence::{
    Database, DatabaseKey, InventorySort, LargeScaleFixtureConfig, MonitoringRootStatus,
    ReviewReasonFilter, ReviewStatusFilter, RootMonitoringConfiguration, database_file_size,
};
use platform::{ChangeHint, ChangeScope, LocalEventKind, ReadOnlyPlatform};
use search::{
    AnnIndexMeta, AnnSearchPolicy, GRANITE_EMBEDDING_DIMENSIONS, PersistentAnnIndex, SearchQuery,
    normalize_vector,
};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const SMOKE_FILES: usize = 2_000;
const FULL_FILES: usize = 100_000;

fn peak_rss() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        let kb = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()?;
        Some(kb.saturating_mul(1024))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_024 * 1_024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1_024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(platform_macos::MacOsPlatform)
    }
    #[cfg(not(target_os = "macos"))]
    {
        compile_error!("M13 scale harness currently expects macOS native platform helpers");
    }
}

fn synthetic_vector(seed: u64) -> Vec<f32> {
    let mut values = vec![0.0_f32; GRANITE_EMBEDDING_DIMENSIONS];
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    for value in &mut values {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let unit = (state >> 33) as f32 / (u32::MAX as f32);
        *value = unit * 2.0 - 1.0;
    }
    normalize_vector(&mut values);
    values
}

struct ScaleRun {
    files: usize,
    label: &'static str,
}

fn run_scale_qualification(run: ScaleRun) {
    let temp = TempDir::new().expect("temp");
    let db_path = temp.path().join("m13-scale.db");
    let ann_dir = temp.path().join("ann");
    fs::create_dir_all(&ann_dir).expect("ann dir");
    let key = DatabaseKey::from_bytes([13; 32]);

    let open_started = Instant::now();
    let database = Arc::new(Database::open(&db_path, &key).expect("db open"));
    let first_open_ms = open_started.elapsed().as_millis();

    let config = LargeScaleFixtureConfig {
        file_count: run.files,
        identity_count: (run.files / 40).max(50),
        project_count: (run.files / 125).max(20),
        review_item_target: (run.files / 8).max(100),
        vector_file_count: run.files,
        root_label: "m13-scale".to_owned(),
        root_path: temp.path().join("synthetic-root"),
    };

    let seed_rss_before = peak_rss();
    let fixture = database
        .seed_large_scale_fixture(&config)
        .unwrap_or_else(|error| panic!("seed: {error}"));
    let seed_rss_after = peak_rss();
    let db_bytes = database_file_size(&db_path);

    database
        .set_current_workspace(fixture.workspace.id)
        .expect("set workspace");

    let restore_started = Instant::now();
    let restored = database
        .restore_current_workspace()
        .expect("restore")
        .expect("workspace");
    assert_eq!(restored.id, fixture.workspace.id);
    let restore_ms = restore_started.elapsed().as_millis();

    let dashboard_started = Instant::now();
    let _dashboard = database
        .monitoring_dashboard_counts(fixture.workspace.id)
        .expect("dashboard");
    let dashboard_ms = dashboard_started.elapsed().as_millis();

    let inventory_started = Instant::now();
    let inventory = database
        .scan_files(fixture.scan_id, InventorySort::Filename, false, 500, 0)
        .expect("inventory");
    let inventory_ms = inventory_started.elapsed().as_millis();
    assert!(inventory.len() <= 500);

    let review_started = Instant::now();
    let reviews = database
        .review_items(
            fixture.workspace.id,
            ReviewStatusFilter::NeedsReview,
            ReviewReasonFilter::All,
            50,
            0,
        )
        .expect("review");
    let review_ms = review_started.elapsed().as_millis();
    assert!(reviews.items.len() <= 50);

    let mut lexical_latencies = Vec::new();
    for query_text in [
        "file_000001",
        "invoice",
        "zxqrareterm",
        "supplier invoice vat",
        "photo",
    ] {
        let started = Instant::now();
        let page = database
            .local_search(
                fixture.workspace.id,
                SearchQuery {
                    text: query_text.to_owned(),
                    page: 0,
                    page_size: 50,
                    ..SearchQuery::default()
                },
            )
            .expect("lexical");
        lexical_latencies.push(started.elapsed());
        assert!(page.results.len() <= 50);
    }
    let page10 = database
        .local_search(
            fixture.workspace.id,
            SearchQuery {
                text: "photo".to_owned(),
                page: 9,
                page_size: 50,
                ..SearchQuery::default()
            },
        )
        .expect("page10");
    assert!(page10.results.len() <= 50);
    lexical_latencies.sort();

    let expected = AnnIndexMeta::for_provider(
        "ibm-granite/granite-embedding-97m-multilingual-r2",
        "835ad14087e140460703cf0fae09f97d469d65c2",
        GRANITE_EMBEDDING_DIMENSIONS,
    );
    let ann = PersistentAnnIndex::open_with_expected(&ann_dir, "m13", expected.clone())
        .expect("ann open");
    ann.begin_build().expect("begin");
    ann.reserve_capacity(run.files + 256).expect("reserve");
    let ann_build_started = Instant::now();
    for key in 1..=run.files {
        ann.upsert_vector(key as u64, &synthetic_vector(key as u64))
            .expect("upsert");
    }
    ann.persist_snapshot().expect("persist");
    let ann_build_ms = ann_build_started.elapsed().as_millis();
    let ann_bytes = fs::read_dir(&ann_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum::<u64>();

    let mut ann_latencies = Vec::new();
    for seed in 1..=32_u64 {
        let started = Instant::now();
        let _hits = ann
            .search(&synthetic_vector(seed * 17), AnnSearchPolicy { top_k: 20 })
            .expect("ann search");
        ann_latencies.push(started.elapsed());
    }
    ann_latencies.sort();

    let occurrence_count = (run.files / 4).min(25_000);
    let occurrences = (0..occurrence_count)
        .map(|index| {
            let (name, domain) = if index < occurrence_count / 10 {
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
            .expect("occurrence")
        })
        .collect::<Vec<_>>();
    let rel_started = Instant::now();
    let generation = generate_candidates(&occurrences, IdentityResolutionPolicy::default());
    let rel_ms = rel_started.elapsed().as_millis();
    let theoretical = (occurrence_count as u128) * ((occurrence_count as u128) - 1) / 2;
    assert!(
        (generation.stats.comparisons as u128) * 100 < theoretical,
        "blocking must stay far below all-pairs"
    );

    // Ensure synthetic root path exists so monitoring can inspect the volume.
    fs::create_dir_all(&config.root_path).expect("synthetic root dir");
    database
        .ensure_workspace_monitoring_state(fixture.workspace.id)
        .expect("monitoring state");
    database
        .configure_root_monitoring(
            fixture.root.id,
            RootMonitoringConfiguration {
                enabled: true,
                status: MonitoringRootStatus::Active,
                size_threshold_bytes: 1_048_576,
                startup_entry_limit: 100_000,
            },
        )
        .expect("configure monitoring");

    let service = ScannerApplicationService::new(database.clone(), native_platform());
    let proposal_rss_before = peak_rss();
    let proposal_started = Instant::now();
    let built = service
        .generate_organization_proposal_for_root(
            fixture.workspace.id,
            fixture.root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("proposal generate+persist");
    let proposal_ms = proposal_started.elapsed().as_millis();
    let proposal_rss_after = peak_rss();
    assert_eq!(built.summary.files_analyzed, run.files as u64);

    // UI-bound proposal projection must not materialize every operation/node.
    let ui_started = Instant::now();
    let ui_proposal = database
        .organization_proposal_for_ui(built.id, 500)
        .expect("ui proposal");
    let ui_ms = ui_started.elapsed().as_millis();
    assert!(ui_proposal.operations.len() <= 500);
    assert!(
        ui_proposal
            .nodes
            .iter()
            .all(|node| node.kind != domain::VirtualNodeKind::File)
    );
    // Mostly repeated paths (coalesce) plus a few distinct create/modify/delete/rename hints.
    let mut burst: Vec<ChangeHint> = (0..1_000)
        .map(|_| ChangeHint {
            root_token: "m13-root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("Documents/Taxes/file_000000.pdf")),
            path_before: None,
            kind: LocalEventKind::Modified,
            scope: ChangeScope::File,
        })
        .collect();
    burst.extend([
        ChangeHint {
            root_token: "m13-root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("Documents/Taxes/new_file.pdf")),
            path_before: None,
            kind: LocalEventKind::Created,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "m13-root".to_owned(),
            native_key: None,
            path_after: None,
            path_before: Some(PathBuf::from("Documents/Taxes/file_000001.pdf")),
            kind: LocalEventKind::Removed,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "m13-root".to_owned(),
            native_key: Some(vec![7; 8]),
            path_after: Some(PathBuf::from("Documents/Taxes/renamed.pdf")),
            path_before: Some(PathBuf::from("Documents/Taxes/file_000002.pdf")),
            kind: LocalEventKind::Moved,
            scope: ChangeScope::File,
        },
        ChangeHint {
            root_token: "m13-root".to_owned(),
            native_key: Some(vec![8; 8]),
            path_after: Some(PathBuf::from("Documents/Taxes-renamed")),
            path_before: Some(PathBuf::from("Documents/Taxes")),
            kind: LocalEventKind::Moved,
            scope: ChangeScope::Directory,
        },
    ]);
    let burst_started = Instant::now();
    let persisted_events = service
        .record_monitoring_hints(fixture.workspace.id, fixture.root.id, &burst)
        .expect("burst");
    let burst_ms = burst_started.elapsed().as_millis();
    let queued = service
        .monitoring_dashboard(fixture.workspace.id)
        .expect("dash")
        .counts
        .pending_jobs;

    let one_file_started = Instant::now();
    ann.upsert_vector(
        (run.files as u64) + 1,
        &synthetic_vector((run.files as u64) + 1),
    )
    .expect("one upsert");
    ann.persist_snapshot().expect("one persist");
    let one_file_ms = one_file_started.elapsed().as_millis();

    let batch100_started = Instant::now();
    for key in (run.files as u64 + 2)..(run.files as u64 + 102) {
        ann.upsert_vector(key, &synthetic_vector(key))
            .expect("batch upsert");
    }
    ann.persist_snapshot().expect("batch persist");
    let batch100_ms = batch100_started.elapsed().as_millis();

    ann.remove_key((run.files as u64) + 1).expect("delete");
    ann.persist_snapshot().expect("delete persist");

    drop(ann);
    drop(service);
    drop(database);

    let second_open_started = Instant::now();
    let database2 = Database::open(&db_path, &key).expect("reopen");
    let second_open_ms = second_open_started.elapsed().as_millis();
    let restored2 = database2
        .restore_current_workspace()
        .expect("restore2")
        .expect("workspace");
    assert_eq!(restored2.id, fixture.workspace.id);
    let files_after = database2
        .scan_files(fixture.scan_id, InventorySort::Filename, false, 1, 0)
        .expect("scan after reopen");
    assert_eq!(files_after.len(), 1);
    // Second open must not require reseeding / full rebuild to serve catalog.
    let reviews_after = database2
        .review_items(
            fixture.workspace.id,
            ReviewStatusFilter::NeedsReview,
            ReviewReasonFilter::All,
            10,
            0,
        )
        .expect("review after reopen");
    assert!(!reviews_after.items.is_empty() || fixture.stats.review_items == 0);

    let ann2 =
        PersistentAnnIndex::open_with_expected(&ann_dir, "m13", expected).expect("ann reload");
    assert!(
        ann2.search(&synthetic_vector(1), AnnSearchPolicy::default())
            .is_ok()
    );

    println!(
        "M13_{} files={} identities={} projects={} reviews={} vectors={} db={} ann={} \
         first_open_ms={} second_open_ms={} restore_ms={} dashboard_ms={} inventory_ms={} review_ms={} \
         lexical_p50_us={} lexical_p95_us={} ann_build_ms={} ann_p50_us={} ann_p95_us={} \
         rel_occurrences={} rel_candidates={} rel_comparisons={} rel_theoretical={} rel_ms={} \
         proposal_ms={} proposal_review={} proposal_conflicts={} proposal_avg_depth={:.2} proposal_max_depth={} \
         ui_proposal_ms={} ui_ops={} ui_nodes={} \
         burst_events={} burst_jobs={} burst_ms={} one_file_ms={} batch100_ms={} \
         rss_seed_before={} rss_seed_after={} rss_proposal_before={} rss_proposal_after={} \
         catalog_ingest_ms={} enrichment_ms={}",
        run.label,
        fixture.stats.files,
        fixture.stats.identities,
        fixture.stats.projects,
        fixture.stats.review_items,
        fixture.stats.vector_rows,
        format_bytes(db_bytes),
        format_bytes(ann_bytes),
        first_open_ms,
        second_open_ms,
        restore_ms,
        dashboard_ms,
        inventory_ms,
        review_ms,
        percentile(&lexical_latencies, 0.50).as_micros(),
        percentile(&lexical_latencies, 0.95).as_micros(),
        ann_build_ms,
        percentile(&ann_latencies, 0.50).as_micros(),
        percentile(&ann_latencies, 0.95).as_micros(),
        occurrence_count,
        generation.stats.candidates,
        generation.stats.comparisons,
        theoretical,
        rel_ms,
        proposal_ms,
        built.summary.needs_review,
        built.summary.conflicts,
        built.summary.average_depth,
        built.summary.maximum_depth,
        ui_ms,
        ui_proposal.operations.len(),
        ui_proposal.nodes.len(),
        persisted_events,
        queued,
        burst_ms,
        one_file_ms,
        batch100_ms,
        seed_rss_before
            .map(format_bytes)
            .unwrap_or_else(|| "n/a".into()),
        seed_rss_after
            .map(format_bytes)
            .unwrap_or_else(|| "n/a".into()),
        proposal_rss_before
            .map(format_bytes)
            .unwrap_or_else(|| "n/a".into()),
        proposal_rss_after
            .map(format_bytes)
            .unwrap_or_else(|| "n/a".into()),
        fixture.stats.catalog_ingest_ms,
        fixture.stats.enrichment_ms,
    );

    assert_eq!(fixture.stats.files, run.files as u64);
    assert!(fixture.stats.review_items > 0);
    assert!(queued <= 64, "monitoring burst should coalesce");
}

#[test]
fn m13_smoke_scale_two_thousand_is_bounded() {
    run_scale_qualification(ScaleRun {
        files: SMOKE_FILES,
        label: "SMOKE_2K",
    });
}

#[test]
#[ignore = "expensive 100k qualification; run with --ignored --release"]
fn m13_full_scale_one_hundred_thousand_qualification() {
    run_scale_qualification(ScaleRun {
        files: FULL_FILES,
        label: "FULL_100K",
    });
}

#[test]
fn m13_identity_detail_occurrence_cap_and_proposal_ui_bound() {
    let database =
        Arc::new(Database::open_in_memory(&DatabaseKey::from_bytes([31; 32])).expect("open"));
    let fixture = database
        .seed_large_scale_fixture(&LargeScaleFixtureConfig {
            file_count: 400,
            identity_count: 5,
            project_count: 2,
            review_item_target: 40,
            vector_file_count: 50,
            ..LargeScaleFixtureConfig::default()
        })
        .expect("seed");
    let service = ScannerApplicationService::new(database.clone(), native_platform());
    let built = service
        .generate_organization_proposal_for_root(
            fixture.workspace.id,
            fixture.root.id,
            false,
            &|| false,
            &mut |_| {},
        )
        .expect("proposal");
    let ui = database
        .organization_proposal_for_ui(built.id, 100)
        .expect("ui");
    assert!(ui.operations.len() <= 100);
    assert!(
        ui.nodes
            .iter()
            .all(|node| node.kind != domain::VirtualNodeKind::File)
    );
    assert_eq!(ui.summary.files_analyzed, 400);
}
