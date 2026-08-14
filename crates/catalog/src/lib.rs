//! Read-only catalog construction.

use domain::{
    DisplayLabel, FileFingerprint, FileId, FileKind, FileObservation, FileVersionId, PathEncoding,
    RootId, ScanId, WorkspaceId,
};
use platform::{
    EnumerationIssue, EnumerationProgress, PlatformError, ReadOnlyEntry, ReadOnlyEnumeration,
    ReadOnlyPlatform,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanPolicy {
    pub max_entries: usize,
    pub max_hash_bytes: u64,
    pub include_hidden: bool,
}

impl Default for ScanPolicy {
    fn default() -> Self {
        Self {
            max_entries: 1_000_000,
            max_hash_bytes: u64::MAX,
            include_hidden: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadabilityStatus {
    Readable,
    Unreadable,
    NotChecked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanItemStatus {
    Indexed,
    IndexedWithErrors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashingStatus {
    NotCandidate,
    Hashed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct CatalogedFile {
    pub absolute_path: PathBuf,
    pub observation: FileObservation,
    pub extension: Option<String>,
    pub parent_relative_path: Option<String>,
    pub accessed_at_ns: Option<i128>,
    pub readability_status: ReadabilityStatus,
    pub scan_status: ScanItemStatus,
    pub hashing_status: HashingStatus,
    pub error: Option<ScanIssueKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanIssue {
    pub relative_path: String,
    pub kind: ScanIssueKind,
    pub message: String,
    pub is_directory: bool,
    pub skipped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanIssueKind {
    ReparsePoint,
    CloudPlaceholder,
    PermissionDenied,
    Unsupported,
    Io,
    InvalidDisplayLabel,
    HashBudgetExceeded,
    HashFailed,
    FileChanged,
    EntryLimitReached,
    Cancelled,
}

impl ScanIssueKind {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ReparsePoint => "reparse_point",
            Self::CloudPlaceholder => "cloud_placeholder",
            Self::PermissionDenied => "permission_denied",
            Self::Unsupported => "unsupported",
            Self::Io => "io",
            Self::InvalidDisplayLabel => "invalid_display_label",
            Self::HashBudgetExceeded => "hash_budget_exceeded",
            Self::HashFailed => "hash_failed",
            Self::FileChanged => "file_changed",
            Self::EntryLimitReached => "entry_limit_reached",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn is_error(self) -> bool {
        matches!(
            self,
            Self::PermissionDenied | Self::Io | Self::HashFailed | Self::FileChanged
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    Discovering,
    Inspecting,
    Hashing,
    Persisting,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub scan_id: ScanId,
    pub phase: ScanPhase,
    pub files_discovered: u64,
    pub files_indexed: u64,
    pub directories_discovered: u64,
    pub bytes_discovered: u64,
    pub files_hashed: u64,
    pub errors: u64,
    pub skipped_items: u64,
    pub duplicate_groups: u64,
}

#[derive(Debug, Clone)]
pub struct ScanOutput {
    pub scan_id: ScanId,
    pub files: Vec<CatalogedFile>,
    pub issues: Vec<ScanIssue>,
    pub duplicate_groups: Vec<DuplicateGroup>,
    pub progress: ScanProgress,
    pub cancelled: bool,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("registered root is not analyzable: {0}")]
    Platform(#[from] PlatformError),
}

pub struct CatalogScanner {
    platform: Arc<dyn ReadOnlyPlatform>,
}

impl CatalogScanner {
    #[must_use]
    pub fn new(platform: Arc<dyn ReadOnlyPlatform>) -> Self {
        Self { platform }
    }

    pub fn scan(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        root: &Path,
        policy: ScanPolicy,
    ) -> Result<ScanOutput, CatalogError> {
        self.scan_with_control(workspace_id, root_id, root, policy, &|| false, &mut |_| {})
    }

    #[allow(clippy::too_many_arguments)]
    pub fn scan_with_control(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        root: &Path,
        policy: ScanPolicy,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanOutput, CatalogError> {
        self.scan_with_id_and_control(
            ScanId::new(),
            workspace_id,
            root_id,
            root,
            policy,
            is_cancelled,
            on_progress,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn scan_with_id_and_control(
        &self,
        scan_id: ScanId,
        workspace_id: WorkspaceId,
        root_id: RootId,
        root: &Path,
        policy: ScanPolicy,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanOutput, CatalogError> {
        self.platform.inspect_volume(root)?;
        let mut progress = ScanProgress {
            scan_id,
            phase: ScanPhase::Discovering,
            files_discovered: 0,
            files_indexed: 0,
            directories_discovered: 0,
            bytes_discovered: 0,
            files_hashed: 0,
            errors: 0,
            skipped_items: 0,
            duplicate_groups: 0,
        };
        on_progress(progress);

        let enumeration = {
            let mut enumeration_progress = |current: EnumerationProgress| {
                progress.files_discovered = current.files_discovered;
                progress.directories_discovered = current.directories_discovered;
                progress.bytes_discovered = current.bytes_discovered;
                progress.errors = current.errors;
                progress.skipped_items = current.skipped_items;
                on_progress(progress);
            };
            self.platform.enumerate_regular_files(
                root,
                policy.max_entries,
                is_cancelled,
                &mut enumeration_progress,
            )?
        };
        Ok(self.finish_scan(
            scan_id,
            workspace_id,
            root_id,
            root,
            policy,
            is_cancelled,
            on_progress,
            progress,
            enumeration,
        ))
    }

    /// Scan only the supplied root-relative paths.
    ///
    /// Paths are deduplicated before inspection. Platform adapters inspect
    /// each target directly, so this path never requires whole-root
    /// enumeration.
    #[allow(clippy::too_many_arguments)]
    pub fn scan_paths_with_id_and_control(
        &self,
        scan_id: ScanId,
        workspace_id: WorkspaceId,
        root_id: RootId,
        root: &Path,
        relative_paths: &[PathBuf],
        policy: ScanPolicy,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanOutput, CatalogError> {
        self.platform.inspect_volume(root)?;
        let mut progress = ScanProgress {
            scan_id,
            phase: ScanPhase::Discovering,
            files_discovered: 0,
            files_indexed: 0,
            directories_discovered: 1,
            bytes_discovered: 0,
            files_hashed: 0,
            errors: 0,
            skipped_items: 0,
            duplicate_groups: 0,
        };
        on_progress(progress);

        let mut seen = HashSet::new();
        let paths = relative_paths
            .iter()
            .filter(|path| seen.insert((*path).clone()))
            .cloned()
            .collect::<Vec<_>>();
        let mut enumeration = ReadOnlyEnumeration::default();
        enumeration.progress.directories_discovered = 1;
        for (index, relative_path) in paths.iter().enumerate() {
            if is_cancelled() {
                enumeration.cancelled = true;
                enumeration.progress.skipped_items =
                    enumeration.progress.skipped_items.saturating_add(
                        u64::try_from(paths.len().saturating_sub(index)).unwrap_or(u64::MAX),
                    );
                break;
            }
            if index >= policy.max_entries {
                enumeration.truncated = true;
                enumeration.progress.skipped_items =
                    enumeration.progress.skipped_items.saturating_add(
                        u64::try_from(paths.len().saturating_sub(index)).unwrap_or(u64::MAX),
                    );
                break;
            }
            enumeration.progress.entries_discovered =
                enumeration.progress.entries_discovered.saturating_add(1);
            match self.platform.inspect_regular_file(root, relative_path) {
                Ok(entry) => {
                    enumeration.progress.files_discovered =
                        enumeration.progress.files_discovered.saturating_add(1);
                    enumeration.progress.bytes_discovered = enumeration
                        .progress
                        .bytes_discovered
                        .saturating_add(entry.byte_size);
                    enumeration.files.push(entry);
                }
                Err(error) => {
                    enumeration.issues.push(EnumerationIssue {
                        path: root.join(relative_path),
                        error,
                        is_directory: false,
                    });
                    enumeration.progress.errors = enumeration.progress.errors.saturating_add(1);
                    enumeration.progress.skipped_items =
                        enumeration.progress.skipped_items.saturating_add(1);
                }
            }
            if index.is_multiple_of(128) {
                progress.files_discovered = enumeration.progress.files_discovered;
                progress.bytes_discovered = enumeration.progress.bytes_discovered;
                progress.errors = enumeration.progress.errors;
                progress.skipped_items = enumeration.progress.skipped_items;
                on_progress(progress);
            }
        }
        progress.files_discovered = enumeration.progress.files_discovered;
        progress.bytes_discovered = enumeration.progress.bytes_discovered;
        progress.errors = enumeration.progress.errors;
        progress.skipped_items = enumeration.progress.skipped_items;
        on_progress(progress);

        Ok(self.finish_scan(
            scan_id,
            workspace_id,
            root_id,
            root,
            policy,
            is_cancelled,
            on_progress,
            progress,
            enumeration,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_scan(
        &self,
        scan_id: ScanId,
        workspace_id: WorkspaceId,
        root_id: RootId,
        root: &Path,
        policy: ScanPolicy,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
        mut progress: ScanProgress,
        enumeration: ReadOnlyEnumeration,
    ) -> ScanOutput {
        progress.files_discovered = enumeration.progress.files_discovered;
        progress.directories_discovered = enumeration.progress.directories_discovered;
        progress.bytes_discovered = enumeration.progress.bytes_discovered;
        progress.errors = enumeration.progress.errors;
        progress.skipped_items = enumeration.progress.skipped_items;
        progress.phase = ScanPhase::Inspecting;
        on_progress(progress);

        let mut files = Vec::new();
        let mut issues = enumeration
            .issues
            .into_iter()
            .map(|issue| issue_from_enumeration(root, issue))
            .collect::<Vec<_>>();

        for (index, entry) in enumeration.files.into_iter().enumerate() {
            if is_cancelled() {
                progress.skipped_items = progress.skipped_items.saturating_add(
                    progress
                        .files_discovered
                        .saturating_sub(u64::try_from(files.len()).unwrap_or(u64::MAX)),
                );
                break;
            }
            if entry.hidden && !policy.include_hidden {
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                issues.push(ScanIssue {
                    relative_path: native_path_for_display(&entry.relative_path),
                    kind: ScanIssueKind::Unsupported,
                    message: "hidden file excluded by the active scan policy".to_owned(),
                    is_directory: false,
                    skipped: true,
                });
                continue;
            }

            let label = entry
                .absolute_path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "fichier".to_owned());
            let display_label = match DisplayLabel::new(label.clone()) {
                Ok(value) => value,
                Err(_) => {
                    issues.push(ScanIssue {
                        relative_path: native_path_for_display(&entry.relative_path),
                        kind: ScanIssueKind::InvalidDisplayLabel,
                        message: "filename cannot be represented safely in the interface"
                            .to_owned(),
                        is_directory: false,
                        skipped: true,
                    });
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                    continue;
                }
            };

            let (readability_status, detected_mime, file_error) =
                match self.platform.read_prefix(&entry.absolute_path, 16 * 1024) {
                    Ok(bytes) => (
                        ReadabilityStatus::Readable,
                        detect_mime(&bytes, &entry.absolute_path),
                        None,
                    ),
                    Err(error) => {
                        let kind = issue_kind_from_platform_error(&error);
                        issues.push(ScanIssue {
                            relative_path: native_path_for_display(&entry.relative_path),
                            kind,
                            message: concise_platform_error(&error),
                            is_directory: false,
                            skipped: false,
                        });
                        (
                            ReadabilityStatus::Unreadable,
                            detect_mime(&[], &entry.absolute_path),
                            Some(kind),
                        )
                    }
                };
            let relative_display = native_path_for_display(&entry.relative_path);
            let parent_relative_path = Path::new(&relative_display)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.to_string_lossy().into_owned());
            let extension = entry
                .absolute_path
                .extension()
                .map(|value| value.to_string_lossy().to_ascii_lowercase());
            let accessed_at_ns = entry.accessed_at_ns;
            let fingerprint = fingerprint_from_entry(&entry);

            files.push(CatalogedFile {
                absolute_path: entry.absolute_path,
                observation: FileObservation {
                    file_id: FileId::new(),
                    version_id: FileVersionId::new(),
                    workspace_id,
                    root_id,
                    scan_id,
                    relative_path: entry.relative_path,
                    display_label,
                    kind: FileKind::Regular,
                    detected_mime,
                    fingerprint,
                    read_only: entry.read_only,
                    hidden: entry.hidden,
                    cloud_placeholder: entry.cloud_placeholder,
                    encrypted: entry.encrypted,
                },
                extension,
                parent_relative_path,
                accessed_at_ns,
                readability_status,
                scan_status: if file_error.is_some() {
                    ScanItemStatus::IndexedWithErrors
                } else {
                    ScanItemStatus::Indexed
                },
                hashing_status: HashingStatus::NotCandidate,
                error: file_error,
            });
            progress.files_indexed = u64::try_from(files.len()).unwrap_or(u64::MAX);
            if index.is_multiple_of(128) {
                progress.errors = count_errors(&issues);
                on_progress(progress);
            }
        }

        if enumeration.truncated {
            issues.push(ScanIssue {
                relative_path: ".".to_owned(),
                kind: ScanIssueKind::EntryLimitReached,
                message: format!(
                    "scan stopped after reaching the {}-entry safety limit",
                    policy.max_entries
                ),
                is_directory: true,
                skipped: true,
            });
        }

        let mut candidates_by_size: HashMap<u64, Vec<usize>> = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            candidates_by_size
                .entry(file.observation.fingerprint.byte_size)
                .or_default()
                .push(index);
        }
        let candidate_indices = candidates_by_size
            .into_values()
            .filter(|indices| indices.len() > 1)
            .flatten()
            .collect::<Vec<_>>();
        progress.phase = ScanPhase::Hashing;
        on_progress(progress);

        for (candidate_number, index) in candidate_indices.into_iter().enumerate() {
            if is_cancelled() {
                for file in &mut files {
                    if file.hashing_status == HashingStatus::NotCandidate {
                        file.hashing_status = HashingStatus::Cancelled;
                    }
                }
                break;
            }
            let file = &mut files[index];
            if file.observation.fingerprint.byte_size > policy.max_hash_bytes {
                file.hashing_status = HashingStatus::Failed;
                file.scan_status = ScanItemStatus::IndexedWithErrors;
                file.error = Some(ScanIssueKind::HashBudgetExceeded);
                issues.push(ScanIssue {
                    relative_path: native_path_for_display(&file.observation.relative_path),
                    kind: ScanIssueKind::HashBudgetExceeded,
                    message: "file exceeds the configured hashing budget".to_owned(),
                    is_directory: false,
                    skipped: false,
                });
                continue;
            }
            match self
                .platform
                .fingerprint(&file.absolute_path, true, policy.max_hash_bytes)
            {
                Ok(fingerprint)
                    if fingerprint.native_identity.object_key
                        == file.observation.fingerprint.native_identity.object_key
                        && fingerprint.byte_size == file.observation.fingerprint.byte_size
                        && fingerprint.modified_at_ns
                            == file.observation.fingerprint.modified_at_ns =>
                {
                    file.observation.fingerprint = fingerprint;
                    file.hashing_status = HashingStatus::Hashed;
                    progress.files_hashed = progress.files_hashed.saturating_add(1);
                }
                Ok(_) => {
                    record_hash_failure(
                        file,
                        &mut issues,
                        ScanIssueKind::FileChanged,
                        "file changed between discovery and hashing",
                    );
                }
                Err(error) => {
                    let kind = if matches!(error, PlatformError::Precondition(_)) {
                        ScanIssueKind::FileChanged
                    } else {
                        ScanIssueKind::HashFailed
                    };
                    record_hash_failure(file, &mut issues, kind, &concise_platform_error(&error));
                }
            }
            if candidate_number.is_multiple_of(16) {
                progress.errors = count_errors(&issues);
                on_progress(progress);
            }
        }

        let duplicate_groups = detect_exact_duplicates(&files);
        progress.duplicate_groups = u64::try_from(duplicate_groups.len()).unwrap_or(u64::MAX);
        progress.errors = count_errors(&issues);
        progress.skipped_items = progress.skipped_items.max(
            u64::try_from(issues.iter().filter(|issue| issue.skipped).count()).unwrap_or(u64::MAX),
        );
        let cancelled = enumeration.cancelled || is_cancelled();
        progress.phase = if cancelled {
            ScanPhase::Cancelled
        } else {
            ScanPhase::Persisting
        };
        on_progress(progress);

        ScanOutput {
            scan_id,
            files,
            issues,
            duplicate_groups,
            progress,
            cancelled,
            truncated: enumeration.truncated,
        }
    }
}

fn fingerprint_from_entry(entry: &ReadOnlyEntry) -> FileFingerprint {
    FileFingerprint {
        native_identity: entry.identity.clone(),
        byte_size: entry.byte_size,
        modified_at_ns: entry.modified_at_ns,
        created_at_ns: entry.created_at_ns,
        attributes: entry.attributes,
        quick_digest: None,
        content_digest: None,
    }
}

fn issue_from_enumeration(root: &Path, issue: EnumerationIssue) -> ScanIssue {
    let kind = issue_kind_from_platform_error(&issue.error);
    ScanIssue {
        relative_path: relative_path_for_display(root, &issue.path),
        kind,
        message: concise_platform_error(&issue.error),
        is_directory: issue.is_directory,
        skipped: true,
    }
}

fn issue_kind_from_platform_error(error: &PlatformError) -> ScanIssueKind {
    match error {
        PlatformError::ReparsePoint | PlatformError::PathPolicyRefusal => {
            ScanIssueKind::ReparsePoint
        }
        PlatformError::CloudPlaceholder => ScanIssueKind::CloudPlaceholder,
        PlatformError::PermissionDenied => ScanIssueKind::PermissionDenied,
        PlatformError::Unsupported(_) | PlatformError::OutsideRoot => ScanIssueKind::Unsupported,
        PlatformError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            ScanIssueKind::PermissionDenied
        }
        PlatformError::Io(_)
        | PlatformError::Precondition(_)
        | PlatformError::DestinationExists
        | PlatformError::SourceMissing
        | PlatformError::SharingViolation
        | PlatformError::LockViolation
        | PlatformError::DiskFull
        | PlatformError::AmbiguousMutationOutcome
        | PlatformError::Cancelled
        | PlatformError::VerificationLimitExceeded { .. }
        | PlatformError::SecretStore(_) => ScanIssueKind::Io,
    }
}

fn concise_platform_error(error: &PlatformError) -> String {
    match error {
        PlatformError::OutsideRoot => "entry resolved outside the selected scan root".to_owned(),
        PlatformError::ReparsePoint => {
            "symbolic link, junction, or reparse point was skipped".to_owned()
        }
        PlatformError::PathPolicyRefusal => "filesystem path policy refused the entry".to_owned(),
        PlatformError::CloudPlaceholder => {
            "cloud placeholder was skipped to avoid remote hydration".to_owned()
        }
        PlatformError::Unsupported(message) | PlatformError::Precondition(message) => {
            message.clone()
        }
        PlatformError::PermissionDenied => "permission denied".to_owned(),
        PlatformError::SharingViolation | PlatformError::LockViolation => {
            "entry is currently in use".to_owned()
        }
        PlatformError::DiskFull => "local volume is full".to_owned(),
        PlatformError::SourceMissing => "entry disappeared during scanning".to_owned(),
        PlatformError::AmbiguousMutationOutcome => {
            "local filesystem outcome is ambiguous".to_owned()
        }
        PlatformError::Cancelled => "local verification was cancelled".to_owned(),
        PlatformError::VerificationLimitExceeded { limit_bytes } => {
            format!("entry exceeds the {limit_bytes}-byte verification bound")
        }
        PlatformError::Io(error) => match error.kind() {
            std::io::ErrorKind::NotFound => "entry disappeared during scanning".to_owned(),
            std::io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
            _ => format!("local filesystem I/O error ({:?})", error.kind()),
        },
        PlatformError::DestinationExists | PlatformError::SecretStore(_) => {
            "local platform operation failed".to_owned()
        }
    }
}

fn relative_path_for_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).map_or_else(
        |_| "[outside selected root]".to_owned(),
        |relative| {
            let display = relative.to_string_lossy();
            if display.is_empty() {
                ".".to_owned()
            } else {
                display.into_owned()
            }
        },
    )
}

fn native_path_for_display(path: &domain::NativePath) -> String {
    match path.encoding {
        PathEncoding::UnixBytes => String::from_utf8_lossy(&path.bytes).into_owned(),
        PathEncoding::WindowsUtf16Le => {
            let units = path
                .bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        }
    }
}

fn detect_mime(prefix: &[u8], path: &Path) -> Option<String> {
    if let Some(kind) = infer::get(prefix) {
        return Some(kind.mime_type().to_owned());
    }
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let mime = match extension.as_str() {
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        _ => return None,
    };
    Some(mime.to_owned())
}

fn record_hash_failure(
    file: &mut CatalogedFile,
    issues: &mut Vec<ScanIssue>,
    kind: ScanIssueKind,
    message: &str,
) {
    file.hashing_status = HashingStatus::Failed;
    file.scan_status = ScanItemStatus::IndexedWithErrors;
    file.error = Some(kind);
    issues.push(ScanIssue {
        relative_path: native_path_for_display(&file.observation.relative_path),
        kind,
        message: message.to_owned(),
        is_directory: false,
        skipped: false,
    });
}

fn count_errors(issues: &[ScanIssue]) -> u64 {
    u64::try_from(issues.iter().filter(|issue| issue.kind.is_error()).count()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateKind {
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub kind: DuplicateKind,
    pub key: Vec<u8>,
    pub byte_size: u64,
    pub members: Vec<FileVersionId>,
}

#[must_use]
pub fn detect_exact_duplicates(files: &[CatalogedFile]) -> Vec<DuplicateGroup> {
    let mut groups: HashMap<(u64, [u8; 32]), Vec<FileVersionId>> = HashMap::new();
    for file in files {
        if let Some(digest) = file.observation.fingerprint.content_digest {
            groups
                .entry((file.observation.fingerprint.byte_size, digest))
                .or_default()
                .push(file.observation.version_id);
        }
    }
    groups
        .into_iter()
        .filter(|(_, members)| members.len() > 1)
        .map(|((byte_size, digest), members)| DuplicateGroup {
            kind: DuplicateKind::Exact,
            key: digest.to_vec(),
            byte_size,
            members,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity};
    use std::{
        fs,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Debug)]
    struct ReadOnlyMock {
        reads: AtomicUsize,
        hashes: AtomicUsize,
    }

    impl ReadOnlyPlatform for ReadOnlyMock {
        fn inspect_volume(&self, _root: &Path) -> Result<VolumeIdentity, PlatformError> {
            Ok(volume())
        }

        fn enumerate_regular_files(
            &self,
            root: &Path,
            _max_entries: usize,
            _is_cancelled: &dyn Fn() -> bool,
            on_progress: &mut dyn FnMut(EnumerationProgress),
        ) -> Result<platform::ReadOnlyEnumeration, PlatformError> {
            let progress = EnumerationProgress {
                entries_discovered: 2,
                files_discovered: 1,
                directories_discovered: 1,
                bytes_discovered: 7,
                errors: 1,
                skipped_items: 1,
            };
            on_progress(progress);
            Ok(platform::ReadOnlyEnumeration {
                files: vec![ReadOnlyEntry {
                    absolute_path: root.join("invoice.txt"),
                    relative_path: native("invoice.txt"),
                    identity: identity(),
                    byte_size: 7,
                    modified_at_ns: Some(1),
                    created_at_ns: Some(1),
                    accessed_at_ns: Some(1),
                    attributes: 0,
                    read_only: false,
                    hidden: false,
                    cloud_placeholder: false,
                    encrypted: false,
                }],
                issues: vec![platform::EnumerationIssue {
                    path: root.join("disappeared.txt"),
                    error: PlatformError::SourceMissing,
                    is_directory: false,
                }],
                progress,
                truncated: false,
                cancelled: false,
            })
        }

        fn read_bounded(&self, _path: &Path, _max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
            Ok(b"invoice".to_vec())
        }

        fn read_prefix(&self, _path: &Path, _max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(b"invoice".to_vec())
        }

        fn fingerprint(
            &self,
            _path: &Path,
            include_content_digest: bool,
            _max_bytes: u64,
        ) -> Result<FileFingerprint, PlatformError> {
            self.hashes.fetch_add(1, Ordering::SeqCst);
            Ok(FileFingerprint {
                native_identity: identity(),
                byte_size: 7,
                modified_at_ns: Some(1),
                created_at_ns: Some(1),
                attributes: 0,
                quick_digest: None,
                content_digest: include_content_digest.then_some([1; 32]),
            })
        }
    }

    #[derive(Debug)]
    struct ChangingMock;

    impl ReadOnlyPlatform for ChangingMock {
        fn inspect_volume(&self, _root: &Path) -> Result<VolumeIdentity, PlatformError> {
            Ok(volume())
        }

        fn enumerate_regular_files(
            &self,
            root: &Path,
            _max_entries: usize,
            _is_cancelled: &dyn Fn() -> bool,
            on_progress: &mut dyn FnMut(EnumerationProgress),
        ) -> Result<platform::ReadOnlyEnumeration, PlatformError> {
            let progress = EnumerationProgress {
                entries_discovered: 2,
                files_discovered: 2,
                directories_discovered: 1,
                bytes_discovered: 8,
                errors: 0,
                skipped_items: 0,
            };
            on_progress(progress);
            Ok(platform::ReadOnlyEnumeration {
                files: vec![entry(root, "first.bin", 1), entry(root, "second.bin", 2)],
                issues: Vec::new(),
                progress,
                truncated: false,
                cancelled: false,
            })
        }

        fn read_bounded(&self, _path: &Path, _max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
            Ok(vec![0; 4])
        }

        fn read_prefix(&self, _path: &Path, _max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
            Ok(vec![0; 4])
        }

        fn fingerprint(
            &self,
            path: &Path,
            include_content_digest: bool,
            _max_bytes: u64,
        ) -> Result<FileFingerprint, PlatformError> {
            let (name, key) = if path.ends_with("first.bin") {
                ("first.bin", 1)
            } else {
                ("second.bin", 2)
            };
            Ok(FileFingerprint {
                native_identity: identity_for(name, key),
                byte_size: 4,
                modified_at_ns: Some(2),
                created_at_ns: Some(1),
                attributes: 0,
                quick_digest: None,
                content_digest: include_content_digest.then_some([key; 32]),
            })
        }
    }

    #[derive(Debug, Default)]
    struct TargetedMock {
        inspected: Mutex<Vec<PathBuf>>,
        enumerations: AtomicUsize,
        reads: AtomicUsize,
    }

    impl ReadOnlyPlatform for TargetedMock {
        fn inspect_volume(&self, _root: &Path) -> Result<VolumeIdentity, PlatformError> {
            Ok(volume())
        }

        fn enumerate_regular_files(
            &self,
            _root: &Path,
            _max_entries: usize,
            _is_cancelled: &dyn Fn() -> bool,
            _on_progress: &mut dyn FnMut(EnumerationProgress),
        ) -> Result<platform::ReadOnlyEnumeration, PlatformError> {
            self.enumerations.fetch_add(1, Ordering::SeqCst);
            Ok(platform::ReadOnlyEnumeration::default())
        }

        fn inspect_regular_file(
            &self,
            root: &Path,
            relative_path: &Path,
        ) -> Result<ReadOnlyEntry, PlatformError> {
            self.inspected
                .lock()
                .unwrap_or_else(|error| panic!("inspection log should be available: {error}"))
                .push(relative_path.to_path_buf());
            if relative_path == Path::new("missing.txt") {
                return Err(PlatformError::SourceMissing);
            }
            if relative_path == Path::new("denied.txt") {
                return Err(PlatformError::PermissionDenied);
            }

            let relative_display = relative_path.to_string_lossy().into_owned();
            let key = targeted_key(relative_path);
            let mut result = entry(root, &relative_display, key);
            result.byte_size = u64::from(key).saturating_add(3);
            result.hidden = relative_path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'));
            Ok(result)
        }

        fn read_bounded(&self, _path: &Path, _max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
            Ok(Vec::new())
        }

        fn read_prefix(&self, _path: &Path, _max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(b"plain local content".to_vec())
        }

        fn fingerprint(
            &self,
            path: &Path,
            include_content_digest: bool,
            _max_bytes: u64,
        ) -> Result<FileFingerprint, PlatformError> {
            let name = path.file_name().map_or_else(
                || "file".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            let key = targeted_key(Path::new(&name));
            Ok(FileFingerprint {
                native_identity: identity_for(&name, key),
                byte_size: u64::from(key).saturating_add(3),
                modified_at_ns: Some(1),
                created_at_ns: Some(1),
                attributes: 0,
                quick_digest: None,
                content_digest: include_content_digest.then_some([key; 32]),
            })
        }
    }

    fn entry(root: &Path, name: &str, key: u8) -> ReadOnlyEntry {
        ReadOnlyEntry {
            absolute_path: root.join(name),
            relative_path: native(name),
            identity: identity_for(name, key),
            byte_size: 4,
            modified_at_ns: Some(1),
            created_at_ns: Some(1),
            accessed_at_ns: Some(1),
            attributes: 0,
            read_only: false,
            hidden: false,
            cloud_placeholder: false,
            encrypted: false,
        }
    }

    fn native(value: &str) -> NativePath {
        NativePath {
            encoding: PathEncoding::UnixBytes,
            bytes: value.as_bytes().to_vec(),
        }
    }

    fn targeted_key(path: &Path) -> u8 {
        path.to_string_lossy().bytes().fold(1_u8, u8::wrapping_add)
    }

    fn volume() -> VolumeIdentity {
        VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: "test-volume".to_owned(),
            filesystem_type: Some("test".to_owned()),
            case_sensitive: true,
            removable: false,
            local: true,
        }
    }

    fn identity() -> NativeFileIdentity {
        identity_for("invoice.txt", 1)
    }

    fn identity_for(name: &str, key: u8) -> NativeFileIdentity {
        NativeFileIdentity {
            volume: volume(),
            object_key: vec![key],
            parent_key: vec![2],
            leaf_name: native(name),
            link_count: 1,
            reparse_tag: None,
        }
    }

    #[test]
    fn scan_uses_only_read_only_capabilities() {
        let platform = Arc::new(ReadOnlyMock {
            reads: AtomicUsize::new(0),
            hashes: AtomicUsize::new(0),
        });
        let scanner = CatalogScanner::new(platform.clone());

        let result = scanner.scan(
            WorkspaceId::new(),
            RootId::new(),
            Path::new("/registered"),
            ScanPolicy::default(),
        );

        let output = result.unwrap_or_else(|error| panic!("scan should recover: {error}"));
        assert_eq!(output.files.len(), 1);
        assert_eq!(output.issues.len(), 1);
        assert_eq!(output.issues[0].relative_path, "disappeared.txt");
        assert_eq!(platform.reads.load(Ordering::SeqCst), 1);
        assert_eq!(platform.hashes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn files_changed_during_hashing_are_reported_and_not_grouped() {
        let scanner = CatalogScanner::new(Arc::new(ChangingMock));

        let output = scanner
            .scan(
                WorkspaceId::new(),
                RootId::new(),
                Path::new("/registered"),
                ScanPolicy::default(),
            )
            .unwrap_or_else(|error| panic!("scan should recover: {error}"));

        assert_eq!(output.files.len(), 2);
        assert_eq!(
            output
                .issues
                .iter()
                .filter(|issue| issue.kind == ScanIssueKind::FileChanged)
                .count(),
            2
        );
        assert!(output.duplicate_groups.is_empty());
    }

    #[test]
    fn targeted_scan_inspects_only_unique_requested_paths_without_mutation() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "working-name-targeted-catalog-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root)
            .unwrap_or_else(|error| panic!("fixture root should be created: {error}"));
        let first = root.join("a.txt");
        let second = root.join("b.json");
        let untouched = root.join("untouched.bin");
        fs::write(&first, b"alpha")
            .unwrap_or_else(|error| panic!("fixture file should be written: {error}"));
        fs::write(&second, b"bravo")
            .unwrap_or_else(|error| panic!("fixture file should be written: {error}"));
        fs::write(&untouched, b"sentinel")
            .unwrap_or_else(|error| panic!("fixture file should be written: {error}"));

        let platform = Arc::new(TargetedMock::default());
        let scanner = CatalogScanner::new(platform.clone());
        let output = scanner
            .scan_paths_with_id_and_control(
                ScanId::new(),
                WorkspaceId::new(),
                RootId::new(),
                &root,
                &[
                    PathBuf::from("a.txt"),
                    PathBuf::from("a.txt"),
                    PathBuf::from("b.json"),
                ],
                ScanPolicy::default(),
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("targeted scan should succeed: {error}"));

        assert_eq!(output.files.len(), 2);
        assert_eq!(platform.enumerations.load(Ordering::SeqCst), 0);
        assert_eq!(platform.reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            *platform
                .inspected
                .lock()
                .unwrap_or_else(|error| panic!("inspection log should be available: {error}")),
            vec![PathBuf::from("a.txt"), PathBuf::from("b.json")]
        );
        assert_eq!(
            output.files[0].observation.detected_mime.as_deref(),
            Some("text/plain")
        );
        assert_eq!(
            output.files[1].observation.detected_mime.as_deref(),
            Some("application/json")
        );
        assert_eq!(
            fs::read(&first)
                .unwrap_or_else(|error| panic!("scan must not mutate requested files: {error}")),
            b"alpha"
        );
        assert_eq!(
            fs::read(&untouched)
                .unwrap_or_else(|error| panic!("scan must not mutate other files: {error}")),
            b"sentinel"
        );

        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
        let _ = fs::remove_file(untouched);
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn targeted_scan_bounds_missing_and_permission_issues() {
        let platform = Arc::new(TargetedMock::default());
        let scanner = CatalogScanner::new(platform.clone());
        let output = scanner
            .scan_paths_with_id_and_control(
                ScanId::new(),
                WorkspaceId::new(),
                RootId::new(),
                Path::new("/registered"),
                &[
                    PathBuf::from("missing.txt"),
                    PathBuf::from("missing.txt"),
                    PathBuf::from("denied.txt"),
                    PathBuf::from("denied.txt"),
                ],
                ScanPolicy::default(),
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("targeted scan should recover: {error}"));

        assert!(output.files.is_empty());
        assert_eq!(output.issues.len(), 2);
        assert_eq!(output.issues[0].kind, ScanIssueKind::Io);
        assert_eq!(
            output.issues[0].message,
            "entry disappeared during scanning"
        );
        assert_eq!(output.issues[1].kind, ScanIssueKind::PermissionDenied);
        assert_eq!(output.issues[1].message, "permission denied");
        assert_eq!(
            platform
                .inspected
                .lock()
                .unwrap_or_else(|error| panic!("inspection log should be available: {error}"))
                .len(),
            2
        );
        assert_eq!(platform.enumerations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn targeted_scan_honors_hidden_policy_and_cancellation() {
        let hidden_platform = Arc::new(TargetedMock::default());
        let hidden_output = CatalogScanner::new(hidden_platform)
            .scan_paths_with_id_and_control(
                ScanId::new(),
                WorkspaceId::new(),
                RootId::new(),
                Path::new("/registered"),
                &[PathBuf::from(".secret.txt")],
                ScanPolicy {
                    include_hidden: false,
                    ..ScanPolicy::default()
                },
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("hidden-path scan should recover: {error}"));
        assert!(hidden_output.files.is_empty());
        assert_eq!(hidden_output.issues.len(), 1);
        assert!(hidden_output.issues[0].skipped);

        let cancelled_platform = Arc::new(TargetedMock::default());
        let cancellation_view = cancelled_platform.clone();
        let scanner = CatalogScanner::new(cancelled_platform.clone());
        let output = scanner
            .scan_paths_with_id_and_control(
                ScanId::new(),
                WorkspaceId::new(),
                RootId::new(),
                Path::new("/registered"),
                &[
                    PathBuf::from("first.txt"),
                    PathBuf::from("second.txt"),
                    PathBuf::from("third.txt"),
                ],
                ScanPolicy::default(),
                &|| {
                    !cancellation_view
                        .inspected
                        .lock()
                        .unwrap_or_else(|error| {
                            panic!("inspection log should be available: {error}")
                        })
                        .is_empty()
                },
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("cancelled scan should return output: {error}"));

        assert!(output.cancelled);
        assert_eq!(output.progress.phase, ScanPhase::Cancelled);
        assert_eq!(
            cancelled_platform
                .inspected
                .lock()
                .unwrap_or_else(|error| panic!("inspection log should be available: {error}"))
                .len(),
            1
        );
        assert_eq!(cancelled_platform.enumerations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn exact_groups_require_matching_size_and_digest() {
        let first = FileVersionId::new();
        let second = FileVersionId::new();
        let third = FileVersionId::new();
        let files = vec![
            cataloged(first, 7, [1; 32]),
            cataloged(second, 7, [1; 32]),
            cataloged(third, 8, [1; 32]),
        ];
        let groups = detect_exact_duplicates(&files);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, DuplicateKind::Exact);
        assert_eq!(groups[0].byte_size, 7);
        assert_eq!(groups[0].members, vec![first, second]);
    }

    fn cataloged(version_id: FileVersionId, byte_size: u64, digest: [u8; 32]) -> CatalogedFile {
        CatalogedFile {
            absolute_path: PathBuf::from("/registered/invoice.txt"),
            observation: FileObservation {
                file_id: FileId::new(),
                version_id,
                workspace_id: WorkspaceId::new(),
                root_id: RootId::new(),
                scan_id: ScanId::new(),
                relative_path: native("invoice.txt"),
                display_label: DisplayLabel::new("invoice.txt")
                    .unwrap_or_else(|error| panic!("valid label: {error}")),
                kind: FileKind::Regular,
                detected_mime: Some("text/plain".to_owned()),
                fingerprint: FileFingerprint {
                    native_identity: identity(),
                    byte_size,
                    modified_at_ns: Some(1),
                    created_at_ns: Some(1),
                    attributes: 0,
                    quick_digest: None,
                    content_digest: Some(digest),
                },
                read_only: false,
                hidden: false,
                cloud_placeholder: false,
                encrypted: false,
            },
            extension: Some("txt".to_owned()),
            parent_relative_path: None,
            accessed_at_ns: Some(1),
            readability_status: ReadabilityStatus::Readable,
            scan_status: ScanItemStatus::Indexed,
            hashing_status: HashingStatus::Hashed,
            error: None,
        }
    }
}
