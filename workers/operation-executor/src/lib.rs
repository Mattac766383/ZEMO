use ipc_contracts::executor_v2::{
    ApprovedOperationManifest, AuthenticatedOperationResponse, AuthenticationError,
    CoordinatorHandshakeFrame, CoordinatorSessionFrame, ExecutorAttemptAudit, ExecutorErrorClass,
    ExecutorFrame, ExecutorOutcome, ExpectedFileStateManifest, FixedBytes32, FrameError,
    HandshakeRefusal, Hello, ImmutableExecutionEnvelope, MAX_MESSAGE_LIFETIME_MS,
    NativePathEncoding, NativePathManifest, OpenSession, OperationDirection,
    OperationPrimitiveManifest, PlatformKindManifest, ProtocolRefusal, ProtocolRefusalCategory,
    QUALIFICATION_CRASH_ENV, ROOT_AUTHORITY_FILE_ENV, ROOT_AUTHORITY_SECRET_NAME,
    ROOT_AUTHORITY_SECRET_SERVICE, SessionOpened, derive_session_key, read_frame, write_frame,
};
use platform::{
    FingerprintProgress, PlatformError, PlatformErrorClass, ReadOnlyPlatform, RenameRequest,
    SafeFileOperations,
};
use privacy::OsSecretStore;
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

pub trait ExecutorHandler {
    /// Handles one already-authenticated manifest selection.
    ///
    /// Implementations that can mutate must durably reject a previously
    /// attempted `(execution_id, operation_id, direction)` before mutation.
    /// Protocol v2 intentionally does not provide arbitrary path arguments.
    fn handle(
        &mut self,
        envelope: &ImmutableExecutionEnvelope,
        operation: &ApprovedOperationManifest,
        direction: &OperationDirection,
    ) -> HandlerOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerOutcome {
    Success {
        observed_state_digest: FixedBytes32,
        audit: ExecutorAttemptAudit,
    },
    ProvenNotApplied {
        code: String,
        detail: String,
        audit: ExecutorAttemptAudit,
    },
    RecoveryRequired {
        code: String,
        detail: String,
        audit: ExecutorAttemptAudit,
    },
}

#[derive(Debug, Default)]
pub struct RefusingExecutorHandler;

impl ExecutorHandler for RefusingExecutorHandler {
    fn handle(
        &mut self,
        _envelope: &ImmutableExecutionEnvelope,
        _operation: &ApprovedOperationManifest,
        _direction: &OperationDirection,
    ) -> HandlerOutcome {
        HandlerOutcome::ProvenNotApplied {
            code: "executor_handler_not_wired".to_owned(),
            detail: "The authenticated executor handler is not wired in this milestone.".to_owned(),
            audit: attempt_audit(1, Some(ExecutorErrorClass::Unsupported)),
        }
    }
}

pub struct NativeExecutorHandler<P> {
    platform: P,
    is_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl<P: std::fmt::Debug> std::fmt::Debug for NativeExecutorHandler<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeExecutorHandler")
            .field("platform", &self.platform)
            .field("is_cancelled", &"<callback>")
            .finish()
    }
}

impl<P> NativeExecutorHandler<P> {
    #[must_use]
    pub fn new(platform: P) -> Self {
        Self {
            platform,
            is_cancelled: Arc::new(|| false),
        }
    }

    #[must_use]
    pub fn with_cancellation(
        platform: P,
        is_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            platform,
            is_cancelled,
        }
    }
}

impl<P> ExecutorHandler for NativeExecutorHandler<P>
where
    P: ReadOnlyPlatform + SafeFileOperations,
{
    fn handle(
        &mut self,
        envelope: &ImmutableExecutionEnvelope,
        operation: &ApprovedOperationManifest,
        direction: &OperationDirection,
    ) -> HandlerOutcome {
        let root = match self.validate_envelope_root(envelope) {
            Ok(root) => root,
            Err(error) => return proven_not_applied("root_binding_invalid", error),
        };
        if envelope.operation(&operation.operation_id) != Some(operation) {
            return proven_not_applied(
                "operation_manifest_invalid",
                "The selected operation is not the exact manifest bound to this envelope.",
            );
        }
        if let Err(error) = operation.primitive.validate(&envelope.root_binding.volume) {
            return proven_not_applied("operation_manifest_invalid", error.to_string());
        }
        if operation_requires_qualified_case_only_staging(operation) {
            if !envelope
                .safety_policy_binding
                .allow_qualified_case_only_rename
            {
                return proven_not_applied(
                    "case_only_rename_unqualified",
                    "The authenticated safety policy does not qualify case-only staging.",
                );
            }
            if !qualified_case_only_stage_chain_valid(envelope, operation) {
                return proven_not_applied(
                    "operation_manifest_invalid",
                    "The case-only operation is not bound to an internal staging transition.",
                );
            }
        }
        match (&operation.primitive, direction) {
            (
                OperationPrimitiveManifest::CreateDirectory {
                    destination_relative_path,
                },
                OperationDirection::Forward,
            ) => self.create_directory(&root, destination_relative_path),
            (
                OperationPrimitiveManifest::CreateDirectory {
                    destination_relative_path,
                },
                OperationDirection::Rollback,
            ) => self.remove_directory(&root, destination_relative_path),
            (
                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {
                    source_relative_path,
                },
                OperationDirection::Forward,
            ) => self.remove_directory(&root, source_relative_path),
            (
                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {
                    source_relative_path,
                },
                OperationDirection::Rollback,
            ) => self.create_directory(&root, source_relative_path),
            (
                OperationPrimitiveManifest::SameVolumeMove {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                }
                | OperationPrimitiveManifest::SameVolumeRename {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                }
                | OperationPrimitiveManifest::SameVolumeMoveAndRename {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                }
                | OperationPrimitiveManifest::InternalStage {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                },
                OperationDirection::Forward,
            ) => {
                qualification_crash("before_mutation");
                let outcome = self.rename(
                    &root,
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                    envelope.safety_policy_binding.maximum_rehash_bytes,
                );
                if matches!(outcome, HandlerOutcome::Success { .. }) {
                    if matches!(
                        operation.primitive,
                        OperationPrimitiveManifest::InternalStage { .. }
                    ) {
                        qualification_crash("after_stage");
                    }
                    qualification_crash("after_mutation");
                }
                outcome
            }
            (
                OperationPrimitiveManifest::SameVolumeMove {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                }
                | OperationPrimitiveManifest::SameVolumeRename {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                }
                | OperationPrimitiveManifest::SameVolumeMoveAndRename {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                }
                | OperationPrimitiveManifest::InternalStage {
                    source_relative_path,
                    destination_relative_path,
                    original_source_relative_path,
                    expected_source,
                },
                OperationDirection::Rollback,
            ) => self.rename(
                &root,
                destination_relative_path,
                source_relative_path,
                original_source_relative_path,
                expected_source,
                envelope.safety_policy_binding.maximum_rehash_bytes,
            ),
        }
    }
}

