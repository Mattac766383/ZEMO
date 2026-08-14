//! Pinned, user-consented download of local embedding model assets.
//! Frontend may only request install of the known model id — never arbitrary URLs.

use crate::EmbeddingError;
use crate::model_manager::{
    EmbeddingModelAssetSpec, EmbeddingModelManifest, EmbeddingModelState, EmbeddingModelStatus,
    GRANITE_EMBEDDING_APPROX_BYTES, LocalEmbeddingModelManager,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Trusted download endpoints are derived only from the pinned revision + asset path.
pub const PINNED_DOWNLOAD_HOST: &str = "huggingface.co";
pub const PINNED_MODEL_REPO: &str = "ibm-granite/granite-embedding-97m-multilingual-r2";

pub trait HttpFetcher: Send + Sync {
    fn fetch_to_file(
        &self,
        url: &str,
        dest: &Path,
        expected_bytes: Option<u64>,
        is_cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), EmbeddingError>;
}

#[derive(Debug, Default)]
pub struct UreqHttpsFetcher;

impl HttpFetcher for UreqHttpsFetcher {
    fn fetch_to_file(
        &self,
        url: &str,
        dest: &Path,
        expected_bytes: Option<u64>,
        is_cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), EmbeddingError> {
        validate_pinned_download_url(url)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        }
        let tmp = dest.with_extension("part");
        if tmp.exists() {
            let _ = fs::remove_file(&tmp);
        }

        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent("SupremacyLocalEmbeddingInstaller/1.0 (+local-only; no telemetry)")
            .build();
        let agent: ureq::Agent = config.into();
        let mut response = agent
            .get(url)
            .call()
            .map_err(|e| EmbeddingError::Failed(format!("download failed: {e}")))?;
        let status = response.status();
        if !(200..300).contains(&status.as_u16()) {
            return Err(EmbeddingError::Failed(format!("download HTTP {}", status)));
        }
        let content_length = response
            .headers()
            .get("Content-Length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .or(expected_bytes);

        let mut reader = response.body_mut().as_reader();
        let mut file = File::create(&tmp).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        let mut buffer = [0_u8; 64 * 1024];
        let mut written = 0_u64;
        loop {
            if is_cancelled.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&tmp);
                return Err(EmbeddingError::Failed("download cancelled".to_owned()));
            }
            let read = reader
                .read(&mut buffer)
                .map_err(|e| EmbeddingError::Failed(e.to_string()))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])
                .map_err(|e| EmbeddingError::Failed(e.to_string()))?;
            written = written.saturating_add(read as u64);
            on_progress(written, content_length);
            if let Some(limit) = expected_bytes
                && written > limit.saturating_mul(2).max(limit.saturating_add(1_048_576))
            {
                let _ = fs::remove_file(&tmp);
                return Err(EmbeddingError::Corrupt);
            }
        }
        file.sync_all()
            .map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        drop(file);
        if let Some(expected) = expected_bytes
            && written != expected
        {
            let _ = fs::remove_file(&tmp);
            return Err(EmbeddingError::Corrupt);
        }
        fs::rename(&tmp, dest).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProgress {
    pub phase: EmbeddingModelStatus,
    pub bytes_downloaded: u64,
    pub bytes_total: Option<u64>,
    pub current_asset: Option<String>,
}

impl LocalEmbeddingModelManager {
    #[must_use]
    pub fn pinned_asset_url(manifest: &EmbeddingModelManifest, asset_path: &str) -> String {
        format!(
            "https://{PINNED_DOWNLOAD_HOST}/{PINNED_MODEL_REPO}/resolve/{}/{}?download=true",
            manifest.revision, asset_path
        )
    }

