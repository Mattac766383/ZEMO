//! M9.1 Step 2 — 100k-file ANN scale qualification.
//!
//! This harness measures persistent HNSW insert/query/incremental behavior using
//! deterministic normalized 384-d vectors (production dimensions). It does NOT
//! claim semantic quality; quality is evaluated separately with real/test embeddings.

use search::{
    AnnIndexMeta, AnnIndexStatus, AnnSearchPolicy, GRANITE_EMBEDDING_DIMENSIONS,
    PersistentAnnIndex, normalize_vector,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const FILES: usize = 100_000;
/// Mean chunks/file for this synthetic catalog. Production chunking often yields
/// more; this keeps the ANN stress at ≥100k vectors while remaining desktop-practical.
const CHUNKS_PER_FILE_AVG: f64 = 1.25;

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

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn peak_rss() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p"])
            .arg(std::process::id().to_string())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let kb = text.trim().parse::<u64>().ok()?;
        Some(kb.saturating_mul(1024))
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())?;
                return Some(kb.saturating_mul(1024));
            }
        }
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0_u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

#[test]
fn ann_scale_100k_build_query_incremental() {
    let temp = TempDir::new().expect("temp");
    let rss_before = peak_rss();
    let expected = AnnIndexMeta::for_provider(
        "ibm-granite/granite-embedding-97m-multilingual-r2",
        "835ad14087e140460703cf0fae09f97d469d65c2",
        GRANITE_EMBEDDING_DIMENSIONS,
    );
    // Production default remains connectivity=16; the scale harness uses the same
    // algorithm/metric/dimensions with identical policy versioning fields.
    let index =
        PersistentAnnIndex::open_with_expected(temp.path(), "ws-100k", expected).expect("open");
    index.begin_build().expect("begin");

    let vector_count = (FILES as f64 * CHUNKS_PER_FILE_AVG).round() as usize;
    index
        .reserve_capacity(vector_count.saturating_add(256))
        .expect("reserve");

    let chunk_started = Instant::now();
    // Sample fabrication cost, then scale to full corpus (avoid doubling wall time).
    let sample = 2_000_usize.min(vector_count);
    for key in 1..=sample {
        let _ = synthetic_vector(key as u64);
    }
    let sample_ms = chunk_started.elapsed();
    let chunk_ms = sample_ms
        .checked_mul(vector_count as u32 / sample.max(1) as u32)
        .unwrap_or(sample_ms);

    let embed_ms = Duration::ZERO; // pre-generated vectors — not real model inference

    let build_started = Instant::now();
    for key in 1..=vector_count {
        let vector = synthetic_vector(key as u64);
        index.upsert_vector(key as u64, &vector).expect("upsert");
    }
    let build_ms = build_started.elapsed();

    let persist_started = Instant::now();
    index.persist_snapshot().expect("persist");
    let persist_ms = persist_started.elapsed();
    assert_eq!(index.status(), AnnIndexStatus::Ready);

    let load_started = Instant::now();
    let reloaded = PersistentAnnIndex::open_with_expected(
        temp.path(),
        "ws-100k",
        AnnIndexMeta::for_provider(
            "ibm-granite/granite-embedding-97m-multilingual-r2",
            "835ad14087e140460703cf0fae09f97d469d65c2",
            GRANITE_EMBEDDING_DIMENSIONS,
        ),
    )
    .expect("reload");
    let load_ms = load_started.elapsed();
    assert_eq!(reloaded.status(), AnnIndexStatus::Ready);

    let mut ann_latencies = Vec::new();
    let mut total_latencies = Vec::new();
    for query_seed in 0..64_u64 {
        let query = synthetic_vector(1_000_000 + query_seed);
        let total_started = Instant::now();
        let ann_started = Instant::now();
        let hits = reloaded
            .search(&query, AnnSearchPolicy { top_k: 64 })
            .expect("search");
        let ann_elapsed = ann_started.elapsed();
        assert!(!hits.is_empty());
        ann_latencies.push(ann_elapsed);
        total_latencies.push(total_started.elapsed());
    }
    ann_latencies.sort();
    total_latencies.sort();

    // Incremental: 1 insert, 100 inserts, 1 update, 1 delete — no full rebuild.
    let one_key = (vector_count as u64).saturating_add(1);
    let one_started = Instant::now();
    reloaded
        .upsert_vector(one_key, &synthetic_vector(one_key))
        .expect("one insert");
    reloaded.persist_snapshot().expect("persist one");
    let one_insert_ms = one_started.elapsed();

    let batch_started = Instant::now();
    for offset in 1..=100_u64 {
        let key = one_key + offset;
        reloaded
            .upsert_vector(key, &synthetic_vector(key))
            .expect("batch insert");
    }
    reloaded.persist_snapshot().expect("persist batch");
    let batch_insert_ms = batch_started.elapsed();

    let update_started = Instant::now();
    reloaded
        .upsert_vector(one_key, &synthetic_vector(one_key.wrapping_mul(7)))
        .expect("update");
    reloaded.persist_snapshot().expect("persist update");
    let update_ms = update_started.elapsed();

    let delete_started = Instant::now();
    reloaded.remove_key(one_key).expect("delete");
    reloaded.persist_snapshot().expect("persist delete");
    let delete_ms = delete_started.elapsed();

    let disk = dir_size(temp.path());
    let rss_peak = peak_rss();
    let total = chunk_ms + embed_ms + build_ms + persist_ms;

    println!("M9.1_STEP2_100K_QUALIFICATION");
    println!("files={FILES}");
    println!("vectors={vector_count}");
    println!("chunk_generation_ms={}", chunk_ms.as_millis());
    println!(
        "embedding_ms={} (deterministic pre-generated; not real-model)",
        embed_ms.as_millis()
    );
    println!("ann_build_ms={}", build_ms.as_millis());
    println!("persistence_ms={}", persist_ms.as_millis());
    println!("load_existing_ms={}", load_ms.as_millis());
    println!("total_ms={}", total.as_millis());
    println!("index_disk_bytes={disk}");
    println!(
        "peak_rss_bytes={}",
        rss_peak
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_owned())
    );
    println!(
        "rss_before_bytes={}",
        rss_before
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_owned())
    );
    println!(
        "ann_median_us={}",
        percentile(&ann_latencies, 0.50).as_micros()
    );
    println!(
        "ann_p95_us={}",
        percentile(&ann_latencies, 0.95).as_micros()
    );
    println!(
        "total_query_median_us={}",
        percentile(&total_latencies, 0.50).as_micros()
    );
    println!(
        "total_query_p95_us={}",
        percentile(&total_latencies, 0.95).as_micros()
    );
    println!("one_insert_ms={}", one_insert_ms.as_millis());
    println!("batch_100_insert_ms={}", batch_insert_ms.as_millis());
    println!("one_update_ms={}", update_ms.as_millis());
    println!("one_delete_ms={}", delete_ms.as_millis());

    // Architecture assertions: persisted, reloadable, incremental without clear().
    assert!(disk > 0);
    assert!(percentile(&ann_latencies, 0.95) < Duration::from_secs(2));
    assert_eq!(reloaded.status(), AnnIndexStatus::Ready);
}
