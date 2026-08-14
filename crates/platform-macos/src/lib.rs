#![cfg(target_os = "macos")]
#![cfg_attr(
    not(feature = "mutation"),
    doc = r#"
The default build is read-only; mutation methods are not compiled into the
desktop process:

```compile_fail
use platform::{RenameRequest, SafeFileOperations};
use platform_macos::MacOsPlatform;

fn cannot_mutate(platform: &MacOsPlatform, request: &RenameRequest) {
    let _ = platform.rename_same_volume_no_replace(request);
}
```
"#
)]

/// Capability marker for packaging and dependency-graph tests.
pub const MUTATION_CAPABILITY_COMPILED: bool = cfg!(feature = "mutation");

use domain::{
    FileFingerprint, NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity,
};
use platform::{
    EnumerationIssue, EnumerationProgress, FingerprintProgress, MAX_EXECUTION_FINGERPRINT_BYTES,
    PlatformError, ReadOnlyEntry, ReadOnlyEnumeration, ReadOnlyPlatform,
    STREAMING_FINGERPRINT_BUFFER_BYTES,
};
#[cfg(feature = "mutation")]
use platform::{RenameOutcome, RenameRequest, SafeFileOperations};
#[cfg(feature = "mutation")]
use std::os::fd::IntoRawFd;
use std::{
    collections::VecDeque,
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    mem::MaybeUninit,
    os::darwin::fs::MetadataExt as DarwinMetadataExt,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
    path::{Component, Path},
};

#[derive(Debug, Default)]
pub struct MacOsPlatform;

impl MacOsPlatform {
    fn native_path(path: &Path) -> NativePath {
        NativePath {
            encoding: PathEncoding::UnixBytes,
            bytes: path.as_os_str().as_bytes().to_vec(),
        }
    }

    fn identity_with_parent_metadata(
        path: &Path,
        metadata: &fs::Metadata,
        parent_metadata: &fs::Metadata,
        filesystem_type: Option<String>,
    ) -> Result<NativeFileIdentity, PlatformError> {
        let leaf_name = path
            .file_name()
            .ok_or_else(|| PlatformError::Unsupported("file entry has no leaf name".to_owned()))?;

        Ok(NativeFileIdentity {
            volume: VolumeIdentity {
                platform: PlatformKind::MacOs,
                stable_identifier: format!("dev:{}", metadata.dev()),
                filesystem_type,
                case_sensitive: false,
                removable: false,
                local: true,
            },
            object_key: metadata.ino().to_le_bytes().to_vec(),
            parent_key: parent_metadata.ino().to_le_bytes().to_vec(),
            leaf_name: NativePath {
                encoding: PathEncoding::UnixBytes,
                bytes: leaf_name.as_bytes().to_vec(),
            },
            link_count: u32::try_from(metadata.nlink()).unwrap_or(u32::MAX),
            reparse_tag: None,
        })
    }

    fn identity(path: &Path, metadata: &fs::Metadata) -> Result<NativeFileIdentity, PlatformError> {
        let parent = path.parent().ok_or_else(|| {
            PlatformError::Unsupported("the filesystem root is not a file entry".to_owned())
        })?;
        let parent = Self::open_directory_no_follow(parent)?;
        let parent_metadata = parent.metadata()?;
        let filesystem_type = Self::local_filesystem_type(&parent)?;
        Self::identity_with_parent_metadata(path, metadata, &parent_metadata, filesystem_type)
    }

    fn identity_from_open_file(
        path: &Path,
        metadata: &fs::Metadata,
        file: &File,
    ) -> Result<NativeFileIdentity, PlatformError> {
        let parent = path.parent().ok_or_else(|| {
            PlatformError::Unsupported("the filesystem root is not a file entry".to_owned())
        })?;
        let parent_metadata = fs::symlink_metadata(parent)?;
        let filesystem_type = Self::local_filesystem_type(file)?;
        Self::identity_with_parent_metadata(path, metadata, &parent_metadata, filesystem_type)
    }

