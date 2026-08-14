use domain::{ExecutionSafetyPolicyBinding, PathEncoding};
use organizer::VirtualPathPolicy;
use serde::Serialize;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub const HARD_MAX_REHASH_BYTES: u64 = platform::MAX_EXECUTION_FINGERPRINT_BYTES;
pub const DEFAULT_MAX_REHASH_BYTES: u64 = HARD_MAX_REHASH_BYTES;
pub const LARGE_BATCH_CONFIRMATION_THRESHOLD: u64 = 1_000;
pub const CONFIRMATION_PHRASE: &str = "ORGANIZE";
pub const STAGING_DIRECTORY_NAME: &str = ".supremacy-staging";
pub const EXECUTION_SAFETY_POLICY_VERSION: &str = domain::EXECUTION_SAFETY_POLICY_VERSION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSafetyPolicy {
    pub maximum_rehash_bytes: u64,
    pub large_batch_confirmation_threshold: u64,
    pub allow_independent_safe_subset: bool,
    pub allow_qualified_case_only_rename: bool,
    protected_paths: Vec<PathBuf>,
}

impl Default for ExecutionSafetyPolicy {
    fn default() -> Self {
        Self {
            maximum_rehash_bytes: DEFAULT_MAX_REHASH_BYTES,
            large_batch_confirmation_threshold: LARGE_BATCH_CONFIRMATION_THRESHOLD,
            allow_independent_safe_subset: true,
            allow_qualified_case_only_rename: false,
            protected_paths: Vec::new(),
        }
    }
}

impl ExecutionSafetyPolicy {
    #[must_use]
    pub fn with_protected_paths(mut self, protected_paths: Vec<PathBuf>) -> Self {
        self.protected_paths = protected_paths;
        self
    }

    pub fn binding(&self) -> Result<ExecutionSafetyPolicyBinding, SafetyPolicyError> {
        if self.maximum_rehash_bytes == 0 || self.maximum_rehash_bytes > HARD_MAX_REHASH_BYTES {
            return Err(SafetyPolicyError::InvalidVerificationBound);
        }
        let mut protected_paths = self
            .protected_paths
            .iter()
            .map(|path| {
                let native = native_path_bytes(path);
                CanonicalPolicyPath {
                    encoding: native.0,
                    bytes: native.1,
                }
            })
            .collect::<Vec<_>>();
        protected_paths.sort_by(|left, right| {
            (path_encoding_order(left.encoding), &left.bytes)
                .cmp(&(path_encoding_order(right.encoding), &right.bytes))
        });
        let material = CanonicalSafetyPolicy {
            version: EXECUTION_SAFETY_POLICY_VERSION,
            maximum_rehash_bytes: self.maximum_rehash_bytes,
            large_batch_confirmation_threshold: self.large_batch_confirmation_threshold,
            confirmation_phrase: CONFIRMATION_PHRASE,
            allow_independent_safe_subset: self.allow_independent_safe_subset,
            allow_qualified_case_only_rename: self.allow_qualified_case_only_rename,
            staging_directory_name: STAGING_DIRECTORY_NAME,
            protected_paths,
        };
        let encoded =
            serde_json::to_vec(&material).map_err(|_| SafetyPolicyError::SerializationFailed)?;
        Ok(ExecutionSafetyPolicyBinding {
            version: EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
            maximum_rehash_bytes: self.maximum_rehash_bytes,
            allow_qualified_case_only_rename: self.allow_qualified_case_only_rename,
            digest_hex: blake3::hash(&encoded).to_hex().to_string(),
        })
    }

