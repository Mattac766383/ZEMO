#![cfg(all(windows, feature = "mutation"))]

use domain::FileFingerprint;
use platform::{
    MAX_EXECUTION_FINGERPRINT_BYTES, PlatformError, ReadOnlyPlatform, RenameOutcome, RenameRequest,
    SafeFileOperations,
};
use platform_windows::WindowsPlatform;
use std::{
    ffi::{OsStr, OsString, c_void},
    fs::{self, File, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    ptr,
};
use tempfile::TempDir;
use windows_sys::Win32::{
    Foundation::{GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING,
    },
    System::IO::DeviceIoControl,
};

const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00a4;
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xa000_0003;
const ERROR_PRIVILEGE_NOT_HELD: i32 = 1_314;

struct NtfsSandbox {
    _temporary: TempDir,
    root: PathBuf,
}

impl NtfsSandbox {
    fn new() -> Self {
        let temporary = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("NTFS sandbox should be created: {error}"));
        let root = temporary
            .path()
            .canonicalize()
            .unwrap_or_else(|error| panic!("NTFS sandbox should canonicalize: {error}"));
        let temporary_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|error| panic!("temporary root should canonicalize: {error}"));
        assert!(
            root.starts_with(&temporary_root),
            "sandbox must remain below the process temporary root: {root:?}"
        );

        let volume = WindowsPlatform
            .inspect_volume(&root)
            .unwrap_or_else(|error| panic!("sandbox volume should be inspectable: {error}"));
        assert!(volume.local, "qualification requires a local volume");
        assert!(
            !volume.removable,
            "qualification requires a non-removable volume"
        );
        assert!(
            volume
                .filesystem_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("NTFS")),
            "qualification requires real NTFS, observed {:?}",
            volume.filesystem_type
        );

        Self {
            _temporary: temporary,
            root,
        }
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        assert!(
            !relative.as_os_str().is_empty() && !relative.is_absolute(),
            "sandbox paths must be non-empty and relative: {relative:?}"
        );
        assert!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "sandbox paths cannot contain traversal or prefixes: {relative:?}"
        );
        let path = self.root.join(relative);
        self.assert_scoped(&path);
        path
    }

    fn assert_scoped(&self, path: &Path) {
        assert!(
            path.is_absolute() && path.starts_with(&self.root) && path != self.root,
            "destructive test path escaped its fresh sandbox: {path:?}"
        );
    }

    fn assert_resolves_within_sandbox(&self, path: &Path) {
        self.assert_scoped(path);
        let anchor = if path.exists() {
            path
        } else {
            path.parent()
                .unwrap_or_else(|| panic!("sandbox path should have a parent: {path:?}"))
        };
        let canonical_anchor = anchor
            .canonicalize()
            .unwrap_or_else(|error| panic!("sandbox path anchor should canonicalize: {error}"));
        assert!(
            canonical_anchor.starts_with(&self.root),
            "destructive test path resolves outside its fresh sandbox: {path:?} -> {canonical_anchor:?}"
        );
    }

    fn create_dir_all(&self, relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        let destination = self.path(relative);
        let mut current = self.root.clone();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                unreachable!("path() already rejected this component")
            };
            current.push(component);
            self.assert_resolves_within_sandbox(&current);
            if !current.exists() {
                fs::create_dir(&current).unwrap_or_else(|error| {
                    panic!("sandbox directory should be created at {current:?}: {error}")
                });
            }
        }
        destination
    }

    fn write_file(&self, relative: impl AsRef<Path>, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        self.assert_resolves_within_sandbox(&path);
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("sandbox file should have a parent: {path:?}"));
        assert!(
            parent.is_dir(),
            "sandbox parent must be created explicitly: {parent:?}"
        );
        fs::write(&path, bytes)
            .unwrap_or_else(|error| panic!("sandbox file should be written at {path:?}: {error}"));
        path
    }

    fn remove_file(&self, relative: impl AsRef<Path>) {
        let path = self.path(relative);
        self.assert_resolves_within_sandbox(&path);
        fs::remove_file(&path)
            .unwrap_or_else(|error| panic!("sandbox file should be removed at {path:?}: {error}"));
    }

    fn fingerprint(&self, path: &Path) -> FileFingerprint {
        self.assert_resolves_within_sandbox(path);
        WindowsPlatform
            .fingerprint(path, true, MAX_EXECUTION_FINGERPRINT_BYTES)
            .unwrap_or_else(|error| panic!("sandbox file should fingerprint at {path:?}: {error}"))
    }

    fn request(&self, source: &Path, destination: &Path) -> RenameRequest {
        self.assert_resolves_within_sandbox(source);
        self.assert_resolves_within_sandbox(destination);
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
                .unwrap_or_else(|| panic!("qualification fingerprints always request a digest")),
            maximum_hash_bytes: MAX_EXECUTION_FINGERPRINT_BYTES,
        }
    }

    fn rename(
        &self,
        platform: &WindowsPlatform,
        request: &RenameRequest,
    ) -> Result<RenameOutcome, PlatformError> {
        self.assert_resolves_within_sandbox(&request.source);
        self.assert_resolves_within_sandbox(&request.destination);
        platform.rename_same_volume_no_replace(request)
    }

    fn qualify_move(&self, source: &Path, destination: &Path, bytes: &[u8]) {
        self.assert_resolves_within_sandbox(source);
        self.assert_resolves_within_sandbox(destination);
        fs::write(source, bytes)
            .unwrap_or_else(|error| panic!("move source should be written at {source:?}: {error}"));
        let before = self.fingerprint(source);
        let request = self.request(source, destination);

        let outcome = self
            .rename(&WindowsPlatform, &request)
            .unwrap_or_else(|error| panic!("native no-replace move should succeed: {error}"));

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination)
                .unwrap_or_else(|error| panic!("move destination should be readable: {error}")),
            bytes
        );
        assert_eq!(
            outcome.observed_identity.object_key,
            before.native_identity.object_key
        );
        assert_eq!(
            self.fingerprint(destination).content_digest,
            before.content_digest
        );
    }

    fn open_exclusively(&self, path: &Path) -> File {
        self.assert_resolves_within_sandbox(path);
        OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap_or_else(|error| panic!("exclusive sandbox lock should open: {error}"))
    }

    fn make_read_only(&self, paths: &[PathBuf]) -> ReadOnlyGuard {
        assert!(!paths.is_empty());
        for path in paths {
            self.assert_resolves_within_sandbox(path);
        }
        let original_permissions = fs::metadata(&paths[0])
            .unwrap_or_else(|error| panic!("read-only source metadata should load: {error}"))
            .permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&paths[0], permissions)
            .unwrap_or_else(|error| panic!("read-only bit should be set: {error}"));
        ReadOnlyGuard {
            paths: paths.to_vec(),
            original_permissions,
        }
    }

    fn deny_delete(&self, path: &Path) -> DeleteDenyGuard {
        self.assert_resolves_within_sandbox(path);
        DeleteDenyGuard::install(path)
    }

    fn create_junction(
        &self,
        link_relative: impl AsRef<Path>,
        target_relative: impl AsRef<Path>,
    ) -> JunctionGuard {
        let link = self.path(link_relative);
        let target = self.path(target_relative);
        self.assert_resolves_within_sandbox(&link);
        self.assert_resolves_within_sandbox(&target);
        assert!(
            target.is_dir(),
            "junction target must be a sandbox directory"
        );
        fs::create_dir(&link)
            .unwrap_or_else(|error| panic!("junction placeholder should be created: {error}"));
        set_mount_point_reparse_data(&link, &target);
        let attributes = fs::symlink_metadata(&link)
            .unwrap_or_else(|error| panic!("junction metadata should load: {error}"))
            .file_attributes();
        assert_ne!(
            attributes & FILE_ATTRIBUTE_REPARSE_POINT,
            0,
            "qualification helper must create a real reparse point"
        );
        JunctionGuard { path: link }
    }

    fn try_file_symlink(
        &self,
        link_relative: impl AsRef<Path>,
        target_relative: impl AsRef<Path>,
    ) -> io::Result<SymlinkGuard> {
        let link = self.path(link_relative);
        let target = self.path(target_relative);
        self.assert_resolves_within_sandbox(&link);
        self.assert_resolves_within_sandbox(&target);
        assert!(target.is_file(), "symlink target must be a sandbox file");
        std::os::windows::fs::symlink_file(&target, &link)?;
        Ok(SymlinkGuard { path: link })
    }
}