impl<P> NativeExecutorHandler<P>
where
    P: ReadOnlyPlatform + SafeFileOperations,
{
    fn validate_envelope_root(
        &self,
        envelope: &ImmutableExecutionEnvelope,
    ) -> Result<PathBuf, String> {
        envelope.validate().map_err(|error| error.to_string())?;
        let root = decode_native_path(&envelope.root_binding.canonical_path)?;
        if !root.is_absolute() {
            return Err("approved root is not absolute".to_owned());
        }
        let metadata = std::fs::symlink_metadata(&root).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("approved root is linked or is not a directory".to_owned());
        }
        let canonical = std::fs::canonicalize(&root).map_err(|error| error.to_string())?;
        if canonical != root {
            return Err("approved root no longer matches its canonical binding".to_owned());
        }
        let observed = self
            .platform
            .inspect_volume(&root)
            .map_err(|error| error.to_string())?;
        let expected = &envelope.root_binding.volume;
        if envelope.safety_policy_binding.maximum_rehash_bytes == 0
            || envelope.safety_policy_binding.maximum_rehash_bytes
                > domain::MAX_EXECUTION_VERIFICATION_BYTES
        {
            return Err("execution verification bound is invalid".to_owned());
        }
        let platform_matches = matches!(
            (observed.platform, expected.platform),
            (domain::PlatformKind::Windows, PlatformKindManifest::Windows)
                | (domain::PlatformKind::MacOs, PlatformKindManifest::MacOs)
                | (domain::PlatformKind::Linux, PlatformKindManifest::Linux)
                | (domain::PlatformKind::Other, PlatformKindManifest::Other)
        );
        if !platform_matches
            || observed.stable_identifier != expected.stable_identifier
            || observed.filesystem_type != expected.filesystem_type
            || observed.case_sensitive != expected.case_sensitive
            || observed.removable != expected.removable
            || observed.local != expected.local
            || !observed.local
            || observed.removable
        {
            return Err("approved root volume binding changed".to_owned());
        }
        if cfg!(windows)
            && (!matches!(expected.platform, PlatformKindManifest::Windows)
                || !expected
                    .filesystem_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("NTFS")))
        {
            return Err("Windows mutation requires the approved local NTFS volume".to_owned());
        }
        if cfg!(target_os = "macos")
            && matches!(expected.platform, PlatformKindManifest::MacOs)
            && !expected.filesystem_type.as_deref().is_some_and(|value| {
                value.eq_ignore_ascii_case("apfs") || value.eq_ignore_ascii_case("hfs")
            })
        {
            return Err("macOS mutation requires the approved local APFS or HFS volume".to_owned());
        }
        Ok(root)
    }

    fn create_directory(&self, root: &Path, relative: &str) -> HandlerOutcome {
        let destination = match resolve_relative(root, relative) {
            Ok(value) => value,
            Err(error) => return proven_not_applied("relative_path_invalid", error),
        };
        match std::fs::symlink_metadata(&destination) {
            Ok(_) => {
                return proven_not_applied(
                    "destination_exists",
                    "directory destination already exists",
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return proven_not_applied("destination_inspection_failed", error),
        }
        if let Err(error) = self.platform.create_directory_no_replace(&destination) {
            return mutation_error(error, 1, None);
        }
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                HandlerOutcome::Success {
                    observed_state_digest: state_digest(b"directory-present", relative.as_bytes()),
                    audit: attempt_audit(1, None),
                }
            }
            Ok(_) => recovery_required(
                "directory_postcondition_failed",
                "created path is not an unlinked directory",
            ),
            Err(error) => recovery_required("directory_postcondition_failed", error),
        }
    }

    fn remove_directory(&self, root: &Path, relative: &str) -> HandlerOutcome {
        let destination = match resolve_relative(root, relative) {
            Ok(value) => value,
            Err(error) => return proven_not_applied("relative_path_invalid", error),
        };
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return proven_not_applied(
                    "rollback_directory_invalid",
                    "rollback path is not an unlinked directory",
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return proven_not_applied(
                    "rollback_directory_absent",
                    "rollback directory is already absent",
                );
            }
            Err(error) => return proven_not_applied("rollback_directory_inspection_failed", error),
        }
        match std::fs::read_dir(&destination) {
            Ok(mut entries) => match entries.next() {
                None => {}
                Some(Ok(_)) => {
                    return proven_not_applied(
                        "rollback_directory_not_empty",
                        "rollback removes a directory only when it is empty",
                    );
                }
                Some(Err(error)) => {
                    return proven_not_applied("rollback_directory_inspection_failed", error);
                }
            },
            Err(error) => {
                return proven_not_applied("rollback_directory_inspection_failed", error);
            }
        }
        if let Err(error) = self.platform.remove_directory_if_empty(&destination) {
            return mutation_error(error, 1, None);
        }
        match std::fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HandlerOutcome::Success {
                observed_state_digest: state_digest(b"directory-absent", relative.as_bytes()),
                audit: attempt_audit(1, None),
            },
            Ok(_) => recovery_required(
                "directory_rollback_postcondition_failed",
                "directory remains after the native rollback primitive returned success",
            ),
            Err(error) => recovery_required("directory_rollback_postcondition_failed", error),
        }
    }

    fn rename(
        &self,
        root: &Path,
        source_relative: &str,
        destination_relative: &str,
        original_source_relative: &str,
        expected: &ExpectedFileStateManifest,
        maximum_hash_bytes: u64,
    ) -> HandlerOutcome {
        if maximum_hash_bytes == 0
            || maximum_hash_bytes > domain::MAX_EXECUTION_VERIFICATION_BYTES
            || expected.byte_size > maximum_hash_bytes
        {
            return proven_not_applied_with_audit(
                "verification_limit_exceeded",
                "The source exceeds the authenticated execution verification bound.",
                attempt_audit(1, Some(ExecutorErrorClass::VerificationLimit)),
            );
        }
        let source = match resolve_relative(root, source_relative) {
            Ok(value) => value,
            Err(error) => return proven_not_applied("source_path_invalid", error),
        };
        let destination = match resolve_relative(root, destination_relative) {
            Ok(value) => value,
            Err(error) => return proven_not_applied("destination_path_invalid", error),
        };
        let require_original_location = source_relative == original_source_relative;
        let mut last_error_class = None;
        let mut successful_attempt = 0_u8;
        for attempt in 1_u8..=3 {
            if (self.is_cancelled)() {
                return proven_not_applied_with_audit(
                    "verification_cancelled",
                    "Streaming verification was cancelled before mutation.",
                    attempt_audit(attempt, Some(ExecutorErrorClass::Cancelled)),
                );
            }
            if let Err(error) = self.platform.validate_destination_absent(&destination) {
                if retry_transient_before_mutation(&error, attempt, &mut last_error_class) {
                    continue;
                }
                return mutation_error(error, attempt, last_error_class);
            }
            let observed = match self.platform.fingerprint_streaming(
                &source,
                true,
                maximum_hash_bytes,
                self.is_cancelled.as_ref(),
                &mut |_progress: FingerprintProgress| {},
            ) {
                Ok(value) => value,
                Err(error) => {
                    if retry_transient_before_mutation(&error, attempt, &mut last_error_class) {
                        continue;
                    }
                    return mutation_error(error, attempt, last_error_class);
                }
            };
            if !expected_matches(expected, &observed, require_original_location) {
                return proven_not_applied_with_audit(
                    "source_precondition_failed",
                    "Source identity, location, size, modified time, or content digest changed.",
                    attempt_audit(attempt, Some(ExecutorErrorClass::Precondition)),
                );
            }
            let request = match rename_request(
                source.clone(),
                destination.clone(),
                expected,
                &observed,
                maximum_hash_bytes,
            ) {
                Ok(value) => value,
                Err(error) => return proven_not_applied("operation_manifest_invalid", error),
            };
            match self.platform.rename_same_volume_no_replace(&request) {
                Ok(_) => {
                    successful_attempt = attempt;
                    break;
                }
                Err(error) => {
                    if retry_transient_before_mutation(&error, attempt, &mut last_error_class) {
                        continue;
                    }
                    return mutation_error(error, attempt, last_error_class);
                }
            }
        }
        if successful_attempt == 0 {
            return recovery_required_with_audit(
                "native_mutation_ambiguous",
                "The bounded retry loop ended without a classified result.",
                attempt_audit(3, Some(ExecutorErrorClass::AmbiguousMutationOutcome)),
            );
        }
        if let Err(error) = self.platform.validate_destination_absent(&source) {
            if !matches!(error, PlatformError::DestinationExists) {
                return recovery_required_with_audit(
                    "rename_postcondition_failed",
                    error,
                    attempt_audit(
                        successful_attempt,
                        Some(ExecutorErrorClass::AmbiguousMutationOutcome),
                    ),
                );
            }
            return recovery_required_with_audit(
                "rename_postcondition_failed",
                "source still exists after native rename returned success",
                attempt_audit(
                    successful_attempt,
                    Some(ExecutorErrorClass::AmbiguousMutationOutcome),
                ),
            );
        }
        let moved = match self.platform.fingerprint_streaming(
            &destination,
            true,
            maximum_hash_bytes,
            self.is_cancelled.as_ref(),
            &mut |_progress: FingerprintProgress| {},
        ) {
            Ok(value) => value,
            Err(error) => {
                return recovery_required_with_audit(
                    "rename_postcondition_failed",
                    error,
                    attempt_audit(
                        successful_attempt,
                        Some(ExecutorErrorClass::AmbiguousMutationOutcome),
                    ),
                );
            }
        };
        if !expected_matches(expected, &moved, false) {
            return recovery_required_with_audit(
                "rename_postcondition_failed",
                "destination identity, size, or content digest does not match the manifest",
                attempt_audit(
                    successful_attempt,
                    Some(ExecutorErrorClass::AmbiguousMutationOutcome),
                ),
            );
        }
        HandlerOutcome::Success {
            observed_state_digest: fingerprint_digest(&moved),
            audit: attempt_audit(successful_attempt, last_error_class),
        }
    }
}

fn operation_requires_qualified_case_only_staging(operation: &ApprovedOperationManifest) -> bool {
    let (source, original, destination) = match &operation.primitive {
        OperationPrimitiveManifest::SameVolumeMove {
            source_relative_path,
            original_source_relative_path,
            destination_relative_path,
            ..
        }
        | OperationPrimitiveManifest::SameVolumeRename {
            source_relative_path,
            original_source_relative_path,
            destination_relative_path,
            ..
        }
        | OperationPrimitiveManifest::SameVolumeMoveAndRename {
            source_relative_path,
            original_source_relative_path,
            destination_relative_path,
            ..
        } => (
            source_relative_path,
            original_source_relative_path,
            destination_relative_path,
        ),
        OperationPrimitiveManifest::CreateDirectory { .. }
        | OperationPrimitiveManifest::InternalStage { .. } => return false,
    };
    (source != destination && source.to_lowercase() == destination.to_lowercase())
        || (original != destination && original.to_lowercase() == destination.to_lowercase())
}

fn qualified_case_only_stage_chain_valid(
    envelope: &ImmutableExecutionEnvelope,
    operation: &ApprovedOperationManifest,
) -> bool {
    let (source, destination, original, expected) = match &operation.primitive {
        OperationPrimitiveManifest::SameVolumeMove {
            source_relative_path,
            destination_relative_path,
            original_source_relative_path,
            expected_source,
            ..
        }
        | OperationPrimitiveManifest::SameVolumeRename {
            source_relative_path,
            destination_relative_path,
            original_source_relative_path,
            expected_source,
            ..
        }
        | OperationPrimitiveManifest::SameVolumeMoveAndRename {
            source_relative_path,
            destination_relative_path,
            original_source_relative_path,
            expected_source,
            ..
        } => (
            source_relative_path,
            destination_relative_path,
            original_source_relative_path,
            expected_source,
        ),
        OperationPrimitiveManifest::CreateDirectory { .. }
        | OperationPrimitiveManifest::InternalStage { .. } => return false,
    };
    let staging_components = source.split('/').collect::<Vec<_>>();
    original != destination
        && original.to_lowercase() == destination.to_lowercase()
        && staging_components.len() == 3
        && staging_components[0] == ".supremacy-staging"
        && staging_components[1] == envelope.execution_id
        && is_canonical_uuid(staging_components[2])
        && operation.dependencies.len() == 1
        && operation
            .dependencies
            .iter()
            .filter(|dependency| {
                envelope.operation(dependency).is_some_and(|candidate| {
                    matches!(
                        &candidate.primitive,
                        OperationPrimitiveManifest::InternalStage {
                            source_relative_path,
                            destination_relative_path,
                            original_source_relative_path,
                            expected_source: stage_expected,
                        } if source_relative_path == original
                            && destination_relative_path == source
                            && original_source_relative_path == original
                            && same_journaled_file_identity(stage_expected, expected)
                            && candidate.sequence < operation.sequence
                    )
                })
            })
            .count()
            == 1
}

fn same_journaled_file_identity(
    left: &ExpectedFileStateManifest,
    right: &ExpectedFileStateManifest,
) -> bool {
    !left.native_identity.object_key.as_slice().is_empty()
        && left.native_identity.object_key == right.native_identity.object_key
        && left.native_identity.volume == right.native_identity.volume
        && left.content_digest == right.content_digest
        && left.byte_size == right.byte_size
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn resolve_relative(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.starts_with(['/', '\\'])
        || relative.ends_with(['/', '\\'])
        || relative.contains(':')
    {
        return Err("relative path is malformed".to_owned());
    }
    let mut path = PathBuf::new();
    for component in relative.split(['/', '\\']) {
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with([' ', '.'])
            || is_windows_device_component(component)
        {
            return Err("relative path contains an unsafe component".to_owned());
        }
        path.push(component);
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("relative path contains a non-normal component".to_owned());
    }
    Ok(root.join(path))
}

fn is_windows_device_component(component: &str) -> bool {
    let stem = component
        .split_once('.')
        .map_or(component, |(stem, _)| stem)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                suffix.len() == 1
                    && suffix
                        .as_bytes()
                        .first()
                        .is_some_and(|digit| (b'1'..=b'9').contains(digit))
            })
}

