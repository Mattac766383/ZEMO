//! Privacy policy, single-task cloud consent, and operating-system secrets.

use async_trait::async_trait;
use domain::{
    ArtifactId, ConsentGrantId, DataClass, ModelReleaseId, ProcessingLocation, WorkspaceId,
};
use platform::{PlatformError, SecretStore};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use zeroize::{Zeroize, Zeroizing};

const SHARED_EXECUTOR_ROOT_FILE_NAME: &str = "executor-root-v2";

/// Filesystem location used when the helper process cannot read the OS keystore.
///
/// The coordinator and the isolated executor are separately signed binaries.
/// On ad-hoc macOS that means the Keychain item created by the app is not
/// readable by `operation-executor`, so both sides also keep a 0600 copy under
/// the application support directory.
#[must_use]
pub fn shared_executor_root_path(service: &str) -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(service).join(SHARED_EXECUTOR_ROOT_FILE_NAME))
}

pub fn persist_shared_executor_root(service: &str, secret: &[u8]) -> Result<(), PlatformError> {
    let path = shared_executor_root_path(service).ok_or_else(|| {
        PlatformError::SecretStore("application support directory is unavailable".to_owned())
    })?;
    persist_shared_executor_root_to(&path, secret)
}

pub fn load_shared_executor_root(service: &str) -> Result<Option<Vec<u8>>, PlatformError> {
    let Some(path) = shared_executor_root_path(service) else {
        return Ok(None);
    };
    load_shared_executor_root_from(&path)
}

pub fn persist_shared_executor_root_to(path: &Path, secret: &[u8]) -> Result<(), PlatformError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| PlatformError::SecretStore(error.to_string()))?;
    }
    let encoded = Zeroizing::new(encode_hex(secret));
    let tmp = path.with_file_name(format!("{SHARED_EXECUTOR_ROOT_FILE_NAME}.tmp"));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|error| PlatformError::SecretStore(error.to_string()))?;
        file.write_all(encoded.as_bytes())
            .map_err(|error| PlatformError::SecretStore(error.to_string()))?;
        file.sync_all()
            .map_err(|error| PlatformError::SecretStore(error.to_string()))?;
    }
    fs::rename(&tmp, path).map_err(|error| PlatformError::SecretStore(error.to_string()))?;
    Ok(())
}

