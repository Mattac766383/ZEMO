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

struct StageLog {
    failed: Vec<String>,
}

impl StageLog {
    fn new() -> Self {
        Self { failed: Vec::new() }
    }

    fn run(&mut self, name: &str, action: impl FnOnce() -> Result<(), String>) {
        match action() {
            Ok(()) => eprintln!("STAGE {name}: PASS"),
            Err(error) => {
                eprintln!("STAGE {name}: FAIL: {error}");
                self.failed.push(format!("{name}: {error}"));
            }
        }
    }

    fn skip(&mut self, name: &str, reason: &str) {
        eprintln!("STAGE {name}: FAIL: skipped ({reason})");
        self.failed.push(format!("{name}: skipped ({reason})"));
    }

    fn finish(self) {
        assert!(
            self.failed.is_empty(),
            "semantic stages failed:\n{}",
            self.failed.join("\n")
        );
    }
}

#[test]
fn windows_ort_granite_install_embed_ann_remove_and_lexical_fallback() {
    let mut stages = StageLog::new();
    let source = model_source();
    let onnx = source.join("onnx/model_quint8_avx2.onnx");
    let tokenizer = source.join("tokenizer.json");
    eprintln!("semantic source dir: {}", source.display());
    eprintln!("arch: {}", env::consts::ARCH);
    eprintln!(
        "dll search: PATH length {}",
        env::var("PATH").unwrap_or_default().len()
    );

    stages.run("MODEL ASSETS", || {
        if !onnx.is_file() {
            return Err(format!("missing onnx model {}", onnx.display()));
        }
        if !tokenizer.is_file() {
            return Err(format!("missing tokenizer {}", tokenizer.display()));
        }
        Ok(())
    });
    if !stages.failed.is_empty() {
        stages.finish();
        return;
    }

    let model_root = m15_temp("supremacy-m15-sandbox-model-");
    let ann_root = m15_temp("supremacy-m15-sandbox-ann-");
    let provider = match OnnxLocalEmbeddingProvider::new(model_root.path()) {
        Ok(provider) => provider,
        Err(error) => {
            stages.skip("CHECKSUM", &error.to_string());
            stages.finish();
            return;
        }
    };

    stages.run("CHECKSUM", || {
        provider
            .activate_from_directory(&source)
            .map_err(|error| error.to_string())?;
        provider
            .verify_installed()
            .map_err(|error| error.to_string())?;
        Ok(())
    });

    let model_path = provider
        .manager()
        .asset_path("onnx_model")
        .unwrap_or_else(|_| model_root.path().join("missing-onnx"));
    stages.run("MODEL PATH", || {
        if !model_path.is_file() {
            return Err(format!(
                "installed model missing at {}",
                model_path.display()
            ));
        }
        eprintln!("resolved model path: {}", model_path.display());
        Ok(())
    });

    let mut invoice = Vec::new();
    let mut related = Vec::new();
    let mut beach = Vec::new();
    if stages
        .failed
        .iter()
        .any(|item| item.starts_with("CHECKSUM"))
    {
        stages.skip("ORT LOAD", "checksum/install failed");
        stages.skip("TOKENIZER", "checksum/install failed");
        stages.skip("ONNX SESSION", "checksum/install failed");
        stages.skip("GRANITE EMBEDDING", "checksum/install failed");
        stages.skip("DIMENSION CHECK", "checksum/install failed");
    } else {
        stages.run("ORT LOAD", || {
            provider
                .embed_batch(&[EmbeddingInput {
                    source_id: "windows-qual".to_owned(),
                    source_kind: "semantic_summary".to_owned(),
                    text: "facture fournisseur matériaux".to_owned(),
                    start_offset: None,
                    end_offset: None,
                }])
                .map(|mut batch| {
                    invoice = batch.remove(0).values;
                })
                .map_err(|error| {
                    format!(
                        "ORT/tokenizer/session/embed failed: {error}; model={}",
                        model_path.display()
                    )
                })
        });
        if stages
            .failed
            .iter()
            .any(|item| item.starts_with("ORT LOAD"))
        {
            stages.skip("TOKENIZER", "ORT load failed");
            stages.skip("ONNX SESSION", "ORT load failed");
            stages.skip("GRANITE EMBEDDING", "ORT load failed");
            stages.skip("DIMENSION CHECK", "ORT load failed");
        } else {
            eprintln!("STAGE TOKENIZER: PASS");
            eprintln!("STAGE ONNX SESSION: PASS");
            stages.run("GRANITE EMBEDDING", || {
                related = provider
                    .embed_batch(&[EmbeddingInput {
                        source_id: "windows-qual".to_owned(),
                        source_kind: "semantic_summary".to_owned(),
                        text: "achat matériaux fournisseur".to_owned(),
                        start_offset: None,
                        end_offset: None,
                    }])
                    .map_err(|error| error.to_string())?
                    .remove(0)
                    .values;
                beach = provider
                    .embed_batch(&[EmbeddingInput {
                        source_id: "windows-qual".to_owned(),
                        source_kind: "semantic_summary".to_owned(),
                        text: "photo de vacances à la plage".to_owned(),
                        start_offset: None,
                        end_offset: None,
                    }])
                    .map_err(|error| error.to_string())?
                    .remove(0)
                    .values;
                if cosine_similarity_quantized(&invoice, &quantize_unit_vector(&related))
                    <= cosine_similarity_quantized(&invoice, &quantize_unit_vector(&beach))
                {
                    return Err(
                        "related French invoice did not outrank unrelated beach text".to_owned(),
                    );
                }
                Ok(())
            });
            stages.run("DIMENSION CHECK", || {
                if invoice.len() != 384 {
                    return Err(format!("expected 384 dimensions, got {}", invoice.len()));
                }
                Ok(())
            });
        }
    }

    let mut index_ready = false;
    stages.run("USEARCH LOAD", || {
        if ANN_LIBRARY != "usearch" || ANN_LIBRARY_VERSION != "2.26.0" {
            return Err(format!(
                "unexpected ANN library {ANN_LIBRARY} {ANN_LIBRARY_VERSION}"
            ));
        }
        Ok(())
    });
    match PersistentAnnIndex::open(ann_root.path(), "windows-qual") {
        Ok(index) => {
            stages.run("INDEX CREATE", || {
                index.begin_build().map_err(|error| error.to_string())
            });
            if invoice.len() == 384 && beach.len() == 384 && related.len() == 384 {
                stages.run("INSERT", || {
                    index
                        .upsert_vector(1, &invoice)
                        .map_err(|error| error.to_string())?;
                    index
                        .upsert_vector(2, &beach)
                        .map_err(|error| error.to_string())?;
                    Ok(())
                });
                stages.run("PERSIST", || {
                    index
                        .persist_snapshot()
                        .map_err(|error| error.to_string())?;
                    if index.status() != AnnIndexStatus::Ready {
                        return Err(format!("persist status {:?}", index.status()));
                    }
                    index_ready = true;
                    Ok(())
                });
            } else {
                stages.skip("INSERT", "no embeddings");
                stages.skip("PERSIST", "no embeddings");
            }
        }
        Err(error) => {
            stages.skip("INDEX CREATE", &error);
            stages.skip("INSERT", "index open failed");
            stages.skip("PERSIST", "index open failed");
        }
    }
    if index_ready && related.len() == 384 {
        stages.run("RELOAD", || {
            let reloaded = PersistentAnnIndex::open(ann_root.path(), "windows-qual")
                .map_err(|error| error.to_string())?;
            if reloaded.status() != AnnIndexStatus::Ready {
                return Err(format!("reload status {:?}", reloaded.status()));
            }
            Ok(())
        });
        stages.run("QUERY", || {
            let reloaded = PersistentAnnIndex::open(ann_root.path(), "windows-qual")
                .map_err(|error| error.to_string())?;
            let hits = reloaded
                .search(&related, AnnSearchPolicy { top_k: 2 })
                .map_err(|error| error.to_string())?;
            if hits.is_empty() {
                return Err("usearch query returned no hits".to_owned());
            }
            if hits[0].key != 1 {
                return Err(format!("expected top hit 1, got {}", hits[0].key));
            }
            Ok(())
        });
    } else {
        stages.skip("RELOAD", "index not persisted");
        stages.skip("QUERY", "index not persisted");
    }

    stages.run("MODEL REMOVE", || {
        provider.remove_model().map_err(|error| error.to_string())
    });
    stages.run("LEXICAL FALLBACK", || {
        if provider
            .embed_batch(&[EmbeddingInput {
                source_id: "windows-qual".to_owned(),
                source_kind: "semantic_summary".to_owned(),
                text: "should fail closed".to_owned(),
                start_offset: None,
                end_offset: None,
            }])
            .is_ok()
        {
            return Err("embedding succeeded after model removal".to_owned());
        }
        if index_ready {
            let entries = fs::read_dir(ann_root.path()).map_err(|error| error.to_string())?;
            let found = entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains("usearch"));
            if !found {
                return Err("USearch snapshot missing after model removal".to_owned());
            }
        }
        Ok(())
    });

    stages.finish();
}
