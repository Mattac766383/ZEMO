//! Sealed operation plans, durable intent logging and conditional rollback.

mod safety;
mod transfer;

pub use safety::*;
pub use transfer::*;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use domain::{
    ApprovalReceipt, ExecutionId, ExecutionState, JournalDiagnostic, JournalDiagnosticScope,
    JournalEventKind, NativePath, OperationJournalEvent, OperationKind, OperationStep,
    OperationStepId, PlanDraft, PlanId, ProposalAction, ProposalRevision, RecoveryObservation,
    ReviewState, SealedPlan, StepPrecondition,
};
use platform::{PlatformError, RenameRequest, SafeFileOperations};
use serde::{Deserialize, Serialize};
use simulation::SimulationOutcome;
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use zeroize::Zeroize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyGate {
    pub enabled: bool,
    pub reason: String,
}

impl ApplyGate {
    #[must_use]
    pub fn from_environment() -> Self {
        let explicitly_enabled = std::env::var("WORKING_NAME_ENABLE_APPLY")
            .is_ok_and(|value| value == "I_UNDERSTAND_THE_RISK");
        let windows = cfg!(windows);
        Self {
            enabled: explicitly_enabled && windows,
            reason: if !windows {
                "Apply environnemental reste limité à Windows NTFS.".to_owned()
            } else if !explicitly_enabled {
                "Apply reste verrouillé tant que le gate local explicite n’est pas activé."
                    .to_owned()
            } else {
                "Apply Windows explicitement activé.".to_owned()
            },
        }
    }

    #[must_use]
    pub fn for_in_process_host() -> Self {
        Self {
            enabled: false,
            reason:
                "Apply exige l’exécuteur isolé; le processus UI ne possède aucune capacité de mutation."
                    .to_owned(),
        }
    }

    /// Capability gate for the dedicated approved-plan application service.
    /// User approval, frozen-plan validation, and durable journaling remain
    /// mandatory; this only describes native mutation availability.
    #[must_use]
    pub fn for_approved_execution_host() -> Self {
        // macOS Apply is product-qualified. Windows Apply is compile-gated:
        // `windows-apply-qualified` is enabled only by the native Windows CI
        // job after NTFS / executor / rollback / sandbox qualification PASS.
        let supported = cfg!(target_os = "macos")
            || (cfg!(windows) && cfg!(feature = "windows-apply-qualified"));
        Self {
            enabled: supported,
            reason: if supported {
                "L’organisation peut être appliquée après votre confirmation.".to_owned()
            } else if cfg!(windows) {
                "Cette version Windows propose une organisation à examiner ; le déplacement réel des fichiers n’est pas encore disponible.".to_owned()
            } else {
                "Cette version propose une organisation à examiner ; le déplacement réel des fichiers n’est pas disponible sur cette plateforme.".to_owned()
            },
        }
    }
}

pub trait DurableJournal: Send + Sync {
    fn append(&self, event: OperationJournalEvent) -> Result<(), OperationsError>;
    fn flush(&self) -> Result<(), OperationsError>;
    fn events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError>;

    fn diagnostics(&self) -> Vec<JournalDiagnostic> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct MemoryJournal {
    events: Mutex<Vec<OperationJournalEvent>>,
}

impl MemoryJournal {
    fn lock(&self) -> Result<MutexGuard<'_, Vec<OperationJournalEvent>>, OperationsError> {
        self.events
            .lock()
            .map_err(|_| OperationsError::Journal("journal mutex poisoned".to_owned()))
    }
}

impl DurableJournal for MemoryJournal {
    fn append(&self, event: OperationJournalEvent) -> Result<(), OperationsError> {
        self.lock()?.push(event);
        Ok(())
    }

    fn flush(&self) -> Result<(), OperationsError> {
        Ok(())
    }

    fn events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError> {
        Ok(self
            .lock()?
            .iter()
            .filter(|event| event.execution_id == execution_id)
            .cloned()
            .collect())
    }
}

/// Append-only recovery journal. SQLite remains the catalog source of truth,
/// while this independently flushed file covers the non-atomic boundary
/// between a database commit and an NTFS rename.
pub struct JournalKey([u8; 32]);

impl JournalKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn derive(key_material: &[u8]) -> Self {
        Self(blake3::derive_key(
            "working-name operation recovery journal v1",
            key_material,
        ))
    }
}

impl Drop for JournalKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedJournalRecord {
    version: u32,
    nonce: String,
    ciphertext: String,
}

type JournalChainHead = (u64, [u8; 32]);
type JournalChainHeads = HashMap<ExecutionId, JournalChainHead>;

pub struct FileJournal {
    path: PathBuf,
    file: Mutex<File>,
    cipher: XChaCha20Poly1305,
    last_events: Mutex<JournalChainHeads>,
}

