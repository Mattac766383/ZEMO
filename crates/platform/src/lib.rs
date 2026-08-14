//! Operating-system ports.
//!
//! Analysis and mutation are deliberately separate traits.  An analysis
//! component cannot obtain a mutation capability by accident.

use async_trait::async_trait;
use domain::{NativeFileIdentity, NativePath, VolumeIdentity};
use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RemoveKind, RenameMode},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet, hash_map::DefaultHasher},
    fmt, fs,
    hash::{Hash, Hasher},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError, TrySendError},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

/// Every native streaming fingerprint implementation uses one reusable buffer
/// of exactly this size. Execution verification must never allocate a buffer
/// proportional to the file size.
pub const STREAMING_FINGERPRINT_BUFFER_BYTES: usize = 1024 * 1024;
/// Absolute execution-verification ceiling shared by coordinator and worker.
pub const MAX_EXECUTION_FINGERPRINT_BYTES: u64 = domain::MAX_EXECUTION_VERIFICATION_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformErrorClass {
    SharingViolation,
    LockViolation,
    PermissionDenied,
    DiskFull,
    DestinationCollision,
    SourceMissing,
    PathPolicyRefusal,
    AmbiguousMutationOutcome,
    Cancelled,
    VerificationLimit,
    Precondition,
    Unsupported,
    Io,
    SecretStore,
}

impl PlatformErrorClass {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SharingViolation => "sharing_violation",
            Self::LockViolation => "lock_violation",
            Self::PermissionDenied => "permission_denied",
            Self::DiskFull => "disk_full",
            Self::DestinationCollision => "destination_collision",
            Self::SourceMissing => "source_missing",
            Self::PathPolicyRefusal => "path_policy_refusal",
            Self::AmbiguousMutationOutcome => "ambiguous_mutation_outcome",
            Self::Cancelled => "cancelled",
            Self::VerificationLimit => "verification_limit",
            Self::Precondition => "precondition",
            Self::Unsupported => "unsupported",
            Self::Io => "io",
            Self::SecretStore => "secret_store",
        }
    }
}

/// Pure Win32 classification kept in the portable crate so mappings can be
/// tested on non-Windows hosts.
#[must_use]
pub const fn classify_windows_error_code(code: u32) -> PlatformErrorClass {
    match code {
        2 | 3 => PlatformErrorClass::SourceMissing,
        5 => PlatformErrorClass::PermissionDenied,
        32 => PlatformErrorClass::SharingViolation,
        33 => PlatformErrorClass::LockViolation,
        39 | 112 => PlatformErrorClass::DiskFull,
        80 | 183 => PlatformErrorClass::DestinationCollision,
        // ERROR_REPARSE_TAG_INVALID / ERROR_REPARSE_TAG_MISMATCH /
        // ERROR_CANT_ACCESS_FILE are all path-policy refusals here.
        4_390 | 4_394 | 1_920 => PlatformErrorClass::PathPolicyRefusal,
        _ => PlatformErrorClass::Io,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("path is outside the registered root")]
    OutsideRoot,
    #[error("path contains a reparse point or symbolic link")]
    ReparsePoint,
    #[error("cloud placeholder access would hydrate remote content")]
    CloudPlaceholder,
    #[error("path policy refused the filesystem entry")]
    PathPolicyRefusal,
    #[error("filesystem operation is unsupported: {0}")]
    Unsupported(String),
    #[error("filesystem precondition failed: {0}")]
    Precondition(String),
    #[error("file is in use because its sharing mode blocks the operation")]
    SharingViolation,
    #[error("file is in use because a byte-range or filesystem lock blocks the operation")]
    LockViolation,
    #[error("permission denied")]
    PermissionDenied,
    #[error("the destination volume has insufficient free space")]
    DiskFull,
    #[error("destination already exists")]
    DestinationExists,
    #[error("source no longer exists")]
    SourceMissing,
    #[error("filesystem mutation outcome is ambiguous")]
    AmbiguousMutationOutcome,
    #[error("streaming verification was cancelled")]
    Cancelled,
    #[error("file exceeds the {limit_bytes}-byte execution verification bound")]
    VerificationLimitExceeded { limit_bytes: u64 },
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("secret store failure: {0}")]
    SecretStore(String),
}

impl PlatformError {
    #[must_use]
    pub const fn class(&self) -> PlatformErrorClass {
        match self {
            Self::OutsideRoot
            | Self::ReparsePoint
            | Self::CloudPlaceholder
            | Self::PathPolicyRefusal => PlatformErrorClass::PathPolicyRefusal,
            Self::Unsupported(_) => PlatformErrorClass::Unsupported,
            Self::Precondition(_) => PlatformErrorClass::Precondition,
            Self::SharingViolation => PlatformErrorClass::SharingViolation,
            Self::LockViolation => PlatformErrorClass::LockViolation,
            Self::PermissionDenied => PlatformErrorClass::PermissionDenied,
            Self::DiskFull => PlatformErrorClass::DiskFull,
            Self::DestinationExists => PlatformErrorClass::DestinationCollision,
            Self::SourceMissing => PlatformErrorClass::SourceMissing,
            Self::AmbiguousMutationOutcome => PlatformErrorClass::AmbiguousMutationOutcome,
            Self::Cancelled => PlatformErrorClass::Cancelled,
            Self::VerificationLimitExceeded { .. } => PlatformErrorClass::VerificationLimit,
            Self::Io(_) => PlatformErrorClass::Io,
            Self::SecretStore(_) => PlatformErrorClass::SecretStore,
        }
    }

    #[must_use]
    pub const fn retryable_before_mutation(&self) -> bool {
        matches!(self, Self::SharingViolation | Self::LockViolation)
    }

