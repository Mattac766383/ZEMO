#![cfg(windows)]

//! Windows ORT / Granite / USearch runtime qualification.
//! Requires `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR` with real Granite assets.
//! Missing model => test failure (harness maps unset env to NOT RUN before invoke).

use search::{
    ANN_LIBRARY, ANN_LIBRARY_VERSION, AnnIndexStatus, AnnSearchPolicy, EmbeddingInput,
    LocalEmbeddingProvider, OnnxLocalEmbeddingProvider, PersistentAnnIndex,
    cosine_similarity_quantized, quantize_unit_vector,
};
use std::{env, fs, path::PathBuf};
use tempfile::{Builder, TempDir};

fn model_source() -> PathBuf {
    env::var_os("SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR must point at real Granite assets for Windows semantic qualification"
            )
        })
}

fn m15_temp(prefix: &str) -> TempDir {
    let dir = Builder::new()
        .prefix(prefix)
        .tempdir()
        .unwrap_or_else(|error| panic!("temp sandbox should be created: {error}"));
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|error| panic!("temp root: {error}"));
    let canonical = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize: {error}"));
    assert!(
        canonical.starts_with(&temporary_root),
        "semantic fixtures must stay under the process temporary root"
    );
    for forbidden in ["Documents", "Desktop", "Downloads"] {
        assert!(
            !canonical
                .components()
                .any(|component| component.as_os_str() == forbidden),
            "must not use profile directory {forbidden}"
        );
    }
    dir
}

#[test]
fn windows_ort_granite_install_embed_ann_remove_and_lexical_fallback() {
    let source = model_source();
    assert!(
        source.join("onnx/model_quint8_avx2.onnx").is_file(),
        "missing onnx model under {}",
        source.display()
    );
    assert!(
        source.join("tokenizer.json").is_file(),
        "missing tokenizer under {}",
        source.display()
    );

    let model_root = m15_temp("supremacy-m15-sandbox-model-");
    let ann_root = m15_temp("supremacy-m15-sandbox-ann-");
    let provider = OnnxLocalEmbeddingProvider::new(model_root.path())
        .unwrap_or_else(|error| panic!("provider: {error}"));

    provider
        .activate_from_directory(&source)
        .unwrap_or_else(|error| panic!("activate/checksum install: {error}"));
    provider
        .verify_installed()
        .unwrap_or_else(|error| panic!("verify installed model: {error}"));

    let embed = |text: &str| {
        provider
            .embed_batch(&[EmbeddingInput {
                source_id: "windows-qual".to_owned(),
                source_kind: "semantic_summary".to_owned(),
                text: text.to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .unwrap_or_else(|error| panic!("embed `{text}`: {error}"))
            .remove(0)
            .values
    };

    let invoice = embed("facture fournisseur matériaux");
    let related = embed("achat matériaux fournisseur");
    let beach = embed("photo de vacances à la plage");
    assert_eq!(invoice.len(), 384);
    assert!(
        cosine_similarity_quantized(&invoice, &quantize_unit_vector(&related))
            > cosine_similarity_quantized(&invoice, &quantize_unit_vector(&beach)),
        "Granite embeddings should rank related French invoices above unrelated beach text"
    );

    let index = PersistentAnnIndex::open(ann_root.path(), "windows-qual")
        .unwrap_or_else(|error| panic!("ann open: {error}"));
    assert_eq!(ANN_LIBRARY, "usearch");
    assert_eq!(ANN_LIBRARY_VERSION, "2.26.0");
    index
        .begin_build()
        .unwrap_or_else(|error| panic!("ann build: {error}"));
    index
        .upsert_vector(1, &invoice)
        .unwrap_or_else(|error| panic!("ann upsert invoice: {error}"));
    index
        .upsert_vector(2, &beach)
        .unwrap_or_else(|error| panic!("ann upsert beach: {error}"));
    index
        .persist_snapshot()
        .unwrap_or_else(|error| panic!("ann persist: {error}"));
    assert_eq!(index.status(), AnnIndexStatus::Ready);

    let reloaded = PersistentAnnIndex::open(ann_root.path(), "windows-qual")
        .unwrap_or_else(|error| panic!("ann reload: {error}"));
    assert_eq!(reloaded.status(), AnnIndexStatus::Ready);
    let hits = reloaded
        .search(&related, AnnSearchPolicy { top_k: 2 })
        .unwrap_or_else(|error| panic!("ann search: {error}"));
    assert!(!hits.is_empty());
    assert_eq!(hits[0].key, 1);

    provider
        .remove_model()
        .unwrap_or_else(|error| panic!("model removal: {error}"));
    assert!(
        provider
            .embed_batch(&[EmbeddingInput {
                source_id: "windows-qual".to_owned(),
                source_kind: "semantic_summary".to_owned(),
                text: "should fail closed".to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .is_err(),
        "embedding must fail closed after model removal so lexical fallback remains authoritative"
    );

    // Persistence path evidence: ANN snapshot files remain under the temp ANN root only.
    let entries = fs::read_dir(ann_root.path())
        .unwrap_or_else(|error| panic!("ann root should enumerate: {error}"))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("ann entries: {error}"));
    assert!(
        entries
            .iter()
            .any(|entry| { entry.file_name().to_string_lossy().contains("usearch") }),
        "USearch snapshot should persist under the Windows ANN storage path"
    );
}
