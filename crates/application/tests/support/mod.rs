use application::{
    ApprovedExecutorClient, ApprovedExecutorError, ApprovedExecutorSession, ExecutorDispatchResult,
    executor_response_digest, fresh_request_nonce, prepare_executor_request_identity,
    synthetic_executor_session_identity,
};
use domain::{
    ExecutorRequestDirection, ExecutorRequestIdentity, ExecutorSessionIdentity, FileFingerprint,
    MAX_EXECUTION_VERIFICATION_BYTES, NativeFileIdentity, OperationStepId,
};
use ipc_contracts::executor_v2::{
    CommittedJournalEventBinding, ExecutorAttemptAudit, ExecutorOutcome, ExpectedFileStateManifest,
    FixedBytes32, ImmutableExecutionEnvelope, NativePathEncoding, OperationDirection,
    OperationPrimitiveManifest, SessionAuthorization,
};
use platform::{PlatformError, ReadOnlyPlatform, RenameOutcome, RenameRequest, SafeFileOperations};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use tempfile::{Builder, TempDir};

#[derive(Debug)]
pub struct MutationSandbox {
    directory: TempDir,
}

impl MutationSandbox {
    pub fn new() -> Self {
        Self {
            directory: Builder::new()
                .prefix("supremacy-m8-sandbox-")
                .tempdir()
                .unwrap_or_else(|error| panic!("sandbox should be created: {error}")),
        }
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path().join(relative);
        assert_is_test_sandbox(self.path(), &path);
        fs::create_dir_all(
            path.parent()
                .unwrap_or_else(|| panic!("sandbox fixture should have a parent")),
        )
        .unwrap_or_else(|error| panic!("fixture parent should be created: {error}"));
        fs::write(path, bytes).unwrap_or_else(|error| panic!("fixture should be written: {error}"));
    }

    pub fn snapshot(&self) -> BTreeMap<PathBuf, (u64, blake3::Hash)> {
        snapshot_tree(self.path())
    }
}

pub struct SandboxFileOperations {
    root: PathBuf,
    reader: std::sync::Arc<dyn ReadOnlyPlatform>,
}

impl SandboxFileOperations {
    pub fn new(root: &Path, reader: std::sync::Arc<dyn ReadOnlyPlatform>) -> Self {
        assert_is_test_sandbox(root, root);
        Self {
            root: root.to_path_buf(),
            reader,
        }
    }

    fn assert_path(&self, path: &Path) {
        assert_is_test_sandbox(&self.root, path);
    }
}

impl SafeFileOperations for SandboxFileOperations {
    fn rename_same_volume_no_replace(
        &self,
        request: &RenameRequest,
    ) -> Result<RenameOutcome, PlatformError> {
        self.assert_path(&request.source);
        self.assert_path(&request.destination);
        if fs::symlink_metadata(&request.destination).is_ok() {
            return Err(PlatformError::DestinationExists);
        }
        let observed =
            self.reader
                .fingerprint(&request.source, true, request.maximum_hash_bytes)?;
        if !same_identity(&observed.native_identity, &request.expected_identity)
            || observed.byte_size != request.expected_byte_size
            || observed.modified_at_ns != request.expected_modified_at_ns
            || observed.attributes != request.expected_attributes
            || observed.content_digest != Some(request.expected_content_digest)
        {
            return Err(PlatformError::Precondition(
                "sandbox source precondition changed".to_owned(),
            ));
        }
        fs::rename(&request.source, &request.destination)?;
        let moved =
            self.reader
                .fingerprint(&request.destination, true, request.maximum_hash_bytes)?;
        Ok(RenameOutcome {
            observed_identity: moved.native_identity,
        })
    }

    fn create_directory_no_replace(&self, path: &Path) -> Result<(), PlatformError> {
        self.assert_path(path);
        fs::create_dir(path).map_err(Into::into)
    }

    fn remove_directory_if_empty(&self, path: &Path) -> Result<(), PlatformError> {
        self.assert_path(path);
        fs::remove_dir(path).map_err(Into::into)
    }
}

