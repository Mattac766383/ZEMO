#![cfg(all(target_os = "macos", feature = "mutation"))]

use platform::{
    PlatformError, ReadOnlyPlatform, RenameRequest, STREAMING_FINGERPRINT_BUFFER_BYTES,
    SafeFileOperations,
};
use platform_macos::MacOsPlatform;
use std::{
    fs::{self, OpenOptions},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};
use tempfile::{Builder, TempDir};

struct MacosSandbox {
    directory: TempDir,
}

impl MacosSandbox {
    fn new() -> Self {
        let directory = Builder::new()
            .prefix("supremacy-m18-macos-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("macos sandbox should be created: {error}"));
        let root = directory
            .path()
            .canonicalize()
            .unwrap_or_else(|error| panic!("macos sandbox should canonicalize: {error}"));
        let temporary_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|error| panic!("temp directory should canonicalize: {error}"));
        assert!(
            root.starts_with(&temporary_root),
            "sandbox must remain below the process temporary root: {root:?}"
        );
        for forbidden in [
            dirs_user("Documents"),
            dirs_user("Desktop"),
            dirs_user("Downloads"),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !root.starts_with(&forbidden),
                "sandbox must not use profile directory {forbidden:?}: {root:?}"
            );
        }
        Self { directory }
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        assert!(
            relative.is_relative() && !relative.as_os_str().is_empty(),
            "sandbox paths must be relative: {relative:?}"
        );
        assert!(
            relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
            "sandbox paths cannot contain traversal: {relative:?}"
        );
        let path = self.directory.path().join(relative);
        assert!(
            path.starts_with(self.directory.path()),
            "path escaped sandbox: {path:?}"
        );
        path
    }

    fn create_dir_all(&self, relative: impl AsRef<Path>) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("sandbox directory should be created: {error}"));
        path
    }

    fn write_file(&self, relative: impl AsRef<Path>, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("sandbox parent should be created: {error}"));
        }
        fs::write(&path, bytes)
            .unwrap_or_else(|error| panic!("sandbox file should be written: {error}"));
        path
    }

    fn request(&self, source: &Path, destination: &Path) -> RenameRequest {
        let fingerprint = MacOsPlatform
            .fingerprint(source, true, u64::MAX)
            .unwrap_or_else(|error| panic!("sandbox fingerprint should succeed: {error}"));
        RenameRequest {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            expected_identity: fingerprint.native_identity,
            expected_byte_size: fingerprint.byte_size,
            expected_modified_at_ns: fingerprint.modified_at_ns,
            expected_attributes: fingerprint.attributes,
            expected_content_digest: fingerprint
                .content_digest
                .unwrap_or_else(|| panic!("sandbox fingerprint should include a digest")),
            maximum_hash_bytes: STREAMING_FINGERPRINT_BUFFER_BYTES as u64 * 4,
        }
    }
}

fn dirs_user(name: &str) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(name))
}

fn assert_same_bytes(path: &Path, expected: &[u8]) {
    assert_eq!(
        fs::read(path).unwrap_or_else(|error| panic!("file should remain readable: {error}")),
        expected
    );
}

#[test]
fn same_volume_move_rename_and_move_rename_are_atomic_and_exclusive() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("move/from");
    sandbox.create_dir_all("move/to");
    sandbox.create_dir_all("rename");
    sandbox.create_dir_all("combined/from");
    sandbox.create_dir_all("combined/to");

    let source = sandbox.write_file("move/from/document.txt", b"moved");
    let destination = sandbox.path("move/to/document.txt");
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .unwrap_or_else(|error| panic!("same-volume move should succeed: {error}"));
    assert!(!source.exists());
    assert_same_bytes(&destination, b"moved");

    let source = sandbox.write_file("rename/original.txt", b"renamed");
    let destination = sandbox.path("rename/renamed.txt");
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .unwrap_or_else(|error| panic!("rename should succeed: {error}"));
    assert!(!source.exists());
    assert_same_bytes(&destination, b"renamed");

    let source = sandbox.write_file("combined/from/original.txt", b"both");
    let destination = sandbox.path("combined/to/final.txt");
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .unwrap_or_else(|error| panic!("move+rename should succeed: {error}"));
    assert!(!source.exists());
    assert_same_bytes(&destination, b"both");
}