    fn open_regular_no_follow(path: &Path) -> Result<(File, fs::Metadata), PlatformError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(PlatformError::Unsupported(
                "only regular files are analyzable".to_owned(),
            ));
        }
        Ok((file, metadata))
    }

    fn metadata_ns(seconds: i64, nanoseconds: i64) -> Option<i128> {
        (seconds >= 0 && nanoseconds >= 0)
            .then(|| i128::from(seconds) * 1_000_000_000_i128 + i128::from(nanoseconds))
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

    fn validated_target_components(relative_path: &Path) -> Result<Vec<&OsStr>, PlatformError> {
        if relative_path.as_os_str().is_empty()
            || relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(PlatformError::OutsideRoot);
        }
        Ok(relative_path
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value),
                _ => None,
            })
            .collect())
    }

    fn anchored_open_error(error: io::Error) -> PlatformError {
        match error.raw_os_error() {
            Some(libc::ELOOP) => PlatformError::ReparsePoint,
            Some(libc::ENOENT) => PlatformError::SourceMissing,
            Some(libc::EACCES) | Some(libc::EPERM) => PlatformError::PermissionDenied,
            Some(libc::ENOTDIR) => PlatformError::Unsupported(
                "an intermediate path component is not a directory".to_owned(),
            ),
            _ => PlatformError::Io(error),
        }
    }

    fn open_directory_no_follow(path: &Path) -> Result<File, PlatformError> {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => Ok(file),
            Err(error)
                if error.raw_os_error() == Some(libc::ENOTDIR)
                    && fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink()) =>
            {
                Err(PlatformError::ReparsePoint)
            }
            Err(error) => Err(Self::anchored_open_error(error)),
        }
    }

    fn anchored_component_error(
        parent: &File,
        component: &CString,
        error: io::Error,
    ) -> PlatformError {
        if error.raw_os_error() == Some(libc::ENOTDIR) {
            let mut status = MaybeUninit::<libc::stat>::zeroed();
            // SAFETY: `status` is writable storage, and both the directory
            // descriptor and NUL-terminated component remain live.
            let result = unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    component.as_ptr(),
                    status.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                // SAFETY: successful `fstatat` initialized the structure.
                let status = unsafe { status.assume_init() };
                if status.st_mode & libc::S_IFMT == libc::S_IFLNK {
                    return PlatformError::ReparsePoint;
                }
            }
        }
        Self::anchored_open_error(error)
    }

    fn openat_no_follow(
        parent: &File,
        component: &OsStr,
        directory: bool,
    ) -> Result<File, PlatformError> {
        let component =
            CString::new(component.as_bytes()).map_err(|_| PlatformError::OutsideRoot)?;
        let mut flags = libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_RDONLY;
        if directory {
            flags |= libc::O_DIRECTORY;
        }
        // SAFETY: `parent` owns a live directory descriptor and `component` is
        // NUL-terminated. No creation flags are used, so no mode is required.
        let descriptor = unsafe { libc::openat(parent.as_raw_fd(), component.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(Self::anchored_component_error(
                parent,
                &component,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: `openat` returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn open_anchored_regular(
        root: &Path,
        relative_path: &Path,
    ) -> Result<(File, fs::Metadata, fs::Metadata), PlatformError> {
        let components = Self::validated_target_components(relative_path)?;
        let (leaf, directories) = components.split_last().ok_or(PlatformError::OutsideRoot)?;
        let mut parent = Self::open_directory_no_follow(root)?;
        for component in directories {
            parent = Self::openat_no_follow(&parent, component, true)?;
        }
        let parent_metadata = parent.metadata()?;
        let file = Self::openat_no_follow(&parent, leaf, false)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(PlatformError::Unsupported(
                "only regular files are analyzable".to_owned(),
            ));
        }
        Ok((file, metadata, parent_metadata))
    }

    fn filesystem_type(raw_name: &[libc::c_char]) -> Option<String> {
        let bytes = raw_name
            .iter()
            .copied()
            .take_while(|value| *value != 0)
            .map(|value| value as u8)
            .collect::<Vec<_>>();
        (!bytes.is_empty())
            .then(|| String::from_utf8(bytes).ok())
            .flatten()
    }

    fn validate_local_mount(
        flags: u64,
        filesystem_type: Option<String>,
    ) -> Result<Option<String>, PlatformError> {
        if flags & (libc::MNT_LOCAL as u64) == 0 {
            return Err(PlatformError::Unsupported(
                "registered root is not on a confirmed local filesystem".to_owned(),
            ));
        }
        Ok(filesystem_type)
    }

    fn local_filesystem_type(file: &File) -> Result<Option<String>, PlatformError> {
        let mut statistics = MaybeUninit::<libc::statfs>::zeroed();
        // SAFETY: `statistics` is writable storage for `statfs`, and `file`
        // owns a live descriptor for the registered root.
        if unsafe { libc::fstatfs(file.as_raw_fd(), statistics.as_mut_ptr()) } != 0 {
            return Err(PlatformError::Io(io::Error::last_os_error()));
        }
        // SAFETY: successful `fstatfs` initialized the complete structure.
        let statistics = unsafe { statistics.assume_init() };
        Self::validate_local_mount(
            statistics.f_flags as u64,
            Self::filesystem_type(&statistics.f_fstypename),
        )
    }

    fn read_only_entry(
        path: &Path,
        relative_path: &Path,
        metadata: &fs::Metadata,
    ) -> Result<ReadOnlyEntry, PlatformError> {
        Self::read_only_entry_with_parent(path, relative_path, metadata, None)
    }

    fn read_only_entry_with_parent(
        path: &Path,
        relative_path: &Path,
        metadata: &fs::Metadata,
        parent_metadata: Option<&fs::Metadata>,
    ) -> Result<ReadOnlyEntry, PlatformError> {
        let hidden = path
            .file_name()
            .is_some_and(|name| name.as_bytes().first() == Some(&b'.'));
        let identity = if let Some(parent_metadata) = parent_metadata {
            let parent = path.parent().ok_or_else(|| {
                PlatformError::Unsupported("file entry has no parent directory".to_owned())
            })?;
            let parent = Self::open_directory_no_follow(parent)?;
            let filesystem_type = Self::local_filesystem_type(&parent)?;
            Self::identity_with_parent_metadata(path, metadata, parent_metadata, filesystem_type)?
        } else {
            Self::identity(path, metadata)?
        };
        Ok(ReadOnlyEntry {
            absolute_path: path.to_path_buf(),
            relative_path: Self::native_path(relative_path),
            identity,
            byte_size: metadata.len(),
            modified_at_ns: Self::metadata_ns(metadata.mtime(), metadata.mtime_nsec()),
            created_at_ns: Self::metadata_ns(metadata.st_birthtime(), metadata.st_birthtime_nsec()),
            accessed_at_ns: Self::metadata_ns(metadata.atime(), metadata.atime_nsec()),
            attributes: metadata.mode().into(),
            read_only: metadata.permissions().readonly(),
            hidden,
            cloud_placeholder: false,
            encrypted: false,
        })
    }

    fn hash_open_file(
        file: &mut File,
        expected_size: u64,
        max_bytes: u64,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(FingerprintProgress),
    ) -> Result<[u8; 32], PlatformError> {
        if max_bytes > MAX_EXECUTION_FINGERPRINT_BYTES {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: MAX_EXECUTION_FINGERPRINT_BYTES,
            });
        }
        if expected_size > max_bytes {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: max_bytes,
            });
        }
        if is_cancelled() {
            return Err(PlatformError::Cancelled);
        }
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; STREAMING_FINGERPRINT_BUFFER_BYTES];
        let mut observed = 0_u64;
        on_progress(FingerprintProgress {
            bytes_hashed: 0,
            total_bytes: expected_size,
        });
        loop {
            if is_cancelled() {
                return Err(PlatformError::Cancelled);
            }
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            observed = observed.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            if observed > expected_size {
                return Err(PlatformError::Precondition(
                    "source grew while it was being hashed".to_owned(),
                ));
            }
            hasher.update(&buffer[..count]);
            on_progress(FingerprintProgress {
                bytes_hashed: observed,
                total_bytes: expected_size,
            });
        }
        if observed != expected_size {
            return Err(PlatformError::Precondition(
                "source size changed while it was being hashed".to_owned(),
            ));
        }
        Ok(*hasher.finalize().as_bytes())
    }

    #[cfg(feature = "mutation")]
    fn is_supported_macos_filesystem(filesystem_type: Option<&str>) -> bool {
        filesystem_type.is_some_and(|value| {
            value.eq_ignore_ascii_case("apfs") || value.eq_ignore_ascii_case("hfs")
        })
    }

    #[cfg(feature = "mutation")]
    fn cstring_component(component: &OsStr) -> Result<CString, PlatformError> {
        CString::new(component.as_bytes()).map_err(|_| PlatformError::OutsideRoot)
    }

    #[cfg(feature = "mutation")]
    fn is_symlink_mode(mode: libc::mode_t) -> bool {
        mode & libc::S_IFMT == libc::S_IFLNK
    }

    #[cfg(feature = "mutation")]
    fn is_directory_mode(mode: libc::mode_t) -> bool {
        mode & libc::S_IFMT == libc::S_IFDIR
    }

    #[cfg(feature = "mutation")]
    fn is_regular_file_mode(mode: libc::mode_t) -> bool {
        mode & libc::S_IFMT == libc::S_IFREG
    }

    #[cfg(feature = "mutation")]
    fn fstatat_nofollow(parent: &File, component: &CString) -> Result<libc::stat, PlatformError> {
        let mut status = MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: `parent` owns a live directory descriptor, `component` is
        // NUL-terminated, and `status` is writable storage.
        let result = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                component.as_ptr(),
                status.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(Self::mutation_io_error(io::Error::last_os_error(), false));
        }
        // SAFETY: successful `fstatat` initialized the structure.
        Ok(unsafe { status.assume_init() })
    }

    #[cfg(feature = "mutation")]
    fn openat_maybe_follow(
        parent: &File,
        component: &OsStr,
        directory: bool,
        follow: bool,
    ) -> Result<File, PlatformError> {
        let component = Self::cstring_component(component)?;
        let mut flags = libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_RDONLY;
        if directory {
            flags |= libc::O_DIRECTORY;
        }
        if !follow {
            flags |= libc::O_NOFOLLOW;
        }
        // SAFETY: `parent` owns a live directory descriptor and `component` is
        // NUL-terminated. No creation flags are used.
        let descriptor = unsafe { libc::openat(parent.as_raw_fd(), component.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(Self::anchored_component_error(
                parent,
                &component,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: `openat` returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(feature = "mutation")]
    fn open_parent_chain(path: &Path) -> Result<(File, CString), PlatformError> {
        let leaf = path
            .file_name()
            .ok_or_else(|| PlatformError::Unsupported("path has no leaf name".to_owned()))?;
        let parent = path
            .parent()
            .ok_or_else(|| PlatformError::Unsupported("path has no parent".to_owned()))?;
        if parent.as_os_str().is_empty() {
            return Err(PlatformError::OutsideRoot);
        }
        if parent
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return Err(PlatformError::OutsideRoot);
        }
        let mut current = Self::open_directory_no_follow(Path::new("/"))?;
        let mut at_filesystem_root = true;
        for component in parent.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            let name_c = Self::cstring_component(name)?;
            match Self::fstatat_nofollow(&current, &name_c) {
                Ok(status) if Self::is_symlink_mode(status.st_mode) => {
                    if at_filesystem_root && matches!(name.as_bytes(), b"tmp" | b"var" | b"etc") {
                        current = Self::openat_maybe_follow(&current, name, true, true)?;
                        at_filesystem_root = false;
                        continue;
                    }
                    return Err(PlatformError::ReparsePoint);
                }
                Ok(status) if Self::is_directory_mode(status.st_mode) => {
                    current = Self::openat_maybe_follow(&current, name, true, false)?;
                    at_filesystem_root = false;
                }
                Ok(_) => {
                    return Err(PlatformError::Unsupported(
                        "an intermediate path component is not a directory".to_owned(),
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Ok((current, Self::cstring_component(leaf)?))
    }

    #[cfg(feature = "mutation")]
    fn leaf_absent(parent: &File, leaf: &CString) -> Result<(), PlatformError> {
        match Self::fstatat_nofollow(parent, leaf) {
            Ok(_) => Err(PlatformError::DestinationExists),
            Err(PlatformError::SourceMissing) => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[cfg(feature = "mutation")]
    fn mutation_io_error(error: io::Error, mutation_started: bool) -> PlatformError {
        if mutation_started {
            return PlatformError::AmbiguousMutationOutcome;
        }
        match error.raw_os_error() {
            Some(libc::EEXIST) => PlatformError::DestinationExists,
            Some(libc::ENOENT) => PlatformError::SourceMissing,
            Some(libc::EACCES | libc::EPERM | libc::EROFS) => PlatformError::PermissionDenied,
            Some(libc::EBUSY | libc::ETXTBSY) => PlatformError::SharingViolation,
            Some(libc::EAGAIN) => PlatformError::LockViolation,
            Some(libc::ENOSPC | libc::EDQUOT) => PlatformError::DiskFull,
            Some(libc::ELOOP) => PlatformError::ReparsePoint,
            Some(libc::EXDEV) => {
                PlatformError::Unsupported("cross-volume move is not implemented".to_owned())
            }
            Some(libc::ENOTEMPTY) => {
                PlatformError::Precondition("directory is not empty".to_owned())
            }
            Some(libc::EISDIR | libc::ENOTDIR) => PlatformError::Precondition(
                "path is not the expected file or directory type".to_owned(),
            ),
            _ => PlatformError::Io(error),
        }
    }

    #[cfg(feature = "mutation")]
    fn exact_identity_matches(
        expected: &NativeFileIdentity,
        observed: &NativeFileIdentity,
    ) -> bool {
        expected.volume.platform == observed.volume.platform
            && expected.volume.stable_identifier == observed.volume.stable_identifier
            && expected.volume.filesystem_type == observed.volume.filesystem_type
            && expected.volume.local == observed.volume.local
            && expected.volume.removable == observed.volume.removable
            && expected.object_key == observed.object_key
            && expected.parent_key == observed.parent_key
            && expected.leaf_name == observed.leaf_name
            && expected.link_count == observed.link_count
            && expected.reparse_tag == observed.reparse_tag
    }

    #[cfg(feature = "mutation")]
    fn approved_macos_source(identity: &NativeFileIdentity) -> Result<(), PlatformError> {
        if identity.link_count != 1
            || identity.reparse_tag.is_some()
            || identity.volume.platform != PlatformKind::MacOs
            || !identity.volume.local
            || identity.volume.removable
            || !Self::is_supported_macos_filesystem(identity.volume.filesystem_type.as_deref())
        {
            return Err(PlatformError::Precondition(
                "source identity is not a single-link local APFS/HFS file".to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(feature = "mutation")]
    fn directory_is_empty(directory: File) -> Result<(), PlatformError> {
        let raw = directory.into_raw_fd();
        // SAFETY: `raw` is an owned directory descriptor transferred to `fdopendir`.
        let stream = unsafe { libc::fdopendir(raw) };
        if stream.is_null() {
            // SAFETY: `fdopendir` failed, so this process still owns `raw`.
            unsafe {
                libc::close(raw);
            }
            return Err(Self::mutation_io_error(io::Error::last_os_error(), false));
        }
        let mut empty = true;
        loop {
            // SAFETY: Darwin exposes thread-local errno through `__error`.
            unsafe {
                *libc::__error() = 0;
            }
            // SAFETY: `stream` is a live `DIR*` from `fdopendir`.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let errno = io::Error::last_os_error();
                if errno.raw_os_error().unwrap_or(0) != 0 {
                    // SAFETY: `stream` still owns the directory descriptor.
                    unsafe {
                        libc::closedir(stream);
                    }
                    return Err(Self::mutation_io_error(errno, false));
                }
                break;
            }
            // SAFETY: `readdir` returned a valid directory entry.
            let name = unsafe { (*entry).d_name.as_ptr() };
            // SAFETY: `d_name` is a NUL-terminated kernel-provided string.
            let bytes = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
            if bytes != b"." && bytes != b".." {
                empty = false;
                break;
            }
        }
        // SAFETY: `stream` still owns the directory descriptor.
        unsafe {
            libc::closedir(stream);
        }
        if empty {
            Ok(())
        } else {
            Err(PlatformError::Precondition(
                "directory is not empty".to_owned(),
            ))
        }
    }
}

impl ReadOnlyPlatform for MacOsPlatform {
    fn inspect_volume(&self, root: &Path) -> Result<VolumeIdentity, PlatformError> {
        let root = Self::open_directory_no_follow(root)?;
        let metadata = root.metadata()?;
        let filesystem_type = Self::local_filesystem_type(&root)?;
        Ok(VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: format!("dev:{}", metadata.dev()),
            filesystem_type,
            case_sensitive: false,
            removable: false,
            local: true,
        })
    }

    fn enumerate_regular_files(
        &self,
        root: &Path,
        max_entries: usize,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(EnumerationProgress),
    ) -> Result<ReadOnlyEnumeration, PlatformError> {
        let root_metadata = fs::symlink_metadata(root)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(PlatformError::ReparsePoint);
        }

        let mut queue = VecDeque::from([root.to_path_buf()]);
        let mut output = ReadOnlyEnumeration::default();
        output.progress.directories_discovered = 1;
        let mut observed = 0_usize;

        'scan: while let Some(directory) = queue.pop_front() {
            if is_cancelled() {
                output.cancelled = true;
                break;
            }
            let candidates = match fs::read_dir(&directory) {
                Ok(candidates) => candidates,
                Err(error) => {
                    output.issues.push(EnumerationIssue {
                        path: directory,
                        error: PlatformError::Io(error),
                        is_directory: true,
                    });
                    output.progress.errors = output.progress.errors.saturating_add(1);
                    output.progress.skipped_items = output.progress.skipped_items.saturating_add(1);
                    on_progress(output.progress);
                    continue;
                }
            };
            for candidate in candidates {
                if is_cancelled() {
                    output.cancelled = true;
                    break 'scan;
                }
                if observed >= max_entries {
                    output.truncated = true;
                    output.progress.skipped_items = output.progress.skipped_items.saturating_add(1);
                    break 'scan;
                }
                observed += 1;
                output.progress.entries_discovered =
                    output.progress.entries_discovered.saturating_add(1);

                let entry = match candidate {
                    Ok(value) => value,
                    Err(error) => {
                        output.issues.push(EnumerationIssue {
                            path: directory.clone(),
                            error: PlatformError::Io(error),
                            is_directory: true,
                        });
                        output.progress.errors = output.progress.errors.saturating_add(1);
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
                        continue;
                    }
                };
                let path = entry.path();
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(value) => value,
                    Err(error) => {
                        output.issues.push(EnumerationIssue {
                            path,
                            error: PlatformError::Io(error),
                            is_directory: false,
                        });
                        output.progress.errors = output.progress.errors.saturating_add(1);
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
                        continue;
                    }
                };

                if metadata.file_type().is_symlink() {
                    output.issues.push(EnumerationIssue {
                        path,
                        error: PlatformError::ReparsePoint,
                        is_directory: false,
                    });
                    output.progress.skipped_items = output.progress.skipped_items.saturating_add(1);
                    continue;
                }
                if metadata.is_dir() {
                    queue.push_back(path);
                    output.progress.directories_discovered =
                        output.progress.directories_discovered.saturating_add(1);
                    continue;
                }
                if !metadata.is_file() {
                    output.issues.push(EnumerationIssue {
                        path,
                        error: PlatformError::Unsupported(
                            "special filesystem entry was skipped".to_owned(),
                        ),
                        is_directory: false,
                    });
                    output.progress.skipped_items = output.progress.skipped_items.saturating_add(1);
                    continue;
                }

                let relative = match path.strip_prefix(root) {
                    Ok(value) => value,
                    Err(_) => {
                        output.issues.push(EnumerationIssue {
                            path,
                            error: PlatformError::OutsideRoot,
                            is_directory: false,
                        });
                        output.progress.errors = output.progress.errors.saturating_add(1);
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
                        continue;
                    }
                };
                let read_only_entry = match Self::read_only_entry(&path, relative, &metadata) {
                    Ok(value) => value,
                    Err(error) => {
                        output.issues.push(EnumerationIssue {
                            path,
                            error,
                            is_directory: false,
                        });
                        output.progress.errors = output.progress.errors.saturating_add(1);
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
                        continue;
                    }
                };

                output.progress.files_discovered =
                    output.progress.files_discovered.saturating_add(1);
                output.progress.bytes_discovered = output
                    .progress
                    .bytes_discovered
                    .saturating_add(metadata.len());
                output.files.push(read_only_entry);
                if observed.is_multiple_of(128) {
                    on_progress(output.progress);
                }
            }
        }

        on_progress(output.progress);
        Ok(output)
    }

    fn inspect_regular_file(
        &self,
        root: &Path,
        relative_path: &Path,
    ) -> Result<ReadOnlyEntry, PlatformError> {
        let target = root.join(relative_path);
        let (_file, metadata, parent_metadata) =
            Self::open_anchored_regular(root, relative_path).map_err(Self::inspection_error)?;
        Self::read_only_entry_with_parent(&target, relative_path, &metadata, Some(&parent_metadata))
            .map_err(Self::inspection_error)
    }

    fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
        let (file, metadata) = Self::open_regular_no_follow(path)?;
        if metadata.len() > max_bytes {
            return Err(PlatformError::Unsupported(format!(
                "file exceeds the {max_bytes}-byte analysis budget"
            )));
        }

        let capacity = usize::try_from(metadata.len()).unwrap_or(0);
        let mut buffer = Vec::with_capacity(capacity);
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut buffer)?;
        if u64::try_from(buffer.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(PlatformError::Unsupported(
                "file changed while being read or exceeds its budget".to_owned(),
            ));
        }
        Ok(buffer)
    }

    fn read_prefix(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
        let (file, _) = Self::open_regular_no_follow(path)?;
        let mut buffer = Vec::with_capacity(max_bytes);
        file.take(u64::try_from(max_bytes).unwrap_or(u64::MAX))
            .read_to_end(&mut buffer)?;
        Ok(buffer)
    }

    fn fingerprint(
        &self,
        path: &Path,
        include_content_digest: bool,
        max_bytes: u64,
    ) -> Result<FileFingerprint, PlatformError> {
        self.fingerprint_streaming(
            path,
            include_content_digest,
            max_bytes.min(MAX_EXECUTION_FINGERPRINT_BYTES),
            &|| false,
            &mut |_| {},
        )
    }

    fn fingerprint_streaming(
        &self,
        path: &Path,
        include_content_digest: bool,
        max_bytes: u64,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(FingerprintProgress),
    ) -> Result<FileFingerprint, PlatformError> {
        let (mut file, metadata) = Self::open_regular_no_follow(path)?;
        if max_bytes > MAX_EXECUTION_FINGERPRINT_BYTES {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: MAX_EXECUTION_FINGERPRINT_BYTES,
            });
        }
        if metadata.len() > max_bytes {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: max_bytes.min(MAX_EXECUTION_FINGERPRINT_BYTES),
            });
        }
        let before_identity = Self::identity_from_open_file(path, &metadata, &file)?;
        let content_digest = if include_content_digest {
            Some(Self::hash_open_file(
                &mut file,
                metadata.len(),
                max_bytes,
                is_cancelled,
                on_progress,
            )?)
        } else {
            if is_cancelled() {
                return Err(PlatformError::Cancelled);
            }
            on_progress(FingerprintProgress {
                bytes_hashed: 0,
                total_bytes: metadata.len(),
            });
            None
        };
        let after = file.metadata()?;
        if after.len() != metadata.len()
            || after.mtime() != metadata.mtime()
            || after.mtime_nsec() != metadata.mtime_nsec()
            || after.dev() != metadata.dev()
            || after.ino() != metadata.ino()
            || after.nlink() != metadata.nlink()
        {
            return Err(PlatformError::Precondition(
                "source changed while it was being fingerprinted".to_owned(),
            ));
        }
        let after_identity = Self::identity_from_open_file(path, &after, &file)?;
        if before_identity != after_identity {
            return Err(PlatformError::Precondition(
                "source identity changed while it was being fingerprinted".to_owned(),
            ));
        }

        Ok(FileFingerprint {
            native_identity: after_identity,
            byte_size: after.len(),
            modified_at_ns: Self::metadata_ns(after.mtime(), after.mtime_nsec()),
            created_at_ns: Self::metadata_ns(after.st_birthtime(), after.st_birthtime_nsec()),
            attributes: after.mode().into(),
            quick_digest: None,
            content_digest,
        })
    }
}

