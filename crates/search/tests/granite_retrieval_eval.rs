//! REAL Granite ONNX retrieval quality gate (M9.1 Step 3).
//!
//! Uses OnnxLocalEmbeddingProvider — NOT DeterministicTestEmbeddingProvider.
//! Requires provisioned assets under SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR or
//! `.local-models/granite-embedding-97m-multilingual-r2` relative to workspace.

#[path = "granite_retrieval_dataset.rs"]
mod granite_retrieval_dataset;

use granite_retrieval_dataset::{docs, queries};
use search::{
    AnnIndexMeta, AnnSearchPolicy, EmbeddingInput, GRANITE_EMBEDDING_DIMENSIONS,
    GRANITE_EMBEDDING_MODEL_ID, GRANITE_EMBEDDING_REVISION, LocalEmbeddingProvider,
    OnnxLocalEmbeddingProvider, PersistentAnnIndex, QueryClock, interpret_query,
};
use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    time::Instant,
};
use tempfile::TempDir;

fn model_source() -> Option<PathBuf> {
    if let Some(value) = env::var_os("SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR") {
        return Some(PathBuf::from(value));
    }
    let local = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local-models/granite-embedding-97m-multilingual-r2");
    if local.join("tokenizer.json").is_file() && local.join("onnx/model_quint8_avx2.onnx").is_file()
    {
        Some(local)
    } else {
        None
    }
}