struct ReadOnlyGuard {
    paths: Vec<PathBuf>,
    original_permissions: fs::Permissions,
}

impl Drop for ReadOnlyGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            if path.exists() {
                let _ = fs::set_permissions(path, self.original_permissions.clone());
            }
        }
    }
}

struct DeleteDenyGuard {
    path: OsString,
    active: bool,
}

impl DeleteDenyGuard {
    fn install(path: &Path) -> Self {
        let path = ordinary_win32_path(path);
        let output = run_icacls(&path, &["/deny", "*S-1-1-0:D"]).unwrap_or_else(|error| {
            panic!("icacls should install a sandbox-only deny ACE: {error}")
        });
        assert_command_succeeded("install sandbox-only deny ACE", &output);
        Self { path, active: true }
    }

    fn restore(mut self) {
        let output = run_icacls(&self.path, &["/remove:d", "*S-1-1-0"])
            .unwrap_or_else(|error| panic!("icacls should restore the sandbox ACL: {error}"));
        assert_command_succeeded("restore sandbox ACL", &output);
        self.active = false;
    }
}

impl Drop for DeleteDenyGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = run_icacls(&self.path, &["/remove:d", "*S-1-1-0"]);
        }
    }
}

struct JunctionGuard {
    path: PathBuf,
}