    #[must_use]
    pub fn from_windows_code(code: u32, mutation_outcome_uncertain: bool) -> Self {
        let class = classify_windows_error_code(code);
        if mutation_outcome_uncertain {
            return Self::AmbiguousMutationOutcome;
        }
        match class {
            PlatformErrorClass::SharingViolation => Self::SharingViolation,
            PlatformErrorClass::LockViolation => Self::LockViolation,
            PlatformErrorClass::PermissionDenied => Self::PermissionDenied,
            PlatformErrorClass::DiskFull => Self::DiskFull,
            PlatformErrorClass::DestinationCollision => Self::DestinationExists,
            PlatformErrorClass::SourceMissing => Self::SourceMissing,
            PlatformErrorClass::PathPolicyRefusal => Self::PathPolicyRefusal,
            _ => Self::Io(std::io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(i32::MAX),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FingerprintProgress {
    pub bytes_hashed: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ReadOnlyEntry {
    pub absolute_path: PathBuf,
    pub relative_path: NativePath,
    pub identity: NativeFileIdentity,
    pub byte_size: u64,
    pub modified_at_ns: Option<i128>,
    pub created_at_ns: Option<i128>,
    pub accessed_at_ns: Option<i128>,
    pub attributes: u64,
    pub read_only: bool,
    pub hidden: bool,
    pub cloud_placeholder: bool,
    pub encrypted: bool,
}

#[derive(Debug)]
pub struct EnumerationIssue {
    pub path: PathBuf,
    pub error: PlatformError,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnumerationProgress {
    pub entries_discovered: u64,
    pub files_discovered: u64,
    pub directories_discovered: u64,
    pub bytes_discovered: u64,
    pub errors: u64,
    pub skipped_items: u64,
}

#[derive(Debug, Default)]
pub struct ReadOnlyEnumeration {
    pub files: Vec<ReadOnlyEntry>,
    pub issues: Vec<EnumerationIssue>,
    pub progress: EnumerationProgress,
    pub truncated: bool,
    pub cancelled: bool,
}

pub trait ReadOnlyPlatform: Send + Sync {
    fn inspect_volume(&self, root: &Path) -> Result<VolumeIdentity, PlatformError>;

    fn enumerate_regular_files(
        &self,
        root: &Path,
        max_entries: usize,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(EnumerationProgress),
    ) -> Result<ReadOnlyEnumeration, PlatformError>;

    /// Inspect one root-relative regular file without granting mutation access.
    ///
    /// Real platform adapters should override this with an anchored, targeted
    /// implementation. The enumeration fallback keeps existing mocks source
    /// compatible and deliberately favors safety over performance.
    fn inspect_regular_file(
        &self,
        root: &Path,
        relative_path: &Path,
    ) -> Result<ReadOnlyEntry, PlatformError> {
        let target = validated_scoped_path(root, relative_path).map_err(inspection_error)?;
        let enumeration = self.enumerate_regular_files(root, usize::MAX, &|| false, &mut |_| {})?;
        if let Some(entry) = enumeration
            .files
            .into_iter()
            .find(|entry| entry.absolute_path == target)
        {
            return Ok(entry);
        }
        if let Some(issue) = enumeration
            .issues
            .into_iter()
            .find(|issue| issue.path == target)
        {
            return Err(inspection_error(issue.error));
        }
        Err(PlatformError::SourceMissing)
    }

    fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>, PlatformError>;

    fn read_prefix(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, PlatformError>;

    fn read_bounded_scoped(
        &self,
        root: &Path,
        relative_path: &Path,
        max_bytes: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        let path = validated_scoped_path(root, relative_path)?;
        self.read_bounded(&path, max_bytes)
    }

    fn read_prefix_scoped(
        &self,
        root: &Path,
        relative_path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<u8>, PlatformError> {
        let path = validated_scoped_path(root, relative_path)?;
        self.read_prefix(&path, max_bytes)
    }

    fn fingerprint(
        &self,
        path: &Path,
        include_content_digest: bool,
        max_bytes: u64,
    ) -> Result<domain::FileFingerprint, PlatformError>;

    /// Conservatively fingerprint a file under a hard byte bound.
    ///
    /// Native adapters override this to hash one anchored handle with the
    /// fixed 1 MiB buffer. The fallback preserves simple test mocks while
    /// still enforcing cancellation and the global bound.
    fn fingerprint_streaming(
        &self,
        path: &Path,
        include_content_digest: bool,
        max_bytes: u64,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(FingerprintProgress),
    ) -> Result<domain::FileFingerprint, PlatformError> {
        if max_bytes > MAX_EXECUTION_FINGERPRINT_BYTES {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: MAX_EXECUTION_FINGERPRINT_BYTES,
            });
        }
        if is_cancelled() {
            return Err(PlatformError::Cancelled);
        }
        on_progress(FingerprintProgress {
            bytes_hashed: 0,
            total_bytes: 0,
        });
        let fingerprint = self.fingerprint(path, include_content_digest, max_bytes)?;
        if fingerprint.byte_size > max_bytes {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: max_bytes,
            });
        }
        if is_cancelled() {
            return Err(PlatformError::Cancelled);
        }
        on_progress(FingerprintProgress {
            bytes_hashed: if include_content_digest {
                fingerprint.byte_size
            } else {
                0
            },
            total_bytes: fingerprint.byte_size,
        });
        Ok(fingerprint)
    }
}

fn validated_scoped_path(root: &Path, relative_path: &Path) -> Result<PathBuf, PlatformError> {
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PlatformError::OutsideRoot);
    }
    let canonical_root = fs::canonicalize(root)?;
    let joined = root.join(relative_path);
    let canonical_target = fs::canonicalize(&joined)?;
    if canonical_target == canonical_root || !canonical_target.starts_with(&canonical_root) {
        return Err(PlatformError::OutsideRoot);
    }
    Ok(joined)
}

fn inspection_error(error: PlatformError) -> PlatformError {
    match error {
        PlatformError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            PlatformError::SourceMissing
        }
        PlatformError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            PlatformError::PermissionDenied
        }
        other => other,
    }
}

#[derive(Debug, Clone)]
pub struct RenameRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub expected_identity: NativeFileIdentity,
    pub expected_byte_size: u64,
    pub expected_modified_at_ns: Option<i128>,
    pub expected_attributes: u64,
    pub expected_content_digest: [u8; 32],
    pub maximum_hash_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct RenameOutcome {
    pub observed_identity: NativeFileIdentity,
}

