//! Persistent local HNSW ANN index (USearch) for M9.1 Step 2.

use crate::chunking::CHUNKING_POLICY_VERSION;
use crate::model_manager::{
    GRANITE_EMBEDDING_DIMENSIONS, GRANITE_EMBEDDING_MODEL_ID, GRANITE_EMBEDDING_REVISION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

pub const ANN_INDEX_FORMAT_VERSION: u32 = 1;
pub const ANN_LIBRARY: &str = "usearch";
pub const ANN_LIBRARY_VERSION: &str = "2.26.0";
pub const ANN_ALGORITHM: &str = "hnsw";
pub const ANN_METRIC: &str = "cosine";
pub const DEFAULT_ANN_TOP_K: usize = 64;
pub const DEFAULT_ANN_CONNECTIVITY: usize = 16;
/// HNSW construction expansion (efConstruction-equivalent).
pub const DEFAULT_ANN_EXPANSION_ADD: usize = 128;
/// HNSW search expansion (efSearch-equivalent). Kept moderately high for recall.
pub const DEFAULT_ANN_EXPANSION_SEARCH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnIndexStatus {
    NotAvailable,
    Building,
    Ready,
    Degraded,
    RebuildRequired,
    Failed,
}

impl AnnIndexStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotAvailable => "not_available",
            Self::Building => "building",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::RebuildRequired => "rebuild_required",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnIndexMeta {
    pub index_format_version: u32,
    pub embedding_model_id: String,
    pub embedding_model_version: String,
    pub embedding_dimension: usize,
    pub chunking_policy_version: String,
    pub ann_policy_version: String,
    pub library: String,
    pub library_version: String,
    pub algorithm: String,
    pub metric: String,
    pub connectivity: usize,
    #[serde(default = "default_ann_expansion_add")]
    pub expansion_add: usize,
    #[serde(default = "default_ann_expansion_search")]
    pub expansion_search: usize,
    pub vector_count: u64,
    pub status: AnnIndexStatus,
    pub snapshot_sha256: Option<String>,
    pub created_at_unix: u64,
    pub last_error: Option<String>,
}

const fn default_ann_expansion_add() -> usize {
    DEFAULT_ANN_EXPANSION_ADD
}

const fn default_ann_expansion_search() -> usize {
    DEFAULT_ANN_EXPANSION_SEARCH
}

impl Default for AnnIndexMeta {
    fn default() -> Self {
        Self {
            index_format_version: ANN_INDEX_FORMAT_VERSION,
            embedding_model_id: GRANITE_EMBEDDING_MODEL_ID.to_owned(),
            embedding_model_version: GRANITE_EMBEDDING_REVISION.to_owned(),
            embedding_dimension: GRANITE_EMBEDDING_DIMENSIONS,
            chunking_policy_version: CHUNKING_POLICY_VERSION.to_owned(),
            ann_policy_version: "ann-v1-usearch-hnsw-cos".to_owned(),
            library: ANN_LIBRARY.to_owned(),
            library_version: ANN_LIBRARY_VERSION.to_owned(),
            algorithm: ANN_ALGORITHM.to_owned(),
            metric: ANN_METRIC.to_owned(),
            connectivity: DEFAULT_ANN_CONNECTIVITY,
            expansion_add: DEFAULT_ANN_EXPANSION_ADD,
            expansion_search: DEFAULT_ANN_EXPANSION_SEARCH,
            vector_count: 0,
            status: AnnIndexStatus::NotAvailable,
            snapshot_sha256: None,
            created_at_unix: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnSearchPolicy {
    pub top_k: usize,
}

impl Default for AnnSearchPolicy {
    fn default() -> Self {
        Self {
            top_k: DEFAULT_ANN_TOP_K,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnnHit {
    pub key: u64,
    pub similarity: f32,
}

pub struct PersistentAnnIndex {
    root: PathBuf,
    workspace_id: String,
    expected: AnnIndexMeta,
    meta: Mutex<AnnIndexMeta>,
    index: Mutex<Option<Index>>,
}

impl AnnIndexMeta {
    #[must_use]
    pub fn for_provider(provider_id: &str, version: &str, dimensions: usize) -> Self {
        Self {
            embedding_model_id: provider_id.to_owned(),
            embedding_model_version: version.to_owned(),
            embedding_dimension: dimensions.max(1),
            ..Self::default()
        }
    }
}

impl std::fmt::Debug for PersistentAnnIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersistentAnnIndex")
            .field("root", &self.root)
            .field("workspace_id", &self.workspace_id)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl PersistentAnnIndex {
    /// Opens the workspace ANN using production Granite defaults.
    pub fn open(root: impl Into<PathBuf>, workspace_id: &str) -> Result<Self, String> {
        Self::open_with_expected(root, workspace_id, AnnIndexMeta::default())
    }

    /// Opens with an expected model/chunking/ANN policy. Incompatible on-disk
    /// metadata is marked `RebuildRequired` without crashing.
    pub fn open_with_expected(
        root: impl Into<PathBuf>,
        workspace_id: &str,
        expected: AnnIndexMeta,
    ) -> Result<Self, String> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let this = Self {
            root,
            workspace_id: workspace_id.to_owned(),
            expected: expected.clone(),
            meta: Mutex::new(expected),
            index: Mutex::new(None),
        };
        this.load_or_init()?;
        Ok(this)
    }

    #[must_use]
    pub fn status(&self) -> AnnIndexStatus {
        self.meta
            .lock()
            .map(|m| m.status)
            .unwrap_or(AnnIndexStatus::Failed)
    }

    #[must_use]
    pub fn meta_snapshot(&self) -> AnnIndexMeta {
        self.meta.lock().map(|m| m.clone()).unwrap_or_default()
    }

    fn meta_path(&self) -> PathBuf {
        self.root
            .join(format!("{}.ann.meta.json", sanitize_id(&self.workspace_id)))
    }

    fn index_path(&self) -> PathBuf {
        self.root
            .join(format!("{}.usearch", sanitize_id(&self.workspace_id)))
    }

    fn load_or_init(&self) -> Result<(), String> {
        let expected = self.expected.clone();
        let meta_path = self.meta_path();
        if !meta_path.exists() {
            let mut meta = expected;
            meta.status = AnnIndexStatus::NotAvailable;
            meta.vector_count = 0;
            meta.snapshot_sha256 = None;
            meta.last_error = None;
            self.persist_meta(&meta)?;
            *self.meta.lock().map_err(|_| "meta lock".to_owned())? = meta;
            return Ok(());
        }
        let bytes = fs::read(&meta_path).map_err(|e| e.to_string())?;
        let meta: AnnIndexMeta = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if !Self::meta_compatible(&expected, &meta) {
            let mut broken = meta;
            broken.status = AnnIndexStatus::RebuildRequired;
            broken.last_error = Some("incompatible ANN index metadata".to_owned());
            self.persist_meta(&broken)?;
            *self.meta.lock().map_err(|_| "meta lock".to_owned())? = broken;
            return Ok(());
        }
        if meta.status == AnnIndexStatus::Ready {
            match self.load_index_file(&meta) {
                Ok(index) => {
                    *self.index.lock().map_err(|_| "index lock".to_owned())? = Some(index);
                    *self.meta.lock().map_err(|_| "meta lock".to_owned())? = meta;
                }
                Err(error) => {
                    let mut broken = meta;
                    broken.status = AnnIndexStatus::RebuildRequired;
                    broken.last_error = Some(error);
                    self.persist_meta(&broken)?;
                    *self.meta.lock().map_err(|_| "meta lock".to_owned())? = broken;
                }
            }
        } else {
            *self.meta.lock().map_err(|_| "meta lock".to_owned())? = meta;
        }
        Ok(())
    }

    fn meta_compatible(expected: &AnnIndexMeta, meta: &AnnIndexMeta) -> bool {
        meta.index_format_version == expected.index_format_version
            && meta.embedding_dimension == expected.embedding_dimension
            && meta.embedding_model_id == expected.embedding_model_id
            && meta.embedding_model_version == expected.embedding_model_version
            && meta.chunking_policy_version == expected.chunking_policy_version
            && meta.algorithm == expected.algorithm
            && meta.metric == expected.metric
            && meta.ann_policy_version == expected.ann_policy_version
    }

    fn load_index_file(&self, meta: &AnnIndexMeta) -> Result<Index, String> {
        let path = self.index_path();
        if !path.is_file() {
            return Err("ANN snapshot missing".to_owned());
        }
        let digest = sha256_file(&path)?;
        if let Some(expected) = &meta.snapshot_sha256
            && !expected.eq_ignore_ascii_case(&digest)
        {
            return Err("ANN snapshot checksum mismatch".to_owned());
        }
        let index = Index::new(&Self::index_options(
            meta.embedding_dimension,
            meta.connectivity,
            meta.expansion_add,
            meta.expansion_search,
        ))
        .map_err(|e| e.to_string())?;
        index
            .load(path.to_str().ok_or("non-utf8 ann path")?)
            .map_err(|e| e.to_string())?;
        if index.dimensions() != meta.embedding_dimension {
            return Err("ANN dimension mismatch".to_owned());
        }
        Ok(index)
    }

    fn index_options(
        dimensions: usize,
        connectivity: usize,
        expansion_add: usize,
        expansion_search: usize,
    ) -> IndexOptions {
        // usearch::IndexOptions fields are assigned after Default; struct update
        // syntax is unavailable for this foreign type.
        // Must not lock `self.meta` — callers may already hold that mutex.
        #[allow(clippy::field_reassign_with_default)]
        {
            let mut options = IndexOptions::default();
            options.dimensions = dimensions;
            options.metric = MetricKind::Cos;
            options.quantization = ScalarKind::F32;
            options.connectivity = connectivity;
            options.expansion_add = expansion_add;
            options.expansion_search = expansion_search;
            options.multi = false;
            options
        }
    }

    fn ensure_mutable_index(&self) -> Result<(), String> {
        let mut guard = self.index.lock().map_err(|_| "index lock".to_owned())?;
        if guard.is_some() {
            return Ok(());
        }
        let meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
        let index = Index::new(&Self::index_options(
            meta.embedding_dimension,
            meta.connectivity,
            meta.expansion_add,
            meta.expansion_search,
        ))
        .map_err(|e| e.to_string())?;
        drop(meta);
        index.reserve(1_024).map_err(|e| e.to_string())?;
        *guard = Some(index);
        Ok(())
    }

    pub fn mark_rebuild_required(&self, reason: impl Into<String>) -> Result<(), String> {
        let mut meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
        meta.status = AnnIndexStatus::RebuildRequired;
        meta.last_error = Some(reason.into());
        self.persist_meta(&meta)
    }

    pub fn begin_build(&self) -> Result<(), String> {
        let mut meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
        meta.status = AnnIndexStatus::Building;
        meta.last_error = None;
        self.persist_meta(&meta)
    }

    /// Pre-reserve ANN capacity for large builds (avoids repeated growth).
    pub fn reserve_capacity(&self, capacity: usize) -> Result<(), String> {
        self.ensure_mutable_index()?;
        let guard = self.index.lock().map_err(|_| "index lock".to_owned())?;
        let index = guard.as_ref().ok_or("ANN index missing")?;
        index.reserve(capacity.max(1)).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        *self.index.lock().map_err(|_| "index lock".to_owned())? = None;
        let path = self.index_path();
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        let mut meta = self.expected.clone();
        meta.status = AnnIndexStatus::NotAvailable;
        meta.vector_count = 0;
        meta.snapshot_sha256 = None;
        meta.last_error = None;
        self.persist_meta(&meta)?;
        *self.meta.lock().map_err(|_| "meta lock".to_owned())? = meta;
        Ok(())
    }

    pub fn upsert_vector(&self, key: u64, vector: &[f32]) -> Result<(), String> {
        let dimensions = self
            .meta
            .lock()
            .map_err(|_| "meta lock".to_owned())?
            .embedding_dimension;
        if vector.len() != dimensions || vector.iter().any(|v| !v.is_finite()) {
            return Err("invalid ANN vector".to_owned());
        }
        self.ensure_mutable_index()?;
        let guard = self.index.lock().map_err(|_| "index lock".to_owned())?;
        let index = guard.as_ref().ok_or("ANN index missing")?;
        if index.contains(key) {
            let _ = index.remove(key);
        }
        if index.size() >= index.capacity() {
            index
                .reserve(index.capacity().saturating_mul(2).max(1_024))
                .map_err(|e| e.to_string())?;
        }
        index.add(key, vector).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_key(&self, key: u64) -> Result<(), String> {
        let guard = self.index.lock().map_err(|_| "index lock".to_owned())?;
        if let Some(index) = guard.as_ref()
            && index.contains(key)
        {
            let _ = index.remove(key);
        }
        Ok(())
    }

    pub fn search(&self, query: &[f32], policy: AnnSearchPolicy) -> Result<Vec<AnnHit>, String> {
        let meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
        if query.len() != meta.embedding_dimension {
            return Err("query dimension mismatch".to_owned());
        }
        if meta.status != AnnIndexStatus::Ready && meta.status != AnnIndexStatus::Degraded {
            return Ok(Vec::new());
        }
        drop(meta);
        let guard = self.index.lock().map_err(|_| "index lock".to_owned())?;
        let Some(index) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        if index.size() == 0 {
            return Ok(Vec::new());
        }
        let matches = index
            .search(query, policy.top_k.max(1))
            .map_err(|e| e.to_string())?;
        let mut hits = Vec::with_capacity(matches.keys.len());
        for (key, distance) in matches.keys.iter().zip(matches.distances.iter()) {
            // Cosine distance in USearch ≈ 1 - similarity for normalized vectors.
            let similarity = (1.0 - distance).clamp(-1.0, 1.0);
            hits.push(AnnHit {
                key: *key,
                similarity,
            });
        }
        Ok(hits)
    }

    /// Crash-safe persistence: write temp snapshot, checksum, then atomic replace.
    pub fn persist_snapshot(&self) -> Result<(), String> {
        let guard = self.index.lock().map_err(|_| "index lock".to_owned())?;
        let Some(index) = guard.as_ref() else {
            let mut meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
            meta.status = AnnIndexStatus::NotAvailable;
            meta.vector_count = 0;
            return self.persist_meta(&meta);
        };
        let final_path = self.index_path();
        let tmp_path = final_path.with_extension("usearch.tmp");
        if tmp_path.exists() {
            fs::remove_file(&tmp_path).map_err(|e| e.to_string())?;
        }
        index
            .save(tmp_path.to_str().ok_or("non-utf8 tmp path")?)
            .map_err(|e| e.to_string())?;
        {
            let file = File::open(&tmp_path).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        let digest = sha256_file(&tmp_path)?;
        let vector_count = index.size() as u64;
        fs::rename(&tmp_path, &final_path).map_err(|e| e.to_string())?;
        if let Ok(file) = File::open(&final_path) {
            let _ = file.sync_all();
        }
        let mut meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
        meta.status = AnnIndexStatus::Ready;
        meta.vector_count = vector_count;
        meta.snapshot_sha256 = Some(digest);
        meta.last_error = None;
        meta.created_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.persist_meta(&meta)
    }

    fn persist_meta(&self, meta: &AnnIndexMeta) -> Result<(), String> {
        let path = self.meta_path();
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
        {
            let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Intentionally corrupt the on-disk snapshot for tests.
    pub fn corrupt_snapshot_for_test(&self) -> Result<(), String> {
        let path = self.index_path();
        fs::write(&path, b"not-a-valid-usearch-index").map_err(|e| e.to_string())?;
        let mut meta = self.meta.lock().map_err(|_| "meta lock".to_owned())?;
        meta.snapshot_sha256 = Some("deadbeef".to_owned());
        meta.status = AnnIndexStatus::Ready;
        self.persist_meta(&meta)
    }
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Quantized int8 storage → f32 unit vector for ANN insert/query rerank.
#[must_use]
pub fn dequantize_unit_vector(stored: &[u8]) -> Vec<f32> {
    let mut values = stored
        .iter()
        .map(|byte| f32::from(i8::from_ne_bytes([*byte])) / 127.0)
        .collect::<Vec<_>>();
    let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in &mut values {
            *value /= norm;
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize_vector;

    fn dim384(first: f32, second: f32) -> Vec<f32> {
        let mut values = vec![0.0_f32; GRANITE_EMBEDDING_DIMENSIONS];
        values[0] = first;
        if GRANITE_EMBEDDING_DIMENSIONS > 1 {
            values[1] = second;
        }
        normalize_vector(&mut values);
        values
    }

    #[test]
    fn insert_search_persist_reload() {
        let dir = tempfile::tempdir().expect("temp");
        let index = PersistentAnnIndex::open(dir.path(), "ws-1").expect("open");
        index.begin_build().expect("build");
        let a = dim384(1.0, 0.0);
        let b = dim384(0.9, 0.1);
        let c = dim384(0.0, 1.0);
        index.upsert_vector(1, &a).expect("a");
        index.upsert_vector(2, &b).expect("b");
        index.upsert_vector(3, &c).expect("c");
        index.persist_snapshot().expect("persist");
        assert_eq!(index.status(), AnnIndexStatus::Ready);

        let reloaded = PersistentAnnIndex::open(dir.path(), "ws-1").expect("reload");
        assert_eq!(reloaded.status(), AnnIndexStatus::Ready);
        let hits = reloaded
            .search(&a, AnnSearchPolicy { top_k: 2 })
            .expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].key, 1);
    }

    #[test]
    fn remove_prevents_stale_hits() {
        let dir = tempfile::tempdir().expect("temp");
        let index = PersistentAnnIndex::open(dir.path(), "ws-2").expect("open");
        index.begin_build().expect("build");
        let a = dim384(1.0, 0.0);
        index.upsert_vector(9, &a).expect("add");
        index.persist_snapshot().expect("persist");
        index.remove_key(9).expect("remove");
        index.persist_snapshot().expect("persist2");
        let hits = index
            .search(&a, AnnSearchPolicy { top_k: 5 })
            .expect("search");
        assert!(hits.iter().all(|hit| hit.key != 9));
    }

    #[test]
    fn corruption_marks_rebuild_required() {
        let dir = tempfile::tempdir().expect("temp");
        let index = PersistentAnnIndex::open(dir.path(), "ws-3").expect("open");
        index.begin_build().expect("build");
        let a = dim384(1.0, 0.0);
        index.upsert_vector(1, &a).expect("add");
        index.persist_snapshot().expect("persist");
        index.corrupt_snapshot_for_test().expect("corrupt");
        let reloaded = PersistentAnnIndex::open(dir.path(), "ws-3").expect("reload");
        assert_eq!(reloaded.status(), AnnIndexStatus::RebuildRequired);
        let hits = reloaded
            .search(&a, AnnSearchPolicy::default())
            .expect("fallback empty");
        assert!(hits.is_empty());
    }

    #[test]
    fn incompatible_model_version_requires_rebuild() {
        let dir = tempfile::tempdir().expect("temp");
        let index = PersistentAnnIndex::open(dir.path(), "ws-4").expect("open");
        let mut meta = index.meta_snapshot();
        meta.embedding_model_version = "other-version".to_owned();
        meta.status = AnnIndexStatus::Ready;
        index.persist_meta(&meta).expect("meta");
        let reloaded = PersistentAnnIndex::open(dir.path(), "ws-4").expect("reload");
        assert_eq!(reloaded.status(), AnnIndexStatus::RebuildRequired);
    }
}