    pub fn validate_root(&self, root: &Path) -> Result<PathBuf, SafetyPolicyError> {
        if !root.is_absolute()
            || root
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return Err(SafetyPolicyError::InvalidRoot);
        }
        let metadata = fs::symlink_metadata(root).map_err(SafetyPolicyError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SafetyPolicyError::LinkedOrInvalidRoot);
        }
        let canonical = fs::canonicalize(root).map_err(SafetyPolicyError::Io)?;
        if canonical.parent().is_none() || is_platform_protected_root(&canonical) {
            return Err(SafetyPolicyError::ProtectedPath);
        }
        for protected in &self.protected_paths {
            let protected = protected
                .canonicalize()
                .unwrap_or_else(|_| protected.to_path_buf());
            if path_contains(&canonical, &protected) || path_contains(&protected, &canonical) {
                return Err(SafetyPolicyError::ProtectedPath);
            }
        }
        Ok(canonical)
    }

    pub fn validate_destination_components(
        &self,
        destination: &[String],
        filename: &str,
    ) -> Result<(), SafetyPolicyError> {
        let path_policy = VirtualPathPolicy::default();
        path_policy
            .validate_user_destination(destination)
            .map_err(|_| SafetyPolicyError::InvalidRelativePath)?;
        path_policy
            .validate_user_filename(filename)
            .map_err(|_| SafetyPolicyError::InvalidRelativePath)?;
        if path_policy.path_length_utf16(destination, filename) > path_policy.maximum_path_utf16 {
            return Err(SafetyPolicyError::InvalidRelativePath);
        }
        Ok(())
    }

    pub fn resolve_existing_source(
        &self,
        root: &Path,
        relative: &Path,
    ) -> Result<PathBuf, SafetyPolicyError> {
        let joined = resolve_lexically(root, relative)?;
        let metadata = fs::symlink_metadata(&joined).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SafetyPolicyError::SourceMissing
            } else {
                SafetyPolicyError::Io(error)
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SafetyPolicyError::LinkedOrSpecialEntry);
        }
        validate_existing_ancestors(root, relative, true)?;
        let canonical_root = fs::canonicalize(root).map_err(SafetyPolicyError::Io)?;
        let canonical_source = fs::canonicalize(&joined).map_err(SafetyPolicyError::Io)?;
        if canonical_source == canonical_root || !path_contains(&canonical_root, &canonical_source)
        {
            return Err(SafetyPolicyError::OutsideRoot);
        }
        self.reject_app_owned_path(&canonical_source)?;
        Ok(joined)
    }

    pub fn resolve_absent_destination(
        &self,
        root: &Path,
        relative: &Path,
        internal_staging: bool,
    ) -> Result<PathBuf, SafetyPolicyError> {
        let joined = resolve_lexically(root, relative)?;
        validate_existing_ancestors(root, relative, false)?;
        match fs::symlink_metadata(&joined) {
            Ok(_) => return Err(SafetyPolicyError::DestinationExists),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(SafetyPolicyError::Io(error)),
        }
        let parent = joined.parent().ok_or(SafetyPolicyError::OutsideRoot)?;
        let existing_parent = nearest_existing_parent(parent)?;
        let canonical_root = fs::canonicalize(root).map_err(SafetyPolicyError::Io)?;
        let canonical_parent = fs::canonicalize(existing_parent).map_err(SafetyPolicyError::Io)?;
        if canonical_parent != canonical_root && !path_contains(&canonical_root, &canonical_parent)
        {
            return Err(SafetyPolicyError::OutsideRoot);
        }
        if !internal_staging {
            self.reject_app_owned_path(&joined)?;
            if relative
                .components()
                .next()
                .is_some_and(|component| component.as_os_str() == STAGING_DIRECTORY_NAME)
            {
                return Err(SafetyPolicyError::ProtectedPath);
            }
        }
        Ok(joined)
    }

    pub fn validate_confirmation(
        &self,
        operation_count: u64,
        confirmed: bool,
        phrase: Option<&str>,
    ) -> Result<(), SafetyPolicyError> {
        if !confirmed {
            return Err(SafetyPolicyError::ConfirmationRequired);
        }
        if operation_count >= self.large_batch_confirmation_threshold
            && phrase.map(str::trim) != Some(CONFIRMATION_PHRASE)
        {
            return Err(SafetyPolicyError::ConfirmationPhraseRequired);
        }
        Ok(())
    }

    fn reject_app_owned_path(&self, candidate: &Path) -> Result<(), SafetyPolicyError> {
        for protected in &self.protected_paths {
            let protected = protected
                .canonicalize()
                .unwrap_or_else(|_| protected.to_path_buf());
            if path_contains(&protected, candidate) || path_contains(candidate, &protected) {
                return Err(SafetyPolicyError::ProtectedPath);
            }
        }
        Ok(())
    }
}

fn path_encoding_order(encoding: PathEncoding) -> u8 {
    match encoding {
        PathEncoding::WindowsUtf16Le => 0,
        PathEncoding::UnixBytes => 1,
    }
}

#[derive(Serialize)]
struct CanonicalSafetyPolicy {
    version: &'static str,
    maximum_rehash_bytes: u64,
    large_batch_confirmation_threshold: u64,
    confirmation_phrase: &'static str,
    allow_independent_safe_subset: bool,
    allow_qualified_case_only_rename: bool,
    staging_directory_name: &'static str,
    protected_paths: Vec<CanonicalPolicyPath>,
}

#[derive(Serialize)]
struct CanonicalPolicyPath {
    encoding: PathEncoding,
    bytes: Vec<u8>,
}

#[cfg(unix)]
fn native_path_bytes(path: &Path) -> (PathEncoding, Vec<u8>) {
    use std::os::unix::ffi::OsStrExt;
    (
        PathEncoding::UnixBytes,
        path.as_os_str().as_bytes().to_vec(),
    )
}