impl Drop for JunctionGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

struct SymlinkGuard {
    path: PathBuf,
}

impl Drop for SymlinkGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn run_icacls(path: &OsStr, arguments: &[&str]) -> io::Result<Output> {
    Command::new("icacls.exe")
        .arg(path)
        .args(arguments)
        .output()
}

fn assert_command_succeeded(operation: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{operation} failed (status {:?}): stdout={}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn ordinary_win32_path(path: &Path) -> OsString {
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let verbatim_prefix = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    if units.starts_with(&verbatim_prefix) {
        OsString::from_wide(&units[verbatim_prefix.len()..])
    } else {
        path.as_os_str().to_os_string()
    }
}

fn set_mount_point_reparse_data(link: &Path, target: &Path) {
    let canonical_target = target
        .canonicalize()
        .unwrap_or_else(|error| panic!("junction target should canonicalize: {error}"));
    let target_units = canonical_target
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    let verbatim_prefix = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    assert!(
        target_units.starts_with(&verbatim_prefix),
        "junction target must be a canonical local drive path"
    );
    let print_name = &target_units[verbatim_prefix.len()..];
    let mut substitute_name = vec![
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    substitute_name.extend_from_slice(print_name);

    let substitute_bytes = substitute_name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or_else(|| panic!("junction substitute name should fit a reparse buffer"));
    let print_bytes = print_name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or_else(|| panic!("junction print name should fit a reparse buffer"));
    let print_offset = substitute_bytes
        .checked_add(u16::try_from(size_of::<u16>()).unwrap_or(2))
        .unwrap_or_else(|| panic!("junction print offset should fit"));
    let path_buffer_bytes = usize::from(print_offset)
        .checked_add(usize::from(print_bytes))
        .and_then(|value| value.checked_add(size_of::<u16>()))
        .unwrap_or_else(|| panic!("junction path buffer should fit"));
    let reparse_data_length = 8_usize
        .checked_add(path_buffer_bytes)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or_else(|| panic!("junction reparse data should fit"));

    let mut buffer = Vec::with_capacity(8 + usize::from(reparse_data_length));
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&reparse_data_length.to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    buffer.extend_from_slice(&substitute_bytes.to_le_bytes());
    buffer.extend_from_slice(&print_offset.to_le_bytes());
    buffer.extend_from_slice(&print_bytes.to_le_bytes());
    for unit in &substitute_name {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    for unit in print_name {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    assert_eq!(buffer.len(), 8 + usize::from(reparse_data_length));

    let link_wide = link
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: `link_wide` is NUL-terminated and names the empty sandbox
    // directory that will become the junction.
    let handle = unsafe {
        CreateFileW(
            link_wide.as_ptr(),
            GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        panic!(
            "junction placeholder should open without traversal: {}",
            io::Error::last_os_error()
        );
    }
    // SAFETY: successful CreateFileW returned one owned handle.
    let directory = unsafe { File::from_raw_handle(handle as _) };
    let mut bytes_returned = 0_u32;
    let buffer_size = u32::try_from(buffer.len())
        .unwrap_or_else(|_| panic!("junction reparse buffer should fit u32"));
    // SAFETY: the directory handle is live, and `buffer` contains a complete
    // mount-point REPARSE_DATA_BUFFER for the exact byte length passed.
    let result = unsafe {
        DeviceIoControl(
            directory.as_raw_handle() as HANDLE,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast::<c_void>(),
            buffer_size,
            ptr::null_mut(),
            0,
            ptr::addr_of_mut!(bytes_returned),
            ptr::null_mut(),
        )
    };
    if result == 0 {
        panic!(
            "sandbox junction should be created with FSCTL_SET_REPARSE_POINT: {}",
            io::Error::last_os_error()
        );
    }
}

#[test]
fn standard_move_rename_and_move_plus_rename_preserve_native_identity() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("move/from");
    sandbox.create_dir_all("move/to");
    sandbox.create_dir_all("rename");
    sandbox.create_dir_all("combined/from");
    sandbox.create_dir_all("combined/to");

    sandbox.qualify_move(
        &sandbox.path("move/from/document.txt"),
        &sandbox.path("move/to/document.txt"),
        b"standard move",
    );
    sandbox.qualify_move(
        &sandbox.path("rename/original.txt"),
        &sandbox.path("rename/renamed.txt"),
        b"same-directory rename",
    );
    sandbox.qualify_move(
        &sandbox.path("combined/from/original.txt"),
        &sandbox.path("combined/to/final.txt"),
        b"move and rename",
    );
}

#[test]
fn case_only_rename_requires_and_supports_safe_staging() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("case");
    let source = sandbox.write_file("case/Report.txt", b"case-preserved");
    let case_only_destination = sandbox.path("case/report.txt");
    let direct = sandbox.request(&source, &case_only_destination);

    assert!(matches!(
        sandbox.rename(&WindowsPlatform, &direct),
        Err(PlatformError::DestinationExists)
    ));
    assert_eq!(
        fs::read(&source).unwrap_or_else(|error| panic!("source should remain: {error}")),
        b"case-preserved"
    );

    let staging = sandbox.path("case/.supremacy-case-stage");
    let stage_request = sandbox.request(&source, &staging);
    sandbox
        .rename(&WindowsPlatform, &stage_request)
        .unwrap_or_else(|error| panic!("case staging move should succeed: {error}"));
    let finish_request = sandbox.request(&staging, &case_only_destination);
    sandbox
        .rename(&WindowsPlatform, &finish_request)
        .unwrap_or_else(|error| panic!("staged case-only rename should succeed: {error}"));

    assert!(!staging.exists());
    assert_eq!(
        fs::read(&case_only_destination)
            .unwrap_or_else(|error| panic!("case-only destination should exist: {error}")),
        b"case-preserved"
    );
    let leaf = fs::read_dir(sandbox.path("case"))
        .unwrap_or_else(|error| panic!("case directory should enumerate: {error}"))
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("report.txt")
        })
        .unwrap_or_else(|| panic!("case-only destination should enumerate"));
    assert_eq!(leaf.file_name(), OsStr::new("report.txt"));
}

#[test]
fn case_only_rename_undo_restores_original_leaf() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("case-undo");
    let source = sandbox.write_file("case-undo/Report.txt", b"case-undo");
    let staging = sandbox.path("case-undo/.supremacy-case-stage");
    let renamed = sandbox.path("case-undo/report.txt");
    sandbox
        .rename(&WindowsPlatform, &sandbox.request(&source, &staging))
        .unwrap_or_else(|error| panic!("case undo staging should succeed: {error}"));
    sandbox
        .rename(&WindowsPlatform, &sandbox.request(&staging, &renamed))
        .unwrap_or_else(|error| panic!("case undo rename should succeed: {error}"));

    let undo_stage = sandbox.path("case-undo/.supremacy-case-undo");
    sandbox
        .rename(&WindowsPlatform, &sandbox.request(&renamed, &undo_stage))
        .unwrap_or_else(|error| panic!("case undo reverse staging should succeed: {error}"));
    let restored = sandbox.path("case-undo/Report.txt");
    sandbox
        .rename(&WindowsPlatform, &sandbox.request(&undo_stage, &restored))
        .unwrap_or_else(|error| panic!("case undo restore should succeed: {error}"));

    assert!(!undo_stage.exists());
    assert_eq!(
        fs::read(&restored).unwrap_or_else(|error| panic!("restored leaf should exist: {error}")),
        b"case-undo"
    );
    let leaf = fs::read_dir(sandbox.path("case-undo"))
        .unwrap_or_else(|error| panic!("case-undo directory should enumerate: {error}"))
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("report.txt")
        })
        .unwrap_or_else(|| panic!("restored leaf should enumerate"));
    assert_eq!(leaf.file_name(), OsStr::new("Report.txt"));
}