pub fn load_shared_executor_root_from(path: &Path) -> Result<Option<Vec<u8>>, PlatformError> {
    match fs::read_to_string(path) {
        Ok(mut encoded) => {
            let decoded = decode_hex(encoded.trim())?;
            encoded.zeroize();
            Ok(Some(decoded))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(PlatformError::SecretStore(error.to_string())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentGrant {
    pub id: ConsentGrantId,
    pub workspace_id: WorkspaceId,
    pub task_id: String,
    pub request_digest: [u8; 32],
    pub provider_id: String,
    pub model_release_id: ModelReleaseId,
    pub purpose: String,
    pub artifact_digests: Vec<[u8; 32]>,
    pub allowed_data_classes: Vec<DataClass>,
    pub max_bytes: u64,
    pub max_calls: u32,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressRequest {
    pub workspace_id: WorkspaceId,
    pub task_id: String,
    pub request_digest: [u8; 32],
    pub provider_id: String,
    pub model_release_id: ModelReleaseId,
    pub artifact_id: ArtifactId,
    pub artifact_digest: [u8; 32],
    pub data_class: DataClass,
    pub byte_count: u64,
    pub now_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisclosureReceipt {
    pub grant_id: ConsentGrantId,
    pub artifact_id: ArtifactId,
    pub provider_id: String,
    pub model_release_id: ModelReleaseId,
    pub data_class: DataClass,
    pub byte_count: u64,
    pub disclosed_at_unix_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum PrivacyError {
    #[error("cloud processing requires an explicit one-task consent grant")]
    MissingGrant,
    #[error("consent grant has expired")]
    Expired,
    #[error("consent grant does not match this request")]
    ScopeMismatch,
    #[error("consent grant byte budget exceeded")]
    ByteBudgetExceeded,
    #[error("consent grant call budget exhausted")]
    CallBudgetExceeded,
    #[error("consent ledger mutex was poisoned")]
    LedgerPoisoned,
}

#[derive(Debug, Clone)]
struct GrantState {
    grant: ConsentGrant,
    calls_used: u32,
    bytes_used: u64,
    revoked: bool,
}

#[derive(Debug, Default)]
pub struct ConsentLedger {
    grants: Mutex<HashMap<ConsentGrantId, GrantState>>,
}

impl ConsentLedger {
    fn lock(&self) -> Result<MutexGuard<'_, HashMap<ConsentGrantId, GrantState>>, PrivacyError> {
        self.grants.lock().map_err(|_| PrivacyError::LedgerPoisoned)
    }

    pub fn grant(&self, grant: ConsentGrant) -> Result<(), PrivacyError> {
        self.lock()?.insert(
            grant.id,
            GrantState {
                grant,
                calls_used: 0,
                bytes_used: 0,
                revoked: false,
            },
        );
        Ok(())
    }

    pub fn revoke(&self, grant_id: ConsentGrantId) -> Result<(), PrivacyError> {
        let mut grants = self.lock()?;
        let state = grants
            .get_mut(&grant_id)
            .ok_or(PrivacyError::MissingGrant)?;
        state.revoked = true;
        Ok(())
    }

    pub fn authorize_once(
        &self,
        grant_id: ConsentGrantId,
        request: &EgressRequest,
    ) -> Result<DisclosureReceipt, PrivacyError> {
        let mut grants = self.lock()?;
        let state = grants
            .get_mut(&grant_id)
            .ok_or(PrivacyError::MissingGrant)?;
        if state.revoked {
            return Err(PrivacyError::MissingGrant);
        }
        if request.now_unix_ms > state.grant.expires_at_unix_ms {
            return Err(PrivacyError::Expired);
        }
        if state.grant.workspace_id != request.workspace_id
            || state.grant.task_id != request.task_id
            || state.grant.request_digest != request.request_digest
            || state.grant.provider_id != request.provider_id
            || state.grant.model_release_id != request.model_release_id
            || !state
                .grant
                .artifact_digests
                .contains(&request.artifact_digest)
            || !state
                .grant
                .allowed_data_classes
                .contains(&request.data_class)
        {
            return Err(PrivacyError::ScopeMismatch);
        }
        if state.calls_used >= state.grant.max_calls {
            return Err(PrivacyError::CallBudgetExceeded);
        }
        if state.bytes_used.saturating_add(request.byte_count) > state.grant.max_bytes {
            return Err(PrivacyError::ByteBudgetExceeded);
        }
        state.calls_used = state.calls_used.saturating_add(1);
        state.bytes_used = state.bytes_used.saturating_add(request.byte_count);

        Ok(DisclosureReceipt {
            grant_id,
            artifact_id: request.artifact_id,
            provider_id: request.provider_id.clone(),
            model_release_id: request.model_release_id,
            data_class: request.data_class,
            byte_count: request.byte_count,
            disclosed_at_unix_ms: request.now_unix_ms,
        })
    }
}

#[derive(Debug)]
pub struct OsSecretStore {
    service: String,
}

impl OsSecretStore {
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, PlatformError> {
        keyring::Entry::new(&self.service, key)
            .map_err(|error| PlatformError::SecretStore(error.to_string()))
    }

    pub fn load_sync(&self, key: &str) -> Result<Option<Vec<u8>>, PlatformError> {
        match self.entry(key)?.get_password() {
            Ok(mut encoded) => {
                let decoded = decode_hex(&encoded)?;
                encoded.zeroize();
                Ok(Some(decoded))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(PlatformError::SecretStore(error.to_string())),
        }
    }

    pub fn store_sync(&self, key: &str, secret: &[u8]) -> Result<(), PlatformError> {
        let encoded = Zeroizing::new(encode_hex(secret));
        self.entry(key)?
            .set_password(encoded.as_str())
            .map_err(|error| PlatformError::SecretStore(error.to_string()))
    }

    pub fn remove_sync(&self, key: &str) -> Result<(), PlatformError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(PlatformError::SecretStore(error.to_string())),
        }
    }
}

#[async_trait]
impl SecretStore for OsSecretStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>, PlatformError> {
        self.load_sync(key)
    }

    async fn store(&self, key: &str, secret: &[u8]) -> Result<(), PlatformError> {
        self.store_sync(key, secret)
    }

    async fn remove(&self, key: &str) -> Result<(), PlatformError> {
        self.remove_sync(key)
    }
}

#[must_use]
pub const fn default_processing_location() -> ProcessingLocation {
    ProcessingLocation::LocalBundled
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Result<Vec<u8>, PlatformError> {
    if !value.len().is_multiple_of(2) {
        return Err(PlatformError::SecretStore(
            "stored secret has an invalid encoding".to_owned(),
        ));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|error| PlatformError::SecretStore(error.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(grant: &ConsentGrant, now: i64) -> EgressRequest {
        EgressRequest {
            workspace_id: grant.workspace_id,
            task_id: grant.task_id.clone(),
            request_digest: grant.request_digest,
            provider_id: grant.provider_id.clone(),
            model_release_id: grant.model_release_id,
            artifact_id: ArtifactId::new(),
            artifact_digest: grant.artifact_digests[0],
            data_class: DataClass::Text,
            byte_count: 5,
            now_unix_ms: now,
        }
    }

    fn grant() -> ConsentGrant {
        ConsentGrant {
            id: ConsentGrantId::new(),
            workspace_id: WorkspaceId::new(),
            task_id: "task".to_owned(),
            request_digest: [1; 32],
            provider_id: "provider".to_owned(),
            model_release_id: ModelReleaseId::new(),
            purpose: "classification".to_owned(),
            artifact_digests: vec![[2; 32]],
            allowed_data_classes: vec![DataClass::Text],
            max_bytes: 10,
            max_calls: 1,
            expires_at_unix_ms: 100,
        }
    }

    #[test]
    fn grant_is_single_task_and_budgeted() {
        let ledger = ConsentLedger::default();
        let grant = grant();
        assert!(ledger.grant(grant.clone()).is_ok());
        assert!(
            ledger
                .authorize_once(grant.id, &request(&grant, 10))
                .is_ok()
        );
        assert!(matches!(
            ledger.authorize_once(grant.id, &request(&grant, 10)),
            Err(PrivacyError::CallBudgetExceeded)
        ));
    }

    #[test]
    fn there_is_no_implicit_cloud_grant() {
        let ledger = ConsentLedger::default();
        let grant = grant();
        assert!(matches!(
            ledger.authorize_once(grant.id, &request(&grant, 10)),
            Err(PrivacyError::MissingGrant)
        ));
    }

    #[test]
    fn shared_executor_root_round_trip_is_restricted() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join(SHARED_EXECUTOR_ROOT_FILE_NAME);
        let secret = [7_u8; 32];
        persist_shared_executor_root_to(&path, &secret).unwrap_or_else(|error| panic!("{error}"));
        let loaded = load_shared_executor_root_from(&path)
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("shared executor root should load"));
        assert_eq!(loaded, secret);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path)
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
