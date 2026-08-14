use crate::{DurableJournal, OperationsError};
use domain::{
    ExecutionId, FileFingerprint, JournalEventKind, OperationJournalEvent, OperationStepId, PlanId,
};
use platform::{ReadOnlyPlatform, RenameRequest, SafeFileOperations};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossVolumeTransferDraft {
    pub id: PlanId,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub recovery_vault_path: PathBuf,
    pub destination_temporary_path: PathBuf,
    pub expected_source: FileFingerprint,
    pub created_at_unix_ms: i64,
}

impl CrossVolumeTransferDraft {
    pub fn seal(self) -> Result<CrossVolumeTransferPlan, TransferError> {
        if self.source == self.destination
            || self.source == self.recovery_vault_path
            || self.destination == self.recovery_vault_path
            || self.destination.parent() != self.destination_temporary_path.parent()
        {
            return Err(TransferError::InvalidPlan);
        }
        if self.expected_source.attributes & 0x4000 != 0
            || self.expected_source.native_identity.link_count != 1
            || self.expected_source.native_identity.reparse_tag.is_some()
        {
            return Err(TransferError::InvalidPlan);
        }
        let encoded =
            serde_json::to_vec(&self).map_err(|error| TransferError::Journal(error.to_string()))?;
        Ok(CrossVolumeTransferPlan {
            id: self.id,
            source: self.source,
            destination: self.destination,
            recovery_vault_path: self.recovery_vault_path,
            destination_temporary_path: self.destination_temporary_path,
            expected_source: self.expected_source,
            digest: *blake3::hash(&encoded).as_bytes(),
            sealed_at_unix_ms: self.created_at_unix_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossVolumeTransferPlan {
    pub id: PlanId,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub recovery_vault_path: PathBuf,
    pub destination_temporary_path: PathBuf,
    pub expected_source: FileFingerprint,
    pub digest: [u8; 32],
    pub sealed_at_unix_ms: i64,
}

impl CrossVolumeTransferPlan {
    pub fn verify_integrity(&self) -> Result<(), TransferError> {
        let draft = CrossVolumeTransferDraft {
            id: self.id,
            source: self.source.clone(),
            destination: self.destination.clone(),
            recovery_vault_path: self.recovery_vault_path.clone(),
            destination_temporary_path: self.destination_temporary_path.clone(),
            expected_source: self.expected_source.clone(),
            created_at_unix_ms: self.sealed_at_unix_ms,
        };
        let encoded = serde_json::to_vec(&draft)
            .map_err(|error| TransferError::Journal(error.to_string()))?;
        if *blake3::hash(&encoded).as_bytes() != self.digest {
            return Err(TransferError::InvalidPlan);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferApproval {
    pub plan_id: PlanId,
    pub plan_digest: [u8; 32],
    pub approved_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryVaultReceipt {
    pub execution_id: ExecutionId,
    pub destination: PathBuf,
    pub retained_source: PathBuf,
    pub content_digest: [u8; 32],
    pub byte_size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("cross-volume transfer is disabled")]
    GateDisabled,
    #[error("transfer plan is invalid")]
    InvalidPlan,
    #[error("approval does not match the transfer plan")]
    ApprovalMismatch,
    #[error("source changed after approval")]
    SourceChanged,
    #[error("destination, temporary file, or recovery path already exists")]
    DestinationExists,
    #[error("source and recovery vault must share a volume")]
    RecoveryVaultWrongVolume,
    #[error("destination must be on another local volume")]
    DestinationVolumeInvalid,
    #[error("copy could not be verified")]
    VerificationFailed,
    #[error("transfer left a recoverable artifact: {0}")]
    RecoverableFailure(String),
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("platform failure: {0}")]
    Platform(#[from] platform::PlatformError),
    #[error("journal failure: {0}")]
    Journal(String),
}

pub struct CrossVolumeTransferService {
    reader: Arc<dyn ReadOnlyPlatform>,
    filesystem: Arc<dyn SafeFileOperations>,
    journal: Arc<dyn DurableJournal>,
    enabled: bool,
}

impl std::fmt::Debug for CrossVolumeTransferService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CrossVolumeTransferService")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl CrossVolumeTransferService {
    #[must_use]
    pub fn new(
        reader: Arc<dyn ReadOnlyPlatform>,
        filesystem: Arc<dyn SafeFileOperations>,
        journal: Arc<dyn DurableJournal>,
    ) -> Self {
        let requested = cfg!(all(windows, feature = "windows-cross-volume-audited"))
            && std::env::var("WORKING_NAME_ENABLE_CROSS_VOLUME")
                .is_ok_and(|value| value == "I_ACCEPT_RECOVERY_VAULT_RETENTION");
        // Remains fail-closed until Windows ACL/ADS/EFS preservation and every
        // post-vault recovery branch pass the dedicated audit suite.
        let enabled = requested && metadata_preservation_audited();
        Self {
            reader,
            filesystem,
            journal,
            enabled,
        }
    }

    pub fn execute(
        &self,
        plan: &CrossVolumeTransferPlan,
        approval: &TransferApproval,
        now_unix_ms: i64,
    ) -> Result<RecoveryVaultReceipt, TransferError> {
        if !self.enabled {
            return Err(TransferError::GateDisabled);
        }
        plan.verify_integrity()?;
        if approval.plan_id != plan.id || approval.plan_digest != plan.digest {
            return Err(TransferError::ApprovalMismatch);
        }
        if path_exists(&plan.destination)
            || path_exists(&plan.destination_temporary_path)
            || path_exists(&plan.recovery_vault_path)
        {
            return Err(TransferError::DestinationExists);
        }

        let current =
            self.reader
                .fingerprint(&plan.source, true, plan.expected_source.byte_size)?;
        if !fingerprints_match(&plan.expected_source, &current) {
            return Err(TransferError::SourceChanged);
        }
        validate_volumes(self.reader.as_ref(), plan)?;

        let execution_id = ExecutionId::new();
        let copy_step = OperationStepId::new();
        let vault_step = OperationStepId::new();
        let publish_step = OperationStepId::new();
        let mut sequence = 0_u64;
        let approved_payload = serde_json::to_vec(&(plan, approval))
            .map_err(|error| TransferError::Journal(error.to_string()))?;
        let approved = OperationJournalEvent::new(
            execution_id,
            sequence,
            None,
            JournalEventKind::ApprovedDurable,
            &approved_payload,
            None,
            now_unix_ms,
        );
        let mut previous = Some(approved.event_digest);
        self.journal.append(approved).map_err(map_journal_error)?;
        self.journal.flush().map_err(map_journal_error)?;

        previous = Some(self.append_intent(
            execution_id,
            &mut sequence,
            copy_step,
            b"copy_to_destination_temporary",
            previous,
            now_unix_ms,
        )?);
        copy_verified(
            &plan.source,
            &plan.destination_temporary_path,
            plan.expected_source.byte_size,
            plan.expected_source
                .content_digest
                .ok_or(TransferError::SourceChanged)?,
        )?;
        previous = Some(self.append_event(
            execution_id,
            &mut sequence,
            Some(copy_step),
            JournalEventKind::AppliedObserved,
            b"destination_temporary_verified",
            previous,
            now_unix_ms,
        )?);

        previous = Some(self.append_intent(
            execution_id,
            &mut sequence,
            vault_step,
            b"rename_source_to_recovery_vault",
            previous,
            now_unix_ms,
        )?);
        self.filesystem
            .rename_same_volume_no_replace(&RenameRequest {
                source: plan.source.clone(),
                destination: plan.recovery_vault_path.clone(),
                expected_identity: plan.expected_source.native_identity.clone(),
                expected_byte_size: plan.expected_source.byte_size,
                expected_modified_at_ns: plan.expected_source.modified_at_ns,
                expected_attributes: plan.expected_source.attributes,
                expected_content_digest: plan
                    .expected_source
                    .content_digest
                    .ok_or(TransferError::SourceChanged)?,
                maximum_hash_bytes: crate::HARD_MAX_REHASH_BYTES,
            })?;
        previous = Some(self.append_event(
            execution_id,
            &mut sequence,
            Some(vault_step),
            JournalEventKind::AppliedObserved,
            b"source_retained_in_recovery_vault",
            previous,
            now_unix_ms,
        )?);

        previous = Some(self.append_intent(
            execution_id,
            &mut sequence,
            publish_step,
            b"publish_verified_destination",
            previous,
            now_unix_ms,
        )?);
        let temporary_fingerprint = self.reader.fingerprint(
            &plan.destination_temporary_path,
            true,
            plan.expected_source.byte_size,
        )?;
        if temporary_fingerprint.content_digest != plan.expected_source.content_digest {
            return Err(TransferError::VerificationFailed);
        }
        if let Err(error) = self
            .filesystem
            .rename_same_volume_no_replace(&RenameRequest {
                source: plan.destination_temporary_path.clone(),
                destination: plan.destination.clone(),
                expected_identity: temporary_fingerprint.native_identity,
                expected_byte_size: temporary_fingerprint.byte_size,
                expected_modified_at_ns: temporary_fingerprint.modified_at_ns,
                expected_attributes: temporary_fingerprint.attributes,
                expected_content_digest: temporary_fingerprint
                    .content_digest
                    .ok_or(TransferError::VerificationFailed)?,
                maximum_hash_bytes: crate::HARD_MAX_REHASH_BYTES,
            })
        {
            let vault_fingerprint = self.reader.fingerprint(
                &plan.recovery_vault_path,
                true,
                plan.expected_source.byte_size,
            )?;
            let rollback = self
                .filesystem
                .rename_same_volume_no_replace(&RenameRequest {
                    source: plan.recovery_vault_path.clone(),
                    destination: plan.source.clone(),
                    expected_identity: vault_fingerprint.native_identity,
                    expected_byte_size: vault_fingerprint.byte_size,
                    expected_modified_at_ns: vault_fingerprint.modified_at_ns,
                    expected_attributes: vault_fingerprint.attributes,
                    expected_content_digest: vault_fingerprint
                        .content_digest
                        .ok_or(TransferError::VerificationFailed)?,
                    maximum_hash_bytes: crate::HARD_MAX_REHASH_BYTES,
                });
            return Err(TransferError::RecoverableFailure(format!(
                "publication failed; source rollback result: {}; cause: {error}",
                if rollback.is_ok() {
                    "restored"
                } else {
                    "manual review required"
                }
            )));
        }
        previous = Some(self.append_event(
            execution_id,
            &mut sequence,
            Some(publish_step),
            JournalEventKind::AppliedObserved,
            b"destination_published_without_replacement",
            previous,
            now_unix_ms,
        )?);

        sequence = sequence.saturating_add(1);
        let completed = OperationJournalEvent::new(
            execution_id,
            sequence,
            None,
            JournalEventKind::ExecutionFinished,
            &plan.digest,
            previous,
            now_unix_ms,
        );
        self.journal.append(completed).map_err(map_journal_error)?;
        self.journal.flush().map_err(map_journal_error)?;

        Ok(RecoveryVaultReceipt {
            execution_id,
            destination: plan.destination.clone(),
            retained_source: plan.recovery_vault_path.clone(),
            content_digest: plan
                .expected_source
                .content_digest
                .ok_or(TransferError::SourceChanged)?,
            byte_size: plan.expected_source.byte_size,
        })
    }

    fn append_intent(
        &self,
        execution_id: ExecutionId,
        sequence: &mut u64,
        step_id: OperationStepId,
        payload: &[u8],
        previous: Option<[u8; 32]>,
        now_unix_ms: i64,
    ) -> Result<[u8; 32], TransferError> {
        self.append_event(
            execution_id,
            sequence,
            Some(step_id),
            JournalEventKind::IntentDurable,
            payload,
            previous,
            now_unix_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn append_event(
        &self,
        execution_id: ExecutionId,
        sequence: &mut u64,
        step_id: Option<OperationStepId>,
        kind: JournalEventKind,
        payload: &[u8],
        previous: Option<[u8; 32]>,
        now_unix_ms: i64,
    ) -> Result<[u8; 32], TransferError> {
        *sequence = sequence.saturating_add(1);
        let event = OperationJournalEvent::new(
            execution_id,
            *sequence,
            step_id,
            kind,
            payload,
            previous,
            now_unix_ms,
        );
        let digest = event.event_digest;
        self.journal.append(event).map_err(map_journal_error)?;
        self.journal.flush().map_err(map_journal_error)?;
        Ok(digest)
    }
}

const fn metadata_preservation_audited() -> bool {
    false
}

fn validate_volumes(
    reader: &dyn ReadOnlyPlatform,
    plan: &CrossVolumeTransferPlan,
) -> Result<(), TransferError> {
    let source_parent = plan.source.parent().ok_or(TransferError::InvalidPlan)?;
    let vault_parent = plan
        .recovery_vault_path
        .parent()
        .ok_or(TransferError::InvalidPlan)?;
    let destination_parent = plan
        .destination
        .parent()
        .ok_or(TransferError::InvalidPlan)?;
    let source_volume = reader.inspect_volume(source_parent)?;
    let vault_volume = reader.inspect_volume(vault_parent)?;
    let destination_volume = reader.inspect_volume(destination_parent)?;
    if [&source_volume, &vault_volume, &destination_volume]
        .into_iter()
        .any(|volume| {
            !volume.local
                || volume.removable
                || !volume
                    .filesystem_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("NTFS"))
        })
    {
        return Err(TransferError::DestinationVolumeInvalid);
    }
    if source_volume.stable_identifier != vault_volume.stable_identifier {
        return Err(TransferError::RecoveryVaultWrongVolume);
    }
    if source_volume.stable_identifier == destination_volume.stable_identifier {
        return Err(TransferError::DestinationVolumeInvalid);
    }
    Ok(())
}

fn copy_verified(
    source: &Path,
    temporary: &Path,
    expected_size: u64,
    expected_digest: [u8; 32],
) -> Result<(), TransferError> {
    let source_bytes = std::fs::read(source)?;
    if u64::try_from(source_bytes.len()).unwrap_or(u64::MAX) != expected_size
        || *blake3::hash(&source_bytes).as_bytes() != expected_digest
    {
        return Err(TransferError::SourceChanged);
    }
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    destination.write_all(&source_bytes)?;
    destination.flush()?;
    destination.sync_all()?;
    if *blake3::hash(&std::fs::read(temporary)?).as_bytes() != expected_digest {
        return Err(TransferError::VerificationFailed);
    }
    Ok(())
}

fn fingerprints_match(expected: &FileFingerprint, observed: &FileFingerprint) -> bool {
    expected.byte_size == observed.byte_size
        && expected.content_digest == observed.content_digest
        && expected.native_identity.volume.stable_identifier
            == observed.native_identity.volume.stable_identifier
        && expected.native_identity.object_key == observed.native_identity.object_key
        && observed.native_identity.link_count == 1
        && observed.native_identity.reparse_tag.is_none()
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn map_journal_error(error: OperationsError) -> TransferError {
    TransferError::Journal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity};

    #[test]
    fn invalid_transfer_aliases_are_rejected_before_io() {
        let path = PathBuf::from("same");
        let result = CrossVolumeTransferDraft {
            id: PlanId::new(),
            source: path.clone(),
            destination: path.clone(),
            recovery_vault_path: PathBuf::from("vault"),
            destination_temporary_path: PathBuf::from("same.partial"),
            expected_source: FileFingerprint {
                native_identity: NativeFileIdentity {
                    volume: VolumeIdentity {
                        platform: PlatformKind::Windows,
                        stable_identifier: "source".to_owned(),
                        filesystem_type: Some("NTFS".to_owned()),
                        case_sensitive: false,
                        removable: false,
                        local: true,
                    },
                    object_key: vec![1],
                    parent_key: vec![2],
                    leaf_name: NativePath {
                        encoding: PathEncoding::WindowsUtf16Le,
                        bytes: vec![1, 0],
                    },
                    link_count: 1,
                    reparse_tag: None,
                },
                byte_size: 1,
                modified_at_ns: Some(1),
                created_at_ns: Some(1),
                attributes: 0,
                quick_digest: None,
                content_digest: Some([1; 32]),
            },
            created_at_unix_ms: 1,
        }
        .seal();
        assert!(matches!(result, Err(TransferError::InvalidPlan)));
    }
}