#[test]
fn verbatim_long_paths_move_without_win32_max_path_truncation() {
    let sandbox = NtfsSandbox::new();
    let deep = (0..5).fold(PathBuf::from("long"), |path, index| {
        path.join(format!("component-{index:02}-{}", "x".repeat(48)))
    });
    sandbox.create_dir_all(&deep);
    let source = sandbox.write_file(deep.join("long-source-name.txt"), b"long path");
    let destination = sandbox.path(deep.join("long-destination-name.txt"));
    assert!(
        source.as_os_str().encode_wide().count() > 260,
        "fixture must exceed legacy MAX_PATH: {source:?}"
    );

    let request = sandbox.request(&source, &destination);
    sandbox
        .rename(&WindowsPlatform, &request)
        .unwrap_or_else(|error| panic!("long-path move should succeed: {error}"));
    assert!(!source.exists());
    assert_eq!(
        fs::read(destination)
            .unwrap_or_else(|error| panic!("long-path destination should read: {error}")),
        b"long path"
    );
}

#[test]
fn sharing_violation_is_retryable_only_after_the_exclusive_lock_is_released() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("locked");
    let source = sandbox.write_file("locked/source.txt", b"locked source");
    let destination = sandbox.path("locked/destination.txt");
    let request = sandbox.request(&source, &destination);
    let lock = sandbox.open_exclusively(&source);

    let blocked = sandbox.rename(&WindowsPlatform, &request).unwrap_err();
    assert!(matches!(blocked, PlatformError::SharingViolation));
    assert!(blocked.retryable_before_mutation());
    assert!(source.is_file());
    assert!(!destination.exists());

    drop(lock);
    sandbox
        .rename(&WindowsPlatform, &request)
        .unwrap_or_else(|error| panic!("retry after releasing lock should succeed: {error}"));
    assert!(!source.exists());
    assert_eq!(
        fs::read(destination)
            .unwrap_or_else(|error| panic!("retried destination should read: {error}")),
        b"locked source"
    );
}