pub struct SandboxApprovedExecutorClient {
    root: PathBuf,
    reader: std::sync::Arc<dyn ReadOnlyPlatform>,
    macos_native: bool,
}

impl SandboxApprovedExecutorClient {
    pub fn new(root: &Path, reader: std::sync::Arc<dyn ReadOnlyPlatform>) -> Self {
        assert_is_test_sandbox(root, root);
        Self {
            root: root.to_path_buf(),
            reader,
            macos_native: false,
        }
    }

    #[cfg(target_os = "macos")]
    #[allow(dead_code)]
    pub fn macos_native(root: &Path, reader: std::sync::Arc<dyn ReadOnlyPlatform>) -> Self {
        let mut client = Self::new(root, reader);
        client.macos_native = true;
        client
    }
}

impl ApprovedExecutorClient for SandboxApprovedExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        envelope
            .validate()
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        authorization
            .validate(&envelope)
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        let envelope_root = decode_native_path(
            envelope.root_binding.canonical_path.encoding,
            envelope.root_binding.canonical_path.bytes.as_slice(),
        )?;
        assert_is_test_sandbox(&self.root, &envelope_root);
        assert_eq!(
            self.root
                .canonicalize()
                .unwrap_or_else(|error| panic!("sandbox root should canonicalize: {error}")),
            envelope_root
        );
        let identity = synthetic_executor_session_identity(&envelope, &authorization)?;
        Ok(Box::new(SandboxApprovedExecutorSession {
            operations: SandboxFileOperations::new(&self.root, self.reader.clone()),
            reader: self.reader.clone(),
            #[cfg(target_os = "macos")]
            macos_native: self.macos_native,
            root: self.root.clone(),
            envelope,
            authorization,
            identity,
            next_sequence: 1,
            attempted: BTreeSet::new(),
            prepared: None,
        }))
    }
}

struct SandboxApprovedExecutorSession {
    operations: SandboxFileOperations,
    reader: std::sync::Arc<dyn ReadOnlyPlatform>,
    #[cfg(target_os = "macos")]
    macos_native: bool,
    root: PathBuf,
    envelope: ImmutableExecutionEnvelope,
    authorization: SessionAuthorization,
    identity: ExecutorSessionIdentity,
    next_sequence: u64,
    attempted: BTreeSet<(OperationStepId, bool)>,
    prepared: Option<ExecutorRequestIdentity>,
}

impl ApprovedExecutorSession for SandboxApprovedExecutorSession {
    fn identity(&self) -> &ExecutorSessionIdentity {
        &self.identity
    }

    fn prepare_operation(
        &mut self,
        operation_id: OperationStepId,
        direction: OperationDirection,
    ) -> Result<ExecutorRequestIdentity, ApprovedExecutorError> {
        if self.prepared.is_some() {
            return Err(ApprovedExecutorError::Ambiguous(
                "sandbox session already has a prepared request".to_owned(),
            ));
        }
        let rollback = matches!(&direction, OperationDirection::Rollback);
        if !self.attempted.insert((operation_id, rollback)) {
            return Err(ApprovedExecutorError::Ambiguous(
                "sandbox session operation replay".to_owned(),
            ));
        }
        let operation_id_text = operation_id.to_string();
        if self.envelope.operation(&operation_id_text).is_none()
            || !self.authorization.permits(&operation_id_text, &direction)
        {
            return Err(ApprovedExecutorError::Ambiguous(
                "sandbox session authorization mismatch".to_owned(),
            ));
        }
        let request = prepare_executor_request_identity(
            &self.identity,
            operation_id,
            direction,
            self.next_sequence,
            fresh_request_nonce()?,
        )?;
        self.next_sequence = self.next_sequence.checked_add(1).ok_or_else(|| {
            ApprovedExecutorError::Ambiguous("sandbox sequence overflow".to_owned())
        })?;
        self.prepared = Some(request.clone());
        Ok(request)
    }