impl FileJournal {
    pub fn open(path: impl Into<PathBuf>, key: JournalKey) -> Result<Self, OperationsError> {
        let path = path.into();
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(OperationsError::Journal(
                "journal path cannot be a symbolic link".to_owned(),
            ));
        }
        let cipher = XChaCha20Poly1305::new(&Key::from(key.0));
        let existing = if path.exists() {
            read_encrypted_events(&path, &cipher)?
        } else {
            Vec::new()
        };
        let mut last_events = HashMap::new();
        validate_event_chains(&existing, &mut last_events)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        Ok(Self {
            path,
            file: Mutex::new(file),
            cipher,
            last_events: Mutex::new(last_events),
        })
    }

    pub fn open_or_locked(
        path: impl Into<PathBuf>,
        key: JournalKey,
        detected_at_unix_ms: i64,
    ) -> Arc<dyn DurableJournal> {
        let path = path.into();
        match Self::open(path, key) {
            Ok(journal) => Arc::new(journal),
            Err(error) => Arc::new(LockedJournal::from_open_error(error, detected_at_unix_ms)),
        }
    }

    fn lock_file(&self) -> Result<MutexGuard<'_, File>, OperationsError> {
        self.file
            .lock()
            .map_err(|_| OperationsError::Journal("journal file mutex poisoned".to_owned()))
    }

    fn lock_last(&self) -> Result<MutexGuard<'_, JournalChainHeads>, OperationsError> {
        self.last_events
            .lock()
            .map_err(|_| OperationsError::Journal("journal chain mutex poisoned".to_owned()))
    }

    pub fn pending_execution_ids(&self) -> Result<Vec<ExecutionId>, OperationsError> {
        let events = read_encrypted_events(&self.path, &self.cipher)?;
        let mut terminal = HashMap::<ExecutionId, bool>::new();
        for event in events {
            let is_terminal = event.kind == JournalEventKind::ExecutionFinished;
            terminal
                .entry(event.execution_id)
                .and_modify(|value| *value = is_terminal)
                .or_insert(is_terminal);
        }
        let mut pending = terminal
            .into_iter()
            .filter_map(|(execution_id, is_terminal)| (!is_terminal).then_some(execution_id))
            .collect::<Vec<_>>();
        pending.sort();
        Ok(pending)
    }

    pub fn contains_plan_id(&self, plan_id: PlanId) -> Result<bool, OperationsError> {
        let events = read_encrypted_events(&self.path, &self.cipher)?;
        for event in events
            .into_iter()
            .filter(|event| event.kind == JournalEventKind::ApprovedDurable)
        {
            if let Ok((plan, _, _)) = serde_json::from_slice::<(
                SealedPlan,
                ApprovalReceipt,
                HashMap<domain::RootId, PathBuf>,
            )>(&event.payload)
                && plan.id == plan_id
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct LockedJournal {
    diagnostic: JournalDiagnostic,
}

impl LockedJournal {
    #[must_use]
    pub fn from_open_error(error: OperationsError, detected_at_unix_ms: i64) -> Self {
        let technical = error.to_string();
        let (code, message) = if technical.contains("decrypt")
            || technical.contains("authentication")
            || technical.contains("cipher")
        {
            (
                "external_journal_authentication_failed",
                "The encrypted recovery journal could not be authenticated.",
            )
        } else if technical.contains("sequence")
            || technical.contains("hash chain")
            || technical.contains("event chain")
            || technical.contains("journal chain")
            || technical.contains("chain verification")
        {
            (
                "external_journal_chain_invalid",
                "The encrypted recovery journal chain is invalid.",
            )
        } else if technical.contains("invalid journal record")
            || technical.contains("unsupported journal version")
            || technical.contains("nonce")
        {
            (
                "external_journal_record_invalid",
                "The encrypted recovery journal contains an invalid record.",
            )
        } else {
            (
                "external_journal_unavailable",
                "The encrypted recovery journal is unavailable.",
            )
        };
        Self {
            diagnostic: JournalDiagnostic {
                scope: JournalDiagnosticScope::External,
                execution_id: None,
                code: code.to_owned(),
                message: message.to_owned(),
                detected_at_unix_ms,
                recovery_available: false,
                rollback_available: false,
            },
        }
    }

    #[must_use]
    pub fn diagnostic(&self) -> &JournalDiagnostic {
        &self.diagnostic
    }

    fn locked_error(&self) -> OperationsError {
        OperationsError::Journal(format!(
            "{}: {}",
            self.diagnostic.code, self.diagnostic.message
        ))
    }
}

impl DurableJournal for LockedJournal {
    fn append(&self, _event: OperationJournalEvent) -> Result<(), OperationsError> {
        Err(self.locked_error())
    }

    fn flush(&self) -> Result<(), OperationsError> {
        Err(self.locked_error())
    }

    fn events(
        &self,
        _execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError> {
        Err(self.locked_error())
    }

    fn diagnostics(&self) -> Vec<JournalDiagnostic> {
        vec![self.diagnostic.clone()]
    }
}

impl std::fmt::Debug for FileJournal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileJournal")
            .field("path", &self.path)
            .field("cipher", &"<XChaCha20-Poly1305>")
            .finish_non_exhaustive()
    }
}

impl DurableJournal for FileJournal {
    fn append(&self, event: OperationJournalEvent) -> Result<(), OperationsError> {
        let mut last_events = self.lock_last()?;
        let expected = last_events.get(&event.execution_id).copied();
        match expected {
            Some((sequence, digest))
                if event.sequence == sequence.saturating_add(1) && event.verify(Some(digest)) => {}
            None if event.sequence == 0 && event.verify(None) => {}
            _ => {
                return Err(OperationsError::Journal(
                    "journal event sequence or hash chain is invalid".to_owned(),
                ));
            }
        }
        let plaintext = serde_json::to_vec(&event)
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).map_err(|error| OperationsError::Journal(error.to_string()))?;
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: b"working-name-operation-journal-v1",
                },
            )
            .map_err(|_| OperationsError::Journal("journal encryption failed".to_owned()))?;
        let record = EncryptedJournalRecord {
            version: 1,
            nonce: BASE64_STANDARD.encode(nonce),
            ciphertext: BASE64_STANDARD.encode(ciphertext),
        };
        let mut encoded = serde_json::to_vec(&record)
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        encoded.push(b'\n');
        let mut file = self.lock_file()?;
        file.write_all(&encoded)
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        last_events.insert(event.execution_id, (event.sequence, event.event_digest));
        Ok(())
    }

    fn flush(&self) -> Result<(), OperationsError> {
        let mut file = self.lock_file()?;
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|error| OperationsError::Journal(error.to_string()))
    }

    fn events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, OperationsError> {
        Ok(read_encrypted_events(&self.path, &self.cipher)?
            .into_iter()
            .filter(|event| event.execution_id == execution_id)
            .collect())
    }
}

