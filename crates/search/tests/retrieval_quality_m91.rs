//! M9.1 Step 2 — labeled retrieval quality (lexical vs vector/ANN vs hybrid).
//! Uses DeterministicTestEmbeddingProvider (not production ONNX). Metrics measure
//! ranking architecture, not Granite semantic quality.

use search::{
    AnnIndexMeta, AnnSearchPolicy, ApproximateTokenCounter, ChunkingPolicy,
    DeterministicTestEmbeddingProvider, EmbeddingDocument, EmbeddingInput, LocalEmbeddingProvider,
    PersistentAnnIndex, chunk_embedding_document, semantic_chunks_to_embedding_inputs,
};
use std::collections::{HashMap, HashSet};
use tempfile::TempDir;

#[derive(Clone)]
struct LabeledDoc {
    id: &'static str,
    text: &'static str,
}

#[derive(Clone)]
struct LabeledQuery {
    text: &'static str,
    relevant: &'static [&'static str],
    hard_negative: Option<&'static str>,
}

fn dataset() -> (Vec<LabeledDoc>, Vec<LabeledQuery>) {
    let docs = vec![
        LabeledDoc {
            id: "toiture-dupont",
            text: "Travaux de réfection de toiture réalisés pour Dupont SARL chantier couverture",
        },
        LabeledDoc {
            id: "facture-pointp",
            text: "Facture Point P matériaux 1 437,82 € achat fournisseur matériaux construction",
        },
        LabeledDoc {
            id: "assurance-maison",
            text: "Assurance habitation résidence principale contrat assurance maison",
        },
        LabeledDoc {
            id: "cv-dupont",
            text: "Jean Dupont — CV développeur logiciel expérience React Rust",
        },
        LabeledDoc {
            id: "invoice-acme-en",
            text: "Invoice ACME Supplies building materials total 1437.82 EUR",
        },
        LabeledDoc {
            id: "roof-repair-en",
            text: "Roof repair works completed for Dupont Ltd covering project",
        },
        LabeledDoc {
            id: "personal-tax",
            text: "Déclaration impôts revenus personnels année fiscale",
        },
        LabeledDoc {
            id: "leroy-merlin",
            text: "Facture Leroy Merlin outillage jardin 89,90 € fournisseur confirmé Leroy Merlin",
        },
        LabeledDoc {
            id: "short-relevant",
            text: "Couverture Dupont toiture urgente",
        },
    ];
    let queries = vec![
        LabeledQuery {
            text: "chantier de couverture chez Dupont",
            relevant: &["toiture-dupont", "short-relevant", "roof-repair-en"],
            hard_negative: Some("cv-dupont"),
        },
        LabeledQuery {
            text: "achat fournisseur matériaux autour de 1400 euros",
            relevant: &["facture-pointp", "invoice-acme-en"],
            hard_negative: Some("leroy-merlin"),
        },
        LabeledQuery {
            text: "contrat assurance maison",
            relevant: &["assurance-maison"],
            hard_negative: None,
        },
        LabeledQuery {
            text: "roof repair Dupont",
            relevant: &["roof-repair-en", "toiture-dupont"],
            hard_negative: Some("cv-dupont"),
        },
    ];
    (docs, queries)
}