    fn dispatch_prepared(
        &mut self,
        request: ExecutorRequestIdentity,
        journal_intent: CommittedJournalEventBinding,
    ) -> Result<ExecutorDispatchResult, ApprovedExecutorError> {
        journal_intent
            .validate()
            .map_err(|error| ApprovedExecutorError::Ambiguous(error.to_string()))?;
        if self.prepared.take().as_ref() != Some(&request) {
            return Err(ApprovedExecutorError::Ambiguous(
                "sandbox committed request does not match its permit".to_owned(),
            ));
        }
        let operation_id_text = request.operation_id.to_string();
        let direction = match request.direction {
            ExecutorRequestDirection::Forward => OperationDirection::Forward,
            ExecutorRequestDirection::Rollback => OperationDirection::Rollback,
        };
        assert_is_test_sandbox(&self.root, &self.root);
        let operation = self.envelope.operation(&operation_id_text).ok_or_else(|| {
            ApprovedExecutorError::Ambiguous(
                "sandbox operation is outside immutable envelope".to_owned(),
            )
        })?;
        let operations: &dyn SafeFileOperations = {
            #[cfg(target_os = "macos")]
            if self.macos_native {
                &platform_macos::MacOsPlatform
            } else {
                &self.operations
            }
            #[cfg(not(target_os = "macos"))]
            {
                &self.operations
            }
        };
        let result = match (&operation.primitive, direction) {
            (
                OperationPrimitiveManifest::CreateDirectory {
                    destination_relative_path,
                },
                OperationDirection::Forward,
            ) => operations.create_directory_no_replace(&self.root.join(destination_relative_path)),
            (
                OperationPrimitiveManifest::CreateDirectory {
                    destination_relative_path,
                },
                OperationDirection::Rollback,
            ) => operations.remove_directory_if_empty(&self.root.join(destination_relative_path)),
            (
                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {
                    source_relative_path,
                },
                OperationDirection::Forward,
            ) => operations.remove_directory_if_empty(&self.root.join(source_relative_path)),
            (
                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {
                    source_relative_path,
                },
                OperationDirection::Rollback,
            ) => operations.create_directory_no_replace(&self.root.join(source_relative_path)),
            (primitive, OperationDirection::Forward) => {
                let (source, destination, original, expected) = file_primitive(primitive)?;
                match live_rename_request(
                    self.reader.as_ref(),
                    self.root.join(source),
                    self.root.join(destination),
                    source,
                    original,
                    expected,
                ) {
                    Ok(request) => operations
                        .rename_same_volume_no_replace(&request)
                        .map(|_| ()),
                    Err(error) => Err(error),
                }
            }
            (primitive, OperationDirection::Rollback) => {
                let (source, destination, original, expected) = file_primitive(primitive)?;
                match live_rename_request(
                    self.reader.as_ref(),
                    self.root.join(destination),
                    self.root.join(source),
                    destination,
                    original,
                    expected,
                ) {
                    Ok(request) => operations
                        .rename_same_volume_no_replace(&request)
                        .map(|_| ()),
                    Err(error) => Err(error),
                }
            }
        };
        let outcome = match result {
            Ok(()) => ExecutorOutcome::Success {
                applied_at_unix_ms: 1,
                observed_state_digest: FixedBytes32::from_bytes([91; 32]),
                audit: single_attempt_audit(),
            },
            Err(PlatformError::DestinationExists) => ExecutorOutcome::ProvenNotApplied {
                code: "destination_exists".to_owned(),
                detail: "The sandbox destination already exists.".to_owned(),
                audit: single_attempt_audit(),
            },
            Err(PlatformError::Precondition(detail)) => ExecutorOutcome::ProvenNotApplied {
                code: "precondition_failed".to_owned(),
                detail,
                audit: single_attempt_audit(),
            },
            Err(PlatformError::Io(error))
                if error.kind() == std::io::ErrorKind::DirectoryNotEmpty =>
            {
                ExecutorOutcome::ProvenNotApplied {
                    code: "rollback_directory_not_empty".to_owned(),
                    detail: error.to_string(),
                    audit: single_attempt_audit(),
                }
            }
            Err(error) => return Err(ApprovedExecutorError::Ambiguous(error.to_string())),
        };
        let response_digest_hex = executor_response_digest(&request, &outcome)?;
        Ok(ExecutorDispatchResult {
            outcome,
            response_digest_hex,
        })
    }
}