#[cfg(windows)]
fn native_path_bytes(path: &Path) -> (PathEncoding, Vec<u8>) {
    use std::os::windows::ffi::OsStrExt;
    (
        PathEncoding::WindowsUtf16Le,
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect(),
    )
}

#[cfg(not(any(unix, windows)))]
fn native_path_bytes(path: &Path) -> (PathEncoding, Vec<u8>) {
    (
        PathEncoding::UnixBytes,
        path.to_string_lossy().as_bytes().to_vec(),
    )
}

#[derive(Debug, thiserror::Error)]
pub enum SafetyPolicyError {
    #[error("the approved root is invalid")]
    InvalidRoot,
    #[error("the approved root is linked or is not a directory")]
    LinkedOrInvalidRoot,
    #[error("the path is outside the approved root")]
    OutsideRoot,
    #[error("the path is protected by execution policy")]
    ProtectedPath,
    #[error("the relative destination is invalid")]
    InvalidRelativePath,
    #[error("the source no longer exists")]
    SourceMissing,
    #[error("the source is linked or is not a regular file")]
    LinkedOrSpecialEntry,
    #[error("the destination already exists")]
    DestinationExists,
    #[error("explicit confirmation is required")]
    ConfirmationRequired,
    #[error("the large-batch confirmation phrase is required")]
    ConfirmationPhraseRequired,
    #[error("the execution verification bound is outside the supported range")]
    InvalidVerificationBound,
    #[error("the execution safety policy could not be serialized")]
    SerializationFailed,
    #[error("filesystem safety inspection failed: {0}")]
    Io(std::io::Error),
}

fn resolve_lexically(root: &Path, relative: &Path) -> Result<PathBuf, SafetyPolicyError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SafetyPolicyError::InvalidRelativePath);
    }
    Ok(root.join(relative))
}

fn validate_existing_ancestors(
    root: &Path,
    relative: &Path,
    include_leaf: bool,
) -> Result<(), SafetyPolicyError> {
    let mut current = root.to_path_buf();
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        if !include_leaf && index + 1 == component_count {
            break;
        }
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SafetyPolicyError::LinkedOrSpecialEntry);
            }
            Ok(metadata) if index + 1 < component_count && !metadata.is_dir() => {
                return Err(SafetyPolicyError::LinkedOrSpecialEntry);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(SafetyPolicyError::Io(error)),
        }
    }
    Ok(())
}

fn nearest_existing_parent(path: &Path) -> Result<&Path, SafetyPolicyError> {
    let mut current = path;
    loop {
        match fs::symlink_metadata(current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(SafetyPolicyError::LinkedOrSpecialEntry);
            }
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = current.parent().ok_or(SafetyPolicyError::OutsideRoot)?;
            }
            Err(error) => return Err(SafetyPolicyError::Io(error)),
        }
    }
}

fn path_contains(parent: &Path, child: &Path) -> bool {
    if cfg!(windows) {
        let parent = parent.to_string_lossy().to_lowercase();
        let child = child.to_string_lossy().to_lowercase();
        child == parent
            || child
                .strip_prefix(&parent)
                .is_some_and(|tail| tail.starts_with(['\\', '/']))
    } else {
        child == parent || child.starts_with(parent)
    }
}