/// This capability is never handed to scanners, parsers, models, or the UI.
pub trait SafeFileOperations: Send + Sync {
    /// Advisory preflight only. The native no-replace primitive remains the
    /// authority against destination races.
    fn validate_destination_absent(&self, path: &Path) -> Result<(), PlatformError> {
        match fs::symlink_metadata(path) {
            Ok(_) => Err(PlatformError::DestinationExists),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(PlatformError::PermissionDenied)
            }
            Err(error) => Err(PlatformError::Io(error)),
        }
    }

    fn rename_same_volume_no_replace(
        &self,
        request: &RenameRequest,
    ) -> Result<RenameOutcome, PlatformError>;

    fn create_directory_no_replace(&self, path: &Path) -> Result<(), PlatformError>;

    fn remove_directory_if_empty(&self, path: &Path) -> Result<(), PlatformError>;
}

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>, PlatformError>;
    async fn store(&self, key: &str, secret: &[u8]) -> Result<(), PlatformError>;
    async fn remove(&self, key: &str) -> Result<(), PlatformError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEventKind {
    Created,
    Modified,
    Moved,
    Removed,
    Metadata,
    Overflow,
    RescanRequired,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeScope {
    File,
    Directory,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChangeHint {
    pub root_token: String,
    pub native_key: Option<Vec<u8>>,
    pub path_after: Option<PathBuf>,
    pub path_before: Option<PathBuf>,
    pub kind: LocalEventKind,
    #[serde(default)]
    pub scope: ChangeScope,
}

pub trait ChangeMonitor: Send + Sync {
    fn start(&self, root: &Path) -> Result<(), PlatformError>;
    fn drain_hints(&self) -> Result<Vec<ChangeHint>, PlatformError>;
    fn drain_hints_with_cancellation(
        &self,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<ChangeHint>, PlatformError> {
        if is_cancelled() {
            Ok(Vec::new())
        } else {
            self.drain_hints()
        }
    }
    fn stop(&self) -> Result<(), PlatformError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PollingStamp {
    scope: ChangeScope,
    byte_size: u64,
    modified_ns: u128,
    read_only: bool,
}

#[derive(Debug)]
struct PollingSnapshot {
    entries: HashMap<PathBuf, PollingStamp>,
    truncated: bool,
    cancelled: bool,
}

#[derive(Debug)]
struct PollingState {
    root: PathBuf,
    root_token: String,
    snapshot: Option<HashMap<PathBuf, PollingStamp>>,
    pending_rescan: bool,
    last_snapshot_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CoalesceKey {
    root_token: String,
    native_key: Option<Vec<u8>>,
    path_after: Option<PathBuf>,
    path_before: Option<PathBuf>,
}

/// Coalesce equivalent local hints while preserving the strongest useful
/// signal for each path pair. A root-level rescan supersedes narrower hints.
#[must_use]
pub fn coalesce_change_hints(hints: impl IntoIterator<Item = ChangeHint>) -> Vec<ChangeHint> {
    let mut output = BTreeMap::<CoalesceKey, ChangeHint>::new();
    let mut rescans = HashSet::new();
    for hint in coalesce_tracked_renames(hints) {
        if hint.kind == LocalEventKind::RescanRequired
            && hint.path_after.is_none()
            && hint.path_before.is_none()
        {
            rescans.insert(hint.root_token.clone());
            output.retain(|key, _| key.root_token != hint.root_token);
        } else if rescans.contains(&hint.root_token) {
            continue;
        }

        let key = CoalesceKey {
            root_token: hint.root_token.clone(),
            native_key: hint.native_key.clone(),
            path_after: hint.path_after.clone(),
            path_before: hint.path_before.clone(),
        };
        output
            .entry(key)
            .and_modify(|existing| {
                existing.kind = merge_event_kinds(existing.kind, hint.kind);
                existing.scope = merge_change_scopes(existing.scope, hint.scope);
            })
            .or_insert(hint);
    }
    output.into_values().collect()
}

fn coalesce_tracked_renames(hints: impl IntoIterator<Item = ChangeHint>) -> Vec<ChangeHint> {
    let mut output = Vec::new();
    let mut pending = BTreeMap::<(String, Vec<u8>), ChangeHint>::new();
    for hint in hints {
        let Some(native_key) = hint.native_key.clone() else {
            output.push(conservative_one_sided_move(hint));
            continue;
        };
        let partial_move = hint.kind == LocalEventKind::Moved
            && hint.path_after.is_some() != hint.path_before.is_some();
        if !partial_move {
            output.push(hint);
            continue;
        }

        let key = (hint.root_token.clone(), native_key);
        if let Some(previous) = pending.remove(&key) {
            let complementary = previous.path_after.is_some() != hint.path_after.is_some();
            if complementary {
                output.push(ChangeHint {
                    root_token: hint.root_token,
                    native_key: hint.native_key,
                    path_after: hint.path_after.or(previous.path_after),
                    path_before: hint.path_before.or(previous.path_before),
                    kind: LocalEventKind::Moved,
                    scope: merge_change_scopes(previous.scope, hint.scope),
                });
            } else {
                output.push(conservative_one_sided_move(previous));
                pending.insert(key, hint);
            }
        } else {
            pending.insert(key, hint);
        }
    }
    output.extend(pending.into_values().map(conservative_one_sided_move));
    output
}

fn conservative_one_sided_move(mut hint: ChangeHint) -> ChangeHint {
    if hint.kind == LocalEventKind::Moved && hint.path_after.is_some() != hint.path_before.is_some()
    {
        hint.kind = LocalEventKind::RescanRequired;
        hint.scope = ChangeScope::Unknown;
    }
    hint
}

fn boundary_aware_move_hint(
    root_token: &str,
    native_key: Option<Vec<u8>>,
    path_before: Option<PathBuf>,
    path_after: Option<PathBuf>,
    scope: ChangeScope,
) -> ChangeHint {
    let kind = match (path_before.is_some(), path_after.is_some()) {
        (false, true) => LocalEventKind::Created,
        (true, false) => LocalEventKind::Removed,
        _ => LocalEventKind::Moved,
    };
    ChangeHint {
        root_token: root_token.to_owned(),
        native_key,
        path_after,
        path_before,
        kind,
        scope,
    }
}

fn merge_change_scopes(previous: ChangeScope, next: ChangeScope) -> ChangeScope {
    match (previous, next) {
        (scope, incoming) if scope == incoming => scope,
        (ChangeScope::Unknown, incoming) => incoming,
        (scope, ChangeScope::Unknown) => scope,
        _ => ChangeScope::Unknown,
    }
}

fn merge_event_kinds(previous: LocalEventKind, next: LocalEventKind) -> LocalEventKind {
    use LocalEventKind::{Created, Metadata, Modified, Moved, Overflow, Removed, RescanRequired};
    match (previous, next) {
        (RescanRequired, _) | (_, RescanRequired) => RescanRequired,
        (Overflow, _) | (_, Overflow) => Overflow,
        (kind, incoming) if kind == incoming => kind,
        (Metadata, incoming) => incoming,
        (kind, Metadata) => kind,
        (Created, Modified) | (Modified, Created) => Created,
        (Created, Removed) | (Modified, Removed) | (Moved, Removed) => Removed,
        (Removed, Created) => Modified,
        (_, incoming) => incoming,
    }
}

fn token_for_root(root: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Portable path-aware fallback. Events remain hints and must always be
/// reconciled by a read-only scan before catalog state changes.
pub const DEFAULT_POLLING_TRAVERSAL_ENTRY_BUDGET: usize = 100_000;
pub const DEFAULT_POLLING_HINT_BUDGET: usize = 8_192;

enum PollingDiff {
    Hints(Vec<ChangeHint>),
    RescanRequired,
    Cancelled,
}

#[derive(Debug)]
pub struct PollingChangeMonitor {
    state: Mutex<Option<PollingState>>,
    poll_interval: Duration,
    traversal_entry_budget: usize,
    hint_budget: usize,
}

impl Default for PollingChangeMonitor {
    fn default() -> Self {
        Self {
            state: Mutex::new(None),
            poll_interval: Duration::from_secs(10),
            traversal_entry_budget: DEFAULT_POLLING_TRAVERSAL_ENTRY_BUDGET,
            hint_budget: DEFAULT_POLLING_HINT_BUDGET,
        }
    }
}

impl PollingChangeMonitor {
    #[must_use]
    pub fn with_poll_interval(poll_interval: Duration) -> Self {
        Self {
            state: Mutex::new(None),
            poll_interval,
            traversal_entry_budget: DEFAULT_POLLING_TRAVERSAL_ENTRY_BUDGET,
            hint_budget: DEFAULT_POLLING_HINT_BUDGET,
        }
    }

    #[must_use]
    pub fn with_traversal_entry_budget(mut self, traversal_entry_budget: usize) -> Self {
        self.traversal_entry_budget = traversal_entry_budget;
        self
    }

    #[must_use]
    pub fn with_hint_budget(mut self, hint_budget: usize) -> Self {
        self.hint_budget = hint_budget.max(1);
        self
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<PollingState>>, PlatformError> {
        self.state
            .lock()
            .map_err(|_| PlatformError::Unsupported("monitor mutex poisoned".to_owned()))
    }

    fn snapshot(
        root: &Path,
        traversal_entry_budget: usize,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<PollingSnapshot, PlatformError> {
        let mut output = HashMap::new();
        let mut pending = vec![root.to_path_buf()];
        let mut traversed = 0_usize;
        let mut truncated = false;
        let mut cancelled = false;
        'scan: while let Some(directory) = pending.pop() {
            if is_cancelled() {
                cancelled = true;
                break;
            }
            for candidate in fs::read_dir(directory)? {
                if is_cancelled() {
                    cancelled = true;
                    break 'scan;
                }
                if traversed >= traversal_entry_budget {
                    truncated = true;
                    break 'scan;
                }
                traversed = traversed.saturating_add(1);
                let entry = candidate?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                let scope = if metadata.is_dir() {
                    ChangeScope::Directory
                } else if metadata.is_file() {
                    ChangeScope::File
                } else {
                    continue;
                };
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| PlatformError::OutsideRoot)?;
                let modified_ns = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |value| value.as_nanos());
                output.insert(
                    relative.to_path_buf(),
                    PollingStamp {
                        scope,
                        byte_size: if scope == ChangeScope::File {
                            metadata.len()
                        } else {
                            0
                        },
                        modified_ns: if scope == ChangeScope::File {
                            modified_ns
                        } else {
                            0
                        },
                        read_only: metadata.permissions().readonly(),
                    },
                );
                if scope == ChangeScope::Directory {
                    pending.push(path);
                }
            }
        }
        Ok(PollingSnapshot {
            entries: output,
            truncated,
            cancelled,
        })
    }

    fn diff_snapshots(
        root_token: &str,
        previous: &HashMap<PathBuf, PollingStamp>,
        current: &HashMap<PathBuf, PollingStamp>,
        hint_budget: usize,
        is_cancelled: &dyn Fn() -> bool,
    ) -> PollingDiff {
        let mut created = Vec::new();
        for (path, stamp) in current {
            if is_cancelled() {
                return PollingDiff::Cancelled;
            }
            if !previous.contains_key(path) {
                created.push((path.clone(), *stamp));
            }
        }
        let mut removed = Vec::new();
        for (path, stamp) in previous {
            if is_cancelled() {
                return PollingDiff::Cancelled;
            }
            if !current.contains_key(path) {
                removed.push((path.clone(), *stamp));
            }
        }
        created.sort_by(|left, right| left.0.cmp(&right.0));
        removed.sort_by(|left, right| left.0.cmp(&right.0));

        let mut created_by_stamp = HashMap::<PollingStamp, Vec<usize>>::new();
        for (index, (_, stamp)) in created.iter().enumerate() {
            created_by_stamp.entry(*stamp).or_default().push(index);
        }
        let mut removed_per_stamp = HashMap::<PollingStamp, usize>::new();
        for (_, stamp) in &removed {
            let count = removed_per_stamp.entry(*stamp).or_default();
            *count = count.saturating_add(1);
        }

        let mut paired_created = vec![false; created.len()];
        let mut paired_removed = vec![false; removed.len()];
        let mut hints =
            Vec::with_capacity(hint_budget.min(created.len().saturating_add(removed.len())));
        macro_rules! push_hint {
            ($hint:expr) => {
                if hints.len() >= hint_budget {
                    return PollingDiff::RescanRequired;
                }
                hints.push($hint);
            };
        }

        for (removed_index, (path_before, old_stamp)) in removed.iter().enumerate() {
            if is_cancelled() {
                return PollingDiff::Cancelled;
            }
            let Some(candidates) = created_by_stamp.get(old_stamp) else {
                continue;
            };
            if candidates.len() != 1
                || removed_per_stamp
                    .get(old_stamp)
                    .copied()
                    .unwrap_or_default()
                    != 1
            {
                continue;
            }
            let created_index = candidates[0];
            let (path_after, new_stamp) = &created[created_index];
            paired_created[created_index] = true;
            paired_removed[removed_index] = true;
            push_hint!(ChangeHint {
                root_token: root_token.to_owned(),
                native_key: None,
                path_after: Some(path_after.clone()),
                path_before: Some(path_before.clone()),
                kind: LocalEventKind::Moved,
                scope: merge_change_scopes(old_stamp.scope, new_stamp.scope),
            });
        }

        for (index, (path, stamp)) in created.into_iter().enumerate() {
            if is_cancelled() {
                return PollingDiff::Cancelled;
            }
            if !paired_created[index] {
                push_hint!(ChangeHint {
                    root_token: root_token.to_owned(),
                    native_key: None,
                    path_after: Some(path),
                    path_before: None,
                    kind: LocalEventKind::Created,
                    scope: stamp.scope,
                });
            }
        }
        for (index, (path, stamp)) in removed.into_iter().enumerate() {
            if is_cancelled() {
                return PollingDiff::Cancelled;
            }
            if !paired_removed[index] {
                push_hint!(ChangeHint {
                    root_token: root_token.to_owned(),
                    native_key: None,
                    path_after: None,
                    path_before: Some(path),
                    kind: LocalEventKind::Removed,
                    scope: stamp.scope,
                });
            }
        }
        for (path, stamp) in current {
            if is_cancelled() {
                return PollingDiff::Cancelled;
            }
            let Some(old_stamp) = previous.get(path) else {
                continue;
            };
            if old_stamp == stamp {
                continue;
            }
            let scope_changed = old_stamp.scope != stamp.scope;
            push_hint!(ChangeHint {
                root_token: root_token.to_owned(),
                native_key: None,
                path_after: Some(path.clone()),
                path_before: scope_changed.then(|| path.clone()),
                kind: if scope_changed {
                    LocalEventKind::RescanRequired
                } else if old_stamp.byte_size != stamp.byte_size
                    || old_stamp.modified_ns != stamp.modified_ns
                {
                    LocalEventKind::Modified
                } else {
                    LocalEventKind::Metadata
                },
                scope: merge_change_scopes(old_stamp.scope, stamp.scope),
            });
        }
        PollingDiff::Hints(coalesce_change_hints(hints))
    }
}

impl ChangeMonitor for PollingChangeMonitor {
    fn start(&self, root: &Path) -> Result<(), PlatformError> {
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(PlatformError::ReparsePoint);
        }
        let canonical_root = fs::canonicalize(root)?;
        let mut guard = self.lock()?;
        if let Some(state) = guard.as_ref() {
            return if state.root == canonical_root {
                Ok(())
            } else {
                Err(PlatformError::Precondition(
                    "monitor is already attached to another root".to_owned(),
                ))
            };
        }
        let snapshot = Self::snapshot(&canonical_root, self.traversal_entry_budget, &|| false)?;
        let pending_rescan = snapshot.truncated;
        *guard = Some(PollingState {
            root_token: token_for_root(&canonical_root),
            root: canonical_root,
            snapshot: (!snapshot.truncated).then_some(snapshot.entries),
            pending_rescan,
            last_snapshot_at: Instant::now(),
        });
        Ok(())
    }

    fn drain_hints(&self) -> Result<Vec<ChangeHint>, PlatformError> {
        self.drain_hints_with_cancellation(&|| false)
    }

    fn drain_hints_with_cancellation(
        &self,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<ChangeHint>, PlatformError> {
        let mut guard = self.lock()?;
        let state = guard
            .as_mut()
            .ok_or_else(|| PlatformError::Unsupported("monitor is not started".to_owned()))?;
        if is_cancelled() {
            return Ok(Vec::new());
        }
        if state.pending_rescan {
            state.pending_rescan = false;
            return Ok(vec![rescan_hint(&state.root_token)]);
        }
        if state.last_snapshot_at.elapsed() < self.poll_interval {
            return Ok(Vec::new());
        }
        let current = Self::snapshot(&state.root, self.traversal_entry_budget, is_cancelled)?;
        if current.cancelled {
            return Ok(Vec::new());
        }
        if current.truncated {
            state.last_snapshot_at = Instant::now();
            return Ok(vec![rescan_hint(&state.root_token)]);
        }
        let Some(previous) = state.snapshot.as_ref() else {
            state.last_snapshot_at = Instant::now();
            state.snapshot = Some(current.entries);
            return Ok(vec![rescan_hint(&state.root_token)]);
        };
        let diff = Self::diff_snapshots(
            &state.root_token,
            previous,
            &current.entries,
            self.hint_budget,
            is_cancelled,
        );
        match diff {
            PollingDiff::Cancelled => Ok(Vec::new()),
            PollingDiff::RescanRequired => {
                state.last_snapshot_at = Instant::now();
                state.snapshot = Some(current.entries);
                Ok(vec![rescan_hint(&state.root_token)])
            }
            PollingDiff::Hints(hints) => {
                state.last_snapshot_at = Instant::now();
                state.snapshot = Some(current.entries);
                Ok(hints)
            }
        }
    }

    fn stop(&self) -> Result<(), PlatformError> {
        *self.lock()? = None;
        Ok(())
    }
}

struct LocalWatcherState {
    root: PathBuf,
    root_token: String,
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    overflowed: Arc<AtomicBool>,
    pending_renames: BTreeMap<(String, Vec<u8>), PendingNativeRename>,
}

#[derive(Debug)]
struct PendingNativeRename {
    hint: ChangeHint,
    expires_at: Instant,
}

const NATIVE_RENAME_EXPIRATION: Duration = Duration::from_millis(250);
const MAX_PENDING_NATIVE_RENAMES: usize = 4_096;

/// Production recursive watcher backed by `notify::RecommendedWatcher`.
///
/// Each instance is bound to at most one root. The callback only forwards
/// events into a standard-library channel; consumers reconcile drained hints.
#[derive(Default)]
pub struct LocalChangeMonitor {
    state: Mutex<Option<LocalWatcherState>>,
}

impl fmt::Debug for LocalChangeMonitor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalChangeMonitor")
            .field(
                "started",
                &self.state.lock().map_or(true, |state| state.is_some()),
            )
            .finish()
    }
}

impl LocalChangeMonitor {
    fn lock(&self) -> Result<MutexGuard<'_, Option<LocalWatcherState>>, PlatformError> {
        self.state
            .lock()
            .map_err(|_| PlatformError::Unsupported("monitor mutex poisoned".to_owned()))
    }
}

impl ChangeMonitor for LocalChangeMonitor {
    fn start(&self, root: &Path) -> Result<(), PlatformError> {
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(PlatformError::ReparsePoint);
        }
        let canonical_root = fs::canonicalize(root)?;
        let mut guard = self.lock()?;
        if let Some(state) = guard.as_ref() {
            return if state.root == canonical_root {
                Ok(())
            } else {
                Err(PlatformError::Precondition(
                    "monitor is already attached to another root".to_owned(),
                ))
            };
        }

        let (sender, receiver) = mpsc::sync_channel(8_192);
        let overflowed = Arc::new(AtomicBool::new(false));
        let callback_overflowed = overflowed.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                if let Err(TrySendError::Full(_)) = sender.try_send(event) {
                    callback_overflowed.store(true, Ordering::Relaxed);
                }
            },
            Config::default(),
        )
        .map_err(notify_platform_error)?;
        watcher
            .watch(&canonical_root, RecursiveMode::Recursive)
            .map_err(notify_platform_error)?;
        *guard = Some(LocalWatcherState {
            root_token: token_for_root(&canonical_root),
            root: canonical_root,
            _watcher: watcher,
            receiver,
            overflowed,
            pending_renames: BTreeMap::new(),
        });
        Ok(())
    }

    fn drain_hints(&self) -> Result<Vec<ChangeHint>, PlatformError> {
        let mut guard = self.lock()?;
        let state = guard
            .as_mut()
            .ok_or_else(|| PlatformError::Unsupported("monitor is not started".to_owned()))?;
        let mut hints = Vec::new();
        if state.overflowed.swap(false, Ordering::Relaxed) {
            hints.push(overflow_hint(&state.root_token));
        }
        loop {
            match state.receiver.try_recv() {
                Ok(Ok(event)) => hints.extend(hints_from_notify_event(
                    &state.root,
                    &state.root_token,
                    event,
                )),
                Ok(Err(_)) => {
                    hints.push(rescan_hint(&state.root_token));
                }
                Err(TryRecvError::Disconnected) => {
                    hints.push(rescan_hint(&state.root_token));
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        Ok(coalesce_change_hints(resolve_native_rename_hints(
            &mut state.pending_renames,
            hints,
            Instant::now(),
        )))
    }

    fn stop(&self) -> Result<(), PlatformError> {
        *self.lock()? = None;
        Ok(())
    }
}

/// Explicit alias for callers that select the recommended native backend.
pub type RecommendedChangeMonitor = LocalChangeMonitor;
/// Compatibility alias emphasizing the underlying watcher implementation.
pub type NotifyChangeMonitor = LocalChangeMonitor;

fn resolve_native_rename_hints(
    pending: &mut BTreeMap<(String, Vec<u8>), PendingNativeRename>,
    hints: Vec<ChangeHint>,
    now: Instant,
) -> Vec<ChangeHint> {
    if hints.iter().any(|hint| {
        matches!(
            hint.kind,
            LocalEventKind::Overflow | LocalEventKind::RescanRequired
        )
    }) {
        pending.clear();
        return hints;
    }

    let mut output = Vec::new();
    let expired = pending
        .iter()
        .filter(|(_, value)| value.expires_at <= now)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in expired {
        if let Some(expired) = pending.remove(&key) {
            output.push(rescan_hint(&expired.hint.root_token));
        }
    }

    for hint in hints {
        let Some(native_key) = hint.native_key.clone() else {
            output.push(conservative_one_sided_move(hint));
            continue;
        };
        let partial_move = hint.kind == LocalEventKind::Moved
            && hint.path_after.is_some() != hint.path_before.is_some();
        if !partial_move {
            output.push(hint);
            continue;
        }
        let key = (hint.root_token.clone(), native_key);
        if let Some(previous) = pending.remove(&key) {
            let complementary = previous.hint.path_after.is_some() != hint.path_after.is_some();
            if complementary {
                output.push(ChangeHint {
                    root_token: hint.root_token,
                    native_key: hint.native_key,
                    path_after: hint.path_after.or(previous.hint.path_after),
                    path_before: hint.path_before.or(previous.hint.path_before),
                    kind: LocalEventKind::Moved,
                    scope: merge_change_scopes(previous.hint.scope, hint.scope),
                });
            } else {
                output.push(rescan_hint(&previous.hint.root_token));
                pending.insert(
                    key,
                    PendingNativeRename {
                        hint,
                        expires_at: now + NATIVE_RENAME_EXPIRATION,
                    },
                );
            }
        } else if pending.len() >= MAX_PENDING_NATIVE_RENAMES {
            pending.clear();
            output.push(rescan_hint(&hint.root_token));
        } else {
            pending.insert(
                key,
                PendingNativeRename {
                    hint,
                    expires_at: now + NATIVE_RENAME_EXPIRATION,
                },
            );
        }
    }
    output
}

fn notify_platform_error(error: notify::Error) -> PlatformError {
    PlatformError::Unsupported(format!("filesystem watcher failed: {error}"))
}

fn rescan_hint(root_token: &str) -> ChangeHint {
    ChangeHint {
        root_token: root_token.to_owned(),
        native_key: None,
        path_after: None,
        path_before: None,
        kind: LocalEventKind::RescanRequired,
        scope: ChangeScope::Unknown,
    }
}

fn overflow_hint(root_token: &str) -> ChangeHint {
    ChangeHint {
        root_token: root_token.to_owned(),
        native_key: None,
        path_after: None,
        path_before: None,
        kind: LocalEventKind::Overflow,
        scope: ChangeScope::Unknown,
    }
}

fn absolute_event_path(root: &Path, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return None;
        }
        Some(root.join(path))
    }
}