fn single_attempt_audit() -> ExecutorAttemptAudit {
    ExecutorAttemptAudit {
        attempt_count: 1,
        error_class: None,
    }
}

fn file_primitive(
    primitive: &OperationPrimitiveManifest,
) -> Result<(&str, &str, &str, &ExpectedFileStateManifest), ApprovedExecutorError> {
    match primitive {
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
        }
        | OperationPrimitiveManifest::InternalStage {
            source_relative_path,
            destination_relative_path,
            original_source_relative_path,
            expected_source,
            ..
        } => Ok((
            source_relative_path,
            destination_relative_path,
            original_source_relative_path,
            expected_source,
        )),
        OperationPrimitiveManifest::CreateDirectory { .. }
        | OperationPrimitiveManifest::RemoveDirectoryIfEmpty { .. } => Err(
            ApprovedExecutorError::Ambiguous("expected a file primitive".to_owned()),
        ),
    }
}

fn live_rename_request(
    reader: &dyn ReadOnlyPlatform,
    source: PathBuf,
    destination: PathBuf,
    live_source_relative: &str,
    original_source_relative: &str,
    expected: &ExpectedFileStateManifest,
) -> Result<RenameRequest, PlatformError> {
    let observed = reader.fingerprint(&source, true, MAX_EXECUTION_VERIFICATION_BYTES)?;
    if !sandbox_expected_matches(
        expected,
        &observed,
        live_source_relative == original_source_relative,
    ) {
        return Err(PlatformError::Precondition(
            "sandbox source precondition changed".to_owned(),
        ));
    }
    Ok(RenameRequest {
        source,
        destination,
        expected_identity: observed.native_identity,
        expected_byte_size: expected.byte_size,
        expected_modified_at_ns: expected.modified_at_ns.map(i128::from),
        expected_attributes: expected.attributes,
        expected_content_digest: *expected.content_digest.as_bytes(),
        maximum_hash_bytes: MAX_EXECUTION_VERIFICATION_BYTES,
    })
}

fn sandbox_expected_matches(
    expected: &ExpectedFileStateManifest,
    observed: &FileFingerprint,
    require_original_location: bool,
) -> bool {
    let expected_identity = native_identity_from_manifest(expected);
    expected_identity.volume.stable_identifier == observed.native_identity.volume.stable_identifier
        && expected_identity.object_key == observed.native_identity.object_key
        && (!require_original_location
            || (expected_identity.parent_key == observed.native_identity.parent_key
                && expected_identity.leaf_name == observed.native_identity.leaf_name))
        && expected_identity.link_count == 1
        && observed.native_identity.link_count == 1
        && expected_identity.reparse_tag.is_none()
        && observed.native_identity.reparse_tag.is_none()
        && observed.byte_size == expected.byte_size
        && observed.modified_at_ns == expected.modified_at_ns.map(i128::from)
        && observed.attributes == expected.attributes
        && observed.content_digest.as_ref() == Some(expected.content_digest.as_bytes())
}

