use crate::{FileId, FileVersionId, RootId, ScanId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    Windows,
    MacOs,
    Linux,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathEncoding {
    WindowsUtf16Le,
    UnixBytes,
}

/// A lossless, non-display representation of a native path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativePath {
    pub encoding: PathEncoding,
    pub bytes: Vec<u8>,
}

impl NativePath {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayLabel(String);

impl DisplayLabel {
    pub fn new(value: impl Into<String>) -> Result<Self, DisplayLabelError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(DisplayLabelError::Empty);
        }
        if trimmed.contains('/') || trimmed.contains('\\') {
            return Err(DisplayLabelError::ContainsPathSeparator);
        }
        Ok(Self(trimmed.chars().take(120).collect()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DisplayLabelError {
    #[error("display label cannot be empty")]
    Empty,
    #[error("display label cannot contain a path separator")]
    ContainsPathSeparator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Regular,
    Directory,
    SymbolicLink,
    Special,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileLifecycle {
    Present,
    Missing,
    Offline,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeIdentity {
    pub platform: PlatformKind,
    pub stable_identifier: String,
    pub filesystem_type: Option<String>,
    pub case_sensitive: bool,
    pub removable: bool,
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFileIdentity {
    pub volume: VolumeIdentity,
    pub object_key: Vec<u8>,
    pub parent_key: Vec<u8>,
    pub leaf_name: NativePath,
    pub link_count: u32,
    pub reparse_tag: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub native_identity: NativeFileIdentity,
    pub byte_size: u64,
    pub modified_at_ns: Option<i128>,
    pub created_at_ns: Option<i128>,
    pub attributes: u64,
    pub quick_digest: Option<[u8; 32]>,
    pub content_digest: Option<[u8; 32]>,
}

impl FileFingerprint {
    #[must_use]
    pub fn stable_for_apply(&self) -> bool {
        self.native_identity.link_count == 1
            && self.native_identity.reparse_tag.is_none()
            && self.content_digest.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileObservation {
    pub file_id: FileId,
    pub version_id: FileVersionId,
    pub workspace_id: WorkspaceId,
    pub root_id: RootId,
    pub scan_id: ScanId,
    pub relative_path: NativePath,
    pub display_label: DisplayLabel,
    pub kind: FileKind,
    pub detected_mime: Option<String>,
    pub fingerprint: FileFingerprint,
    pub read_only: bool,
    pub hidden: bool,
    pub cloud_placeholder: bool,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    LowConfidence,
    Uncalibrated,
    OutOfDistribution,
    UnsupportedFormat,
    ExtractionFailed,
    EncryptedContent,
    ReparsePoint,
    HardLink,
    CloudPlaceholder,
    NonLocalVolume,
    NonNtfsVolume,
    RemovableVolume,
    SourceChanged,
    DestinationConflict,
    InvalidPath,
    FileLocked,
    PermissionDenied,
    CrossVolume,
    AmbiguousRecovery,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_labels_cannot_smuggle_paths() {
        assert!(DisplayLabel::new("invoice.pdf").is_ok());
        assert_eq!(
            DisplayLabel::new("../private/invoice.pdf"),
            Err(DisplayLabelError::ContainsPathSeparator)
        );
    }
}