#[cfg(feature = "mutation")]
impl SafeFileOperations for MacOsPlatform {
    fn validate_destination_absent(&self, path: &Path) -> Result<(), PlatformError> {
        let (parent, leaf) = Self::open_parent_chain(path)?;
        Self::leaf_absent(&parent, &leaf)
    }

    fn rename_same_volume_no_replace(
        &self,
        request: &RenameRequest,
    ) -> Result<RenameOutcome, PlatformError> {
        if request.maximum_hash_bytes == 0
            || request.maximum_hash_bytes > MAX_EXECUTION_FINGERPRINT_BYTES
            || request.expected_byte_size > request.maximum_hash_bytes
        {
            return Err(PlatformError::VerificationLimitExceeded {
                limit_bytes: request
                    .maximum_hash_bytes
                    .min(MAX_EXECUTION_FINGERPRINT_BYTES),
            });
        }
        Self::approved_macos_source(&request.expected_identity)?;
        let (source_parent, source_leaf) = Self::open_parent_chain(&request.source)?;
        let (destination_parent, destination_leaf) = Self::open_parent_chain(&request.destination)?;

        let source_status = Self::fstatat_nofollow(&source_parent, &source_leaf)?;
        if Self::is_symlink_mode(source_status.st_mode) {
            return Err(PlatformError::ReparsePoint);
        }
        if !Self::is_regular_file_mode(source_status.st_mode) {
            return Err(PlatformError::Precondition(
                "source is not a regular file".to_owned(),
            ));
        }
        if source_status.st_mode & 0o222 == 0 {
            return Err(PlatformError::PermissionDenied);
        }

        let mut source_file = Self::openat_maybe_follow(
            &source_parent,
            OsStr::from_bytes(source_leaf.as_bytes()),
            false,
            false,
        )?;
        let metadata = source_file.metadata()?;
        if !metadata.is_file() {
            return Err(PlatformError::Precondition(
                "source is not a regular file".to_owned(),
            ));
        }
        if metadata.permissions().readonly() {
            return Err(PlatformError::PermissionDenied);
        }

        let destination_parent_metadata = destination_parent.metadata()?;
        if metadata.dev() != destination_parent_metadata.dev() {
            return Err(PlatformError::Unsupported(
                "cross-volume move is not implemented".to_owned(),
            ));
        }
        let filesystem_type = Self::local_filesystem_type(&destination_parent)?;
        if !Self::is_supported_macos_filesystem(filesystem_type.as_deref())
            || format!("dev:{}", destination_parent_metadata.dev())
                != request.expected_identity.volume.stable_identifier
        {
            return Err(PlatformError::Precondition(
                "destination parent volume is not the approved local APFS/HFS volume".to_owned(),
            ));
        }

        let observed_identity =
            Self::identity_from_open_file(&request.source, &metadata, &source_file)?;
        if !Self::exact_identity_matches(&request.expected_identity, &observed_identity) {
            return Err(PlatformError::Precondition(
                "source native identity changed".to_owned(),
            ));
        }
        if metadata.len() != request.expected_byte_size
            || Self::metadata_ns(metadata.mtime(), metadata.mtime_nsec())
                != request.expected_modified_at_ns
            || u64::from(metadata.mode()) != request.expected_attributes
        {
            return Err(PlatformError::Precondition(
                "source size or modified time changed".to_owned(),
            ));
        }
        let observed_digest = Self::hash_open_file(
            &mut source_file,
            request.expected_byte_size,
            request.maximum_hash_bytes,
            &|| false,
            &mut |_| {},
        )?;
        if observed_digest != request.expected_content_digest {
            return Err(PlatformError::Precondition(
                "source content changed".to_owned(),
            ));
        }
        let after_hash = source_file.metadata()?;
        let after_hash_identity =
            Self::identity_from_open_file(&request.source, &after_hash, &source_file)?;
        if !Self::exact_identity_matches(&request.expected_identity, &after_hash_identity)
            || after_hash.len() != request.expected_byte_size
            || Self::metadata_ns(after_hash.mtime(), after_hash.mtime_nsec())
                != request.expected_modified_at_ns
            || u64::from(after_hash.mode()) != request.expected_attributes
        {
            return Err(PlatformError::Precondition(
                "source changed during final native verification".to_owned(),
            ));
        }

        match Self::fstatat_nofollow(&destination_parent, &destination_leaf) {
            Err(PlatformError::SourceMissing) => {}
            Err(error) => return Err(error),
            Ok(existing) => {
                if existing.st_dev as u64 == metadata.dev() && existing.st_ino == metadata.ino() {
                    return Err(PlatformError::Precondition(
                        "case-only rename requires authenticated staging".to_owned(),
                    ));
                }
                return Err(PlatformError::DestinationExists);
            }
        }

        const RENAME_EXCL: libc::c_uint = 0x0004;
        // SAFETY: both parents are live no-follow directory descriptors and
        // both leaves are NUL-terminated. RENAME_EXCL refuses replacement.
        let renamed = unsafe {
            libc::renameatx_np(
                source_parent.as_raw_fd(),
                source_leaf.as_ptr(),
                destination_parent.as_raw_fd(),
                destination_leaf.as_ptr(),
                RENAME_EXCL,
            )
        };
        if renamed != 0 {
            return Err(Self::mutation_io_error(io::Error::last_os_error(), false));
        }

        if Self::leaf_absent(&source_parent, &source_leaf).is_err() {
            return Err(PlatformError::AmbiguousMutationOutcome);
        }
        let destination_status = Self::fstatat_nofollow(&destination_parent, &destination_leaf)
            .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
        if Self::is_symlink_mode(destination_status.st_mode)
            || !Self::is_regular_file_mode(destination_status.st_mode)
            || destination_status.st_ino != metadata.ino()
            || destination_status.st_dev as u64 != metadata.dev()
            || u64::try_from(destination_status.st_size).unwrap_or(u64::MAX)
                != request.expected_byte_size
        {
            return Err(PlatformError::AmbiguousMutationOutcome);
        }
        let dest_metadata = source_file
            .metadata()
            .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
        let observed_identity =
            Self::identity_from_open_file(&request.destination, &dest_metadata, &source_file)
                .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
        if observed_identity.object_key != request.expected_identity.object_key
            || observed_identity.volume.stable_identifier
                != request.expected_identity.volume.stable_identifier
            || observed_identity.link_count != 1
            || observed_identity.reparse_tag.is_some()
            || observed_identity.leaf_name
                != Self::native_path(Path::new(OsStr::from_bytes(destination_leaf.as_bytes())))
            || dest_metadata.len() != request.expected_byte_size
        {
            return Err(PlatformError::AmbiguousMutationOutcome);
        }
        Ok(RenameOutcome { observed_identity })
    }