    /// Production install: download pinned assets into a staging dir, verify, then promote.
    pub fn install_from_pinned_network(
        &self,
        fetcher: &dyn HttpFetcher,
        is_cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<EmbeddingModelState, EmbeddingError> {
        self.ensure_disk_budget()?;
        let staging = self.staging_dir();
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        }
        fs::create_dir_all(&staging).map_err(|e| EmbeddingError::Failed(e.to_string()))?;

        let mut state = EmbeddingModelState {
            status: EmbeddingModelStatus::Downloading,
            last_error: None,
            ..EmbeddingModelState::default()
        };
        self.save_state(&state)?;
        let approx_bytes = self.manifest().approximate_disk_bytes;
        on_progress(InstallProgress {
            phase: EmbeddingModelStatus::Downloading,
            bytes_downloaded: 0,
            bytes_total: Some(approx_bytes),
            current_asset: None,
        });

        let assets = self.manifest().assets.clone();
        let mut total_downloaded = 0_u64;
        for asset in &assets {
            if is_cancelled.load(Ordering::Relaxed) {
                self.abort_install("installation cancelled")?;
                return Err(EmbeddingError::Failed("installation cancelled".to_owned()));
            }
            let url = Self::pinned_asset_url(self.manifest(), &asset.path);
            validate_pinned_download_url(&url)?;
            let dest = staging.join(&asset.path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
            }
            let asset_path = asset.path.clone();
            fetcher.fetch_to_file(
                &url,
                &dest,
                Some(asset.bytes),
                is_cancelled,
                &mut |written, _total| {
                    on_progress(InstallProgress {
                        phase: EmbeddingModelStatus::Downloading,
                        bytes_downloaded: total_downloaded.saturating_add(written),
                        bytes_total: Some(approx_bytes),
                        current_asset: Some(asset_path.clone()),
                    });
                },
            )?;
            verify_asset_file(&dest, asset)?;
            total_downloaded = total_downloaded.saturating_add(asset.bytes);
        }

        if is_cancelled.load(Ordering::Relaxed) {
            self.abort_install("installation cancelled")?;
            return Err(EmbeddingError::Failed("installation cancelled".to_owned()));
        }

        state.status = EmbeddingModelStatus::Installing;
        self.save_state(&state)?;
        on_progress(InstallProgress {
            phase: EmbeddingModelStatus::Installing,
            bytes_downloaded: total_downloaded,
            bytes_total: Some(approx_bytes),
            current_asset: None,
        });

        let model_dir = self.model_dir();
        let backup = self
            .root()
            .join(format!("{}.bak", self.manifest().model_id));
        if backup.exists() {
            let _ = fs::remove_dir_all(&backup);
        }
        if model_dir.exists() {
            fs::rename(&model_dir, &backup).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        }
        fs::rename(&staging, &model_dir).map_err(|e| {
            if backup.exists() {
                let _ = fs::rename(&backup, &model_dir);
            }
            EmbeddingError::Failed(e.to_string())
        })?;
        if backup.exists() {
            let _ = fs::remove_dir_all(&backup);
        }

        match self.verify() {
            Ok(ready) => {
                on_progress(InstallProgress {
                    phase: EmbeddingModelStatus::Ready,
                    bytes_downloaded: total_downloaded,
                    bytes_total: Some(approx_bytes),
                    current_asset: None,
                });
                Ok(ready)
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&model_dir);
                let failed = EmbeddingModelState {
                    status: EmbeddingModelStatus::Corrupt,
                    last_error: Some(error.to_string()),
                    ..EmbeddingModelState::default()
                };
                let _ = self.save_state(&failed);
                Err(error)
            }
        }
    }

    fn staging_dir(&self) -> PathBuf {
        self.root()
            .join(format!("{}.download", self.manifest().model_id))
    }

    fn abort_install(&self, reason: &str) -> Result<(), EmbeddingError> {
        let staging = self.staging_dir();
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        let state = EmbeddingModelState {
            status: EmbeddingModelStatus::NotInstalled,
            last_error: Some(reason.to_owned()),
            ..EmbeddingModelState::default()
        };
        self.save_state(&state)
    }

    fn ensure_disk_budget(&self) -> Result<(), EmbeddingError> {
        let needed = GRANITE_EMBEDDING_APPROX_BYTES
            .saturating_mul(2)
            .saturating_add(64 * 1024 * 1024);
        let available = available_bytes(self.root()).unwrap_or(u64::MAX);
        if available < needed {
            return Err(EmbeddingError::Failed(format!(
                "insufficient disk space: need ~{needed} bytes, available {available}"
            )));
        }
        Ok(())
    }
}

pub fn validate_pinned_download_url(url: &str) -> Result<(), EmbeddingError> {
    let parsed = url::Url::parse(url).map_err(|_| EmbeddingError::Failed("invalid URL".into()))?;
    if parsed.scheme() != "https" {
        return Err(EmbeddingError::Failed(
            "only HTTPS downloads allowed".into(),
        ));
    }
    if parsed.host_str() != Some(PINNED_DOWNLOAD_HOST) {
        return Err(EmbeddingError::Failed("untrusted download host".into()));
    }
    let expected_prefix = format!("/{PINNED_MODEL_REPO}/resolve/");
    if !parsed.path().starts_with(&expected_prefix) {
        return Err(EmbeddingError::Failed("untrusted download path".into()));
    }
    if parsed.path().contains("..") {
        return Err(EmbeddingError::Failed("path traversal rejected".into()));
    }
    Ok(())
}