#[test]
fn sandbox_only_delete_deny_acl_is_reported_as_permission_denied() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("acl");
    let source = sandbox.write_file("acl/source.txt", b"ACL protected");
    let destination = sandbox.path("acl/destination.txt");
    let request = sandbox.request(&source, &destination);
    let deny = sandbox.deny_delete(&source);

    assert!(matches!(
        sandbox.rename(&WindowsPlatform, &request),
        Err(PlatformError::PermissionDenied)
    ));
    assert!(source.is_file());
    assert!(!destination.exists());

    deny.restore();
    assert_eq!(
        fs::read(source)
            .unwrap_or_else(|error| panic!("ACL-protected source should remain: {error}")),
        b"ACL protected"
    );
}

#[test]
fn read_only_source_is_refused_without_clearing_its_attribute() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("read-only");
    let source = sandbox.write_file("read-only/source.txt", b"read only");
    let destination = sandbox.path("read-only/destination.txt");
    let guard = sandbox.make_read_only(&[source.clone(), destination.clone()]);
    let before = fs::metadata(&source)
        .unwrap_or_else(|error| panic!("read-only metadata should load: {error}"))
        .file_attributes();
    let request = sandbox.request(&source, &destination);

    assert!(matches!(
        sandbox.rename(&WindowsPlatform, &request),
        Err(PlatformError::PermissionDenied)
    ));
    let after = fs::metadata(&source)
        .unwrap_or_else(|error| panic!("read-only source should remain: {error}"))
        .file_attributes();
    assert_eq!(before, after);
    assert!(!destination.exists());
    drop(guard);
}