fn root_relative_event_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let absolute = absolute_event_path(root, path)?;
    let relative = absolute.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    if absolute.exists()
        && fs::canonicalize(&absolute).is_ok_and(|canonical| !canonical.starts_with(root))
    {
        return None;
    }
    Some(relative.to_path_buf())
}

fn observed_event_scope(root: &Path, path: &Path) -> Option<ChangeScope> {
    root_relative_event_path(root, path)?;
    let absolute = absolute_event_path(root, path)?;
    let metadata = fs::symlink_metadata(absolute).ok()?;
    if metadata.is_dir() {
        Some(ChangeScope::Directory)
    } else if metadata.is_file() {
        Some(ChangeScope::File)
    } else {
        Some(ChangeScope::Unknown)
    }
}

fn create_scope(kind: CreateKind) -> ChangeScope {
    match kind {
        CreateKind::File => ChangeScope::File,
        CreateKind::Folder => ChangeScope::Directory,
        CreateKind::Any | CreateKind::Other => ChangeScope::Unknown,
    }
}

fn remove_scope(kind: RemoveKind) -> ChangeScope {
    match kind {
        RemoveKind::File => ChangeScope::File,
        RemoveKind::Folder => ChangeScope::Directory,
        RemoveKind::Any | RemoveKind::Other => ChangeScope::Unknown,
    }
}