fn decode_native_path(path: &NativePathManifest) -> Result<PathBuf, String> {
    match path.encoding {
        NativePathEncoding::WindowsUtf16Le => {
            let chunks = path.bytes.as_slice().chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return Err("Windows native path has an odd byte length".to_owned());
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
                    .map_err(|_| "Windows native path is invalid UTF-16".to_owned())
            }
        }
        NativePathEncoding::UnixBytes => {
            #[cfg(unix)]
            {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                Ok(PathBuf::from(OsString::from_vec(
                    path.bytes.as_slice().to_vec(),
                )))
            }
            #[cfg(not(unix))]
            {
                String::from_utf8(path.bytes.as_slice().to_vec())
                    .map(PathBuf::from)
                    .map_err(|_| "Unix native path is invalid on this platform".to_owned())
            }
        }
    }
}

fn rename_request(
    source: PathBuf,
    destination: PathBuf,
    expected: &ExpectedFileStateManifest,
    observed: &domain::FileFingerprint,
    maximum_hash_bytes: u64,
) -> Result<RenameRequest, String> {
    Ok(RenameRequest {
        source,
        destination,
        expected_identity: observed.native_identity.clone(),
        expected_byte_size: expected.byte_size,
        expected_modified_at_ns: expected.modified_at_ns.map(i128::from),
        expected_attributes: expected.attributes,
        expected_content_digest: *expected.content_digest.as_bytes(),
        maximum_hash_bytes,
    })
}

fn manifest_volume(
    value: &ipc_contracts::executor_v2::VolumeIdentityManifest,
) -> domain::VolumeIdentity {
    domain::VolumeIdentity {
        platform: match value.platform {
            PlatformKindManifest::Windows => domain::PlatformKind::Windows,
            PlatformKindManifest::MacOs => domain::PlatformKind::MacOs,
            PlatformKindManifest::Linux => domain::PlatformKind::Linux,
            PlatformKindManifest::Other => domain::PlatformKind::Other,
        },
        stable_identifier: value.stable_identifier.clone(),
        filesystem_type: value.filesystem_type.clone(),
        case_sensitive: value.case_sensitive,
        removable: value.removable,
        local: value.local,
    }
}

fn expected_matches(
    expected: &ExpectedFileStateManifest,
    observed: &domain::FileFingerprint,
    require_original_location: bool,
) -> bool {
    let identity = &observed.native_identity;
    identity.volume == manifest_volume(&expected.native_identity.volume)
        && identity.object_key.as_slice() == expected.native_identity.object_key.as_slice()
        && (!require_original_location
            || (identity.parent_key.as_slice() == expected.native_identity.parent_key.as_slice()
                && identity.leaf_name.encoding
                    == match expected.native_identity.leaf_name.encoding {
                        NativePathEncoding::WindowsUtf16Le => domain::PathEncoding::WindowsUtf16Le,
                        NativePathEncoding::UnixBytes => domain::PathEncoding::UnixBytes,
                    }
                && identity.leaf_name.bytes.as_slice()
                    == expected.native_identity.leaf_name.bytes.as_slice()))
        && expected.native_identity.link_count == 1
        && identity.link_count == 1
        && expected.native_identity.reparse_tag.is_none()
        && identity.reparse_tag.is_none()
        && observed.byte_size == expected.byte_size
        && observed.modified_at_ns == expected.modified_at_ns.map(i128::from)
        && observed.attributes == expected.attributes
        && observed.content_digest.as_ref() == Some(expected.content_digest.as_bytes())
}

fn fingerprint_digest(value: &domain::FileFingerprint) -> FixedBytes32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"com.workingname.operation-executor/v2/observed-file-state\0");
    hasher.update(value.native_identity.volume.stable_identifier.as_bytes());
    hasher.update(&value.native_identity.object_key);
    hasher.update(&value.byte_size.to_le_bytes());
    if let Some(digest) = value.content_digest {
        hasher.update(&digest);
    }
    FixedBytes32::from_bytes(*hasher.finalize().as_bytes())
}

fn state_digest(domain: &[u8], value: &[u8]) -> FixedBytes32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"com.workingname.operation-executor/v2/observed-directory-state\0");
    hasher.update(domain);
    hasher.update(value);
    FixedBytes32::from_bytes(*hasher.finalize().as_bytes())
}

fn retry_transient_before_mutation(
    error: &PlatformError,
    attempt: u8,
    last_error_class: &mut Option<ExecutorErrorClass>,
) -> bool {
    if !error.retryable_before_mutation() || attempt >= 3 {
        return false;
    }
    *last_error_class = Some(executor_error_class(error.class()));
    std::thread::sleep(retry_backoff(attempt));
    true
}

const fn retry_backoff(attempt: u8) -> Duration {
    match attempt {
        1 => Duration::from_millis(25),
        2 => Duration::from_millis(50),
        _ => Duration::ZERO,
    }
}

const fn executor_error_class(class: PlatformErrorClass) -> ExecutorErrorClass {
    match class {
        PlatformErrorClass::SharingViolation => ExecutorErrorClass::SharingViolation,
        PlatformErrorClass::LockViolation => ExecutorErrorClass::LockViolation,
        PlatformErrorClass::PermissionDenied => ExecutorErrorClass::PermissionDenied,
        PlatformErrorClass::DiskFull => ExecutorErrorClass::DiskFull,
        PlatformErrorClass::DestinationCollision => ExecutorErrorClass::DestinationCollision,
        PlatformErrorClass::SourceMissing => ExecutorErrorClass::SourceMissing,
        PlatformErrorClass::PathPolicyRefusal => ExecutorErrorClass::PathPolicyRefusal,
        PlatformErrorClass::Precondition => ExecutorErrorClass::Precondition,
        PlatformErrorClass::VerificationLimit => ExecutorErrorClass::VerificationLimit,
        PlatformErrorClass::Cancelled => ExecutorErrorClass::Cancelled,
        PlatformErrorClass::Unsupported => ExecutorErrorClass::Unsupported,
        PlatformErrorClass::AmbiguousMutationOutcome => {
            ExecutorErrorClass::AmbiguousMutationOutcome
        }
        PlatformErrorClass::Io | PlatformErrorClass::SecretStore => ExecutorErrorClass::Io,
    }
}

fn mutation_error(
    error: PlatformError,
    attempt_count: u8,
    _last_error_class: Option<ExecutorErrorClass>,
) -> HandlerOutcome {
    let error_class = executor_error_class(error.class());
    let audit = attempt_audit(attempt_count, Some(error_class));
    match error {
        PlatformError::SharingViolation | PlatformError::LockViolation => {
            proven_not_applied_with_audit(
                "file_in_use",
                "This file is currently in use and was not moved.",
                audit,
            )
        }
        PlatformError::DestinationExists => proven_not_applied_with_audit(
            "destination_exists",
            "The destination already exists.",
            audit,
        ),
        PlatformError::Precondition(detail) => {
            proven_not_applied_with_audit("source_precondition_failed", detail, audit)
        }
        PlatformError::SourceMissing => {
            proven_not_applied_with_audit("source_missing", "The source no longer exists.", audit)
        }
        PlatformError::OutsideRoot
        | PlatformError::ReparsePoint
        | PlatformError::CloudPlaceholder
        | PlatformError::PathPolicyRefusal
        | PlatformError::Unsupported(_) => {
            proven_not_applied_with_audit("native_primitive_refused", error, audit)
        }
        PlatformError::PermissionDenied => proven_not_applied_with_audit(
            "permission_denied",
            "Permission policy refused the operation; no permissions were changed.",
            audit,
        ),
        PlatformError::DiskFull => proven_not_applied_with_audit(
            "disk_full",
            "The destination volume is full and the file was not moved.",
            audit,
        ),
        PlatformError::Cancelled => proven_not_applied_with_audit(
            "verification_cancelled",
            "Streaming verification was cancelled before mutation.",
            audit,
        ),
        PlatformError::VerificationLimitExceeded { .. } => proven_not_applied_with_audit(
            "verification_limit_exceeded",
            "The file exceeds the authenticated execution verification bound.",
            audit,
        ),
        PlatformError::AmbiguousMutationOutcome
        | PlatformError::Io(_)
        | PlatformError::SecretStore(_) => {
            recovery_required_with_audit("native_mutation_ambiguous", error, audit)
        }
    }
}

fn proven_not_applied(code: &str, detail: impl ToString) -> HandlerOutcome {
    proven_not_applied_with_audit(code, detail, attempt_audit(1, None))
}

fn proven_not_applied_with_audit(
    code: &str,
    detail: impl ToString,
    audit: ExecutorAttemptAudit,
) -> HandlerOutcome {
    HandlerOutcome::ProvenNotApplied {
        code: code.to_owned(),
        detail: detail.to_string(),
        audit,
    }
}

fn recovery_required(code: &str, detail: impl ToString) -> HandlerOutcome {
    recovery_required_with_audit(code, detail, attempt_audit(1, None))
}

fn recovery_required_with_audit(
    code: &str,
    detail: impl ToString,
    audit: ExecutorAttemptAudit,
) -> HandlerOutcome {
    HandlerOutcome::RecoveryRequired {
        code: code.to_owned(),
        detail: detail.to_string(),
        audit,
    }
}

const fn attempt_audit(
    attempt_count: u8,
    error_class: Option<ExecutorErrorClass>,
) -> ExecutorAttemptAudit {
    ExecutorAttemptAudit {
        attempt_count,
        error_class,
    }
}

pub trait Clock {
    fn now_unix_ms(&self) -> Result<i64, ServerError>;
}

pub trait NonceSource {
    fn next_nonce(&mut self) -> Result<FixedBytes32, ServerError>;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> Result<i64, ServerError> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ServerError::Clock)?
            .as_millis();
        i64::try_from(millis).map_err(|_| ServerError::Clock)
    }
}

#[derive(Debug, Default)]
pub struct SystemNonceSource;