#[test]
fn case_only_rename_requires_staging_then_preserves_bytes() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("case");
    let source = sandbox.write_file("case/Invoice.pdf", b"case-preserved");
    let destination = sandbox.path("case/invoice.pdf");
    let direct = sandbox.request(&source, &destination);
    let blocked = MacOsPlatform
        .rename_same_volume_no_replace(&direct)
        .expect_err("direct case-only rename must not replace in place");
    assert!(
        matches!(
            blocked,
            PlatformError::Precondition(_) | PlatformError::DestinationExists
        ),
        "direct case-only should fail closed: {blocked:?}"
    );
    assert_same_bytes(&source, b"case-preserved");

    let staging = sandbox.path("case/.supremacy-case-stage");
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &staging))
        .unwrap_or_else(|error| panic!("case-only staging should succeed: {error}"));
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&staging, &destination))
        .unwrap_or_else(|error| panic!("case-only finish should succeed: {error}"));
    assert_same_bytes(&destination, b"case-preserved");
    let leaf = fs::read_dir(sandbox.path("case"))
        .unwrap_or_else(|error| panic!("case directory should be readable: {error}"))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("dirent: {error}"))
                .file_name()
        })
        .find(|name| name.as_bytes().eq_ignore_ascii_case(b"invoice.pdf"))
        .unwrap_or_else(|| panic!("case-preserving leaf should exist"));
    assert_eq!(leaf.as_bytes(), b"invoice.pdf");
}

#[test]
fn unicode_filename_uses_native_bytes() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("unicode/from");
    sandbox.create_dir_all("unicode/to");
    let source = sandbox.write_file(
        Path::new("unicode/from").join("facture-été-发票.txt"),
        b"unicode",
    );
    let destination = sandbox.path(Path::new("unicode/to").join("facture-été-发票.txt"));
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .unwrap_or_else(|error| panic!("unicode rename should succeed: {error}"));
    assert_same_bytes(&destination, b"unicode");
}

#[test]
fn no_overwrite_and_destination_race_fail_closed() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("collision");
    let source = sandbox.write_file("collision/source.txt", b"source");
    let destination = sandbox.write_file("collision/destination.txt", b"keep-me");
    let error = MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .expect_err("occupied destination must not be replaced");
    assert!(matches!(error, PlatformError::DestinationExists));
    assert_same_bytes(&source, b"source");
    assert_same_bytes(&destination, b"keep-me");
}

#[test]
fn symlink_components_and_leaf_links_are_rejected() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("safe");
    let outside = Builder::new()
        .prefix("supremacy-m18-macos-outside-")
        .tempdir()
        .unwrap_or_else(|error| panic!("outside fixture: {error}"));
    fs::write(outside.path().join("private.txt"), b"secret")
        .unwrap_or_else(|error| panic!("outside file: {error}"));
    std::os::unix::fs::symlink(outside.path(), sandbox.path("escape"))
        .unwrap_or_else(|error| panic!("dir symlink: {error}"));
    std::os::unix::fs::symlink(
        outside.path().join("private.txt"),
        sandbox.path("safe/leaf-link.txt"),
    )
    .unwrap_or_else(|error| panic!("leaf symlink: {error}"));
    let source = sandbox.write_file("safe/source.txt", b"payload");

    let via_dir = sandbox.path("escape/stolen.txt");
    let error = MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &via_dir))
        .expect_err("symlink directory must be rejected");
    assert!(
        matches!(
            error,
            PlatformError::ReparsePoint
                | PlatformError::OutsideRoot
                | PlatformError::Unsupported(_)
        ),
        "symlink directory should fail closed: {error:?}"
    );
    assert_same_bytes(&source, b"payload");

    let leaf = sandbox.path("safe/leaf-link.txt");
    assert!(
        matches!(
            MacOsPlatform.fingerprint(&leaf, true, u64::MAX),
            Err(PlatformError::ReparsePoint | PlatformError::Io(_))
        ),
        "symlink leaf must not fingerprint as a regular file"
    );
}