fn path_hints(
    root: &Path,
    root_token: &str,
    paths: &[PathBuf],
    kind: LocalEventKind,
    before: bool,
    native_key: &Option<Vec<u8>>,
    fallback_scope: ChangeScope,
) -> Vec<ChangeHint> {
    let hints = paths
        .iter()
        .filter_map(|path| {
            let relative = root_relative_event_path(root, path)?;
            let scope = observed_event_scope(root, path).unwrap_or(fallback_scope);
            Some(ChangeHint {
                root_token: root_token.to_owned(),
                native_key: native_key.clone(),
                path_after: (!before).then_some(relative.clone()),
                path_before: before.then_some(relative),
                kind,
                scope,
            })
        })
        .collect::<Vec<_>>();
    if hints.is_empty()
        && (paths.is_empty()
            || paths
                .iter()
                .filter_map(|path| absolute_event_path(root, path))
                .any(|path| path.starts_with(root)))
    {
        vec![rescan_hint(root_token)]
    } else {
        hints
    }
}

fn unresolved_event_hints(root: &Path, root_token: &str, paths: &[PathBuf]) -> Vec<ChangeHint> {
    if paths.is_empty()
        || paths
            .iter()
            .filter_map(|path| absolute_event_path(root, path))
            .any(|path| path.starts_with(root))
    {
        vec![rescan_hint(root_token)]
    } else {
        Vec::new()
    }
}