impl NonceSource for SystemNonceSource {
    fn next_nonce(&mut self) -> Result<FixedBytes32, ServerError> {
        loop {
            let mut nonce = [0_u8; 32];
            getrandom::fill(&mut nonce).map_err(|_| ServerError::Randomness)?;
            let nonce = FixedBytes32::from_bytes(nonce);
            if !nonce.is_zero() {
                return Ok(nonce);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerExit {
    CoordinatorEof,
    Refused,
    RecoveryRequired,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("the executor root authority secret is unavailable")]
    RootAuthorityUnavailable,
    #[error("the system clock is unavailable")]
    Clock,
    #[error("secure randomness is unavailable")]
    Randomness,
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Authentication(#[from] AuthenticationError),
}

pub fn run_stdio() -> Result<ServerExit, ServerError> {
    let key = load_root_authority_key()?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    #[cfg(windows)]
    let mut handler = NativeExecutorHandler::new(platform_windows::WindowsPlatform);
    #[cfg(target_os = "macos")]
    let mut handler = NativeExecutorHandler::new(platform_macos::MacOsPlatform);
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut handler = RefusingExecutorHandler;
    serve(
        &mut reader,
        &mut writer,
        &key,
        std::process::id(),
        &SystemClock,
        &mut SystemNonceSource,
        &mut handler,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn serve<R, W, C, N, H>(
    reader: &mut R,
    writer: &mut W,
    root_authority_key: &[u8; 32],
    worker_pid: u32,
    clock: &C,
    nonces: &mut N,
    handler: &mut H,
) -> Result<ServerExit, ServerError>
where
    R: Read,
    W: Write,
    C: Clock,
    N: NonceSource,
    H: ExecutorHandler,
{
    let hello = Hello::signed(
        worker_pid,
        nonces.next_nonce()?,
        clock.now_unix_ms()?,
        root_authority_key,
    )?;
    write_frame(writer, &ExecutorFrame::Hello(hello.clone()))?;

    let handshake = match read_frame::<_, CoordinatorHandshakeFrame>(reader) {
        Ok(Some(CoordinatorHandshakeFrame::OpenSession(open_session))) => open_session,
        Ok(None) => return Ok(ServerExit::CoordinatorEof),
        Err(error) => {
            return refuse_handshake(
                writer,
                &hello,
                root_authority_key,
                clock,
                nonces,
                ProtocolRefusalCategory::Protocol,
                frame_error_code(&error),
                "The coordinator handshake frame was rejected.",
            );
        }
    };
    let now = clock.now_unix_ms()?;
    if let Err(error) = handshake.verify(&hello, root_authority_key, now) {
        return refuse_handshake(
            writer,
            &hello,
            root_authority_key,
            clock,
            nonces,
            error.refusal_category(),
            error.refusal_code(),
            "The execution session could not be authenticated.",
        );
    }
    let session_key = derive_session_key(root_authority_key, &hello, &handshake)?;
    let opened = SessionOpened::signed(
        &handshake,
        nonces.next_nonce()?,
        clock.now_unix_ms()?,
        &session_key,
    )?;
    write_frame(writer, &ExecutorFrame::SessionOpened(opened))?;
    serve_session(
        reader,
        writer,
        &handshake,
        &session_key,
        clock,
        nonces,
        handler,
        &hello,
        root_authority_key,
    )
}

#[allow(clippy::too_many_arguments)]
fn serve_session<R, W, C, N, H>(
    reader: &mut R,
    writer: &mut W,
    open_session: &OpenSession,
    session_key: &ipc_contracts::executor_v2::SessionKey,
    clock: &C,
    nonces: &mut N,
    handler: &mut H,
    hello: &Hello,
    root_authority_key: &[u8; 32],
) -> Result<ServerExit, ServerError>
where
    R: Read,
    W: Write,
    C: Clock,
    N: NonceSource,
    H: ExecutorHandler,
{
    let mut next_sequence = 1_u64;
    let mut seen_nonces = BTreeSet::new();
    let mut attempted_bindings = BTreeSet::new();
    let mut applied_forward = BTreeSet::new();
    let mut remaining_rollback = open_session.authorization.rollback_eligible_ids();

    loop {
        let frame = match read_frame::<_, CoordinatorSessionFrame>(reader) {
            Ok(Some(CoordinatorSessionFrame::ExecuteOperation(request))) => request,
            Ok(None) => return Ok(ServerExit::CoordinatorEof),
            Err(error) => {
                return refuse_handshake(
                    writer,
                    hello,
                    root_authority_key,
                    clock,
                    nonces,
                    ProtocolRefusalCategory::Protocol,
                    frame_error_code(&error),
                    "The authenticated session frame was malformed.",
                );
            }
        };
        let now = clock.now_unix_ms()?;
        if let Err(error) = frame.verify(open_session, next_sequence, session_key, now) {
            send_protocol_refusal(
                writer,
                open_session,
                session_key,
                clock,
                nonces,
                next_sequence,
                frame.operation,
                error.refusal_category(),
                error.refusal_code(),
                "The operation request failed authenticated session validation.",
            )?;
            return Ok(ServerExit::Refused);
        }
        if !seen_nonces.insert(frame.message_nonce) {
            send_protocol_refusal(
                writer,
                open_session,
                session_key,
                clock,
                nonces,
                next_sequence,
                frame.operation,
                ProtocolRefusalCategory::Replay,
                "message_nonce_replay",
                "The operation request reused a session message nonce.",
            )?;
            return Ok(ServerExit::Refused);
        }
        if !open_session
            .authorization
            .permits(&frame.operation.operation_id, &frame.operation.direction)
        {
            send_protocol_refusal(
                writer,
                open_session,
                session_key,
                clock,
                nonces,
                next_sequence,
                frame.operation,
                ProtocolRefusalCategory::Protocol,
                "operation_direction_not_authorized",
                "The root-authenticated session does not authorize this operation direction.",
            )?;
            return Ok(ServerExit::Refused);
        }

        let operation = match open_session
            .envelope
            .operation(&frame.operation.operation_id)
        {
            Some(operation) => operation,
            None => {
                send_protocol_refusal(
                    writer,
                    open_session,
                    session_key,
                    clock,
                    nonces,
                    next_sequence,
                    frame.operation,
                    ProtocolRefusalCategory::Protocol,
                    "operation_not_in_envelope",
                    "The requested operation is not in the immutable session envelope.",
                )?;
                return Ok(ServerExit::Refused);
            }
        };
        let replay_key = (
            frame.operation.operation_id.clone(),
            matches!(frame.operation.direction, OperationDirection::Rollback),
        );
        if !attempted_bindings.insert(replay_key) {
            send_protocol_refusal(
                writer,
                open_session,
                session_key,
                clock,
                nonces,
                next_sequence,
                frame.operation,
                ProtocolRefusalCategory::Replay,
                "operation_replay",
                "The operation direction was already attempted in this child process.",
            )?;
            return Ok(ServerExit::Refused);
        }
        if frame.operation.direction == OperationDirection::Forward
            && operation
                .dependencies
                .iter()
                .any(|dependency| !applied_forward.contains(dependency))
        {
            send_protocol_refusal(
                writer,
                open_session,
                session_key,
                clock,
                nonces,
                next_sequence,
                frame.operation,
                ProtocolRefusalCategory::Protocol,
                "dependency_not_applied",
                "An approved dependency has not completed in this child session.",
            )?;
            return Ok(ServerExit::Refused);
        }
        if frame.operation.direction == OperationDirection::Rollback
            && open_session.envelope.operations.iter().any(|candidate| {
                remaining_rollback.contains(&candidate.operation_id)
                    && candidate.dependencies.contains(&operation.operation_id)
            })
        {
            send_protocol_refusal(
                writer,
                open_session,
                session_key,
                clock,
                nonces,
                next_sequence,
                frame.operation,
                ProtocolRefusalCategory::Protocol,
                "rollback_dependency_still_applied",
                "A dependent operation must be rolled back before this operation.",
            )?;
            return Ok(ServerExit::Refused);
        }

        let operation_binding = frame.operation;
        let handler_outcome = handler.handle(
            &open_session.envelope,
            operation,
            &operation_binding.direction,
        );
        let recovery_required = matches!(handler_outcome, HandlerOutcome::RecoveryRequired { .. });
        let succeeded = matches!(handler_outcome, HandlerOutcome::Success { .. });
        let outcome = match handler_outcome {
            HandlerOutcome::Success {
                observed_state_digest,
                audit,
            } => ExecutorOutcome::Success {
                applied_at_unix_ms: now,
                observed_state_digest,
                audit,
            },
            HandlerOutcome::ProvenNotApplied {
                code,
                detail,
                audit,
            } => ExecutorOutcome::ProvenNotApplied {
                code,
                detail,
                audit,
            },
            HandlerOutcome::RecoveryRequired {
                code,
                detail,
                audit,
            } => ExecutorOutcome::RecoveryRequired {
                code,
                detail,
                audit,
            },
        };
        send_response(
            writer,
            open_session,
            session_key,
            clock,
            nonces,
            next_sequence,
            operation_binding.clone(),
            outcome,
        )?;
        if succeeded {
            match operation_binding.direction {
                OperationDirection::Forward => {
                    applied_forward.insert(operation_binding.operation_id);
                }
                OperationDirection::Rollback => {
                    remaining_rollback.remove(&operation_binding.operation_id);
                }
            }
        }
        next_sequence = next_sequence
            .checked_add(1)
            .ok_or(AuthenticationError::SequenceMismatch)?;
        if recovery_required {
            return Ok(ServerExit::RecoveryRequired);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn send_protocol_refusal<W, C, N>(
    writer: &mut W,
    open_session: &OpenSession,
    session_key: &ipc_contracts::executor_v2::SessionKey,
    clock: &C,
    nonces: &mut N,
    sequence: u64,
    operation: ipc_contracts::executor_v2::OperationBinding,
    category: ProtocolRefusalCategory,
    code: &str,
    detail: &str,
) -> Result<(), ServerError>
where
    W: Write,
    C: Clock,
    N: NonceSource,
{
    send_response(
        writer,
        open_session,
        session_key,
        clock,
        nonces,
        sequence,
        operation,
        ExecutorOutcome::ProtocolRefusal {
            refusal: ProtocolRefusal {
                category,
                code: code.to_owned(),
                detail: detail.to_owned(),
            },
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn send_response<W, C, N>(
    writer: &mut W,
    open_session: &OpenSession,
    session_key: &ipc_contracts::executor_v2::SessionKey,
    clock: &C,
    nonces: &mut N,
    sequence: u64,
    operation: ipc_contracts::executor_v2::OperationBinding,
    outcome: ExecutorOutcome,
) -> Result<(), ServerError>
where
    W: Write,
    C: Clock,
    N: NonceSource,
{
    let now = clock.now_unix_ms()?;
    let expires_at = now
        .saturating_add(MAX_MESSAGE_LIFETIME_MS)
        .min(open_session.expires_at_unix_ms);
    let response = AuthenticatedOperationResponse::signed(
        open_session,
        sequence,
        nonces.next_nonce()?,
        now,
        expires_at,
        operation,
        outcome,
        session_key,
    )?;
    write_frame(writer, &ExecutorFrame::OperationResponse(response))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn qualification_crash(phase: &str) {
    if std::env::var(QUALIFICATION_CRASH_ENV).ok().as_deref() == Some(phase) {
        std::process::exit(76);
    }
}

#[allow(clippy::too_many_arguments)]
fn refuse_handshake<W, C, N>(
    writer: &mut W,
    hello: &Hello,
    root_authority_key: &[u8; 32],
    clock: &C,
    nonces: &mut N,
    category: ProtocolRefusalCategory,
    code: &str,
    detail: &str,
) -> Result<ServerExit, ServerError>
where
    W: Write,
    C: Clock,
    N: NonceSource,
{
    let refusal = HandshakeRefusal::signed(
        hello,
        nonces.next_nonce()?,
        clock.now_unix_ms()?,
        ProtocolRefusal {
            category,
            code: code.to_owned(),
            detail: detail.to_owned(),
        },
        root_authority_key,
    )?;
    write_frame(writer, &ExecutorFrame::HandshakeRefusal(refusal))?;
    Ok(ServerExit::Refused)
}

fn load_root_authority_key() -> Result<Zeroizing<[u8; 32]>, ServerError> {
    if let Some(path) = std::env::var_os(ROOT_AUTHORITY_FILE_ENV) {
        let stored = privacy::load_shared_executor_root_from(Path::new(&path))
            .map_err(|_| ServerError::RootAuthorityUnavailable)?
            .ok_or(ServerError::RootAuthorityUnavailable)?;
        return root_authority_from_bytes(&stored);
    }
    #[cfg(target_os = "macos")]
    {
        match privacy::load_shared_executor_root(ROOT_AUTHORITY_SECRET_SERVICE) {
            Ok(Some(stored)) => return root_authority_from_bytes(&stored),
            Ok(None) => {}
            Err(_) => return Err(ServerError::RootAuthorityUnavailable),
        }
    }
    let store = OsSecretStore::new(ROOT_AUTHORITY_SECRET_SERVICE);
    let stored = Zeroizing::new(
        store
            .load_sync(ROOT_AUTHORITY_SECRET_NAME)
            .map_err(|_| ServerError::RootAuthorityUnavailable)?
            .ok_or(ServerError::RootAuthorityUnavailable)?,
    );
    root_authority_from_bytes(&stored)
}

fn root_authority_from_bytes(stored: &[u8]) -> Result<Zeroizing<[u8; 32]>, ServerError> {
    if stored.len() != 32 {
        return Err(ServerError::RootAuthorityUnavailable);
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(stored);
    Ok(key)
}

const fn frame_error_code(error: &FrameError) -> &'static str {
    match error {
        FrameError::Oversized => "frame_oversized",
        FrameError::Truncated => "frame_truncated",
        FrameError::Decode => "frame_invalid",
        FrameError::Encode => "frame_encode_failed",
        FrameError::Io(_) => "pipe_io_failed",
    }
}

#[cfg(unix)]
#[cfg(test)]
mod native_handler_tests {
    use super::*;
    use domain::{
        FileFingerprint, NativeFileIdentity, NativePath, OperationStepId, PathEncoding,
        PlatformKind,
    };
    use ipc_contracts::executor_v2::{
        ApprovedOperationManifest, AttestedConsentManifest, ExpectedFileStateManifest,
        FixedBytes32, FrozenPlanManifest, HexBytes, ImmutableExecutionEnvelope,
        NativeFileIdentityManifest, NativePathEncoding, NativePathManifest,
        OperationPrimitiveManifest, PlatformKindManifest, RootBindingManifest, SCHEMA_VERSION,
        SafetyPolicyBindingManifest, VolumeIdentityManifest,
    };
    use platform::{EnumerationProgress, ReadOnlyEnumeration, RenameOutcome, SafeFileOperations};
    use std::{
        collections::VecDeque,
        os::unix::ffi::OsStrExt,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tempfile::TempDir;

    #[derive(Debug)]
    struct GuardedPlatform {
        root: PathBuf,
        rename_failures: Arc<Mutex<VecDeque<PlatformErrorClass>>>,
        rename_calls: Arc<AtomicUsize>,
    }

    impl GuardedPlatform {
        fn new(root: &Path) -> Self {
            let root = root
                .canonicalize()
                .unwrap_or_else(|error| panic!("sandbox root should canonicalize: {error}"));
            let canonical_temp = std::env::temp_dir()
                .canonicalize()
                .unwrap_or_else(|error| panic!("temp directory should canonicalize: {error}"));
            assert!(
                root.starts_with(canonical_temp),
                "native handler tests must remain under the system temp directory"
            );
            Self {
                root,
                rename_failures: Arc::new(Mutex::new(VecDeque::new())),
                rename_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn scripted(
            root: &Path,
            failures: impl IntoIterator<Item = PlatformErrorClass>,
        ) -> (Self, Arc<AtomicUsize>) {
            let mut platform = Self::new(root);
            platform.rename_failures = Arc::new(Mutex::new(failures.into_iter().collect()));
            let calls = Arc::clone(&platform.rename_calls);
            (platform, calls)
        }

        fn scripted_error(class: PlatformErrorClass) -> PlatformError {
            match class {
                PlatformErrorClass::SharingViolation => PlatformError::SharingViolation,
                PlatformErrorClass::LockViolation => PlatformError::LockViolation,
                PlatformErrorClass::PermissionDenied => PlatformError::PermissionDenied,
                PlatformErrorClass::DiskFull => PlatformError::DiskFull,
                PlatformErrorClass::DestinationCollision => PlatformError::DestinationExists,
                PlatformErrorClass::SourceMissing => PlatformError::SourceMissing,
                PlatformErrorClass::PathPolicyRefusal => PlatformError::PathPolicyRefusal,
                PlatformErrorClass::AmbiguousMutationOutcome => {
                    PlatformError::AmbiguousMutationOutcome
                }
                PlatformErrorClass::Cancelled => PlatformError::Cancelled,
                PlatformErrorClass::VerificationLimit => {
                    PlatformError::VerificationLimitExceeded { limit_bytes: 1 }
                }
                PlatformErrorClass::Precondition => {
                    PlatformError::Precondition("scripted precondition".to_owned())
                }
                PlatformErrorClass::Unsupported => {
                    PlatformError::Unsupported("scripted unsupported".to_owned())
                }
                PlatformErrorClass::Io => PlatformError::Io(std::io::Error::other("scripted I/O")),
                PlatformErrorClass::SecretStore => {
                    PlatformError::SecretStore("scripted secret store".to_owned())
                }
            }
        }

        fn guard(&self, path: &Path) -> Result<(), PlatformError> {
            if path != self.root && !path.starts_with(&self.root) {
                return Err(PlatformError::OutsideRoot);
            }
            Ok(())
        }

        fn volume() -> domain::VolumeIdentity {
            domain::VolumeIdentity {
                platform: PlatformKind::Other,
                stable_identifier: "guarded-temp-volume".to_owned(),
                filesystem_type: Some("test".to_owned()),
                case_sensitive: true,
                removable: false,
                local: true,
            }
        }

        fn identity(path: &Path, digest: [u8; 32]) -> NativeFileIdentity {
            NativeFileIdentity {
                volume: Self::volume(),
                object_key: digest.to_vec(),
                parent_key: blake3::hash(
                    path.parent()
                        .unwrap_or_else(|| panic!("test file should have a parent"))
                        .as_os_str()
                        .as_bytes(),
                )
                .as_bytes()
                .to_vec(),
                leaf_name: NativePath {
                    encoding: PathEncoding::UnixBytes,
                    bytes: path
                        .file_name()
                        .unwrap_or_else(|| panic!("test file should have a name"))
                        .as_bytes()
                        .to_vec(),
                },
                link_count: 1,
                reparse_tag: None,
            }
        }
    }

    impl ReadOnlyPlatform for GuardedPlatform {
        fn inspect_volume(&self, root: &Path) -> Result<domain::VolumeIdentity, PlatformError> {
            self.guard(root)?;
            Ok(Self::volume())
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

        fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
            self.guard(path)?;
            let bytes = std::fs::read(path)?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
                return Err(PlatformError::Unsupported(
                    "test read exceeded its bound".to_owned(),
                ));
            }
            Ok(bytes)
        }

        fn read_prefix(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
            let mut bytes = self.read_bounded(path, u64::MAX)?;
            bytes.truncate(max_bytes);
            Ok(bytes)
        }

        fn fingerprint(
            &self,
            path: &Path,
            include_content_digest: bool,
            max_bytes: u64,
        ) -> Result<FileFingerprint, PlatformError> {
            self.guard(path)?;
            let bytes = self.read_bounded(path, max_bytes)?;
            let digest = *blake3::hash(&bytes).as_bytes();
            Ok(FileFingerprint {
                native_identity: Self::identity(path, digest),
                byte_size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                modified_at_ns: None,
                created_at_ns: None,
                attributes: 0,
                quick_digest: None,
                content_digest: include_content_digest.then_some(digest),
            })
        }
    }

    impl SafeFileOperations for GuardedPlatform {
        fn rename_same_volume_no_replace(
            &self,
            request: &RenameRequest,
        ) -> Result<RenameOutcome, PlatformError> {
            self.rename_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(class) = self
                .rename_failures
                .lock()
                .map_err(|_| PlatformError::Precondition("test script poisoned".to_owned()))?
                .pop_front()
            {
                return Err(Self::scripted_error(class));
            }
            self.guard(&request.source)?;
            self.guard(&request.destination)?;
            if request.destination.exists() {
                return Err(PlatformError::DestinationExists);
            }
            let before = self.fingerprint(&request.source, true, u64::MAX)?;
            if before.native_identity.object_key != request.expected_identity.object_key
                || before.byte_size != request.expected_byte_size
                || before.content_digest != Some(request.expected_content_digest)
            {
                return Err(PlatformError::Precondition(
                    "guarded source no longer matches".to_owned(),
                ));
            }
            std::fs::rename(&request.source, &request.destination)?;
            Ok(RenameOutcome {
                observed_identity: self
                    .fingerprint(&request.destination, true, u64::MAX)?
                    .native_identity,
            })
        }

        fn create_directory_no_replace(&self, path: &Path) -> Result<(), PlatformError> {
            self.guard(path)?;
            std::fs::create_dir(path).map_err(Into::into)
        }

        fn remove_directory_if_empty(&self, path: &Path) -> Result<(), PlatformError> {
            self.guard(path)?;
            std::fs::remove_dir(path).map_err(Into::into)
        }
    }

    #[test]
    fn native_handler_applies_and_rolls_back_only_manifest_primitives() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let platform = GuardedPlatform::new(sandbox.path());
        let envelope = envelope(sandbox.path(), &platform);
        let mut handler = NativeExecutorHandler::new(platform);

        let directory = &envelope.operations[0];
        assert!(matches!(
            handler.handle(&envelope, directory, &OperationDirection::Forward),
            HandlerOutcome::Success { .. }
        ));
        assert!(sandbox.path().join("organized").is_dir());

        let rename = &envelope.operations[1];
        assert!(matches!(
            handler.handle(&envelope, rename, &OperationDirection::Forward),
            HandlerOutcome::Success { .. }
        ));
        assert!(!sandbox.path().join("before.txt").exists());
        assert_eq!(
            std::fs::read(sandbox.path().join("organized/after.txt"))
                .unwrap_or_else(|error| panic!("destination should be readable: {error}")),
            b"safety"
        );

        assert!(matches!(
            handler.handle(&envelope, rename, &OperationDirection::Rollback),
            HandlerOutcome::Success { .. }
        ));
        assert!(sandbox.path().join("before.txt").is_file());
        assert!(matches!(
            handler.handle(&envelope, directory, &OperationDirection::Rollback),
            HandlerOutcome::Success { .. }
        ));
        assert!(!sandbox.path().join("organized").exists());
    }

    #[test]
    fn native_handler_rejects_invalid_relative_paths_before_mutation() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let platform = GuardedPlatform::new(sandbox.path());
        let mut manifest = envelope(sandbox.path(), &platform);
        let OperationPrimitiveManifest::SameVolumeMove {
            destination_relative_path,
            ..
        } = &mut manifest.operations[1].primitive
        else {
            panic!("test operation should be a move");
        };
        *destination_relative_path = "../outside.txt".to_owned();
        let mut handler = NativeExecutorHandler::new(platform);

        assert!(matches!(
            handler.handle(
                &manifest,
                &manifest.operations[1],
                &OperationDirection::Forward
            ),
            HandlerOutcome::ProvenNotApplied { .. }
        ));
        assert_eq!(
            std::fs::read(sandbox.path().join("before.txt"))
                .unwrap_or_else(|error| panic!("source should remain readable: {error}")),
            b"safety"
        );
    }

    #[test]
    fn native_handler_rejects_detached_source_or_destination_tampering() {
        for tamper_source in [true, false] {
            let sandbox =
                TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
            std::fs::write(sandbox.path().join("before.txt"), b"safety")
                .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
            let (platform, rename_calls) = GuardedPlatform::scripted(sandbox.path(), []);
            let envelope = envelope(sandbox.path(), &platform);
            let mut detached = envelope.operations[1].clone();
            let OperationPrimitiveManifest::SameVolumeMove {
                source_relative_path,
                destination_relative_path,
                ..
            } = &mut detached.primitive
            else {
                panic!("fixture should contain a move");
            };
            if tamper_source {
                *source_relative_path = "forged-source.txt".to_owned();
            } else {
                *destination_relative_path = "organized/forged-destination.txt".to_owned();
            }
            let mut handler = NativeExecutorHandler::new(platform);

            let outcome = handler.handle(&envelope, &detached, &OperationDirection::Forward);

            assert_eq!(rename_calls.load(Ordering::SeqCst), 0);
            assert!(matches!(
                outcome,
                HandlerOutcome::ProvenNotApplied { ref code, .. }
                    if code == "operation_manifest_invalid"
            ));
            assert_eq!(
                std::fs::read(sandbox.path().join("before.txt"))
                    .unwrap_or_else(|error| panic!("source should remain readable: {error}")),
                b"safety"
            );
            assert!(
                !sandbox
                    .path()
                    .join("organized/forged-destination.txt")
                    .exists()
            );
        }
    }

    #[test]
    fn native_handler_retries_only_bounded_transient_pre_mutation_errors() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::create_dir(sandbox.path().join("organized"))
            .unwrap_or_else(|error| panic!("destination parent should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let (platform, calls) = GuardedPlatform::scripted(
            sandbox.path(),
            [
                PlatformErrorClass::SharingViolation,
                PlatformErrorClass::LockViolation,
            ],
        );
        let envelope = envelope(sandbox.path(), &platform);
        let rename = &envelope.operations[1];
        let mut handler = NativeExecutorHandler::new(platform);

        let outcome = handler.handle(&envelope, rename, &OperationDirection::Forward);

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(matches!(
            outcome,
            HandlerOutcome::Success {
                audit: ExecutorAttemptAudit {
                    attempt_count: 3,
                    error_class: Some(ExecutorErrorClass::LockViolation),
                },
                ..
            }
        ));
        assert!(sandbox.path().join("organized/after.txt").is_file());
    }

    #[test]
    fn permanent_and_ambiguous_errors_are_never_retried() {
        for (class, recovery_required) in [
            (PlatformErrorClass::PermissionDenied, false),
            (PlatformErrorClass::DiskFull, false),
            (PlatformErrorClass::DestinationCollision, false),
            (PlatformErrorClass::AmbiguousMutationOutcome, true),
        ] {
            let sandbox =
                TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
            std::fs::create_dir(sandbox.path().join("organized"))
                .unwrap_or_else(|error| panic!("destination parent should be created: {error}"));
            std::fs::write(sandbox.path().join("before.txt"), b"safety")
                .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
            let (platform, calls) = GuardedPlatform::scripted(sandbox.path(), [class]);
            let envelope = envelope(sandbox.path(), &platform);
            let rename = &envelope.operations[1];
            let mut handler = NativeExecutorHandler::new(platform);

            let outcome = handler.handle(&envelope, rename, &OperationDirection::Forward);

            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                matches!(outcome, HandlerOutcome::RecoveryRequired { .. }),
                recovery_required
            );
            assert!(sandbox.path().join("before.txt").is_file());
        }
    }

    #[test]
    fn retry_audit_records_the_terminal_error_class() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::create_dir(sandbox.path().join("organized"))
            .unwrap_or_else(|error| panic!("destination parent should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let (platform, calls) = GuardedPlatform::scripted(
            sandbox.path(),
            [
                PlatformErrorClass::SharingViolation,
                PlatformErrorClass::PermissionDenied,
            ],
        );
        let envelope = envelope(sandbox.path(), &platform);
        let rename = &envelope.operations[1];
        let mut handler = NativeExecutorHandler::new(platform);

        let outcome = handler.handle(&envelope, rename, &OperationDirection::Forward);

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(
            outcome,
            HandlerOutcome::ProvenNotApplied {
                audit: ExecutorAttemptAudit {
                    attempt_count: 2,
                    error_class: Some(ExecutorErrorClass::PermissionDenied),
                },
                ..
            }
        ));
        assert!(sandbox.path().join("before.txt").is_file());
    }

    #[test]
    fn exhausted_sharing_retries_return_stable_in_use_outcome() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let (platform, calls) = GuardedPlatform::scripted(
            sandbox.path(),
            [
                PlatformErrorClass::SharingViolation,
                PlatformErrorClass::SharingViolation,
                PlatformErrorClass::SharingViolation,
            ],
        );
        let envelope = envelope(sandbox.path(), &platform);
        let rename = &envelope.operations[1];
        let mut handler = NativeExecutorHandler::new(platform);

        let outcome = handler.handle(&envelope, rename, &OperationDirection::Forward);

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(matches!(
            outcome,
            HandlerOutcome::ProvenNotApplied {
                ref code,
                ref detail,
                audit: ExecutorAttemptAudit {
                    attempt_count: 3,
                    error_class: Some(ExecutorErrorClass::SharingViolation),
                },
            } if code == "file_in_use"
                && detail == "This file is currently in use and was not moved."
        ));
        assert!(sandbox.path().join("before.txt").is_file());
    }

    #[test]
    fn cancellation_is_observed_before_any_mutation_call() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let (platform, calls) = GuardedPlatform::scripted(sandbox.path(), []);
        let envelope = envelope(sandbox.path(), &platform);
        let rename = &envelope.operations[1];
        let mut handler = NativeExecutorHandler::with_cancellation(platform, Arc::new(|| true));

        let outcome = handler.handle(&envelope, rename, &OperationDirection::Forward);

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            outcome,
            HandlerOutcome::ProvenNotApplied {
                ref code,
                audit: ExecutorAttemptAudit {
                    attempt_count: 1,
                    error_class: Some(ExecutorErrorClass::Cancelled),
                },
                ..
            } if code == "verification_cancelled"
        ));
        assert!(sandbox.path().join("before.txt").is_file());
    }

    #[test]
    fn direct_case_only_rename_is_refused_when_qualification_gate_is_off() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let (platform, calls) = GuardedPlatform::scripted(sandbox.path(), []);
        let mut envelope = envelope(sandbox.path(), &platform);
        let OperationPrimitiveManifest::SameVolumeMove {
            destination_relative_path,
            ..
        } = &mut envelope.operations[1].primitive
        else {
            panic!("fixture should contain a move");
        };
        *destination_relative_path = "BEFORE.TXT".to_owned();
        envelope
            .validate()
            .unwrap_or_else(|error| panic!("case-only envelope should remain valid: {error}"));
        let mut handler = NativeExecutorHandler::new(platform);

        let outcome = handler.handle(
            &envelope,
            &envelope.operations[1],
            &OperationDirection::Forward,
        );

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            outcome,
            HandlerOutcome::ProvenNotApplied { ref code, .. }
                if code == "case_only_rename_unqualified"
        ));
        assert!(sandbox.path().join("before.txt").is_file());
    }

    #[test]
    fn qualified_case_only_chain_requires_exact_uuid_stage_binding() {
        let sandbox =
            TempDir::new().unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"safety")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let platform = GuardedPlatform::new(sandbox.path());
        let mut envelope = envelope(sandbox.path(), &platform);
        envelope
            .safety_policy_binding
            .allow_qualified_case_only_rename = true;
        let final_operation = envelope.operations[1].clone();
        let OperationPrimitiveManifest::SameVolumeMove {
            original_source_relative_path,
            expected_source,
            ..
        } = final_operation.primitive
        else {
            panic!("fixture should contain a move");
        };
        let stage_id = OperationStepId::new().to_string();
        let staging_path = format!(
            ".supremacy-staging/{}/01989f5e-7b1a-7000-8000-000000000001",
            envelope.execution_id
        );
        let stage = ApprovedOperationManifest {
            operation_id: stage_id.clone(),
            proposal_operation_id: None,
            sequence: 0,
            dependencies: Vec::new(),
            primitive: OperationPrimitiveManifest::InternalStage {
                source_relative_path: original_source_relative_path.clone(),
                destination_relative_path: staging_path.clone(),
                original_source_relative_path: original_source_relative_path.clone(),
                expected_source: expected_source.clone(),
            },
        };
        let final_operation = ApprovedOperationManifest {
            operation_id: OperationStepId::new().to_string(),
            proposal_operation_id: envelope.operations[1].proposal_operation_id.clone(),
            sequence: 1,
            dependencies: vec![stage_id],
            primitive: OperationPrimitiveManifest::SameVolumeRename {
                source_relative_path: staging_path,
                destination_relative_path: "BEFORE.TXT".to_owned(),
                original_source_relative_path,
                expected_source,
            },
        };
        envelope.operations = vec![stage, final_operation];
        envelope
            .validate()
            .unwrap_or_else(|error| panic!("qualified envelope should validate: {error}"));
        assert!(qualified_case_only_stage_chain_valid(
            &envelope,
            &envelope.operations[1]
        ));

        let mut malformed = envelope.clone();
        let OperationPrimitiveManifest::InternalStage {
            destination_relative_path,
            ..
        } = &mut malformed.operations[0].primitive
        else {
            panic!("fixture should contain a stage");
        };
        *destination_relative_path =
            format!(".supremacy-staging/{}/not-a-uuid", malformed.execution_id);
        assert!(!qualified_case_only_stage_chain_valid(
            &malformed,
            &malformed.operations[1]
        ));

        let mut relocated = envelope.clone();
        let OperationPrimitiveManifest::SameVolumeRename {
            expected_source, ..
        } = &mut relocated.operations[1].primitive
        else {
            panic!("fixture should contain a rename");
        };
        expected_source.native_identity.parent_key = HexBytes::new(vec![0xAA; 8])
            .unwrap_or_else(|error| panic!("parent key should encode: {error}"));
        expected_source.native_identity.leaf_name.bytes = HexBytes::new(b"BEFORE.TXT".to_vec())
            .unwrap_or_else(|error| panic!("leaf should encode: {error}"));
        assert!(
            qualified_case_only_stage_chain_valid(&relocated, &relocated.operations[1]),
            "rollback fingerprints keep the same journaled file across parent/leaf changes"
        );

        let mut colliding = relocated.clone();
        let OperationPrimitiveManifest::SameVolumeRename {
            expected_source, ..
        } = &mut colliding.operations[1].primitive
        else {
            panic!("fixture should contain a rename");
        };
        expected_source.native_identity.object_key = HexBytes::new(vec![0xFF; 8])
            .unwrap_or_else(|error| panic!("object key should encode: {error}"));
        assert!(
            !qualified_case_only_stage_chain_valid(&colliding, &colliding.operations[1]),
            "a different native object must not satisfy the case-only stage chain"
        );
        assert!(!same_journaled_file_identity(
            match &envelope.operations[0].primitive {
                OperationPrimitiveManifest::InternalStage {
                    expected_source, ..
                } => expected_source,
                _ => panic!("fixture should contain a stage"),
            },
            match &colliding.operations[1].primitive {
                OperationPrimitiveManifest::SameVolumeRename {
                    expected_source, ..
                } => {
                    expected_source
                }
                _ => panic!("fixture should contain a rename"),
            }
        ));
    }

    fn envelope(root: &Path, platform: &GuardedPlatform) -> ImmutableExecutionEnvelope {
        let root = root
            .canonicalize()
            .unwrap_or_else(|error| panic!("sandbox should canonicalize: {error}"));
        let fingerprint = platform
            .fingerprint(&root.join("before.txt"), true, u64::MAX)
            .unwrap_or_else(|error| panic!("fixture should fingerprint: {error}"));
        let volume = volume_manifest();
        let proposal_operation_id = "00000000-0000-4000-8000-000000000004".to_owned();
        let expected = ExpectedFileStateManifest {
            native_identity: NativeFileIdentityManifest {
                volume: volume.clone(),
                object_key: HexBytes::new(fingerprint.native_identity.object_key)
                    .unwrap_or_else(|error| panic!("identity should encode: {error}")),
                parent_key: HexBytes::new(fingerprint.native_identity.parent_key)
                    .unwrap_or_else(|error| panic!("parent should encode: {error}")),
                leaf_name: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(fingerprint.native_identity.leaf_name.bytes)
                        .unwrap_or_else(|error| panic!("leaf should encode: {error}")),
                },
                link_count: 1,
                reparse_tag: None,
            },
            byte_size: fingerprint.byte_size,
            modified_at_ns: None,
            attributes: fingerprint.attributes,
            content_digest: FixedBytes32::from_bytes(
                fingerprint
                    .content_digest
                    .unwrap_or_else(|| panic!("fixture digest should be present")),
            ),
        };
        let envelope = ImmutableExecutionEnvelope {
            schema_version: SCHEMA_VERSION,
            execution_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            root_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            plan: FrozenPlanManifest {
                material_version: 2,
                plan_id: "00000000-0000-4000-8000-000000000003".to_owned(),
                proposal_id: "00000000-0000-4000-8000-000000000005".to_owned(),
                proposal_revision_id: "00000000-0000-4000-8000-000000000006".to_owned(),
                proposal_revision: 1,
                source_snapshot_version: "00000000-0000-4000-8000-000000000007".to_owned(),
                approved_operation_ids: vec![proposal_operation_id.clone()],
                operation_count: 1,
                approval_timestamp: "2026-08-11T00:00:00Z".to_owned(),
                user_confirmed: true,
                digest: FixedBytes32::from_bytes([9; 32]),
            },
            root_binding: RootBindingManifest {
                canonical_path: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(root.as_os_str().as_bytes().to_vec())
                        .unwrap_or_else(|error| panic!("root should encode: {error}")),
                },
                display_path: root.display().to_string(),
                volume: volume.clone(),
            },
            safety_policy_binding: SafetyPolicyBindingManifest {
                version: domain::EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
                digest: FixedBytes32::from_bytes([8; 32]),
                maximum_rehash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
                allow_qualified_case_only_rename: false,
            },
            consent: AttestedConsentManifest {
                issued_at_unix_ms: 1,
                expires_at_unix_ms: 10_000,
                attested_at_unix_ms: 2,
                consent_nonce: FixedBytes32::from_bytes([7; 32]),
                attestation_mac: FixedBytes32::from_bytes([6; 32]),
            },
            operations: vec![
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000008".to_owned(),
                    proposal_operation_id: None,
                    sequence: 0,
                    dependencies: Vec::new(),
                    primitive: OperationPrimitiveManifest::CreateDirectory {
                        destination_relative_path: "organized".to_owned(),
                    },
                },
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000009".to_owned(),
                    proposal_operation_id: Some(proposal_operation_id),
                    sequence: 1,
                    dependencies: vec!["00000000-0000-4000-8000-000000000008".to_owned()],
                    primitive: OperationPrimitiveManifest::SameVolumeMove {
                        source_relative_path: "before.txt".to_owned(),
                        destination_relative_path: "organized/after.txt".to_owned(),
                        original_source_relative_path: "before.txt".to_owned(),
                        expected_source: expected,
                    },
                },
            ],
        };
        envelope
            .validate()
            .unwrap_or_else(|error| panic!("test envelope should validate: {error}"));
        envelope
    }

    fn volume_manifest() -> VolumeIdentityManifest {
        VolumeIdentityManifest {
            platform: PlatformKindManifest::Other,
            stable_identifier: "guarded-temp-volume".to_owned(),
            filesystem_type: Some("test".to_owned()),
            case_sensitive: true,
            removable: false,
            local: true,
        }
    }

    #[cfg(target_os = "macos")]
    fn macos_volume_manifest(root: &Path) -> VolumeIdentityManifest {
        let volume = platform_macos::MacOsPlatform
            .inspect_volume(root)
            .unwrap_or_else(|error| panic!("macos volume should inspect: {error}"));
        VolumeIdentityManifest {
            platform: PlatformKindManifest::MacOs,
            stable_identifier: volume.stable_identifier,
            filesystem_type: volume.filesystem_type,
            case_sensitive: volume.case_sensitive,
            removable: volume.removable,
            local: volume.local,
        }
    }

    #[cfg(target_os = "macos")]
    fn macos_native_envelope(root: &Path) -> ImmutableExecutionEnvelope {
        let root = root
            .canonicalize()
            .unwrap_or_else(|error| panic!("sandbox should canonicalize: {error}"));
        let fingerprint = platform_macos::MacOsPlatform
            .fingerprint(&root.join("before.txt"), true, u64::MAX)
            .unwrap_or_else(|error| panic!("fixture should fingerprint: {error}"));
        let volume = macos_volume_manifest(&root);
        let proposal_operation_id = "00000000-0000-4000-8000-000000000004".to_owned();
        let expected = ExpectedFileStateManifest {
            native_identity: NativeFileIdentityManifest {
                volume: volume.clone(),
                object_key: HexBytes::new(fingerprint.native_identity.object_key)
                    .unwrap_or_else(|error| panic!("identity should encode: {error}")),
                parent_key: HexBytes::new(fingerprint.native_identity.parent_key)
                    .unwrap_or_else(|error| panic!("parent should encode: {error}")),
                leaf_name: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(fingerprint.native_identity.leaf_name.bytes)
                        .unwrap_or_else(|error| panic!("leaf should encode: {error}")),
                },
                link_count: 1,
                reparse_tag: None,
            },
            byte_size: fingerprint.byte_size,
            modified_at_ns: fingerprint
                .modified_at_ns
                .and_then(|value| i64::try_from(value).ok()),
            attributes: fingerprint.attributes,
            content_digest: FixedBytes32::from_bytes(
                fingerprint
                    .content_digest
                    .unwrap_or_else(|| panic!("fixture digest should be present")),
            ),
        };
        let envelope = ImmutableExecutionEnvelope {
            schema_version: SCHEMA_VERSION,
            execution_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            root_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            plan: FrozenPlanManifest {
                material_version: 2,
                plan_id: "00000000-0000-4000-8000-000000000003".to_owned(),
                proposal_id: "00000000-0000-4000-8000-000000000005".to_owned(),
                proposal_revision_id: "00000000-0000-4000-8000-000000000006".to_owned(),
                proposal_revision: 1,
                source_snapshot_version: "00000000-0000-4000-8000-000000000007".to_owned(),
                approved_operation_ids: vec![proposal_operation_id.clone()],
                operation_count: 1,
                approval_timestamp: "2026-08-11T00:00:00Z".to_owned(),
                user_confirmed: true,
                digest: FixedBytes32::from_bytes([9; 32]),
            },
            root_binding: RootBindingManifest {
                canonical_path: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(root.as_os_str().as_bytes().to_vec())
                        .unwrap_or_else(|error| panic!("root should encode: {error}")),
                },
                display_path: root.display().to_string(),
                volume: volume.clone(),
            },
            safety_policy_binding: SafetyPolicyBindingManifest {
                version: domain::EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
                digest: FixedBytes32::from_bytes([8; 32]),
                maximum_rehash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
                allow_qualified_case_only_rename: false,
            },
            consent: AttestedConsentManifest {
                issued_at_unix_ms: 1,
                expires_at_unix_ms: 10_000,
                attested_at_unix_ms: 2,
                consent_nonce: FixedBytes32::from_bytes([7; 32]),
                attestation_mac: FixedBytes32::from_bytes([6; 32]),
            },
            operations: vec![
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000008".to_owned(),
                    proposal_operation_id: None,
                    sequence: 0,
                    dependencies: Vec::new(),
                    primitive: OperationPrimitiveManifest::CreateDirectory {
                        destination_relative_path: "organized".to_owned(),
                    },
                },
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000009".to_owned(),
                    proposal_operation_id: Some(proposal_operation_id),
                    sequence: 1,
                    dependencies: vec!["00000000-0000-4000-8000-000000000008".to_owned()],
                    primitive: OperationPrimitiveManifest::SameVolumeMove {
                        source_relative_path: "before.txt".to_owned(),
                        destination_relative_path: "organized/after.txt".to_owned(),
                        original_source_relative_path: "before.txt".to_owned(),
                        expected_source: expected,
                    },
                },
            ],
        };
        envelope
            .validate()
            .unwrap_or_else(|error| panic!("test envelope should validate: {error}"));
        envelope
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_native_handler_applies_and_rolls_back_only_manifest_primitives() {
        let sandbox = tempfile::Builder::new()
            .prefix("supremacy-m18-executor-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"macos-native")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let envelope = macos_native_envelope(sandbox.path());
        let mut handler = NativeExecutorHandler::new(platform_macos::MacOsPlatform);

        let directory = &envelope.operations[0];
        assert!(matches!(
            handler.handle(&envelope, directory, &OperationDirection::Forward),
            HandlerOutcome::Success { .. }
        ));
        assert!(sandbox.path().join("organized").is_dir());

        let rename = &envelope.operations[1];
        assert!(matches!(
            handler.handle(&envelope, rename, &OperationDirection::Forward),
            HandlerOutcome::Success { .. }
        ));
        assert!(!sandbox.path().join("before.txt").exists());
        assert_eq!(
            std::fs::read(sandbox.path().join("organized/after.txt"))
                .unwrap_or_else(|error| panic!("destination should be readable: {error}")),
            b"macos-native"
        );

        assert!(matches!(
            handler.handle(&envelope, rename, &OperationDirection::Rollback),
            HandlerOutcome::Success { .. }
        ));
        assert!(sandbox.path().join("before.txt").is_file());
        assert!(matches!(
            handler.handle(&envelope, directory, &OperationDirection::Rollback),
            HandlerOutcome::Success { .. }
        ));
        assert!(!sandbox.path().join("organized").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_native_handler_refuses_overwrite() {
        let sandbox = tempfile::Builder::new()
            .prefix("supremacy-m18-executor-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("before.txt"), b"macos-native")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        std::fs::create_dir(sandbox.path().join("organized"))
            .unwrap_or_else(|error| panic!("destination parent: {error}"));
        std::fs::write(sandbox.path().join("organized/after.txt"), b"keep")
            .unwrap_or_else(|error| panic!("occupant should be written: {error}"));
        let envelope = macos_native_envelope(sandbox.path());
        let mut handler = NativeExecutorHandler::new(platform_macos::MacOsPlatform);
        let outcome = handler.handle(
            &envelope,
            &envelope.operations[1],
            &OperationDirection::Forward,
        );
        assert!(matches!(
            outcome,
            HandlerOutcome::ProvenNotApplied { ref code, .. }
                if code == "destination_exists"
        ));
        assert_eq!(
            std::fs::read(sandbox.path().join("before.txt"))
                .unwrap_or_else(|error| panic!("source should remain: {error}")),
            b"macos-native"
        );
        assert_eq!(
            std::fs::read(sandbox.path().join("organized/after.txt"))
                .unwrap_or_else(|error| panic!("occupant should remain: {error}")),
            b"keep"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_native_handler_qualified_case_only_rename_and_rollback() {
        let sandbox = tempfile::Builder::new()
            .prefix("supremacy-m18-executor-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        std::fs::write(sandbox.path().join("Invoice.pdf"), b"case-only")
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        let root = sandbox
            .path()
            .canonicalize()
            .unwrap_or_else(|error| panic!("sandbox should canonicalize: {error}"));
        let fingerprint = platform_macos::MacOsPlatform
            .fingerprint(&root.join("Invoice.pdf"), true, u64::MAX)
            .unwrap_or_else(|error| panic!("fixture should fingerprint: {error}"));
        let volume = macos_volume_manifest(&root);
        let proposal_operation_id = "00000000-0000-4000-8000-000000000004".to_owned();
        let execution_id = "00000000-0000-4000-8000-000000000001".to_owned();
        let stage_id = "01989f5e-7b1a-7000-8000-0000000000aa".to_owned();
        let staging_leaf = "01989f5e-7b1a-7000-8000-0000000000bb";
        let staging_path = format!(".supremacy-staging/{execution_id}/{staging_leaf}");
        let expected = ExpectedFileStateManifest {
            native_identity: NativeFileIdentityManifest {
                volume: volume.clone(),
                object_key: HexBytes::new(fingerprint.native_identity.object_key)
                    .unwrap_or_else(|error| panic!("identity should encode: {error}")),
                parent_key: HexBytes::new(fingerprint.native_identity.parent_key)
                    .unwrap_or_else(|error| panic!("parent should encode: {error}")),
                leaf_name: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(fingerprint.native_identity.leaf_name.bytes)
                        .unwrap_or_else(|error| panic!("leaf should encode: {error}")),
                },
                link_count: 1,
                reparse_tag: None,
            },
            byte_size: fingerprint.byte_size,
            modified_at_ns: fingerprint
                .modified_at_ns
                .and_then(|value| i64::try_from(value).ok()),
            attributes: fingerprint.attributes,
            content_digest: FixedBytes32::from_bytes(
                fingerprint
                    .content_digest
                    .unwrap_or_else(|| panic!("fixture digest should be present")),
            ),
        };
        let envelope = ImmutableExecutionEnvelope {
            schema_version: SCHEMA_VERSION,
            execution_id: execution_id.clone(),
            root_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            plan: FrozenPlanManifest {
                material_version: 2,
                plan_id: "00000000-0000-4000-8000-000000000003".to_owned(),
                proposal_id: "00000000-0000-4000-8000-000000000005".to_owned(),
                proposal_revision_id: "00000000-0000-4000-8000-000000000006".to_owned(),
                proposal_revision: 1,
                source_snapshot_version: "00000000-0000-4000-8000-000000000007".to_owned(),
                approved_operation_ids: vec![proposal_operation_id.clone()],
                operation_count: 1,
                approval_timestamp: "2026-08-11T00:00:00Z".to_owned(),
                user_confirmed: true,
                digest: FixedBytes32::from_bytes([9; 32]),
            },
            root_binding: RootBindingManifest {
                canonical_path: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(root.as_os_str().as_bytes().to_vec())
                        .unwrap_or_else(|error| panic!("root should encode: {error}")),
                },
                display_path: root.display().to_string(),
                volume: volume.clone(),
            },
            safety_policy_binding: SafetyPolicyBindingManifest {
                version: domain::EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
                digest: FixedBytes32::from_bytes([8; 32]),
                maximum_rehash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
                allow_qualified_case_only_rename: true,
            },
            consent: AttestedConsentManifest {
                issued_at_unix_ms: 1,
                expires_at_unix_ms: 10_000,
                attested_at_unix_ms: 2,
                consent_nonce: FixedBytes32::from_bytes([7; 32]),
                attestation_mac: FixedBytes32::from_bytes([6; 32]),
            },
            operations: vec![
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000008".to_owned(),
                    proposal_operation_id: None,
                    sequence: 0,
                    dependencies: Vec::new(),
                    primitive: OperationPrimitiveManifest::CreateDirectory {
                        destination_relative_path: ".supremacy-staging".to_owned(),
                    },
                },
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000018".to_owned(),
                    proposal_operation_id: None,
                    sequence: 1,
                    dependencies: vec!["00000000-0000-4000-8000-000000000008".to_owned()],
                    primitive: OperationPrimitiveManifest::CreateDirectory {
                        destination_relative_path: format!(".supremacy-staging/{execution_id}"),
                    },
                },
                ApprovedOperationManifest {
                    operation_id: stage_id.clone(),
                    proposal_operation_id: None,
                    sequence: 2,
                    dependencies: vec!["00000000-0000-4000-8000-000000000018".to_owned()],
                    primitive: OperationPrimitiveManifest::InternalStage {
                        source_relative_path: "Invoice.pdf".to_owned(),
                        destination_relative_path: staging_path.clone(),
                        original_source_relative_path: "Invoice.pdf".to_owned(),
                        expected_source: expected.clone(),
                    },
                },
                ApprovedOperationManifest {
                    operation_id: "00000000-0000-4000-8000-000000000009".to_owned(),
                    proposal_operation_id: Some(proposal_operation_id),
                    sequence: 3,
                    dependencies: vec![stage_id],
                    primitive: OperationPrimitiveManifest::SameVolumeRename {
                        source_relative_path: staging_path,
                        destination_relative_path: "invoice.pdf".to_owned(),
                        original_source_relative_path: "Invoice.pdf".to_owned(),
                        expected_source: expected,
                    },
                },
            ],
        };
        envelope
            .validate()
            .unwrap_or_else(|error| panic!("case-only envelope should validate: {error}"));
        let mut handler = NativeExecutorHandler::new(platform_macos::MacOsPlatform);
        for operation in &envelope.operations {
            assert!(
                matches!(
                    handler.handle(&envelope, operation, &OperationDirection::Forward),
                    HandlerOutcome::Success { .. }
                ),
                "forward {} should succeed",
                operation.operation_id
            );
        }
        let leaf = std::fs::read_dir(&root)
            .unwrap_or_else(|error| panic!("root should be readable: {error}"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|error| panic!("dirent: {error}"))
                    .file_name()
            })
            .find(|name| name.as_bytes() == b"invoice.pdf")
            .unwrap_or_else(|| panic!("case-preserving invoice.pdf should exist"));
        assert_eq!(leaf.as_bytes(), b"invoice.pdf");
        assert_eq!(
            std::fs::read(root.join("invoice.pdf"))
                .unwrap_or_else(|error| panic!("renamed file should be readable: {error}")),
            b"case-only"
        );

        for operation in envelope.operations.iter().rev() {
            assert!(
                matches!(
                    handler.handle(&envelope, operation, &OperationDirection::Rollback),
                    HandlerOutcome::Success { .. }
                ),
                "rollback {} should succeed",
                operation.operation_id
            );
        }
        assert_eq!(
            std::fs::read(root.join("Invoice.pdf"))
                .unwrap_or_else(|error| panic!("original case should be restored: {error}")),
            b"case-only"
        );
    }
}