    fn create_directory_no_replace(&self, path: &Path) -> Result<(), PlatformError> {
        let (parent, leaf) = Self::open_parent_chain(path)?;
        Self::leaf_absent(&parent, &leaf)?;
        // SAFETY: `parent` is a live no-follow directory descriptor and `leaf`
        // is NUL-terminated. `mkdirat` never replaces an existing entry.
        let created = unsafe { libc::mkdirat(parent.as_raw_fd(), leaf.as_ptr(), 0o755) };
        if created != 0 {
            return Err(Self::mutation_io_error(io::Error::last_os_error(), false));
        }
        let status = Self::fstatat_nofollow(&parent, &leaf)
            .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
        if Self::is_symlink_mode(status.st_mode) || !Self::is_directory_mode(status.st_mode) {
            return Err(PlatformError::AmbiguousMutationOutcome);
        }
        Ok(())
    }

    fn remove_directory_if_empty(&self, path: &Path) -> Result<(), PlatformError> {
        let (parent, leaf) = Self::open_parent_chain(path)?;
        let status = Self::fstatat_nofollow(&parent, &leaf)?;
        if Self::is_symlink_mode(status.st_mode) {
            return Err(PlatformError::ReparsePoint);
        }
        if !Self::is_directory_mode(status.st_mode) {
            return Err(PlatformError::Precondition(
                "rollback path is not a directory".to_owned(),
            ));
        }
        let directory =
            Self::openat_maybe_follow(&parent, OsStr::from_bytes(leaf.as_bytes()), true, false)?;
        Self::directory_is_empty(directory)?;
        // SAFETY: `parent` is a live no-follow directory descriptor and `leaf`
        // is NUL-terminated. AT_REMOVEDIR refuses non-empty directories.
        let removed =
            unsafe { libc::unlinkat(parent.as_raw_fd(), leaf.as_ptr(), libc::AT_REMOVEDIR) };
        if removed != 0 {
            return Err(Self::mutation_io_error(io::Error::last_os_error(), false));
        }
        match Self::leaf_absent(&parent, &leaf) {
            Ok(()) => Ok(()),
            Err(_) => Err(PlatformError::AmbiguousMutationOutcome),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io::Write};

    #[cfg(not(feature = "mutation"))]
    #[test]
    fn mutation_capability_is_not_compiled() {
        const {
            assert!(!MUTATION_CAPABILITY_COMPILED);
        }
    }

    #[cfg(feature = "mutation")]
    #[test]
    fn mutation_capability_is_compiled_when_enabled() {
        const {
            assert!(MUTATION_CAPABILITY_COMPILED);
        }
    }

    #[test]
    fn mount_validation_fails_closed_and_preserves_filesystem_type() {
        assert!(matches!(
            MacOsPlatform::validate_local_mount(0, Some("network".to_owned())),
            Err(PlatformError::Unsupported(_))
        ));

        let mut raw_name = [0 as libc::c_char; 16];
        for (target, source) in raw_name.iter_mut().zip(b"apfs") {
            *target = *source as libc::c_char;
        }
        let filesystem_type = MacOsPlatform::filesystem_type(&raw_name);
        assert_eq!(filesystem_type.as_deref(), Some("apfs"));
        let validated =
            MacOsPlatform::validate_local_mount(libc::MNT_LOCAL as u64, filesystem_type)
                .unwrap_or_else(|error| panic!("local mount should be accepted: {error}"));
        assert_eq!(validated.as_deref(), Some("apfs"));
    }

    #[test]
    fn volume_inspection_confirms_local_mount_and_exposes_type() {
        let fixture = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let volume = MacOsPlatform
            .inspect_volume(fixture.path())
            .unwrap_or_else(|error| panic!("local volume should be inspectable: {error}"));

        assert!(volume.local);
        assert!(
            volume
                .filesystem_type
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        );
    }

    #[test]
    fn targeted_inspection_is_scoped_and_read_only() {
        let fixture = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let nested = fixture.path().join("nested");
        fs::create_dir(&nested)
            .unwrap_or_else(|error| panic!("nested fixture should be created: {error}"));
        let file = nested.join("document.txt");
        fs::write(&file, b"unchanged")
            .unwrap_or_else(|error| panic!("fixture file should be created: {error}"));

        let entry = MacOsPlatform
            .inspect_regular_file(fixture.path(), Path::new("nested/document.txt"))
            .unwrap_or_else(|error| panic!("targeted inspection should succeed: {error}"));

        assert_eq!(entry.absolute_path, file);
        assert_eq!(entry.byte_size, 9);
        assert_eq!(
            fs::read(&entry.absolute_path)
                .unwrap_or_else(|error| panic!("inspection must not alter content: {error}")),
            b"unchanged"
        );
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new("../outside.txt")),
            Err(PlatformError::OutsideRoot)
        ));
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new("missing.txt")),
            Err(PlatformError::SourceMissing)
        ));
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new("nested")),
            Err(PlatformError::Unsupported(_))
        ));
    }

    #[test]
    fn targeted_inspection_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let outside = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        fs::write(outside.path().join("document.txt"), b"private")
            .unwrap_or_else(|error| panic!("outside file should be created: {error}"));
        symlink(outside.path(), fixture.path().join("escape"))
            .unwrap_or_else(|error| panic!("fixture symlink should be created: {error}"));
        symlink(
            outside.path().join("document.txt"),
            fixture.path().join("leaf-link.txt"),
        )
        .unwrap_or_else(|error| panic!("fixture symlink should be created: {error}"));
        let root_holder = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let root_link = root_holder.path().join("root-link");
        symlink(fixture.path(), &root_link)
            .unwrap_or_else(|error| panic!("root symlink should be created: {error}"));

        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new("escape/document.txt")),
            Err(PlatformError::ReparsePoint)
        ));
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new("leaf-link.txt")),
            Err(PlatformError::ReparsePoint)
        ));
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(&root_link, Path::new("leaf-link.txt")),
            Err(PlatformError::ReparsePoint)
        ));
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new("escape")),
            Err(PlatformError::ReparsePoint)
        ));
        assert!(matches!(
            MacOsPlatform.inspect_regular_file(fixture.path(), Path::new(".")),
            Err(PlatformError::OutsideRoot)
        ));
    }

    #[test]
    fn streaming_fingerprint_is_chunked_bounded_and_reports_progress() {
        assert_eq!(STREAMING_FINGERPRINT_BUFFER_BYTES, 1024 * 1024);
        let fixture = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let path = fixture.path().join("large.bin");
        let mut file = File::create(&path)
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        for byte in [0x11_u8, 0x22, 0x33] {
            file.write_all(&vec![byte; STREAMING_FINGERPRINT_BUFFER_BYTES])
                .unwrap_or_else(|error| panic!("fixture chunk should be written: {error}"));
        }
        drop(file);
        let exact_size = u64::try_from(3 * STREAMING_FINGERPRINT_BUFFER_BYTES)
            .unwrap_or_else(|_| panic!("fixture size should fit u64"));
        let mut progress = Vec::new();

        let fingerprint = MacOsPlatform
            .fingerprint_streaming(&path, true, exact_size, &|| false, &mut |value| {
                progress.push(value)
            })
            .unwrap_or_else(|error| panic!("exact-bound fingerprint should succeed: {error}"));

        assert_eq!(fingerprint.byte_size, exact_size);
        assert_eq!(progress.first().map(|value| value.bytes_hashed), Some(0));
        assert_eq!(
            progress.last().map(|value| value.bytes_hashed),
            Some(exact_size)
        );
        assert!(progress.windows(2).all(|pair| {
            pair[0].bytes_hashed <= pair[1].bytes_hashed
                && pair[0].total_bytes == exact_size
                && pair[1].total_bytes == exact_size
        }));
        assert!(matches!(
            MacOsPlatform.fingerprint_streaming(
                &path,
                true,
                exact_size - 1,
                &|| false,
                &mut |_| {}
            ),
            Err(PlatformError::VerificationLimitExceeded { limit_bytes })
                if limit_bytes == exact_size - 1
        ));
        assert!(matches!(
            MacOsPlatform.fingerprint_streaming(
                &path,
                true,
                MAX_EXECUTION_FINGERPRINT_BYTES + 1,
                &|| false,
                &mut |_| {}
            ),
            Err(PlatformError::VerificationLimitExceeded { limit_bytes })
                if limit_bytes == MAX_EXECUTION_FINGERPRINT_BYTES
        ));
    }

    #[test]
    fn streaming_fingerprint_honors_cancellation_between_chunks() {
        let fixture = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let path = fixture.path().join("cancel.bin");
        let mut file = File::create(&path)
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        file.write_all(&vec![0x44; STREAMING_FINGERPRINT_BUFFER_BYTES * 2])
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        drop(file);
        let cancelled = Cell::new(false);

        let result = MacOsPlatform.fingerprint_streaming(
            &path,
            true,
            u64::try_from(STREAMING_FINGERPRINT_BUFFER_BYTES * 2)
                .unwrap_or_else(|_| panic!("fixture size should fit")),
            &|| cancelled.get(),
            &mut |progress| {
                if progress.bytes_hashed > 0 {
                    cancelled.set(true);
                }
            },
        );

        assert!(matches!(result, Err(PlatformError::Cancelled)));
    }

    #[test]
    fn streaming_fingerprint_fails_closed_when_file_changes_mid_hash() {
        let fixture = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        let path = fixture.path().join("changing.bin");
        let mut file = File::create(&path)
            .unwrap_or_else(|error| panic!("fixture should be created: {error}"));
        file.write_all(&vec![0x55; STREAMING_FINGERPRINT_BUFFER_BYTES * 2])
            .unwrap_or_else(|error| panic!("fixture should be written: {error}"));
        drop(file);
        let changed = Cell::new(false);
        let mut mutation_error = None;

        let result = MacOsPlatform.fingerprint_streaming(
            &path,
            true,
            u64::try_from(STREAMING_FINGERPRINT_BUFFER_BYTES * 2)
                .unwrap_or_else(|_| panic!("fixture size should fit")),
            &|| false,
            &mut |progress| {
                if progress.bytes_hashed > 0 && !changed.replace(true) {
                    mutation_error = OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .and_then(|mut file| file.write_all(b"changed"))
                        .err();
                }
            },
        );

        assert!(
            mutation_error.is_none(),
            "test mutation should succeed: {mutation_error:?}"
        );
        assert!(matches!(result, Err(PlatformError::Precondition(_))));
    }
}
