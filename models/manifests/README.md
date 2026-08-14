# Local embedding model packaging (M9.1)

## Production assets

Pinned model: `ibm-granite/granite-embedding-97m-multilingual-r2`  
Revision: `835ad14087e140460703cf0fae09f97d469d65c2`  
Manifest: `granite-embedding-97m-multilingual-r2.v1.json`

Install locations (application-controlled only):

- macOS / Windows desktop: `<app_local_data>/models/embeddings/<model_id>/`
- ANN snapshots: `<app_local_data>/models/embeddings/ann/`

## Install paths

1. **Production (normal user):** UI action “Activer la recherche sémantique locale” downloads only pinned HTTPS assets declared by the backend manifest (Hugging Face resolve URLs for the fixed revision). No arbitrary frontend URL or path is accepted.
2. **Offline / developer:** set `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR` to a directory containing `onnx/model_quint8_avx2.onnx` and `tokenizer.json`, then activate — assets are copied and checksum-verified into app storage.

## Runtime packaging

| Component | Packaging strategy |
| --- | --- |
| ONNX Runtime (`ort`) | Cargo feature `download-binaries` vendors the platform native ORT library at **build time**. Release builds do not require a machine-wide ORT install. |
| USearch | Compiled into the Rust binary via `cxx` (no separate daemon / sidecar). |
| Model weights + tokenizer | Data assets (~118 MiB), installed after explicit user consent, verified by SHA-256 + size. |
| ANN index | Local USearch snapshot + SQLite chunk↔key mapping under app data. |

## Windows / macOS notes

- Windows-target compile of `search` / desktop crates is supported where the Rust toolchain target is installed.
- Native Windows ORT/USearch **runtime** execution remains separately qualified when a Windows host is available.
- Architecture-specific ORT binaries are selected by the `ort` build script for the active target triple.
- Qualification harness: `npm run windows:qualification` (see `docs/qualification/windows.md`). On non-Windows hosts the SEMANTIC / ORT sections are reported `NOT RUN`, never auto-`PASS`.

## Privacy

Model download traffic (if used) contacts only the pinned host/path and never uploads user documents, queries, embeddings, or paths.