#[test]
fn exact_and_case_insensitive_destination_collisions_never_overwrite() {
    for (existing_name, requested_name) in [
        ("occupied.txt", "occupied.txt"),
        ("Occupied.TXT", "occupied.txt"),
    ] {
        let sandbox = NtfsSandbox::new();
        sandbox.create_dir_all("collision");
        let source = sandbox.write_file("collision/source.txt", b"source");
        let existing = sandbox.write_file(
            Path::new("collision").join(existing_name),
            b"existing destination",
        );
        let destination = sandbox.path(Path::new("collision").join(requested_name));
        let request = sandbox.request(&source, &destination);

        assert!(matches!(
            sandbox.rename(&WindowsPlatform, &request),
            Err(PlatformError::DestinationExists)
        ));
        assert_eq!(
            fs::read(&source).unwrap_or_else(|error| panic!("source should remain: {error}")),
            b"source"
        );
        assert_eq!(
            fs::read(&existing)
                .unwrap_or_else(|error| panic!("existing destination should remain: {error}")),
            b"existing destination"
        );
    }
}

#[test]
fn junction_leaf_ancestor_and_destination_escape_are_refused_without_traversal() {
    let sandbox = NtfsSandbox::new();
    let registered = sandbox.create_dir_all("registered");
    sandbox.create_dir_all("outside");
    let protected = sandbox.write_file("outside/protected.txt", b"outside target");
    let junction = sandbox.create_junction("registered/escape", "outside");
    let junction_path = sandbox.path("registered/escape");

    assert!(matches!(
        WindowsPlatform.fingerprint(&junction_path, true, MAX_EXECUTION_FINGERPRINT_BYTES),
        Err(PlatformError::ReparsePoint)
    ));
    assert!(matches!(
        WindowsPlatform.inspect_regular_file(&registered, Path::new("escape/protected.txt")),
        Err(PlatformError::ReparsePoint)
    ));

    let enumeration = WindowsPlatform
        .enumerate_regular_files(&registered, 32, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("registered root should enumerate: {error}"));
    assert!(
        enumeration
            .files
            .iter()
            .all(|entry| entry.absolute_path != protected)
    );
    assert!(enumeration.issues.iter().any(|issue| {
        issue.path == junction_path && matches!(&issue.error, PlatformError::ReparsePoint)
    }));

    let source = sandbox.write_file("registered/source.txt", b"source");
    let escaped_destination = sandbox.path("registered/escape/created.txt");
    let request = sandbox.request(&source, &escaped_destination);
    assert!(matches!(
        sandbox.rename(&WindowsPlatform, &request),
        Err(PlatformError::ReparsePoint)
    ));
    assert!(source.is_file());
    assert!(!sandbox.path("outside/created.txt").exists());
    assert_eq!(
        fs::read(&protected)
            .unwrap_or_else(|error| panic!("junction target should remain unchanged: {error}")),
        b"outside target"
    );

    drop(junction);
}

