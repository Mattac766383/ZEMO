#![cfg(all(windows, feature = "mutation"))]

//! Native Windows path identity qualification (Unicode, reserved names, case).
//! Mutations stay inside a fresh temporary `supremacy-m15-sandbox-*` root.

use domain::FileFingerprint;
use platform::{
    MAX_EXECUTION_FINGERPRINT_BYTES, PlatformError, ReadOnlyPlatform, RenameRequest,
    SafeFileOperations,
};
use platform_windows::WindowsPlatform;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};
use tempfile::{Builder, TempDir};

const FORBIDDEN_PROFILE_DIRS: &[&str] = &["Documents", "Desktop", "Downloads"];

struct PathSandbox {
    _temporary: TempDir,
    root: PathBuf,
}

impl PathSandbox {
    fn new() -> Self {
        let temporary = Builder::new()
            .prefix("supremacy-m15-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("M15 sandbox should be created: {error}"));
        let root = temporary
            .path()
            .canonicalize()
            .unwrap_or_else(|error| panic!("M15 sandbox should canonicalize: {error}"));
        let temporary_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|error| panic!("temporary root should canonicalize: {error}"));
        assert!(
            root.starts_with(&temporary_root),
            "sandbox must remain below the process temporary root: {root:?}"
        );
        for forbidden in FORBIDDEN_PROFILE_DIRS {
            assert!(
                !root
                    .components()
                    .any(|component| component.as_os_str() == *forbidden),
                "sandbox must not use profile directory {forbidden}: {root:?}"
            );
        }
        let volume = WindowsPlatform
            .inspect_volume(&root)
            .unwrap_or_else(|error| panic!("sandbox volume should be inspectable: {error}"));
        assert!(volume.local, "qualification requires a local volume");
        Self {
            _temporary: temporary,
            root,
        }
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        assert!(
            !relative.as_os_str().is_empty() && !relative.is_absolute(),
            "sandbox paths must be relative: {relative:?}"
        );
        assert!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "sandbox paths cannot contain traversal: {relative:?}"
        );
        let path = self.root.join(relative);
        assert!(
            path.starts_with(&self.root) && path != self.root,
            "path escaped sandbox: {path:?}"
        );
        path
    }

    fn create_dir_all(&self, relative: impl AsRef<Path>) -> PathBuf {
        let destination = self.path(&relative);
        fs::create_dir_all(&destination)
            .unwrap_or_else(|error| panic!("directory should be created: {error}"));
        destination
    }

    fn write_file(&self, relative: impl AsRef<Path>, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("parent should be created: {error}"));
        }
        fs::write(&path, bytes).unwrap_or_else(|error| panic!("file should be written: {error}"));
        path
    }

    fn fingerprint(&self, path: &Path) -> FileFingerprint {
        WindowsPlatform
            .fingerprint(path, true, MAX_EXECUTION_FINGERPRINT_BYTES)
            .unwrap_or_else(|error| panic!("fingerprint should succeed: {error}"))
    }

    fn request(&self, source: &Path, destination: &Path) -> RenameRequest {
        let fingerprint = self.fingerprint(source);
        RenameRequest {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            expected_identity: fingerprint.native_identity,
            expected_byte_size: fingerprint.byte_size,
            expected_modified_at_ns: fingerprint.modified_at_ns,
            expected_attributes: fingerprint.attributes,
            expected_content_digest: fingerprint
                .content_digest
                .unwrap_or_else(|| panic!("digest required")),
            maximum_hash_bytes: MAX_EXECUTION_FINGERPRINT_BYTES,
        }
    }
}

#[test]
fn unicode_and_non_ascii_paths_preserve_native_identity_on_move() {
    let sandbox = PathSandbox::new();
    sandbox.create_dir_all("unicode/from");
    sandbox.create_dir_all("unicode/to");
    let source = sandbox.write_file(
        Path::new("unicode/from").join("facture-été-发票.txt"),
        "contenu été".as_bytes(),
    );
    let destination = sandbox.path(Path::new("unicode/to").join("facture-été-发票.txt"));
    let before = sandbox.fingerprint(&source);
    let request = sandbox.request(&source, &destination);

    WindowsPlatform
        .rename_same_volume_no_replace(&request)
        .unwrap_or_else(|error| panic!("unicode move should succeed: {error}"));

    assert!(!source.exists());
    assert_eq!(
        fs::read(&destination).unwrap_or_else(|error| panic!("destination read: {error}")),
        "contenu été".as_bytes()
    );
    let after = sandbox.fingerprint(&destination);
    assert_eq!(
        after.native_identity.object_key,
        before.native_identity.object_key
    );
    assert_eq!(after.content_digest, before.content_digest);
    assert_eq!(
        after.native_identity.leaf_name.encoding,
        domain::PathEncoding::WindowsUtf16Le
    );
}

#[test]
fn case_insensitive_volume_reports_stable_native_identity_keys() {
    let sandbox = PathSandbox::new();
    sandbox.create_dir_all("case");
    let path = sandbox.write_file("case/Invoice.TXT", b"case");
    let first = sandbox.fingerprint(&path);
    let second = sandbox.fingerprint(&path);
    assert_eq!(
        first.native_identity.object_key,
        second.native_identity.object_key
    );
    assert!(!first.native_identity.volume.case_sensitive);
    assert_eq!(
        first.native_identity.volume.filesystem_type.as_deref(),
        Some("NTFS")
    );
}

#[test]
fn reserved_device_names_are_refused_by_path_policy() {
    let sandbox = PathSandbox::new();
    let registered = sandbox.create_dir_all("reserved");
    for name in ["CON.txt", "NUL.txt", "PRN.txt", "COM1.txt", "LPT1.txt"] {
        let result = WindowsPlatform.inspect_regular_file(&registered, Path::new(name));
        assert!(
            matches!(result, Err(PlatformError::PathPolicyRefusal)),
            "reserved name {name} must be refused, got {result:?}"
        );
    }
}

#[test]
fn long_unicode_component_names_round_trip_without_lossy_utf8_identity() {
    let sandbox = PathSandbox::new();
    let leaf = format!("{}-{}", "ドキュメント", "x".repeat(40));
    sandbox.create_dir_all("long-unicode");
    let source = sandbox.write_file(Path::new("long-unicode").join(&leaf), b"long unicode");
    let fingerprint = sandbox.fingerprint(&source);
    assert_eq!(
        fingerprint.native_identity.leaf_name.encoding,
        domain::PathEncoding::WindowsUtf16Le
    );
    assert!(
        fingerprint.native_identity.leaf_name.bytes.len() >= leaf.encode_utf16().count() * 2,
        "leaf identity must retain UTF-16 units, not lossy UTF-8"
    );
}
