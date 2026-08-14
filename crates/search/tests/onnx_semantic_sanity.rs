//! Real-model semantic sanity. Requires provisioned assets:
//! SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR=/path/to/model/root
//! where the root contains onnx/model_quint8_avx2.onnx and tokenizer.json.

use search::{
    EmbeddingInput, LocalEmbeddingProvider, OnnxLocalEmbeddingProvider,
    cosine_similarity_quantized, quantize_unit_vector,
};
use std::env;
use tempfile::TempDir;

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    cosine_similarity_quantized(a, &quantize_unit_vector(b))
}

#[test]
fn real_provider_french_and_cross_language_sanity() {
    let Some(source) = env::var_os("SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR") else {
        eprintln!("skipping real ONNX sanity: SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR unset");
        return;
    };
    let dir = TempDir::new().expect("temp");
    let provider = OnnxLocalEmbeddingProvider::new(dir.path()).expect("provider");
    provider
        .activate_from_directory(std::path::Path::new(&source))
        .unwrap_or_else(|error| panic!("activate model: {error}"));

    let embed = |text: &str| {
        provider
            .embed_batch(&[EmbeddingInput {
                source_id: "t".to_owned(),
                source_kind: "semantic_summary".to_owned(),
                text: text.to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .unwrap_or_else(|error| panic!("embed `{text}`: {error}"))
            .remove(0)
            .values
    };

    let invoice_a = embed("facture fournisseur pour matériaux de construction");
    let invoice_b = embed("achat de matériaux auprès du fournisseur");
    let beach = embed("photo de vacances à la plage");
    assert!(
        cosine(&invoice_a, &invoice_b) > cosine(&invoice_a, &beach),
        "related French invoices should outrank beach photo"
    );

    let fr = embed("facture de rénovation");
    let en = embed("renovation invoice");
    let unrelated = embed("recette de cuisine italienne");
    assert!(
        cosine(&fr, &en) > cosine(&fr, &unrelated),
        "French/English renovation pair should outrank unrelated text"
    );
    assert_eq!(invoice_a.len(), 384);
}