fn native_identity_from_manifest(expected: &ExpectedFileStateManifest) -> NativeFileIdentity {
    NativeFileIdentity {
        volume: domain::VolumeIdentity {
            platform: match expected.native_identity.volume.platform {
                ipc_contracts::executor_v2::PlatformKindManifest::Windows => {
                    domain::PlatformKind::Windows
                }
                ipc_contracts::executor_v2::PlatformKindManifest::MacOs => {
                    domain::PlatformKind::MacOs
                }
                ipc_contracts::executor_v2::PlatformKindManifest::Linux => {
                    domain::PlatformKind::Linux
                }
                ipc_contracts::executor_v2::PlatformKindManifest::Other => {
                    domain::PlatformKind::Other
                }
            },
            stable_identifier: expected.native_identity.volume.stable_identifier.clone(),
            filesystem_type: expected.native_identity.volume.filesystem_type.clone(),
            case_sensitive: expected.native_identity.volume.case_sensitive,
            removable: expected.native_identity.volume.removable,
            local: expected.native_identity.volume.local,
        },
        object_key: expected.native_identity.object_key.as_slice().to_vec(),
        parent_key: expected.native_identity.parent_key.as_slice().to_vec(),
        leaf_name: domain::NativePath {
            encoding: match expected.native_identity.leaf_name.encoding {
                NativePathEncoding::WindowsUtf16Le => domain::PathEncoding::WindowsUtf16Le,
                NativePathEncoding::UnixBytes => domain::PathEncoding::UnixBytes,
            },
            bytes: expected.native_identity.leaf_name.bytes.as_slice().to_vec(),
        },
        link_count: expected.native_identity.link_count,
        reparse_tag: expected.native_identity.reparse_tag,
    }
}

fn decode_native_path(
    encoding: NativePathEncoding,
    bytes: &[u8],
) -> Result<PathBuf, ApprovedExecutorError> {
    match encoding {
        NativePathEncoding::UnixBytes => {
            #[cfg(unix)]
            {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
            }
            #[cfg(not(unix))]
            {
                String::from_utf8(bytes.to_vec())
                    .map(PathBuf::from)
                    .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))
            }
        }
        NativePathEncoding::WindowsUtf16Le => {
            let chunks = bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return Err(ApprovedExecutorError::Unavailable(
                    "invalid UTF-16 root binding".to_owned(),
                ));
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
                    .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))
            }
        }
    }
}

pub fn assert_is_test_sandbox(root: &Path, candidate: &Path) {
    let marker = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    assert!(
        marker.starts_with("supremacy-m8-sandbox-"),
        "mutation root is not an isolated M8 sandbox"
    );
    assert!(root.is_absolute(), "sandbox root must be absolute");
    let checked = if candidate.exists() {
        candidate
            .canonicalize()
            .unwrap_or_else(|error| panic!("sandbox path should canonicalize: {error}"))
    } else {
        let parent = nearest_existing_parent(candidate);
        let canonical_parent = parent
            .canonicalize()
            .unwrap_or_else(|error| panic!("sandbox parent should canonicalize: {error}"));
        canonical_parent.join(
            candidate
                .strip_prefix(parent)
                .unwrap_or_else(|_| panic!("candidate should remain below checked parent")),
        )
    };
    let canonical_root = root
        .canonicalize()
        .unwrap_or_else(|error| panic!("sandbox root should canonicalize: {error}"));
    assert!(
        checked == canonical_root || checked.starts_with(&canonical_root),
        "mutation escaped the isolated test sandbox"
    );
}

fn nearest_existing_parent(path: &Path) -> &Path {
    let mut current = path;
    while !current.exists() {
        current = current
            .parent()
            .unwrap_or_else(|| panic!("sandbox candidate should have an existing ancestor"));
    }
    current
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, (u64, blake3::Hash)> {
    let mut output = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("sandbox should be readable: {error}"))
        {
            let entry = entry.unwrap_or_else(|error| panic!("entry should be readable: {error}"));
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("metadata should be readable: {error}"));
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("file should be readable: {error}"));
                output.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|_| panic!("snapshot should remain scoped"))
                        .to_path_buf(),
                    (metadata.len(), blake3::hash(&bytes)),
                );
            }
        }
    }
    output
}

fn same_identity(left: &NativeFileIdentity, right: &NativeFileIdentity) -> bool {
    left.volume.stable_identifier == right.volume.stable_identifier
        && left.object_key == right.object_key
        && left.link_count == right.link_count
}