fn hints_from_notify_event(root: &Path, root_token: &str, event: Event) -> Vec<ChangeHint> {
    if event.need_rescan() {
        return vec![overflow_hint(root_token)];
    }
    if !matches!(&event.kind, EventKind::Access(_))
        && event
            .paths
            .iter()
            .filter_map(|path| absolute_event_path(root, path))
            .any(|path| path == root)
    {
        return vec![rescan_hint(root_token)];
    }
    let native_key = event
        .tracker()
        .map(|tracker| tracker.to_le_bytes().to_vec());
    match event.kind {
        EventKind::Access(_) => Vec::new(),
        EventKind::Create(create_kind) => path_hints(
            root,
            root_token,
            &event.paths,
            LocalEventKind::Created,
            false,
            &native_key,
            create_scope(create_kind),
        ),
        EventKind::Remove(remove_kind) => path_hints(
            root,
            root_token,
            &event.paths,
            LocalEventKind::Removed,
            true,
            &native_key,
            remove_scope(remove_kind),
        ),
        EventKind::Modify(ModifyKind::Metadata(_)) => path_hints(
            root,
            root_token,
            &event.paths,
            LocalEventKind::Metadata,
            false,
            &native_key,
            ChangeScope::Unknown,
        ),
        EventKind::Modify(ModifyKind::Name(mode)) => match mode {
            RenameMode::Both if event.paths.len() < 2 => path_hints(
                root,
                root_token,
                &event.paths,
                LocalEventKind::RescanRequired,
                false,
                &native_key,
                ChangeScope::Unknown,
            ),
            RenameMode::Both => {
                let path_before = event
                    .paths
                    .first()
                    .and_then(|path| root_relative_event_path(root, path));
                let path_after = event
                    .paths
                    .last()
                    .and_then(|path| root_relative_event_path(root, path));
                if path_before.is_none() && path_after.is_none() {
                    unresolved_event_hints(root, root_token, &event.paths)
                } else {
                    let scope = event
                        .paths
                        .iter()
                        .rev()
                        .find_map(|path| observed_event_scope(root, path))
                        .unwrap_or_default();
                    vec![boundary_aware_move_hint(
                        root_token,
                        native_key,
                        path_before,
                        path_after,
                        scope,
                    )]
                }
            }
            RenameMode::From => path_hints(
                root,
                root_token,
                &event.paths,
                LocalEventKind::Moved,
                true,
                &native_key,
                ChangeScope::Unknown,
            ),
            RenameMode::To => path_hints(
                root,
                root_token,
                &event.paths,
                LocalEventKind::Moved,
                false,
                &native_key,
                ChangeScope::Unknown,
            ),
            _ if event.paths.len() >= 2 => {
                let path_before = event
                    .paths
                    .first()
                    .and_then(|path| root_relative_event_path(root, path));
                let path_after = event
                    .paths
                    .last()
                    .and_then(|path| root_relative_event_path(root, path));
                if path_before.is_none() && path_after.is_none() {
                    unresolved_event_hints(root, root_token, &event.paths)
                } else {
                    let scope = event
                        .paths
                        .iter()
                        .rev()
                        .find_map(|path| observed_event_scope(root, path))
                        .unwrap_or_default();
                    vec![boundary_aware_move_hint(
                        root_token,
                        native_key,
                        path_before,
                        path_after,
                        scope,
                    )]
                }
            }
            _ => path_hints(
                root,
                root_token,
                &event.paths,
                LocalEventKind::RescanRequired,
                false,
                &native_key,
                ChangeScope::Unknown,
            ),
        },
        EventKind::Modify(_) => path_hints(
            root,
            root_token,
            &event.paths,
            LocalEventKind::Modified,
            false,
            &native_key,
            ChangeScope::Unknown,
        ),
        EventKind::Any | EventKind::Other => vec![rescan_hint(root_token)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_error_codes_have_portable_structured_classes() {
        for (code, expected) in [
            (2, PlatformErrorClass::SourceMissing),
            (3, PlatformErrorClass::SourceMissing),
            (5, PlatformErrorClass::PermissionDenied),
            (32, PlatformErrorClass::SharingViolation),
            (33, PlatformErrorClass::LockViolation),
            (39, PlatformErrorClass::DiskFull),
            (80, PlatformErrorClass::DestinationCollision),
            (112, PlatformErrorClass::DiskFull),
            (183, PlatformErrorClass::DestinationCollision),
            (1_920, PlatformErrorClass::PathPolicyRefusal),
            (4_390, PlatformErrorClass::PathPolicyRefusal),
            (4_394, PlatformErrorClass::PathPolicyRefusal),
        ] {
            assert_eq!(classify_windows_error_code(code), expected);
        }
        assert_eq!(classify_windows_error_code(0xffff), PlatformErrorClass::Io);
    }

    #[test]
    fn only_known_pre_mutation_lock_classes_are_retryable() {
        assert!(PlatformError::SharingViolation.retryable_before_mutation());
        assert!(PlatformError::LockViolation.retryable_before_mutation());
        for error in [
            PlatformError::PermissionDenied,
            PlatformError::DiskFull,
            PlatformError::DestinationExists,
            PlatformError::PathPolicyRefusal,
            PlatformError::AmbiguousMutationOutcome,
        ] {
            assert!(!error.retryable_before_mutation());
        }
        assert!(matches!(
            PlatformError::from_windows_code(32, true),
            PlatformError::AmbiguousMutationOutcome
        ));
        assert!(matches!(
            PlatformError::from_windows_code(80, false),
            PlatformError::DestinationExists
        ));
    }

    #[test]
    fn analysis_and_mutation_are_distinct_traits() {
        fn accepts_read_only<T: ReadOnlyPlatform>(_value: &T) {}
        let _ = accepts_read_only::<ReadOnlyTestMarker>;
    }

    #[test]
    fn local_event_kind_is_a_serializable_value_contract() {
        fn assert_contract<T: Serialize + for<'de> Deserialize<'de> + Eq>() {}
        assert_contract::<LocalEventKind>();
        assert_contract::<ChangeScope>();
    }

    #[test]
    fn scoped_reads_reject_absolute_and_parent_paths_before_opening() {
        let root = std::env::temp_dir();
        assert!(matches!(
            validated_scoped_path(&root, Path::new("../outside")),
            Err(PlatformError::OutsideRoot)
        ));
        assert!(matches!(
            validated_scoped_path(&root, &root.join("absolute")),
            Err(PlatformError::OutsideRoot)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn scoped_reads_reject_symlink_scope_escape() {
        use std::os::unix::fs::symlink;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let base = std::env::temp_dir().join(format!(
            "working-name-scope-{}-{unique}",
            std::process::id()
        ));
        let root = base.join("root");
        let outside = base.join("outside.txt");
        fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("scope fixture root should be created: {error}"));
        fs::write(&outside, b"private")
            .unwrap_or_else(|error| panic!("scope fixture should be written: {error}"));
        let link = root.join("escape.txt");
        symlink(&outside, &link)
            .unwrap_or_else(|error| panic!("scope fixture symlink should be created: {error}"));

        assert!(matches!(
            validated_scoped_path(&root, Path::new("escape.txt")),
            Err(PlatformError::OutsideRoot)
        ));

        let _ = fs::remove_file(link);
        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir(root);
        let _ = fs::remove_dir(base);
    }

    #[test]
    fn polling_monitor_tracks_create_modify_rename_remove_without_mutation() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "working-name-monitor-{}-{unique}",
            std::process::id()
        ));
        let other_root = std::env::temp_dir().join(format!(
            "working-name-other-monitor-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&root)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
        std::fs::create_dir(&other_root)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
        let monitor = PollingChangeMonitor::with_poll_interval(Duration::ZERO);
        assert!(monitor.start(&root).is_ok());
        assert!(monitor.start(&root).is_ok());
        assert!(matches!(
            monitor.start(&other_root),
            Err(PlatformError::Precondition(_))
        ));
        assert!(
            monitor
                .drain_hints()
                .unwrap_or_else(|error| panic!("monitor should drain: {error}"))
                .is_empty()
        );

        let file = root.join("new-file.txt");
        std::fs::write(&file, b"content")
            .unwrap_or_else(|error| panic!("fixture file should be written: {error}"));
        let created = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].kind, LocalEventKind::Created);
        assert_eq!(created[0].scope, ChangeScope::File);
        assert_eq!(
            created[0].path_after.as_deref(),
            Some(Path::new("new-file.txt"))
        );
        assert_eq!(
            std::fs::read(&file)
                .unwrap_or_else(|error| panic!("monitor must not alter the fixture: {error}")),
            b"content"
        );

        std::fs::write(&file, b"longer content")
            .unwrap_or_else(|error| panic!("fixture file should be modified: {error}"));
        let modified = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].kind, LocalEventKind::Modified);

        let renamed = root.join("renamed-file.txt");
        std::fs::rename(&file, &renamed)
            .unwrap_or_else(|error| panic!("fixture file should be renamed: {error}"));
        let moved = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].kind, LocalEventKind::Moved);
        assert_eq!(moved[0].scope, ChangeScope::File);
        assert_eq!(
            moved[0].path_before.as_deref(),
            Some(Path::new("new-file.txt"))
        );
        assert_eq!(
            moved[0].path_after.as_deref(),
            Some(Path::new("renamed-file.txt"))
        );

        std::fs::remove_file(&renamed)
            .unwrap_or_else(|error| panic!("fixture file should be removed: {error}"));
        let removed = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].kind, LocalEventKind::Removed);
        assert_eq!(
            removed[0].path_before.as_deref(),
            Some(Path::new("renamed-file.txt"))
        );

        assert!(monitor.stop().is_ok());
        assert!(monitor.stop().is_ok());
        let _ = std::fs::remove_dir(root);
        let _ = std::fs::remove_dir(other_root);
    }

    #[test]
    fn polling_monitor_emits_directory_scoped_descendant_reconciliation() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "working-name-directory-monitor-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
        let monitor = PollingChangeMonitor::with_poll_interval(Duration::ZERO);
        monitor
            .start(&root)
            .unwrap_or_else(|error| panic!("monitor should start: {error}"));

        let before = root.join("before");
        fs::create_dir(&before)
            .unwrap_or_else(|error| panic!("nested directory should be created: {error}"));
        let created = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].kind, LocalEventKind::Created);
        assert_eq!(created[0].scope, ChangeScope::Directory);

        fs::write(before.join("nested.txt"), b"content")
            .unwrap_or_else(|error| panic!("nested file should be created: {error}"));
        let _ = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));

        let after = root.join("after");
        fs::rename(&before, &after)
            .unwrap_or_else(|error| panic!("directory should be renamed: {error}"));
        let moved = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert!(moved.iter().any(|hint| {
            hint.kind == LocalEventKind::Moved
                && hint.scope == ChangeScope::Directory
                && hint.path_before.as_deref() == Some(Path::new("before"))
                && hint.path_after.as_deref() == Some(Path::new("after"))
        }));

        fs::remove_file(after.join("nested.txt"))
            .unwrap_or_else(|error| panic!("nested file should be removed: {error}"));
        fs::remove_dir(&after)
            .unwrap_or_else(|error| panic!("nested directory should be removed: {error}"));
        let removed = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert!(removed.iter().any(|hint| {
            hint.kind == LocalEventKind::Removed
                && hint.scope == ChangeScope::Directory
                && hint.path_before.as_deref() == Some(Path::new("after"))
        }));

        let _ = monitor.stop();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn polling_monitor_reports_rescan_when_traversal_or_output_is_truncated() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let traversal_root = std::env::temp_dir().join(format!(
            "working-name-traversal-budget-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&traversal_root)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
        fs::write(traversal_root.join("one.txt"), b"one")
            .unwrap_or_else(|error| panic!("fixture file should be created: {error}"));
        fs::write(traversal_root.join("two.txt"), b"two")
            .unwrap_or_else(|error| panic!("fixture file should be created: {error}"));
        let traversal_limited =
            PollingChangeMonitor::with_poll_interval(Duration::ZERO).with_traversal_entry_budget(1);
        traversal_limited
            .start(&traversal_root)
            .unwrap_or_else(|error| panic!("monitor should start: {error}"));
        let truncated = traversal_limited
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].kind, LocalEventKind::RescanRequired);
        assert!(truncated[0].path_before.is_none());
        assert!(truncated[0].path_after.is_none());

        let output_root = std::env::temp_dir().join(format!(
            "working-name-output-budget-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&output_root)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
        let output_limited =
            PollingChangeMonitor::with_poll_interval(Duration::ZERO).with_hint_budget(1);
        output_limited
            .start(&output_root)
            .unwrap_or_else(|error| panic!("monitor should start: {error}"));
        fs::write(output_root.join("one.txt"), b"one")
            .unwrap_or_else(|error| panic!("fixture file should be created: {error}"));
        fs::write(output_root.join("two.txt"), b"two")
            .unwrap_or_else(|error| panic!("fixture file should be created: {error}"));
        let backpressure = output_limited
            .drain_hints()
            .unwrap_or_else(|error| panic!("monitor should drain: {error}"));
        assert_eq!(backpressure.len(), 1);
        assert_eq!(backpressure[0].kind, LocalEventKind::RescanRequired);
        assert!(backpressure[0].path_before.is_none());
        assert!(backpressure[0].path_after.is_none());

        let _ = traversal_limited.stop();
        let _ = output_limited.stop();
        let _ = fs::remove_dir_all(traversal_root);
        let _ = fs::remove_dir_all(output_root);
    }

    #[test]
    fn polling_monitor_cancellation_preserves_the_last_complete_snapshot() {
        use std::cell::Cell;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "working-name-cancelled-monitor-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root)
            .unwrap_or_else(|error| panic!("fixture directory should be created: {error}"));
        let monitor = PollingChangeMonitor::with_poll_interval(Duration::ZERO);
        monitor
            .start(&root)
            .unwrap_or_else(|error| panic!("monitor should start: {error}"));
        fs::write(root.join("created.txt"), b"content")
            .unwrap_or_else(|error| panic!("fixture file should be created: {error}"));

        let checks = Cell::new(0_usize);
        let cancel_during_diff = || {
            let observed = checks.get();
            checks.set(observed.saturating_add(1));
            observed >= 2
        };
        let cancelled = monitor
            .drain_hints_with_cancellation(&cancel_during_diff)
            .unwrap_or_else(|error| panic!("cancelled drain should succeed: {error}"));
        assert!(cancelled.is_empty());
        assert!(checks.get() >= 3);
        let retried = monitor
            .drain_hints_with_cancellation(&|| false)
            .unwrap_or_else(|error| panic!("monitor should retry: {error}"));
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].kind, LocalEventKind::Created);

        let _ = monitor.stop();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ambiguous_polling_rename_candidates_remain_conservative() {
        let stamp = PollingStamp {
            scope: ChangeScope::File,
            byte_size: 7,
            modified_ns: 11,
            read_only: false,
        };
        let previous = HashMap::from([
            (PathBuf::from("before-a.txt"), stamp),
            (PathBuf::from("before-b.txt"), stamp),
        ]);
        let current = HashMap::from([
            (PathBuf::from("after-a.txt"), stamp),
            (PathBuf::from("after-b.txt"), stamp),
        ]);

        let PollingDiff::Hints(hints) =
            PollingChangeMonitor::diff_snapshots("root", &previous, &current, 8, &|| false)
        else {
            panic!("small diff should produce bounded hints");
        };
        assert_eq!(hints.len(), 4);
        assert!(hints.iter().all(|hint| hint.kind != LocalEventKind::Moved));
    }

    #[test]
    fn duplicate_hints_coalesce_deterministically() {
        let hint = ChangeHint {
            root_token: "root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("document.txt")),
            path_before: None,
            kind: LocalEventKind::Modified,
            scope: ChangeScope::File,
        };
        let hints = coalesce_change_hints([
            hint.clone(),
            ChangeHint {
                kind: LocalEventKind::Metadata,
                ..hint.clone()
            },
            hint.clone(),
        ]);
        assert_eq!(hints, vec![hint]);
    }

    #[test]
    fn notify_rename_mapping_preserves_relative_sides_and_tracker() {
        let root = std::env::temp_dir().join("working-name-notify-mapping");
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(root.join("before.txt"))
            .add_path(root.join("after.txt"))
            .set_tracker(42);

        let hints = hints_from_notify_event(&root, "root", event);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].kind, LocalEventKind::Moved);
        assert_eq!(
            hints[0].path_before.as_deref(),
            Some(Path::new("before.txt"))
        );
        assert_eq!(hints[0].path_after.as_deref(), Some(Path::new("after.txt")));
        assert_eq!(hints[0].native_key, Some(42_usize.to_le_bytes().to_vec()));
    }

    #[test]
    fn tracked_rename_sides_coalesce_into_one_hint() {
        let native_key = Some(7_usize.to_le_bytes().to_vec());
        let hints = coalesce_change_hints([
            ChangeHint {
                root_token: "root".to_owned(),
                native_key: native_key.clone(),
                path_after: None,
                path_before: Some(PathBuf::from("before.txt")),
                kind: LocalEventKind::Moved,
                scope: ChangeScope::File,
            },
            ChangeHint {
                root_token: "root".to_owned(),
                native_key,
                path_after: Some(PathBuf::from("after.txt")),
                path_before: None,
                kind: LocalEventKind::Moved,
                scope: ChangeScope::File,
            },
        ]);

        assert_eq!(hints.len(), 1);
        assert_eq!(
            hints[0].path_before.as_deref(),
            Some(Path::new("before.txt"))
        );
        assert_eq!(hints[0].path_after.as_deref(), Some(Path::new("after.txt")));
    }

    #[test]
    fn unmatched_native_rename_sides_require_conservative_reconciliation() {
        let root = std::env::temp_dir().join("working-name-notify-unmatched-rename");
        let from = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
            .add_path(root.join("before.txt"))
            .set_tracker(99);
        let to = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
            .add_path(root.join("after.txt"))
            .set_tracker(100);
        let incomplete_both = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(root.join("only-side.txt"))
            .set_tracker(101);

        let from_hints = coalesce_change_hints(hints_from_notify_event(&root, "root", from));
        let to_hints = coalesce_change_hints(hints_from_notify_event(&root, "root", to));
        let incomplete_hints =
            coalesce_change_hints(hints_from_notify_event(&root, "root", incomplete_both));

        assert_eq!(from_hints.len(), 1);
        assert_eq!(from_hints[0].kind, LocalEventKind::RescanRequired);
        assert_eq!(
            from_hints[0].path_before.as_deref(),
            Some(Path::new("before.txt"))
        );
        assert_eq!(to_hints.len(), 1);
        assert_eq!(to_hints[0].kind, LocalEventKind::RescanRequired);
        assert_eq!(
            to_hints[0].path_after.as_deref(),
            Some(Path::new("after.txt"))
        );
        assert_eq!(incomplete_hints.len(), 1);
        assert_eq!(incomplete_hints[0].kind, LocalEventKind::RescanRequired);
        assert_eq!(
            incomplete_hints[0].path_after.as_deref(),
            Some(Path::new("only-side.txt"))
        );
    }

    #[test]
    fn native_rename_sides_pair_before_expiration_and_reconcile_after_timeout() {
        let now = Instant::now();
        let native_key = 77_usize.to_le_bytes().to_vec();
        let mut pending = BTreeMap::new();
        let from = ChangeHint {
            root_token: "root".to_owned(),
            native_key: Some(native_key.clone()),
            path_after: None,
            path_before: Some(PathBuf::from("before.txt")),
            kind: LocalEventKind::Moved,
            scope: ChangeScope::File,
        };
        assert!(resolve_native_rename_hints(&mut pending, vec![from.clone()], now).is_empty());
        let paired = resolve_native_rename_hints(
            &mut pending,
            vec![ChangeHint {
                root_token: "root".to_owned(),
                native_key: Some(native_key),
                path_after: Some(PathBuf::from("after.txt")),
                path_before: None,
                kind: LocalEventKind::Moved,
                scope: ChangeScope::File,
            }],
            now + Duration::from_millis(100),
        );
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].kind, LocalEventKind::Moved);
        assert_eq!(
            paired[0].path_before.as_deref(),
            Some(Path::new("before.txt"))
        );
        assert_eq!(
            paired[0].path_after.as_deref(),
            Some(Path::new("after.txt"))
        );

        assert!(resolve_native_rename_hints(&mut pending, vec![from], now).is_empty());
        let expired =
            resolve_native_rename_hints(&mut pending, Vec::new(), now + NATIVE_RENAME_EXPIRATION);
        assert_eq!(expired, vec![rescan_hint("root")]);
        assert!(pending.is_empty());
    }

    #[test]
    fn native_folder_events_are_directory_scoped() {
        let root = std::env::temp_dir().join("working-name-notify-folder");
        let created = hints_from_notify_event(
            &root,
            "root",
            Event::new(EventKind::Create(CreateKind::Folder)).add_path(root.join("created")),
        );
        let removed = hints_from_notify_event(
            &root,
            "root",
            Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(root.join("removed")),
        );

        assert_eq!(created.len(), 1);
        assert_eq!(created[0].scope, ChangeScope::Directory);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].scope, ChangeScope::Directory);
    }

    #[test]
    fn notify_move_boundaries_and_access_noise_are_conservative() {
        use notify::event::AccessKind;

        let root = std::env::temp_dir().join("working-name-notify-boundary");
        let move_in = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(PathBuf::from("/outside-selected-root/in.txt"))
            .add_path(root.join("in.txt"));
        let move_out = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(root.join("out.txt"))
            .add_path(PathBuf::from("/outside-selected-root/out.txt"));

        let in_hints = hints_from_notify_event(&root, "root", move_in);
        let out_hints = hints_from_notify_event(&root, "root", move_out);
        let access_hints = hints_from_notify_event(
            &root,
            "root",
            Event::new(EventKind::Access(AccessKind::Any)).add_path(root.join("ignored.txt")),
        );

        assert_eq!(in_hints.len(), 1);
        assert_eq!(in_hints[0].kind, LocalEventKind::Created);
        assert_eq!(in_hints[0].path_after.as_deref(), Some(Path::new("in.txt")));
        assert!(in_hints[0].path_before.is_none());
        assert_eq!(out_hints.len(), 1);
        assert_eq!(out_hints[0].kind, LocalEventKind::Removed);
        assert_eq!(
            out_hints[0].path_before.as_deref(),
            Some(Path::new("out.txt"))
        );
        assert!(out_hints[0].path_after.is_none());
        assert!(access_hints.is_empty());
    }

    #[test]
    fn coalescing_one_thousand_events_has_bounded_output() {
        let started = std::time::Instant::now();
        let hints = coalesce_change_hints((0..1_000).map(|_| ChangeHint {
            root_token: "root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from("document.txt")),
            path_before: None,
            kind: LocalEventKind::Modified,
            scope: ChangeScope::File,
        }));
        assert_eq!(hints.len(), 1);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "coalescing a small watcher burst should remain inexpensive"
        );
    }

    #[derive(Debug)]
    struct ReadOnlyTestMarker;

    impl ReadOnlyPlatform for ReadOnlyTestMarker {
        fn inspect_volume(&self, _root: &Path) -> Result<VolumeIdentity, PlatformError> {
            Err(PlatformError::Unsupported("test".to_owned()))
        }

        fn enumerate_regular_files(
            &self,
            _root: &Path,
            _max_entries: usize,
            _is_cancelled: &dyn Fn() -> bool,
            _on_progress: &mut dyn FnMut(EnumerationProgress),
        ) -> Result<ReadOnlyEnumeration, PlatformError> {
            Ok(ReadOnlyEnumeration::default())
        }

        fn read_bounded(&self, _path: &Path, _max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
            Ok(Vec::new())
        }

        fn read_prefix(&self, _path: &Path, _max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
            Ok(Vec::new())
        }

        fn fingerprint(
            &self,
            _path: &Path,
            _include_content_digest: bool,
            _max_bytes: u64,
        ) -> Result<domain::FileFingerprint, PlatformError> {
            Err(PlatformError::Unsupported("test".to_owned()))
        }
    }
}