fn lexical_rank(docs: &[LabeledDoc], query: &str) -> Vec<String> {
    let q = query.to_lowercase();
    let tokens = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 3)
        .collect::<Vec<_>>();
    let mut scored = docs
        .iter()
        .map(|doc| {
            let hay = doc.text.to_lowercase();
            let score = tokens.iter().filter(|token| hay.contains(*token)).count() as f32;
            (doc.id.to_owned(), score)
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
    let hit = ranked
        .iter()
        .take(k)
        .any(|id| relevant.contains(id.as_str()));
    if hit { 1.0 } else { 0.0 }
}

fn mrr(ranked: &[String], relevant: &HashSet<&str>) -> f32 {
    for (index, id) in ranked.iter().enumerate() {
        if relevant.contains(id.as_str()) {
            return 1.0 / (index as f32 + 1.0);
        }
    }
    0.0
}

#[test]
fn retrieval_quality_lexical_vector_hybrid() {
    let (docs, queries) = dataset();
    let provider = DeterministicTestEmbeddingProvider::new(384);
    let temp = TempDir::new().expect("temp");
    let expected = AnnIndexMeta::for_provider(
        &provider.descriptor().provider_id,
        &provider.descriptor().version,
        384,
    );
    let index =
        PersistentAnnIndex::open_with_expected(temp.path(), "quality", expected).expect("open");
    index.begin_build().expect("build");

    let policy = ChunkingPolicy::default();
    let counter = ApproximateTokenCounter::from_policy(&policy);
    let mut key_to_doc = HashMap::<u64, String>::new();
    let mut next_key = 1_u64;
    for doc in &docs {
        let chunks = chunk_embedding_document(
            &EmbeddingDocument {
                filename: format!("{}.txt", doc.id),
                semantic_fields: vec![],
                identities: vec![],
                extracted_text: doc.text.to_owned(),
            },
            &policy,
            &counter,
        );
        let inputs = semantic_chunks_to_embedding_inputs(&chunks);
        let outputs = provider.embed_batch(&inputs).expect("embed");
        for output in outputs {
            index
                .upsert_vector(next_key, &output.values)
                .expect("upsert");
            key_to_doc.insert(next_key, doc.id.to_owned());
            next_key += 1;
        }
    }
    index.persist_snapshot().expect("persist");

    let mut lexical_r1 = 0.0;
    let mut lexical_r5 = 0.0;
    let mut lexical_r10 = 0.0;
    let mut lexical_mrr = 0.0;
    let mut vector_r1 = 0.0;
    let mut vector_r5 = 0.0;
    let mut vector_r10 = 0.0;
    let mut vector_mrr = 0.0;
    let mut hybrid_r1 = 0.0;
    let mut hybrid_r5 = 0.0;
    let mut hybrid_r10 = 0.0;
    let mut hybrid_mrr = 0.0;
    let mut hard_negative_ok = 0_usize;
    let mut hard_negative_total = 0_usize;

    for query in &queries {
        let relevant = query.relevant.iter().copied().collect::<HashSet<_>>();
        let lexical = lexical_rank(&docs, query.text);
        let qvec = provider
            .embed_batch(&[EmbeddingInput {
                source_id: "q".to_owned(),
                source_kind: "semantic_summary".to_owned(),
                text: query.text.to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .expect("q")
            .remove(0)
            .values;
        let hits = index
            .search(&qvec, AnnSearchPolicy { top_k: 20 })
            .expect("ann");
        let mut best = HashMap::<String, f32>::new();
        for hit in hits {
            if let Some(doc_id) = key_to_doc.get(&hit.key) {
                best.entry(doc_id.clone())
                    .and_modify(|score| *score = (*score).max(hit.similarity))
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

        // Simple RRF hybrid over lexical + vector ranks.
        let mut fused = HashMap::<String, f32>::new();
        for (rank, id) in lexical.iter().enumerate() {
            *fused.entry(id.clone()).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
        }
        for (rank, id) in vector_ids.iter().enumerate() {
            *fused.entry(id.clone()).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
        }
        let mut hybrid = fused.into_iter().collect::<Vec<_>>();
        hybrid.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let hybrid_ids = hybrid.into_iter().map(|(id, _)| id).collect::<Vec<_>>();

        lexical_r1 += recall_at(&lexical, &relevant, 1);
        lexical_r5 += recall_at(&lexical, &relevant, 5);
        lexical_r10 += recall_at(&lexical, &relevant, 10);
        lexical_mrr += mrr(&lexical, &relevant);
        vector_r1 += recall_at(&vector_ids, &relevant, 1);
        vector_r5 += recall_at(&vector_ids, &relevant, 5);
        vector_r10 += recall_at(&vector_ids, &relevant, 10);
        vector_mrr += mrr(&vector_ids, &relevant);
        hybrid_r1 += recall_at(&hybrid_ids, &relevant, 1);
        hybrid_r5 += recall_at(&hybrid_ids, &relevant, 5);
        hybrid_r10 += recall_at(&hybrid_ids, &relevant, 10);
        hybrid_mrr += mrr(&hybrid_ids, &relevant);

        if let Some(negative) = query.hard_negative {
            hard_negative_total += 1;
            let neg_pos = hybrid_ids.iter().position(|id| id == negative);
            let rel_pos = hybrid_ids
                .iter()
                .position(|id| relevant.contains(id.as_str()));
            if rel_pos.is_some_and(|pos| neg_pos.is_none_or(|neg| pos < neg)) {
                hard_negative_ok += 1;
            }
        }
    }

    let n = queries.len() as f32;
    println!("M9.1_STEP2_RETRIEVAL_QUALITY");
    println!("lexical_recall@1={}", lexical_r1 / n);
    println!("lexical_recall@5={}", lexical_r5 / n);
    println!("lexical_recall@10={}", lexical_r10 / n);
    println!("lexical_mrr={}", lexical_mrr / n);
    println!("vector_recall@1={}", vector_r1 / n);
    println!("vector_recall@5={}", vector_r5 / n);
    println!("vector_recall@10={}", vector_r10 / n);
    println!("vector_mrr={}", vector_mrr / n);
    println!("hybrid_recall@1={}", hybrid_r1 / n);
    println!("hybrid_recall@5={}", hybrid_r5 / n);
    println!("hybrid_recall@10={}", hybrid_r10 / n);
    println!("hybrid_mrr={}", hybrid_mrr / n);
    println!("hard_negatives_ok={hard_negative_ok}/{hard_negative_total}");

    assert!(hybrid_r5 / n >= 0.5, "hybrid Recall@5 too low");
    assert!(hard_negative_ok >= hard_negative_total.saturating_sub(1));
}

#[test]
fn long_document_does_not_outrank_short_by_chunk_count_alone() {
    let provider = DeterministicTestEmbeddingProvider::new(384);
    let temp = TempDir::new().expect("temp");
    let expected = AnnIndexMeta::for_provider(
        &provider.descriptor().provider_id,
        &provider.descriptor().version,
        384,
    );
    let index =
        PersistentAnnIndex::open_with_expected(temp.path(), "bias", expected).expect("open");
    index.begin_build().expect("build");
    let policy = ChunkingPolicy::default();
    let counter = ApproximateTokenCounter::from_policy(&policy);

    let short = "Couverture Dupont toiture urgente";
    let long = format!(
        "{} {}",
        "Conditions générales. ".repeat(400),
        "mention secondaire couverture"
    );
    let mut key_to_doc = HashMap::<u64, &str>::new();
    let mut next = 1_u64;
    for (id, text) in [("short", short.to_owned()), ("long", long)] {
        let chunks = chunk_embedding_document(
            &EmbeddingDocument {
                filename: format!("{id}.txt"),
                semantic_fields: vec![],
                identities: vec![],
                extracted_text: text,
            },
            &policy,
            &counter,
        );
        let inputs = semantic_chunks_to_embedding_inputs(&chunks);
        let outputs = provider.embed_batch(&inputs).expect("embed");
        for output in outputs {
            index.upsert_vector(next, &output.values).expect("add");
            key_to_doc.insert(next, id);
            next += 1;
        }
    }
    index.persist_snapshot().expect("persist");
    let q = provider
        .embed_batch(&[EmbeddingInput {
            source_id: "q".to_owned(),
            source_kind: "semantic_summary".to_owned(),
            text: "chantier couverture Dupont".to_owned(),
            start_offset: None,
            end_offset: None,
        }])
        .expect("q")
        .remove(0)
        .values;
    let hits = index
        .search(&q, AnnSearchPolicy { top_k: 32 })
        .expect("search");
    let mut best = HashMap::<&str, f32>::new();
    for hit in hits {
        if let Some(doc) = key_to_doc.get(&hit.key) {
            best.entry(*doc)
                .and_modify(|s| *s = (*s).max(hit.similarity))
                .or_insert(hit.similarity);
        }
    }
    let short_score = *best.get("short").unwrap_or(&0.0);
    let long_score = *best.get("long").unwrap_or(&0.0);
    assert!(
        short_score >= long_score,
        "short relevant doc should not lose solely due to long chunk count ({short_score} vs {long_score})"
    );
}