fn is_platform_protected_root(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let candidate = path.to_string_lossy().to_lowercase();
        let protected = [
            std::env::var_os("WINDIR"),
            std::env::var_os("ProgramFiles"),
            std::env::var_os("ProgramFiles(x86)"),
            std::env::var_os("ProgramData"),
        ];
        if protected.into_iter().flatten().any(|value| {
            let value = PathBuf::from(value)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(""));
            !value.as_os_str().is_empty() && path_contains(&value, path)
        }) {
            return true;
        }
        let components = path.components().count();
        let contains_protected_internal = candidate
            .split(['\\', '/'])
            .any(|component| matches!(component, "$recycle.bin" | "system volume information"));
        return components <= 2 || contains_protected_internal;
    }
    #[cfg(target_os = "macos")]
    {
        ["/System", "/Library", "/Applications"]
            .into_iter()
            .map(Path::new)
            .any(|protected| path_contains(protected, path))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        path == Path::new("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn policy_rejects_traversal_and_protected_staging_names() {
        let fixture = TempDir::new().unwrap_or_else(|error| panic!("sandbox: {error}"));
        let policy = ExecutionSafetyPolicy::default();
        assert!(policy.validate_root(fixture.path()).is_ok());
        assert!(matches!(
            policy.resolve_absent_destination(fixture.path(), Path::new("../escape"), false),
            Err(SafetyPolicyError::InvalidRelativePath)
        ));
        assert!(matches!(
            policy.resolve_absent_destination(
                fixture.path(),
                Path::new(".supremacy-staging/user-name"),
                false
            ),
            Err(SafetyPolicyError::ProtectedPath)
        ));
    }

    #[test]
    fn large_batches_require_the_exact_phrase() {
        let policy = ExecutionSafetyPolicy::default();
        assert!(matches!(
            policy.validate_confirmation(1_000, true, Some("organize")),
            Err(SafetyPolicyError::ConfirmationPhraseRequired)
        ));
        assert!(
            policy
                .validate_confirmation(1_000, true, Some(CONFIRMATION_PHRASE))
                .is_ok()
        );
    }

    #[test]
    fn windows_malicious_destination_components_fail_closed() {
        let policy = ExecutionSafetyPolicy::default();
        let invalid = [
            (vec!["..".to_owned()], "escape.txt".to_owned()),
            (vec!["../../Windows".to_owned()], "escape.txt".to_owned()),
            (vec!["C:\\Windows".to_owned()], "escape.txt".to_owned()),
            (
                vec!["\\\\server\\share".to_owned()],
                "escape.txt".to_owned(),
            ),
            (vec!["Safe".to_owned()], "CON.txt".to_owned()),
            (vec!["Safe".to_owned()], "<script>.txt".to_owned()),
            (vec!["Safe".to_owned()], "null\0byte.txt".to_owned()),
            (vec!["Safe".to_owned()], format!("{}.txt", "x".repeat(300))),
        ];
        for (destination, filename) in invalid {
            assert!(
                policy
                    .validate_destination_components(&destination, &filename)
                    .is_err(),
                "malicious path should be rejected: {destination:?}/{filename}"
            );
        }
        assert!(
            policy
                .validate_destination_components(
                    &["Invoices".to_owned()],
                    "'; DROP TABLE files; --.txt"
                )
                .is_ok(),
            "SQL-like text is a filename, not executable SQL"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_ancestor_cannot_escape_the_approved_root() {
        use std::os::unix::fs::symlink;

        let sandbox = tempfile::Builder::new()
            .prefix("supremacy-m8-policy-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("policy sandbox should be created: {error}"));
        let outside = tempfile::Builder::new()
            .prefix("supremacy-m8-policy-outside-")
            .tempdir()
            .unwrap_or_else(|error| panic!("outside fixture should be created: {error}"));
        symlink(outside.path(), sandbox.path().join("linked"))
            .unwrap_or_else(|error| panic!("symlink fixture should be created: {error}"));
        let policy = ExecutionSafetyPolicy::default();
        assert!(matches!(
            policy.resolve_absent_destination(
                sandbox.path(),
                Path::new("linked/escaped.txt"),
                false
            ),
            Err(SafetyPolicyError::LinkedOrSpecialEntry)
        ));
    }

    #[test]
    fn app_owned_paths_make_an_overlapping_root_ineligible() {
        let sandbox = tempfile::Builder::new()
            .prefix("supremacy-m8-protected-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("protected sandbox should be created: {error}"));
        let protected = sandbox.path().join("catalog.db");
        fs::write(&protected, b"application-owned")
            .unwrap_or_else(|error| panic!("protected fixture should be written: {error}"));
        let policy = ExecutionSafetyPolicy::default().with_protected_paths(vec![protected]);
        assert!(matches!(
            policy.validate_root(sandbox.path()),
            Err(SafetyPolicyError::ProtectedPath)
        ));
    }

    #[test]
    fn policy_v2_binds_the_numeric_hard_limit_and_case_only_gate() {
        let baseline = ExecutionSafetyPolicy::default()
            .binding()
            .unwrap_or_else(|error| panic!("default policy should bind: {error}"));
        assert_eq!(baseline.version, EXECUTION_SAFETY_POLICY_VERSION);
        assert_eq!(baseline.maximum_rehash_bytes, 64 * 1024 * 1024 * 1024);
        assert!(!baseline.allow_qualified_case_only_rename);

        let qualified = ExecutionSafetyPolicy {
            allow_qualified_case_only_rename: true,
            ..ExecutionSafetyPolicy::default()
        };
        let qualified = qualified
            .binding()
            .unwrap_or_else(|error| panic!("qualified test policy should bind: {error}"));
        assert_ne!(baseline.digest_hex, qualified.digest_hex);
        assert!(qualified.allow_qualified_case_only_rename);
    }

    #[test]
    fn policy_rejects_bounds_above_the_64_gib_hard_limit() {
        let policy = ExecutionSafetyPolicy {
            maximum_rehash_bytes: HARD_MAX_REHASH_BYTES.saturating_add(1),
            ..ExecutionSafetyPolicy::default()
        };
        assert!(matches!(
            policy.binding(),
            Err(SafetyPolicyError::InvalidVerificationBound)
        ));
    }
}