fn read_encrypted_events(
    path: &Path,
    cipher: &XChaCha20Poly1305,
) -> Result<Vec<OperationJournalEvent>, OperationsError> {
    let file = File::open(path).map_err(|error| OperationsError::Journal(error.to_string()))?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| OperationsError::Journal(error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: EncryptedJournalRecord = serde_json::from_str(&line).map_err(|error| {
            OperationsError::Journal(format!("invalid journal record: {error}"))
        })?;
        if record.version != 1 {
            return Err(OperationsError::Journal(
                "unsupported journal version".to_owned(),
            ));
        }
        let nonce = BASE64_STANDARD
            .decode(record.nonce)
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        let nonce: [u8; 24] = nonce
            .try_into()
            .map_err(|_| OperationsError::Journal("invalid journal nonce".to_owned()))?;
        let ciphertext = BASE64_STANDARD
            .decode(record.ciphertext)
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        let plaintext = cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &ciphertext,
                    aad: b"working-name-operation-journal-v1",
                },
            )
            .map_err(|_| OperationsError::Journal("journal authentication failed".to_owned()))?;
        events.push(
            serde_json::from_slice::<OperationJournalEvent>(&plaintext)
                .map_err(|error| OperationsError::Journal(error.to_string()))?,
        );
    }
    let mut last = HashMap::new();
    validate_event_chains(&events, &mut last)?;
    Ok(events)
}