#[test]
fn source_replacement_is_detected_as_precondition_failure() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("drift");
    let source = sandbox.write_file("drift/source.txt", b"original");
    let mut request = sandbox.request(&source, &sandbox.path("drift/destination.txt"));
    fs::write(&source, b"replaced-after-approval")
        .unwrap_or_else(|error| panic!("replacement should write: {error}"));
    let error = MacOsPlatform
        .rename_same_volume_no_replace(&request)
        .expect_err("replaced source must fail closed");
    assert!(matches!(error, PlatformError::Precondition(_)));
    assert_same_bytes(&source, b"replaced-after-approval");
    request.destination = sandbox.path("drift/untouched.txt");
    assert!(!request.destination.exists());
}

#[test]
fn permission_denied_and_read_only_do_not_chmod() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("readonly");
    let source = sandbox.write_file("readonly/source.txt", b"locked-bytes");
    let destination = sandbox.path("readonly/destination.txt");
    let mut permissions = fs::metadata(&source)
        .unwrap_or_else(|error| panic!("metadata: {error}"))
        .permissions();
    permissions.set_mode(0o444);
    fs::set_permissions(&source, permissions.clone())
        .unwrap_or_else(|error| panic!("chmod should apply in sandbox: {error}"));
    let error = MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .expect_err("read-only source must fail");
    assert!(matches!(error, PlatformError::PermissionDenied));
    assert_same_bytes(&source, b"locked-bytes");
    assert!(!destination.exists());
    permissions.set_mode(0o644);
    fs::set_permissions(&source, permissions)
        .unwrap_or_else(|error| panic!("restore mode: {error}"));
}

#[test]
fn directory_create_is_exclusive_and_empty_remove_only() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("dirs");
    let created = sandbox.path("dirs/new-folder");
    MacOsPlatform
        .create_directory_no_replace(&created)
        .unwrap_or_else(|error| panic!("exclusive mkdir should succeed: {error}"));
    let error = MacOsPlatform
        .create_directory_no_replace(&created)
        .expect_err("second mkdir must not replace");
    assert!(matches!(error, PlatformError::DestinationExists));
    sandbox.write_file("dirs/new-folder/child.txt", b"keep");
    let error = MacOsPlatform
        .remove_directory_if_empty(&created)
        .expect_err("non-empty directory must not be removed");
    assert!(matches!(error, PlatformError::Precondition(_)));
    fs::remove_file(sandbox.path("dirs/new-folder/child.txt"))
        .unwrap_or_else(|error| panic!("child cleanup: {error}"));
    MacOsPlatform
        .remove_directory_if_empty(&created)
        .unwrap_or_else(|error| panic!("empty directory rollback should succeed: {error}"));
    assert!(!created.exists());
}

#[test]
fn hidden_file_and_long_filename_move() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("hidden");
    let source = sandbox.write_file("hidden/.secret.txt", b"hidden");
    let destination = sandbox.path("hidden/.moved-secret.txt");
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .unwrap_or_else(|error| panic!("hidden file move should succeed: {error}"));
    assert_same_bytes(&destination, b"hidden");

    sandbox.create_dir_all("long");
    let long_name = format!("{}.txt", "n".repeat(180));
    let source = sandbox.write_file(Path::new("long").join(&long_name), b"long");
    let destination = sandbox.path(Path::new("long").join(format!("d{long_name}")));
    MacOsPlatform
        .rename_same_volume_no_replace(&sandbox.request(&source, &destination))
        .unwrap_or_else(|error| panic!("long filename move should succeed: {error}"));
    assert_same_bytes(&destination, b"long");
}

#[test]
fn open_handle_does_not_hang_and_move_still_classifies() {
    let sandbox = MacosSandbox::new();
    sandbox.create_dir_all("busy");
    let source = sandbox.write_file("busy/source.txt", b"busy");
    let destination = sandbox.path("busy/destination.txt");
    let _held = OpenOptions::new()
        .read(true)
        .open(&source)
        .unwrap_or_else(|error| panic!("held handle: {error}"));
    let result =
        MacOsPlatform.rename_same_volume_no_replace(&sandbox.request(&source, &destination));
    match result {
        Ok(_) => assert_same_bytes(&destination, b"busy"),
        Err(error) => {
            assert!(
                matches!(
                    error,
                    PlatformError::SharingViolation
                        | PlatformError::LockViolation
                        | PlatformError::PermissionDenied
                ),
                "busy file must classify, not hang: {error:?}"
            );
            assert_same_bytes(&source, b"busy");
        }
    }
}
