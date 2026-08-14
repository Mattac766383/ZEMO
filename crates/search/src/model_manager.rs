//! App-local embedding model lifecycle: verify, register, install, load, remove.
//! Network download is optional and uses only the pinned backend manifest.

use crate::EmbeddingError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const GRANITE_EMBEDDING_MODEL_ID: &str = "granite-embedding-97m-multilingual-r2";
pub const GRANITE_EMBEDDING_REVISION: &str = "835ad14087e140460703cf0fae09f97d469d65c2";
pub const GRANITE_EMBEDDING_DIMENSIONS: usize = 384;
pub const GRANITE_EMBEDDING_MAX_TOKENS: usize = 512;
pub const GRANITE_EMBEDDING_APPROX_BYTES: u64 = 123_549_550;
pub const LOCAL_EMBEDDING_MODEL_ENV: &str = "SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR";

const BUNDLED_MANIFEST: &str =
    include_str!("../../../models/manifests/granite-embedding-97m-multilingual-r2.v1.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingModelStatus {
    NotInstalled,
    Downloading,
    Installing,
    Ready,
    Loading,
    Unavailable,
    Corrupt,
    IncompatibleVersion,
    Failed,
}

impl EmbeddingModelStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Downloading => "downloading",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Loading => "loading",
            Self::Unavailable => "unavailable",
            Self::Corrupt => "corrupt",
            Self::IncompatibleVersion => "incompatible_version",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingModelAssetSpec {
    pub path: String,
    pub role: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingModelManifest {
    pub model_id: String,
    pub provider_id: String,
    pub revision: String,
    pub version: String,
    pub license: String,
    pub dimensions: usize,
    pub max_sequence_tokens: usize,
    pub pooling: String,
    pub normalize: String,
    pub runtime: String,
    pub approximate_disk_bytes: u64,
    pub assets: Vec<EmbeddingModelAssetSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelState {
    pub model_id: String,
    pub version: String,
    pub dimensions: usize,
    pub status: EmbeddingModelStatus,
    pub installed_at_unix: Option<u64>,
    pub last_error: Option<String>,
    pub asset_checksums: Vec<(String, String)>,
}

impl Default for EmbeddingModelState {
    fn default() -> Self {
        Self {
            model_id: GRANITE_EMBEDDING_MODEL_ID.to_owned(),
            version: GRANITE_EMBEDDING_REVISION.to_owned(),
            dimensions: GRANITE_EMBEDDING_DIMENSIONS,
            status: EmbeddingModelStatus::NotInstalled,
            installed_at_unix: None,
            last_error: None,
            asset_checksums: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelStatusView {
    pub model_id: String,
    pub version: String,
    pub dimensions: usize,
    pub status: EmbeddingModelStatus,
    pub approximate_disk_bytes: u64,
    pub license: String,
    pub local_only: bool,
    pub download_implemented: bool,
    pub last_error: Option<String>,
    pub install_root: String,
}

#[derive(Debug)]
pub struct LocalEmbeddingModelManager {
    root: PathBuf,
    manifest: EmbeddingModelManifest,
}

impl LocalEmbeddingModelManager {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, EmbeddingError> {
        let root = root.into();
        let manifest = bundled_embedding_manifest()?;
        Ok(Self { root, manifest })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn manifest(&self) -> &EmbeddingModelManifest {
        &self.manifest
    }

    #[must_use]
    pub fn model_dir(&self) -> PathBuf {
        self.root.join(&self.manifest.model_id)
    }

    #[must_use]
    pub fn status_path(&self) -> PathBuf {
        self.model_dir().join("status.json")
    }

    pub fn get_status(&self) -> EmbeddingModelStatusView {
        let state = self.load_state().unwrap_or_default();
        EmbeddingModelStatusView {
            model_id: self.manifest.model_id.clone(),
            version: self.manifest.version.clone(),
            dimensions: self.manifest.dimensions,
            status: state.status,
            approximate_disk_bytes: self.manifest.approximate_disk_bytes,
            license: self.manifest.license.clone(),
            local_only: true,
            download_implemented: true,
            last_error: state.last_error,
            install_root: self.model_dir().display().to_string(),
        }
    }

    pub fn load_state(&self) -> Result<EmbeddingModelState, EmbeddingError> {
        let path = self.status_path();
        if !path.exists() {
            return Ok(EmbeddingModelState::default());
        }
        let bytes = fs::read(&path).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|error| EmbeddingError::Failed(error.to_string()))
    }

    pub fn save_state(&self, state: &EmbeddingModelState) -> Result<(), EmbeddingError> {
        let dir = self.model_dir();
        fs::create_dir_all(&dir).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        let tmp = dir.join("status.json.tmp");
        fs::write(&tmp, bytes).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        fs::rename(&tmp, self.status_path())
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        Ok(())
    }

    /// Registers assets from a source directory into the app-controlled model dir.
    /// Source may be `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR` or an explicit path.
    pub fn register_from_directory(
        &self,
        source: &Path,
    ) -> Result<EmbeddingModelState, EmbeddingError> {
        let source = resolve_existing_dir(source)?;
        let dest = self.model_dir();
        fs::create_dir_all(&dest).map_err(|error| EmbeddingError::Failed(error.to_string()))?;

        for asset in &self.manifest.assets {
            let relative = safe_relative_path(&asset.path)?;
            let from = source.join(&relative);
            let to = dest.join(&relative);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
            }
            if !from.is_file() {
                let state = EmbeddingModelState {
                    status: EmbeddingModelStatus::NotInstalled,
                    last_error: Some(format!("missing asset: {}", asset.path)),
                    ..Default::default()
                };
                self.save_state(&state)?;
                return Err(EmbeddingError::Unavailable);
            }
            if is_symlink(&from) {
                return Err(EmbeddingError::Failed(format!(
                    "refusing symlink asset: {}",
                    asset.path
                )));
            }
            fs::copy(&from, &to).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        }

        match self.verify() {
            Ok(state) => Ok(state),
            Err(error) => {
                let mut state = self.load_state().unwrap_or_default();
                state.status = match error {
                    EmbeddingError::Corrupt => EmbeddingModelStatus::Corrupt,
                    EmbeddingError::Failed(_) => EmbeddingModelStatus::Failed,
                    _ => EmbeddingModelStatus::Unavailable,
                };
                state.last_error = Some(error.to_string());
                let _ = self.save_state(&state);
                Err(error)
            }
        }
    }

    pub fn register_from_env_or_error(&self) -> Result<EmbeddingModelState, EmbeddingError> {
        let source = std::env::var_os(LOCAL_EMBEDDING_MODEL_ENV).ok_or_else(|| {
            EmbeddingError::Failed(format!(
                "model assets not provisioned; set {LOCAL_EMBEDDING_MODEL_ENV} or copy assets into {}",
                self.model_dir().display()
            ))
        })?;
        self.register_from_directory(Path::new(&source))
    }

    /// Verifies installed assets under the app model directory.
    pub fn verify(&self) -> Result<EmbeddingModelState, EmbeddingError> {
        let dest = self.model_dir();
        if !dest.is_dir() {
            let state = EmbeddingModelState {
                status: EmbeddingModelStatus::NotInstalled,
                ..EmbeddingModelState::default()
            };
            self.save_state(&state)?;
            return Ok(state);
        }

        let mut checksums = Vec::new();
        for asset in &self.manifest.assets {
            let relative = safe_relative_path(&asset.path)?;
            let path = dest.join(&relative);
            if !path.is_file() {
                let state = EmbeddingModelState {
                    status: EmbeddingModelStatus::NotInstalled,
                    last_error: Some(format!("missing asset: {}", asset.path)),
                    ..EmbeddingModelState::default()
                };
                self.save_state(&state)?;
                return Ok(state);
            }
            if is_symlink(&path) {
                let state = EmbeddingModelState {
                    status: EmbeddingModelStatus::Corrupt,
                    last_error: Some(format!("symlink rejected: {}", asset.path)),
                    ..EmbeddingModelState::default()
                };
                self.save_state(&state)?;
                return Err(EmbeddingError::Corrupt);
            }
            let meta =
                fs::metadata(&path).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
            if meta.len() != asset.bytes {
                let state = EmbeddingModelState {
                    status: EmbeddingModelStatus::Corrupt,
                    last_error: Some(format!(
                        "size mismatch for {}: expected {} got {}",
                        asset.path,
                        asset.bytes,
                        meta.len()
                    )),
                    ..EmbeddingModelState::default()
                };
                self.save_state(&state)?;
                return Err(EmbeddingError::Corrupt);
            }
            let digest = sha256_file(&path)?;
            if !digest.eq_ignore_ascii_case(&asset.sha256) {
                let state = EmbeddingModelState {
                    status: EmbeddingModelStatus::Corrupt,
                    last_error: Some(format!("checksum mismatch for {}", asset.path)),
                    ..EmbeddingModelState::default()
                };
                self.save_state(&state)?;
                return Err(EmbeddingError::Corrupt);
            }
            checksums.push((asset.path.clone(), digest));
        }

        if self.manifest.dimensions != GRANITE_EMBEDDING_DIMENSIONS
            || self.manifest.version != GRANITE_EMBEDDING_REVISION
        {
            let state = EmbeddingModelState {
                status: EmbeddingModelStatus::IncompatibleVersion,
                last_error: Some("manifest version/dimensions incompatible".to_owned()),
                ..EmbeddingModelState::default()
            };
            self.save_state(&state)?;
            return Err(EmbeddingError::Failed(
                "incompatible embedding model version".to_owned(),
            ));
        }

        let installed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .ok();
        let state = EmbeddingModelState {
            model_id: self.manifest.model_id.clone(),
            version: self.manifest.version.clone(),
            dimensions: self.manifest.dimensions,
            status: EmbeddingModelStatus::Ready,
            installed_at_unix: installed_at,
            last_error: None,
            asset_checksums: checksums,
        };
        self.save_state(&state)?;
        Ok(state)
    }

    pub fn mark_status(
        &self,
        status: EmbeddingModelStatus,
        last_error: Option<String>,
    ) -> Result<EmbeddingModelState, EmbeddingError> {
        let mut state = self.load_state().unwrap_or_default();
        state.status = status;
        state.last_error = last_error;
        self.save_state(&state)?;
        Ok(state)
    }

    /// Removes only application-owned model artifacts under the model root.
    pub fn remove(&self) -> Result<(), EmbeddingError> {
        let dir = self.model_dir();
        if dir.exists() {
            ensure_path_under_root(&self.root, &dir)?;
            fs::remove_dir_all(&dir).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        }
        let state = EmbeddingModelState {
            status: EmbeddingModelStatus::NotInstalled,
            ..EmbeddingModelState::default()
        };
        // Recreate status parent only when needed; leave empty root.
        fs::create_dir_all(self.model_dir())
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        self.save_state(&state)?;
        Ok(())
    }

    pub fn asset_path(&self, role: &str) -> Result<PathBuf, EmbeddingError> {
        let asset = self
            .manifest
            .assets
            .iter()
            .find(|asset| asset.role == role)
            .ok_or_else(|| EmbeddingError::Failed(format!("unknown asset role: {role}")))?;
        let relative = safe_relative_path(&asset.path)?;
        let path = self.model_dir().join(relative);
        ensure_path_under_root(&self.root, &path)?;
        Ok(path)
    }
}

pub fn bundled_embedding_manifest() -> Result<EmbeddingModelManifest, EmbeddingError> {
    serde_json::from_str(BUNDLED_MANIFEST)
        .map_err(|error| EmbeddingError::Failed(format!("invalid bundled manifest: {error}")))
}

fn safe_relative_path(value: &str) -> Result<PathBuf, EmbeddingError> {
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(EmbeddingError::Failed(
            "absolute model asset paths are rejected".to_owned(),
        ));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(EmbeddingError::Failed(
                    "path traversal rejected in model asset path".to_owned(),
                ));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(EmbeddingError::Failed("empty model asset path".to_owned()));
    }
    Ok(clean)
}

fn resolve_existing_dir(path: &Path) -> Result<PathBuf, EmbeddingError> {
    let canonical = path
        .canonicalize()
        .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
    if !canonical.is_dir() {
        return Err(EmbeddingError::Failed(
            "model source path is not a directory".to_owned(),
        ));
    }
    Ok(canonical)
}

fn ensure_path_under_root(root: &Path, path: &Path) -> Result<(), EmbeddingError> {
    let root = root
        .canonicalize()
        .or_else(|_| {
            fs::create_dir_all(root)?;
            root.canonicalize()
        })
        .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
    let candidate = if path.exists() {
        path.canonicalize()
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?
    } else {
        path.to_path_buf()
    };
    if !candidate.starts_with(&root) {
        return Err(EmbeddingError::Failed(
            "model path escapes application model root".to_owned(),
        ));
    }
    Ok(())
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn sha256_file(path: &Path) -> Result<String, EmbeddingError> {
    let file = File::open(path).map_err(|error| EmbeddingError::Failed(error.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| EmbeddingError::Failed(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fake_assets(dir: &Path, manifest: &EmbeddingModelManifest) {
        for asset in &manifest.assets {
            let relative = safe_relative_path(&asset.path).expect("path");
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent");
            }
            let mut file = File::create(&path).expect("create");
            // Deterministic content sized exactly; checksums will not match production.
            let content = vec![b'a'; asset.bytes as usize];
            file.write_all(&content).expect("write");
        }
    }

    #[test]
    fn not_installed_by_default() {
        let dir = tempfile::tempdir().expect("temp");
        let manager = LocalEmbeddingModelManager::new(dir.path()).expect("manager");
        assert_eq!(
            manager.get_status().status,
            EmbeddingModelStatus::NotInstalled
        );
    }

    #[test]
    fn checksum_mismatch_marks_corrupt() {
        let dir = tempfile::tempdir().expect("temp");
        let manager = LocalEmbeddingModelManager::new(dir.path()).expect("manager");
        let source = dir.path().join("source");
        fs::create_dir_all(&source).expect("source");
        write_fake_assets(&source, manager.manifest());
        let error = manager
            .register_from_directory(&source)
            .expect_err("checksum");
        assert!(matches!(error, EmbeddingError::Corrupt));
        assert_eq!(manager.get_status().status, EmbeddingModelStatus::Corrupt);
    }

    #[test]
    fn missing_asset_stays_not_installed() {
        let dir = tempfile::tempdir().expect("temp");
        let manager = LocalEmbeddingModelManager::new(dir.path()).expect("manager");
        let source = dir.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let error = manager
            .register_from_directory(&source)
            .expect_err("missing");
        assert!(matches!(error, EmbeddingError::Unavailable));
    }

    #[test]
    fn remove_clears_install_state() {
        let dir = tempfile::tempdir().expect("temp");
        let manager = LocalEmbeddingModelManager::new(dir.path()).expect("manager");
        manager
            .mark_status(EmbeddingModelStatus::Failed, Some("boom".to_owned()))
            .expect("mark");
        manager.remove().expect("remove");
        assert_eq!(
            manager.get_status().status,
            EmbeddingModelStatus::NotInstalled
        );
    }

    #[test]
    fn rejects_path_traversal_in_asset_names() {
        assert!(safe_relative_path("../escape.bin").is_err());
        assert!(safe_relative_path("/abs.bin").is_err());
    }

    #[test]
    fn status_persists_across_reopen() {
        let dir = tempfile::tempdir().expect("temp");
        {
            let manager = LocalEmbeddingModelManager::new(dir.path()).expect("manager");
            manager
                .mark_status(EmbeddingModelStatus::Failed, Some("load failed".to_owned()))
                .expect("mark");
        }
        let manager = LocalEmbeddingModelManager::new(dir.path()).expect("reopen");
        let status = manager.get_status();
        assert_eq!(status.status, EmbeddingModelStatus::Failed);
        assert_eq!(status.last_error.as_deref(), Some("load failed"));
    }
}