fn lexical_rank(doc_texts: &HashMap<&str, &str>, query: &str) -> Vec<String> {
    let tokens = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 4)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut scored = doc_texts
        .iter()
        .map(|(id, text)| {
            let hay = text.to_lowercase();
            let score = tokens
                .iter()
                .filter(|token| hay.contains(token.as_str()))
                .count() as f32;
            ((*id).to_owned(), score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored
        .into_iter()
        .filter(|(_, score)| *score > 0.0)
        .map(|(id, _)| id)
        .collect()
}

fn recall_at(ranked: &[String], relevant: &HashSet<&str>, k: usize) -> f32 {
    if relevant.is_empty() {
        return 0.0;
    }
    if ranked
        .iter()
        .take(k)
        .any(|id| relevant.contains(id.as_str()))
    {
        1.0
    } else {
        0.0
    }
}

fn mrr(ranked: &[String], relevant: &HashSet<&str>) -> f32 {
    for (index, id) in ranked.iter().enumerate() {
        if relevant.contains(id.as_str()) {
            return 1.0 / (index as f32 + 1.0);
        }
    }
    0.0
}

fn ndcg_at(ranked: &[String], relevant: &HashSet<&str>, k: usize) -> f32 {
    let mut dcg = 0.0_f32;
    for (index, id) in ranked.iter().take(k).enumerate() {
        if relevant.contains(id.as_str()) {
            dcg += 1.0 / ((index as f32) + 2.0).log2();
        }
    }
    let ideal = relevant.len().min(k) as f32;
    if ideal <= 0.0 {
        return 0.0;
    }
    let mut idcg = 0.0_f32;
    for index in 0..ideal as usize {
        idcg += 1.0 / ((index as f32) + 2.0).log2();
    }
    if idcg <= f32::EPSILON {
        0.0
    } else {
        dcg / idcg
    }
}

fn peak_rss() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p"])
            .arg(std::process::id().to_string())
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

#[test]
fn real_granite_labeled_retrieval_and_ann_integration() {
    let Some(source) = model_source() else {
        eprintln!(
            "skipping REAL Granite retrieval gate: model assets not found (set SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR)"
        );
        return;
    };

    let rss_before = peak_rss();
    let temp = TempDir::new().expect("temp");
    let provider = OnnxLocalEmbeddingProvider::new(temp.path().join("model")).expect("provider");
    provider
        .activate_from_directory(&source)
        .unwrap_or_else(|e| panic!("activate Granite: {e}"));

    let docs = docs();
    let queries = queries();
    assert!(queries.len() >= 50, "dataset must have ≥50 queries");
    let doc_map = docs
        .iter()
        .map(|d| (d.id, d.text))
        .collect::<HashMap<_, _>>();

    let expected = AnnIndexMeta::for_provider(
        GRANITE_EMBEDDING_MODEL_ID,
        GRANITE_EMBEDDING_REVISION,
        GRANITE_EMBEDDING_DIMENSIONS,
    );
    let ann = PersistentAnnIndex::open_with_expected(temp.path().join("ann"), "eval", expected)
        .expect("ann");
    ann.begin_build().expect("begin");
    ann.reserve_capacity(docs.len().saturating_mul(2))
        .expect("reserve");

    let mut key_to_doc = HashMap::<u64, String>::new();
    let mut next_key = 1_u64;
    let embed_started = Instant::now();
    let mut chunk_count = 0_usize;
    for doc in &docs {
        let outputs = provider
            .embed_batch(&[EmbeddingInput {
                source_id: doc.id.to_owned(),
                source_kind: "text_chunk".to_owned(),
                text: doc.text.chars().take(1800).collect(),
                start_offset: None,
                end_offset: None,
            }])
            .unwrap_or_else(|e| panic!("embed {}: {e}", doc.id));
        for output in outputs {
            ann.upsert_vector(next_key, &output.values)
                .unwrap_or_else(|e| panic!("upsert: {e}"));
            key_to_doc.insert(next_key, doc.id.to_owned());
            next_key += 1;
            chunk_count += 1;
        }
    }
    let embed_elapsed = embed_started.elapsed();
    ann.persist_snapshot().expect("persist");
    let rss_model_ann = peak_rss();

    // Persist/reload consistency probe
    let reloaded = PersistentAnnIndex::open_with_expected(
        temp.path().join("ann"),
        "eval",
        AnnIndexMeta::for_provider(
            GRANITE_EMBEDDING_MODEL_ID,
            GRANITE_EMBEDDING_REVISION,
            GRANITE_EMBEDDING_DIMENSIONS,
        ),
    )
    .expect("reload");
    let probe_q = provider
        .embed_batch(&[EmbeddingInput {
            source_id: "probe".into(),
            source_kind: "semantic_summary".into(),
            text: "devis toiture Dupont".into(),
            start_offset: None,
            end_offset: None,
        }])
        .expect("probe")
        .remove(0)
        .values;
    let before = ann
        .search(&probe_q, AnnSearchPolicy { top_k: 5 })
        .expect("before");
    let after = reloaded
        .search(&probe_q, AnnSearchPolicy { top_k: 5 })
        .expect("after");
    assert_eq!(
        before.first().map(|h| h.key),
        after.first().map(|h| h.key),
        "persisted Granite ANN reload must keep top hit"
    );

    // Warm latency samples
    let warm_queries = [
        "travaux de toiture chez Dupont",
        "facture Point P matériaux",
        "assurance de la maison",
        "roof renovation Dupont",
        "CV développeur Jean Dupont",
    ];
    let mut cold_ms = None;
    let mut warm_totals = Vec::new();
    let mut warm_embed = Vec::new();
    let mut warm_ann = Vec::new();
    for (index, text) in warm_queries.iter().enumerate() {
        let total_started = Instant::now();
        let embed_started = Instant::now();
        let vector = provider
            .embed_batch(&[EmbeddingInput {
                source_id: "q".into(),
                source_kind: "semantic_summary".into(),
                text: (*text).to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .expect("q")
            .remove(0)
            .values;
        let embed_ms = embed_started.elapsed().as_secs_f64() * 1000.0;
        let ann_started = Instant::now();
        let _ = reloaded
            .search(&vector, AnnSearchPolicy { top_k: 64 })
            .expect("search");
        let ann_ms = ann_started.elapsed().as_secs_f64() * 1000.0;
        let total_ms = total_started.elapsed().as_secs_f64() * 1000.0;
        if index == 0 {
            cold_ms = Some(total_ms);
        } else {
            warm_totals.push(total_ms);
            warm_embed.push(embed_ms);
            warm_ann.push(ann_ms);
        }
    }
    warm_totals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    warm_embed.sort_by(|a, b| a.partial_cmp(b).unwrap());
    warm_ann.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut lex_r1 = 0.0;
    let mut lex_r5 = 0.0;
    let mut lex_r10 = 0.0;
    let mut lex_mrr = 0.0;
    let mut lex_ndcg = 0.0;
    let mut vec_r1 = 0.0;
    let mut vec_r5 = 0.0;
    let mut vec_r10 = 0.0;
    let mut vec_mrr = 0.0;
    let mut vec_ndcg = 0.0;
    let mut hyb_r1 = 0.0;
    let mut hyb_r5 = 0.0;
    let mut hyb_r10 = 0.0;
    let mut hyb_mrr = 0.0;
    let mut hyb_ndcg = 0.0;
    let mut hard_ok = 0_usize;
    let mut hard_total = 0_usize;
    let mut failures = Vec::<String>::new();

    for query in &queries {
        let relevant = query.relevant.iter().copied().collect::<HashSet<_>>();
        let lexical = lexical_rank(&doc_map, query.text);
        let qvec = provider
            .embed_batch(&[EmbeddingInput {
                source_id: "q".into(),
                source_kind: "semantic_summary".into(),
                text: query.text.to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .expect("embed query")
            .remove(0)
            .values;
        let hits = reloaded
            .search(&qvec, AnnSearchPolicy { top_k: 32 })
            .expect("ann");
        let mut best = HashMap::<String, f32>::new();
        for hit in hits {
            if let Some(doc_id) = key_to_doc.get(&hit.key) {
                best.entry(doc_id.clone())
                    .and_modify(|s| *s = (*s).max(hit.similarity))
                    .or_insert(hit.similarity);
            }
        }
        let mut vector = best.into_iter().collect::<Vec<_>>();
        vector.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let vector_ids = vector.into_iter().map(|(id, _)| id).collect::<Vec<_>>();

        // Hybrid ≈ production fusion signals available without the SQLCipher corpus:
        // RRF(lexical, Granite ANN) + structured interpretation boosts (party/amount/type).
        // Structured contribution uses human-facing party/supplier/project labels only
        // (not machine document_type codes that can false-match English words).
        let interpretation = interpret_query(query.text, QueryClock::new(2026, 8, 12), &[]);
        let mut structured_needles = Vec::<String>::new();
        for optional in [
            interpretation.supplier.as_deref(),
            interpretation.customer.as_deref(),
            interpretation.project.as_deref(),
            interpretation.party.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if optional.chars().count() >= 3 {
                structured_needles.push(optional.to_lowercase());
            }
        }
        for chip in &interpretation.chips {
            if matches!(
                chip.kind.as_str(),
                "supplier" | "customer" | "project" | "party"
            ) && chip.label.chars().count() >= 3
            {
                structured_needles.push(chip.label.to_lowercase());
            }
        }
        let mut fused = HashMap::<String, f32>::new();
        for (rank, id) in lexical.iter().enumerate() {
            *fused.entry(id.clone()).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
        }
        for (rank, id) in vector_ids.iter().enumerate() {
            *fused.entry(id.clone()).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
        }
        for (id, text) in &doc_map {
            let hay = text.to_lowercase();
            let mut structured = 0.0_f32;
            for needle in &structured_needles {
                if hay.contains(needle.as_str()) {
                    structured += 0.08;
                }
            }
            if structured > 0.0 {
                *fused.entry((*id).to_owned()).or_default() += structured;
            }
        }
        let mut hybrid = fused.into_iter().collect::<Vec<_>>();
        hybrid.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let hybrid_ids = hybrid.into_iter().map(|(id, _)| id).collect::<Vec<_>>();

        lex_r1 += recall_at(&lexical, &relevant, 1);
        lex_r5 += recall_at(&lexical, &relevant, 5);
        lex_r10 += recall_at(&lexical, &relevant, 10);
        lex_mrr += mrr(&lexical, &relevant);
        lex_ndcg += ndcg_at(&lexical, &relevant, 10);
        vec_r1 += recall_at(&vector_ids, &relevant, 1);
        vec_r5 += recall_at(&vector_ids, &relevant, 5);
        vec_r10 += recall_at(&vector_ids, &relevant, 10);
        vec_mrr += mrr(&vector_ids, &relevant);
        vec_ndcg += ndcg_at(&vector_ids, &relevant, 10);
        hyb_r1 += recall_at(&hybrid_ids, &relevant, 1);
        hyb_r5 += recall_at(&hybrid_ids, &relevant, 5);
        hyb_r10 += recall_at(&hybrid_ids, &relevant, 10);
        hyb_mrr += mrr(&hybrid_ids, &relevant);
        hyb_ndcg += ndcg_at(&hybrid_ids, &relevant, 10);

        if recall_at(&hybrid_ids, &relevant, 5) < 1.0 {
            failures.push(format!(
                "MISS@5 q=`{}` top={:?}",
                query.text,
                hybrid_ids.iter().take(3).collect::<Vec<_>>()
            ));
        }

        for negative in query.hard_negatives {
            hard_total += 1;
            let neg_pos = hybrid_ids.iter().position(|id| id == negative);
            let rel_pos = hybrid_ids
                .iter()
                .position(|id| relevant.contains(id.as_str()));
            if rel_pos.is_some_and(|pos| neg_pos.is_none_or(|neg| pos < neg)) {
                hard_ok += 1;
            }
        }
    }

    let n = queries.len() as f32;
    println!("M9.1_STEP3_REAL_GRANITE_RETRIEVAL");
    println!("queries={}", queries.len());
    println!("docs={}", docs.len());
    println!("chunks_embedded={chunk_count}");
    println!("embed_total_ms={}", embed_elapsed.as_millis());
    println!(
        "chunks_per_sec={:.2}",
        chunk_count as f64 / embed_elapsed.as_secs_f64().max(0.001)
    );
    println!("lexical_recall@1={:.3}", lex_r1 / n);
    println!("lexical_recall@5={:.3}", lex_r5 / n);
    println!("lexical_recall@10={:.3}", lex_r10 / n);
    println!("lexical_mrr={:.3}", lex_mrr / n);
    println!("lexical_ndcg@10={:.3}", lex_ndcg / n);
    println!("granite_recall@1={:.3}", vec_r1 / n);
    println!("granite_recall@5={:.3}", vec_r5 / n);
    println!("granite_recall@10={:.3}", vec_r10 / n);
    println!("granite_mrr={:.3}", vec_mrr / n);
    println!("granite_ndcg@10={:.3}", vec_ndcg / n);
    println!("hybrid_recall@1={:.3}", hyb_r1 / n);
    println!("hybrid_recall@5={:.3}", hyb_r5 / n);
    println!("hybrid_recall@10={:.3}", hyb_r10 / n);
    println!("hybrid_mrr={:.3}", hyb_mrr / n);
    println!("hybrid_ndcg@10={:.3}", hyb_ndcg / n);
    println!("hard_negatives_ok={hard_ok}/{hard_total}");
    println!("cold_first_query_ms={:?}", cold_ms);
    println!(
        "warm_total_median_ms={:.2}",
        warm_totals
            .get(warm_totals.len() / 2)
            .copied()
            .unwrap_or(0.0)
    );
    println!(
        "warm_total_p95_ms={:.2}",
        warm_totals
            .get((warm_totals.len().saturating_sub(1) * 95) / 100)
            .copied()
            .unwrap_or(0.0)
    );
    println!(
        "warm_embed_median_ms={:.2}",
        warm_embed.get(warm_embed.len() / 2).copied().unwrap_or(0.0)
    );
    println!(
        "warm_ann_median_ms={:.2}",
        warm_ann.get(warm_ann.len() / 2).copied().unwrap_or(0.0)
    );
    println!(
        "rss_before={:?} rss_model_ann={:?}",
        rss_before, rss_model_ann
    );
    for failure in failures.iter().take(8) {
        println!("FAILURE_EXAMPLE={failure}");
    }

    // Real-model quality floor: hybrid should beat trivial chance and clear hard negatives mostly.
    assert!(
        hyb_r5 / n >= 0.45,
        "hybrid Recall@5 too low for real Granite gate: {}",
        hyb_r5 / n
    );
    assert!(
        hard_ok * 100 / hard_total.max(1) >= 60,
        "hard-negative ranking too weak: {hard_ok}/{hard_total}"
    );

    // Incremental update with real vectors
    let update_key = next_key;
    let old = provider
        .embed_batch(&[EmbeddingInput {
            source_id: "old".into(),
            source_kind: "text_chunk".into(),
            text: "Ancien sujet : plomberie cuisine uniquement".into(),
            start_offset: None,
            end_offset: None,
        }])
        .expect("old")
        .remove(0)
        .values;
    reloaded
        .upsert_vector(update_key, &old)
        .expect("insert old");
    reloaded.persist_snapshot().expect("p1");
    let new = provider
        .embed_batch(&[EmbeddingInput {
            source_id: "new".into(),
            source_kind: "text_chunk".into(),
            text: "Nouveau sujet : réfection complète couverture zinc Dupont".into(),
            start_offset: None,
            end_offset: None,
        }])
        .expect("new")
        .remove(0)
        .values;
    reloaded.upsert_vector(update_key, &new).expect("update");
    reloaded.persist_snapshot().expect("p2");
    let q = provider
        .embed_batch(&[EmbeddingInput {
            source_id: "q".into(),
            source_kind: "semantic_summary".into(),
            text: "travaux toiture zinc Dupont".into(),
            start_offset: None,
            end_offset: None,
        }])
        .expect("q")
        .remove(0)
        .values;
    let hits = reloaded
        .search(&q, AnnSearchPolicy { top_k: 5 })
        .expect("search update");
    assert!(
        hits.iter().any(|hit| hit.key == update_key),
        "updated Granite vector should surface for new meaning"
    );
    reloaded.remove_key(update_key).expect("delete");
    reloaded.persist_snapshot().expect("p3");
    let hits_after_delete = reloaded
        .search(&q, AnnSearchPolicy { top_k: 10 })
        .expect("search delete");
    assert!(hits_after_delete.iter().all(|hit| hit.key != update_key));
}

#[test]
fn model_install_rejects_arbitrary_url_and_supports_offline_register() {
    let temp = TempDir::new().expect("temp");
    let manager = search::LocalEmbeddingModelManager::new(temp.path()).expect("mgr");
    assert!(search::validate_pinned_download_url("https://evil.example/model.onnx").is_err());
    let Some(source) = model_source() else {
        eprintln!("skipping offline register portion: no local model");
        return;
    };
    let state = manager
        .register_from_directory(&source)
        .expect("offline register");
    assert_eq!(state.status, search::EmbeddingModelStatus::Ready);
    manager.remove().expect("remove");
    let after = manager.verify().expect("verify after remove");
    assert_eq!(after.status, search::EmbeddingModelStatus::NotInstalled);
}

fn _ensure_path_type(_: &Path) {}