#[test]
fn file_symlink_leaf_is_refused_when_windows_policy_allows_fixture_creation() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("symlink");
    let target = sandbox.write_file("symlink/target.txt", b"symlink target");
    let link = sandbox.path("symlink/link.txt");

    let guard = match sandbox.try_file_symlink("symlink/link.txt", "symlink/target.txt") {
        Ok(guard) => guard,
        Err(error)
            if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD)
                || error.kind() == io::ErrorKind::PermissionDenied =>
        {
            eprintln!(
                "file-symlink subcase not qualified: Windows Developer Mode or symlink privilege is unavailable ({error})"
            );
            return;
        }
        Err(error) => panic!("sandbox file symlink should be created: {error}"),
    };

    assert!(matches!(
        WindowsPlatform.fingerprint(&link, true, MAX_EXECUTION_FINGERPRINT_BYTES),
        Err(PlatformError::ReparsePoint)
    ));
    assert_eq!(
        fs::read(&target)
            .unwrap_or_else(|error| panic!("symlink target should remain unchanged: {error}")),
        b"symlink target"
    );
    drop(guard);
}

#[test]
fn fresh_adapter_reconciles_committed_facts_and_round_trip_restores_original_path() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("recovery");
    let source = sandbox.write_file("recovery/original.txt", b"round trip");
    let destination = sandbox.path("recovery/committed.txt");
    let original = sandbox.fingerprint(&source);
    let forward = sandbox.request(&source, &destination);
    sandbox
        .rename(&WindowsPlatform, &forward)
        .unwrap_or_else(|error| panic!("forward rename should commit: {error}"));

    // WindowsPlatform has no in-memory transaction state. A fresh instance
    // therefore models the facts available after a coordinator restart.
    let recovered = WindowsPlatform;
    assert!(matches!(
        recovered.fingerprint(&source, true, MAX_EXECUTION_FINGERPRINT_BYTES),
        Err(PlatformError::SourceMissing)
    ));
    let committed = recovered
        .fingerprint(&destination, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("committed destination should reconcile: {error}"));
    assert_eq!(
        committed.native_identity.object_key,
        original.native_identity.object_key
    );
    assert_eq!(committed.content_digest, original.content_digest);

    let reverse = sandbox.request(&destination, &source);
    sandbox
        .rename(&recovered, &reverse)
        .unwrap_or_else(|error| panic!("round-trip rollback should succeed: {error}"));
    assert!(matches!(
        WindowsPlatform.fingerprint(&destination, true, MAX_EXECUTION_FINGERPRINT_BYTES),
        Err(PlatformError::SourceMissing)
    ));
    let restored = sandbox.fingerprint(&source);
    assert_eq!(
        restored.native_identity.object_key,
        original.native_identity.object_key
    );
    assert_eq!(restored.content_digest, original.content_digest);
}

#[test]
fn source_disappearance_before_mutation_is_structured_and_leaves_destination_absent() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("missing");
    let source = sandbox.write_file("missing/source.txt", b"will disappear");
    let destination = sandbox.path("missing/destination.txt");
    let request = sandbox.request(&source, &destination);
    sandbox.remove_file("missing/source.txt");

    assert!(matches!(
        sandbox.rename(&WindowsPlatform, &request),
        Err(PlatformError::SourceMissing)
    ));
    assert!(!destination.exists());
}

#[test]
fn source_content_drift_before_mutation_is_refused_without_overwrite() {
    let sandbox = NtfsSandbox::new();
    sandbox.create_dir_all("drift");
    let source = sandbox.write_file("drift/source.txt", b"original");
    let destination = sandbox.path("drift/destination.txt");
    let request = sandbox.request(&source, &destination);
    fs::write(&source, b"changed-after-fingerprint")
        .unwrap_or_else(|error| panic!("source drift write should succeed: {error}"));

    assert!(matches!(
        sandbox.rename(&WindowsPlatform, &request),
        Err(PlatformError::Precondition(_))
    ));
    assert!(!destination.exists(), "drift must not create a destination");
    assert_eq!(
        fs::read(&source).unwrap_or_else(|error| panic!("drifted source should remain: {error}")),
        b"changed-after-fingerprint"
    );
}