fn verify_asset_file(path: &Path, asset: &EmbeddingModelAssetSpec) -> Result<(), EmbeddingError> {
    let meta = fs::metadata(path).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
    if meta.len() != asset.bytes {
        return Err(EmbeddingError::Corrupt);
    }
    let digest = sha256_file(path)?;
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        return Err(EmbeddingError::Corrupt);
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, EmbeddingError> {
    let mut file = File::open(path).map_err(|e| EmbeddingError::Failed(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| EmbeddingError::Failed(e.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn available_bytes(path: &Path) -> Option<u64> {
    let path_str = path.to_str()?;
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::mem::MaybeUninit;
        let c_path = CString::new(path_str).ok()?;
        let mut stat = MaybeUninit::<libc::statfs>::uninit();
        let rc = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let stat = unsafe { stat.assume_init() };
        Some(stat.f_bavail.saturating_mul(u64::from(stat.f_bsize)))
    }
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::mem::MaybeUninit;
        let c_path = CString::new(path_str).ok()?;
        let mut stat = MaybeUninit::<libc::statvfs>::uninit();
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let stat = unsafe { stat.assume_init() };
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path_str;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeFetcher {
        payloads: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl HttpFetcher for FakeFetcher {
        fn fetch_to_file(
            &self,
            url: &str,
            dest: &Path,
            expected_bytes: Option<u64>,
            is_cancelled: &AtomicBool,
            on_progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<(), EmbeddingError> {
            validate_pinned_download_url(url)?;
            if is_cancelled.load(Ordering::Relaxed) {
                return Err(EmbeddingError::Failed("download cancelled".into()));
            }
            let guard = self.payloads.lock().expect("lock");
            let bytes = guard
                .get(url)
                .cloned()
                .ok_or_else(|| EmbeddingError::Failed("missing fake payload".into()))?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(dest, &bytes).unwrap();
            on_progress(bytes.len() as u64, expected_bytes);
            Ok(())
        }
    }

    #[test]
    fn rejects_arbitrary_download_hosts() {
        assert!(validate_pinned_download_url(
            "https://evil.example/ibm-granite/granite-embedding-97m-multilingual-r2/resolve/x/onnx/model.onnx"
        )
        .is_err());
        assert!(validate_pinned_download_url("http://huggingface.co/x").is_err());
    }

    #[test]
    fn accepts_pinned_huggingface_url() {
        let manifest = crate::bundled_embedding_manifest().expect("manifest");
        let url = LocalEmbeddingModelManager::pinned_asset_url(&manifest, "tokenizer.json");
        assert!(validate_pinned_download_url(&url).is_ok());
    }

    #[test]
    fn install_cancels_cleanly_without_marking_ready() {
        let temp = tempfile::TempDir::new().expect("temp");
        let manager = LocalEmbeddingModelManager::new(temp.path()).expect("mgr");
        let cancel = AtomicBool::new(true);
        let fetcher = FakeFetcher {
            payloads: Mutex::new(std::collections::HashMap::new()),
        };
        let err = manager
            .install_from_pinned_network(&fetcher, &cancel, &mut |_| {})
            .expect_err("must cancel");
        assert!(err.to_string().contains("cancelled"));
        let state = manager.verify().expect("verify");
        assert_ne!(state.status, EmbeddingModelStatus::Ready);
        assert!(!manager.staging_dir().exists());
        // status.json may remain under model_dir after abort; assets must not.
        assert!(!manager.model_dir().join("tokenizer.json").exists());
        assert!(
            !manager
                .model_dir()
                .join("onnx/model_quint8_avx2.onnx")
                .exists()
        );
    }

    #[test]
    fn checksum_mismatch_rejects_and_leaves_not_ready() {
        let temp = tempfile::TempDir::new().expect("temp");
        let manager = LocalEmbeddingModelManager::new(temp.path()).expect("mgr");
        let manifest = manager.manifest().clone();
        let mut payloads = std::collections::HashMap::new();
        for asset in &manifest.assets {
            let url = LocalEmbeddingModelManager::pinned_asset_url(&manifest, &asset.path);
            payloads.insert(url, vec![0_u8; asset.bytes as usize]);
        }
        let fetcher = FakeFetcher {
            payloads: Mutex::new(payloads),
        };
        let cancel = AtomicBool::new(false);
        let err = manager
            .install_from_pinned_network(&fetcher, &cancel, &mut |_| {})
            .expect_err("checksum must fail");
        assert!(matches!(err, EmbeddingError::Corrupt));
        let state = manager.verify().expect("verify");
        assert_ne!(state.status, EmbeddingModelStatus::Ready);
    }
}