fn validate_event_chains(
    events: &[OperationJournalEvent],
    last: &mut JournalChainHeads,
) -> Result<(), OperationsError> {
    for event in events {
        let previous = last.get(&event.execution_id).copied();
        let valid = match previous {
            Some((sequence, digest)) => {
                event.sequence == sequence.saturating_add(1) && event.verify(Some(digest))
            }
            None => event.sequence == 0 && event.verify(None),
        };
        if !valid {
            return Err(OperationsError::Journal(
                "journal chain verification failed".to_owned(),
            ));
        }
        last.insert(event.execution_id, (event.sequence, event.event_digest));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub id: ExecutionId,
    pub state: ExecutionState,
    pub applied_steps: Vec<OperationStepId>,
    pub error: Option<String>,
}

pub struct OperationExecutor {
    filesystem: Arc<dyn SafeFileOperations>,
    journal: Arc<dyn DurableJournal>,
    gate: ApplyGate,
}

impl std::fmt::Debug for OperationExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationExecutor")
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OperationsError {
    #[error("Apply is disabled: {0}")]
    GateDisabled(String),
    #[error("approval does not match the sealed plan")]
    ApprovalMismatch,
    #[error("sealed plan failed canonical integrity validation")]
    PlanIntegrity,
    #[error("proposal and simulation revisions do not match")]
    StaleSimulation,
    #[error("root is not registered")]
    RootMissing,
    #[error("native path is invalid")]
    InvalidNativePath,
    #[error("resolved path escapes its registered root")]
    OutsideRoot,
    #[error("operation precondition is incomplete")]
    MissingPrecondition,
    #[error("filesystem operation failed: {0}")]
    Platform(#[from] PlatformError),
    #[error("journal failure: {0}")]
    Journal(String),
    #[error("plan cannot be sealed: {0}")]
    Seal(#[from] domain::PlanSealError),
}

impl OperationExecutor {
    #[must_use]
    pub fn new(
        filesystem: Arc<dyn SafeFileOperations>,
        journal: Arc<dyn DurableJournal>,
        gate: ApplyGate,
    ) -> Self {
        Self {
            filesystem,
            journal,
            gate,
        }
    }

    #[must_use]
    pub const fn gate(&self) -> &ApplyGate {
        &self.gate
    }

    pub fn execute(
        &self,
        plan: &SealedPlan,
        approval: &ApprovalReceipt,
        roots: &HashMap<domain::RootId, PathBuf>,
        now_unix_ms: i64,
    ) -> Result<ExecutionReport, OperationsError> {
        if !self.gate.enabled {
            return Err(OperationsError::GateDisabled(self.gate.reason.clone()));
        }
        if plan.verify_integrity().is_err() {
            return Err(OperationsError::PlanIntegrity);
        }
        if approval.plan_id != plan.id
            || approval.plan_digest != plan.digest
            || approval.scope_digest != plan.digest
        {
            return Err(OperationsError::ApprovalMismatch);
        }

        let execution_id = ExecutionId::new();
        let mut sequence = 0_u64;
        let mut previous = None;
        let approved_payload = serde_json::to_vec(&(plan, approval, roots))
            .map_err(|error| OperationsError::Journal(error.to_string()))?;
        let approved = journal_event(
            execution_id,
            sequence,
            None,
            JournalEventKind::ApprovedDurable,
            &approved_payload,
            previous,
            now_unix_ms,
        );
        previous = Some(approved.event_digest);
        self.journal.append(approved)?;
        self.journal.flush()?;

        let mut applied_steps = Vec::new();
        for step in &plan.steps {
            sequence = sequence.saturating_add(1);
            let payload = serde_json::to_vec(step)
                .map_err(|error| OperationsError::Journal(error.to_string()))?;
            let intent = journal_event(
                execution_id,
                sequence,
                Some(step.id),
                JournalEventKind::IntentDurable,
                &payload,
                previous,
                now_unix_ms,
            );
            previous = Some(intent.event_digest);
            self.journal.append(intent)?;
            self.journal.flush()?;

            let result = self.execute_step(step, roots);
            sequence = sequence.saturating_add(1);
            match result {
                Ok(()) => {
                    applied_steps.push(step.id);
                    let observed = journal_event(
                        execution_id,
                        sequence,
                        Some(step.id),
                        JournalEventKind::AppliedObserved,
                        &payload,
                        previous,
                        now_unix_ms,
                    );
                    previous = Some(observed.event_digest);
                    self.journal.append(observed)?;
                    self.journal.flush()?;
                }
                Err(error) => {
                    let failed = journal_event(
                        execution_id,
                        sequence,
                        Some(step.id),
                        JournalEventKind::StepFailed,
                        error.to_string().as_bytes(),
                        previous,
                        now_unix_ms,
                    );
                    self.journal.append(failed)?;
                    self.journal.flush()?;
                    return Ok(ExecutionReport {
                        id: execution_id,
                        state: if applied_steps.is_empty() {
                            ExecutionState::Failed
                        } else {
                            ExecutionState::Partial
                        },
                        applied_steps,
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        sequence = sequence.saturating_add(1);
        self.journal.append(journal_event(
            execution_id,
            sequence,
            None,
            JournalEventKind::ExecutionFinished,
            &plan.digest,
            previous,
            now_unix_ms,
        ))?;
        self.journal.flush()?;
        Ok(ExecutionReport {
            id: execution_id,
            state: ExecutionState::Applied,
            applied_steps,
            error: None,
        })
    }

    fn execute_step(
        &self,
        step: &OperationStep,
        roots: &HashMap<domain::RootId, PathBuf>,
    ) -> Result<(), OperationsError> {
        match step.kind {
            OperationKind::NoOp => Ok(()),
            OperationKind::CreateDirectory => {
                let root_id = step
                    .destination_root_id
                    .ok_or(OperationsError::RootMissing)?;
                let root = roots.get(&root_id).ok_or(OperationsError::RootMissing)?;
                let relative = step
                    .destination_path
                    .as_ref()
                    .ok_or(OperationsError::InvalidNativePath)?;
                let destination = resolve_within_root(root, relative)?;
                self.filesystem.create_directory_no_replace(&destination)?;
                Ok(())
            }
            OperationKind::RemoveDirectoryIfEmpty => {
                let root_id = step
                    .destination_root_id
                    .ok_or(OperationsError::RootMissing)?;
                let root = roots.get(&root_id).ok_or(OperationsError::RootMissing)?;
                let relative = step
                    .destination_path
                    .as_ref()
                    .ok_or(OperationsError::InvalidNativePath)?;
                let destination = resolve_within_root(root, relative)?;
                self.filesystem.remove_directory_if_empty(&destination)?;
                Ok(())
            }
            OperationKind::RenameEntrySameVolume => {
                let source_root = roots
                    .get(&step.source_root_id.ok_or(OperationsError::RootMissing)?)
                    .ok_or(OperationsError::RootMissing)?;
                let destination_root = roots
                    .get(
                        &step
                            .destination_root_id
                            .ok_or(OperationsError::RootMissing)?,
                    )
                    .ok_or(OperationsError::RootMissing)?;
                if source_root != destination_root {
                    return Err(OperationsError::Platform(PlatformError::Unsupported(
                        "cross-root move is disabled".to_owned(),
                    )));
                }
                let source = resolve_within_root(
                    source_root,
                    step.source_path
                        .as_ref()
                        .ok_or(OperationsError::InvalidNativePath)?,
                )?;
                let destination = resolve_within_root(
                    destination_root,
                    step.destination_path
                        .as_ref()
                        .ok_or(OperationsError::InvalidNativePath)?,
                )?;
                let fingerprint = step
                    .preconditions
                    .iter()
                    .find_map(|condition| match condition {
                        StepPrecondition::SourceMatches { fingerprint } => {
                            Some(fingerprint.as_ref())
                        }
                        _ => None,
                    })
                    .ok_or(OperationsError::MissingPrecondition)?;
                let content_digest = fingerprint
                    .content_digest
                    .ok_or(OperationsError::MissingPrecondition)?;
                self.filesystem
                    .rename_same_volume_no_replace(&RenameRequest {
                        source,
                        destination,
                        expected_identity: fingerprint.native_identity.clone(),
                        expected_byte_size: fingerprint.byte_size,
                        expected_modified_at_ns: fingerprint.modified_at_ns,
                        expected_attributes: fingerprint.attributes,
                        expected_content_digest: content_digest,
                        maximum_hash_bytes: HARD_MAX_REHASH_BYTES,
                    })?;
                Ok(())
            }
        }
    }
}

pub fn compile_plan(
    proposal: &ProposalRevision,
    simulation: &SimulationOutcome,
) -> Result<SealedPlan, OperationsError> {
    let proposal_digest = *blake3::hash(
        &serde_json::to_vec(proposal)
            .map_err(|error| OperationsError::Journal(error.to_string()))?,
    )
    .as_bytes();
    if simulation.simulation.proposal_id != proposal.id
        || simulation.simulation.proposal_digest != proposal_digest
    {
        return Err(OperationsError::StaleSimulation);
    }
    let items = proposal
        .items
        .iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let mut steps = Vec::new();
    for directory in &simulation.directories {
        let inverse = OperationStep {
            id: OperationStepId::new(),
            proposal_item_id: None,
            sequence: u32::try_from(steps.len() + 1).unwrap_or(u32::MAX),
            kind: OperationKind::RemoveDirectoryIfEmpty,
            source_root_id: None,
            source_path: None,
            destination_root_id: Some(directory.root_id),
            destination_path: Some(directory.path.clone()),
            preconditions: Vec::new(),
            inverse: None,
        };
        steps.push(OperationStep {
            id: OperationStepId::new(),
            proposal_item_id: None,
            sequence: u32::try_from(steps.len() + 1).unwrap_or(u32::MAX),
            kind: OperationKind::CreateDirectory,
            source_root_id: None,
            source_path: None,
            destination_root_id: Some(directory.root_id),
            destination_path: Some(directory.path.clone()),
            preconditions: vec![StepPrecondition::DestinationAbsent {
                root_id: directory.root_id,
                path: directory.path.clone(),
            }],
            inverse: Some(Box::new(inverse)),
        });
    }
    for (index, planned) in simulation.moves.iter().enumerate() {
        let Some(item) = items.get(&planned.item_id) else {
            continue;
        };
        if item.review_state != ReviewState::Accepted {
            continue;
        }
        if !matches!(
            item.action,
            ProposalAction::Move { .. } | ProposalAction::PlaceInReview { .. }
        ) {
            continue;
        }
        let inverse = OperationStep {
            id: OperationStepId::new(),
            proposal_item_id: Some(item.id),
            sequence: u32::try_from(steps.len() + index + 1).unwrap_or(u32::MAX),
            kind: OperationKind::RenameEntrySameVolume,
            source_root_id: Some(planned.destination_root_id),
            source_path: Some(planned.destination_path.clone()),
            destination_root_id: Some(planned.source_root_id),
            destination_path: Some(planned.source_path.clone()),
            preconditions: vec![
                StepPrecondition::SourceMatches {
                    fingerprint: Box::new(planned.fingerprint.clone()),
                },
                StepPrecondition::DestinationAbsent {
                    root_id: planned.source_root_id,
                    path: planned.source_path.clone(),
                },
                StepPrecondition::SameVolume {
                    stable_volume_id: planned
                        .fingerprint
                        .native_identity
                        .volume
                        .stable_identifier
                        .clone(),
                },
                StepPrecondition::SingleLink,
                StepPrecondition::NotReparsePoint,
                StepPrecondition::LocalNtfsVolume,
            ],
            inverse: None,
        };
        steps.push(OperationStep {
            id: OperationStepId::new(),
            proposal_item_id: Some(item.id),
            sequence: u32::try_from(steps.len() + 1).unwrap_or(u32::MAX),
            kind: OperationKind::RenameEntrySameVolume,
            source_root_id: Some(planned.source_root_id),
            source_path: Some(planned.source_path.clone()),
            destination_root_id: Some(planned.destination_root_id),
            destination_path: Some(planned.destination_path.clone()),
            preconditions: vec![
                StepPrecondition::SourceMatches {
                    fingerprint: Box::new(planned.fingerprint.clone()),
                },
                StepPrecondition::DestinationAbsent {
                    root_id: planned.destination_root_id,
                    path: planned.destination_path.clone(),
                },
                StepPrecondition::SameVolume {
                    stable_volume_id: planned
                        .fingerprint
                        .native_identity
                        .volume
                        .stable_identifier
                        .clone(),
                },
                StepPrecondition::SingleLink,
                StepPrecondition::NotReparsePoint,
                StepPrecondition::LocalNtfsVolume,
            ],
            inverse: Some(Box::new(inverse)),
        });
    }

    PlanDraft {
        id: PlanId::new(),
        workspace_id: proposal.workspace_id,
        proposal_id: proposal.id,
        proposal_digest,
        steps,
        created_at_unix_ms: simulation.simulation.simulated_at_unix_ms,
    }
    .seal(proposal_digest)
    .map_err(OperationsError::Seal)
}

#[must_use]
pub fn reconcile_observation(
    expected: &domain::NativeFileIdentity,
    source: Option<&domain::NativeFileIdentity>,
    destination: Option<&domain::NativeFileIdentity>,
) -> RecoveryObservation {
    let source_matches = source.is_some_and(|value| same_identity(expected, value));
    let destination_matches = destination.is_some_and(|value| same_identity(expected, value));
    match (source, destination, source_matches, destination_matches) {
        (Some(_), None, true, _) => RecoveryObservation::NotApplied,
        (None, Some(_), _, true) => RecoveryObservation::Applied,
        (Some(_), Some(_), _, _) => RecoveryObservation::BothEntriesPresent,
        (None, None, _, _) => RecoveryObservation::NeitherEntryPresent,
        _ => RecoveryObservation::IdentityMismatch,
    }
}

fn same_identity(
    expected: &domain::NativeFileIdentity,
    observed: &domain::NativeFileIdentity,
) -> bool {
    expected.volume.stable_identifier == observed.volume.stable_identifier
        && expected.object_key == observed.object_key
}

fn journal_event(
    execution_id: ExecutionId,
    sequence: u64,
    step_id: Option<OperationStepId>,
    kind: JournalEventKind,
    payload: &[u8],
    previous: Option<[u8; 32]>,
    now_unix_ms: i64,
) -> OperationJournalEvent {
    OperationJournalEvent::new(
        execution_id,
        sequence,
        step_id,
        kind,
        payload,
        previous,
        now_unix_ms,
    )
}

fn resolve_within_root(root: &Path, native: &NativePath) -> Result<PathBuf, OperationsError> {
    let relative = decode_native_path(native)?;
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(OperationsError::OutsideRoot);
    }
    Ok(root.join(relative))
}

fn decode_native_path(native: &NativePath) -> Result<PathBuf, OperationsError> {
    match native.encoding {
        domain::PathEncoding::UnixBytes => {
            #[cfg(unix)]
            {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                Ok(PathBuf::from(OsString::from_vec(native.bytes.clone())))
            }
            #[cfg(not(unix))]
            {
                String::from_utf8(native.bytes.clone())
                    .map(PathBuf::from)
                    .map_err(|_| OperationsError::InvalidNativePath)
            }
        }
        domain::PathEncoding::WindowsUtf16Le => {
            let chunks = native.bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return Err(OperationsError::InvalidNativePath);
            }
            let units = chunks
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            #[cfg(windows)]
            {
                use std::{ffi::OsString, os::windows::ffi::OsStringExt};
                Ok(PathBuf::from(OsString::from_wide(&units)))
            }
            #[cfg(not(windows))]
            {
                String::from_utf16(&units)
                    .map(PathBuf::from)
                    .map_err(|_| OperationsError::InvalidNativePath)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{NativeFileIdentity, PathEncoding, PlatformKind, VolumeIdentity};
    use platform::{RenameOutcome, RenameRequest};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct CountingFilesystem {
        mutations: AtomicUsize,
    }

    impl SafeFileOperations for CountingFilesystem {
        fn rename_same_volume_no_replace(
            &self,
            request: &RenameRequest,
        ) -> Result<RenameOutcome, PlatformError> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(RenameOutcome {
                observed_identity: request.expected_identity.clone(),
            })
        }

        fn create_directory_no_replace(&self, _path: &Path) -> Result<(), PlatformError> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn remove_directory_if_empty(&self, _path: &Path) -> Result<(), PlatformError> {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn native(value: &str) -> NativePath {
        NativePath {
            encoding: PathEncoding::UnixBytes,
            bytes: value.as_bytes().to_vec(),
        }
    }

    fn identity() -> NativeFileIdentity {
        NativeFileIdentity {
            volume: VolumeIdentity {
                platform: PlatformKind::Windows,
                stable_identifier: "volume-serial:1".to_owned(),
                filesystem_type: Some("NTFS".to_owned()),
                case_sensitive: false,
                removable: false,
                local: true,
            },
            object_key: vec![1],
            parent_key: vec![2],
            leaf_name: native("source.txt"),
            link_count: 1,
            reparse_tag: None,
        }
    }

    fn rename_plan() -> SealedPlan {
        let source = native("source.txt");
        let destination = native("organized/source.txt");
        let root = domain::RootId::new();
        let fingerprint = domain::FileFingerprint {
            native_identity: identity(),
            byte_size: 10,
            modified_at_ns: Some(1),
            created_at_ns: Some(1),
            attributes: 0,
            quick_digest: None,
            content_digest: Some([5; 32]),
        };
        let inverse = OperationStep {
            id: OperationStepId::new(),
            proposal_item_id: None,
            sequence: 1,
            kind: OperationKind::RenameEntrySameVolume,
            source_root_id: Some(root),
            source_path: Some(destination.clone()),
            destination_root_id: Some(root),
            destination_path: Some(source.clone()),
            preconditions: vec![StepPrecondition::DestinationAbsent {
                root_id: root,
                path: source.clone(),
            }],
            inverse: None,
        };
        PlanDraft {
            id: PlanId::new(),
            workspace_id: domain::WorkspaceId::new(),
            proposal_id: domain::ProposalId::new(),
            proposal_digest: [1; 32],
            steps: vec![OperationStep {
                id: OperationStepId::new(),
                proposal_item_id: None,
                sequence: 1,
                kind: OperationKind::RenameEntrySameVolume,
                source_root_id: Some(root),
                source_path: Some(source),
                destination_root_id: Some(root),
                destination_path: Some(destination.clone()),
                preconditions: vec![
                    StepPrecondition::SourceMatches {
                        fingerprint: Box::new(fingerprint),
                    },
                    StepPrecondition::DestinationAbsent {
                        root_id: root,
                        path: destination,
                    },
                ],
                inverse: Some(Box::new(inverse)),
            }],
            created_at_unix_ms: 1,
        }
        .seal([1; 32])
        .unwrap_or_else(|error| panic!("test plan should seal: {error}"))
    }

    #[test]
    fn approved_execution_host_enables_apply_only_when_platform_is_qualified() {
        let gate = ApplyGate::for_approved_execution_host();
        let expected = cfg!(target_os = "macos")
            || (cfg!(windows) && cfg!(feature = "windows-apply-qualified"));
        assert_eq!(gate.enabled, expected);
        if cfg!(all(windows, not(feature = "windows-apply-qualified"))) {
            assert!(
                gate.reason.contains("n’est pas encore disponible"),
                "Windows Apply must stay propose-only until NTFS qualification: {}",
                gate.reason
            );
        }
    }

    #[test]
    fn disabled_gate_mutates_nothing() {
        let filesystem = Arc::new(CountingFilesystem {
            mutations: AtomicUsize::new(0),
        });
        let executor = OperationExecutor::new(
            filesystem.clone(),
            Arc::new(MemoryJournal::default()),
            ApplyGate {
                enabled: false,
                reason: "test".to_owned(),
            },
        );
        let plan = SealedPlan {
            id: PlanId::new(),
            workspace_id: domain::WorkspaceId::new(),
            proposal_id: domain::ProposalId::new(),
            proposal_digest: [1; 32],
            digest: [2; 32],
            steps: Vec::new(),
            sealed_at_unix_ms: 1,
        };
        let approval = ApprovalReceipt {
            plan_id: plan.id,
            plan_digest: plan.digest,
            actor_id: domain::ActorId::new(),
            scope_digest: plan.digest,
            approved_at_unix_ms: 1,
        };

        assert!(matches!(
            executor.execute(&plan, &approval, &HashMap::new(), 1),
            Err(OperationsError::GateDisabled(_))
        ));
        assert_eq!(filesystem.mutations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn durable_intent_precedes_the_mutation_result() {
        let filesystem = Arc::new(CountingFilesystem {
            mutations: AtomicUsize::new(0),
        });
        let journal = Arc::new(MemoryJournal::default());
        let executor = OperationExecutor::new(
            filesystem.clone(),
            journal.clone(),
            ApplyGate {
                enabled: true,
                reason: "test".to_owned(),
            },
        );
        let plan = rename_plan();
        let root = plan.steps[0].source_root_id.unwrap_or_default();
        let approval = ApprovalReceipt {
            plan_id: plan.id,
            plan_digest: plan.digest,
            actor_id: domain::ActorId::new(),
            scope_digest: plan.digest,
            approved_at_unix_ms: 1,
        };
        let report = executor
            .execute(
                &plan,
                &approval,
                &HashMap::from([(root, PathBuf::from("/registered"))]),
                2,
            )
            .unwrap_or_else(|error| panic!("execution should succeed: {error}"));
        let events = journal
            .events(report.id)
            .unwrap_or_else(|error| panic!("events should be readable: {error}"));

        assert_eq!(filesystem.mutations.load(Ordering::SeqCst), 1);
        assert_eq!(events[0].kind, JournalEventKind::ApprovedDurable);
        assert_eq!(events[1].kind, JournalEventKind::IntentDurable);
        assert_eq!(events[2].kind, JournalEventKind::AppliedObserved);
        assert_eq!(
            events[2].previous_event_digest,
            Some(events[1].event_digest)
        );
    }

    #[test]
    fn recovery_never_guesses_when_both_entries_exist() {
        let expected = identity();
        assert_eq!(
            reconcile_observation(&expected, Some(&expected), Some(&expected)),
            RecoveryObservation::BothEntriesPresent
        );
        assert_eq!(
            reconcile_observation(&expected, Some(&expected), None),
            RecoveryObservation::NotApplied
        );
        assert_eq!(
            reconcile_observation(&expected, None, Some(&expected)),
            RecoveryObservation::Applied
        );
    }

    #[test]
    fn resolved_operation_paths_cannot_escape_the_root() {
        assert!(matches!(
            resolve_within_root(Path::new("/registered"), &native("../escape")),
            Err(OperationsError::OutsideRoot)
        ));
    }

    #[test]
    fn file_journal_is_authenticated_and_fails_closed() {
        let sandbox = tempfile::Builder::new()
            .prefix("supremacy-m8-journal-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("journal sandbox should be created: {error}"));
        let path = sandbox.path().join("operation-recovery.jsonl.enc");
        let journal = FileJournal::open(&path, JournalKey::from_bytes([4; 32]))
            .unwrap_or_else(|error| panic!("journal should open: {error}"));
        let execution_id = ExecutionId::new();
        let event = OperationJournalEvent::new(
            execution_id,
            0,
            None,
            JournalEventKind::ApprovedDurable,
            b"sealed plan and root bindings",
            None,
            1,
        );
        assert!(journal.append(event).is_ok());
        assert!(journal.flush().is_ok());
        assert_eq!(
            journal
                .events(execution_id)
                .unwrap_or_else(|error| panic!("journal should decrypt: {error}"))
                .len(),
            1
        );
        drop(journal);
        let reopened = FileJournal::open(&path, JournalKey::from_bytes([4; 32]))
            .unwrap_or_else(|error| panic!("durable journal should reopen: {error}"));
        assert_eq!(
            reopened
                .events(execution_id)
                .unwrap_or_else(|error| panic!("reopened journal should decrypt: {error}"))
                .len(),
            1
        );
        drop(reopened);

        let mut bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("journal should be readable for tamper test: {error}"));
        if let Some(byte) = bytes.iter_mut().find(|byte| **byte == b'A') {
            *byte = b'B';
        } else if let Some(byte) = bytes.first_mut() {
            *byte ^= 1;
        }
        std::fs::write(&path, &bytes)
            .unwrap_or_else(|error| panic!("tamper write should succeed: {error}"));
        assert!(FileJournal::open(&path, JournalKey::from_bytes([4; 32])).is_err());
        let locked = FileJournal::open_or_locked(&path, JournalKey::from_bytes([4; 32]), 42);
        let diagnostics = locked.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert!(!diagnostics[0].recovery_available);
        assert!(!diagnostics[0].rollback_available);
        assert_eq!(diagnostics[0].detected_at_unix_ms, 42);
        assert!(locked.events(execution_id).is_err());
        assert!(
            locked
                .append(OperationJournalEvent::new(
                    execution_id,
                    1,
                    None,
                    JournalEventKind::ExecutionFinished,
                    b"must not append while locked",
                    None,
                    43,
                ))
                .is_err()
        );
        assert_eq!(
            std::fs::read(&path)
                .unwrap_or_else(|error| panic!("locked journal should remain readable: {error}")),
            bytes,
            "diagnostic open must not rewrite or repair corrupted bytes"
        );
    }
}
