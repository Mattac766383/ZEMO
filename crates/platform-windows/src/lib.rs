#![cfg_attr(
    not(feature = "mutation"),
    doc = r#"
The default build is read-only; mutation methods are not available:

```compile_fail
use platform::{SafeFileOperations, RenameRequest};
use platform_windows::WindowsPlatform;

fn cannot_mutate(platform: &WindowsPlatform, request: &RenameRequest) {
    let _ = platform.rename_same_volume_no_replace(request);
}
```
"#
)]

/// Capability marker for packaging and dependency-graph tests.
pub const MUTATION_CAPABILITY_COMPILED: bool = cfg!(feature = "mutation");

mod nt_create;
mod volume_root;

pub use nt_create::{
    DESTINATION_PARENT_RENAME_ACCESS, DIRECTORY_CREATE_OPTION_MASK, FILE_ADD_FILE,
    FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_NO_RECALL,
    FILE_OPEN_REPARSE_POINT, FILE_RENAME_REPLACE_IF_EXISTS, FILE_SYNCHRONOUS_IO_NONALERT,
    FILE_TRAVERSE, anchored_create_options, directory_create_options_are_legal,
    relative_object_name_is_legal, rename_flags_are_no_replace,
};
pub use volume_root::{
    ParsedWindowsDrivePrefix, format_win32_drive_root, is_legal_win32_mount_point,
    parse_windows_drive_prefix,
};

#[cfg(windows)]
mod windows {
    use crate::nt_create::anchored_create_options;
    use crate::volume_root::format_win32_drive_root;
    use domain::{
        FileFingerprint, NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity,
    };
    use platform::{
        EnumerationIssue, EnumerationProgress, FingerprintProgress,
        MAX_EXECUTION_FINGERPRINT_BYTES, PlatformError, ReadOnlyEntry, ReadOnlyEnumeration,
        ReadOnlyPlatform, STREAMING_FINGERPRINT_BUFFER_BYTES,
    };
    #[cfg(feature = "mutation")]
    use platform::{RenameOutcome, RenameRequest, SafeFileOperations};
    use std::{
        collections::VecDeque,
        ffi::{OsStr, OsString, c_void},
        fs::{self, File},
        io::{Read, Seek, SeekFrom},
        mem,
        os::windows::{
            ffi::OsStrExt,
            fs::MetadataExt,
            io::{AsRawHandle, FromRawHandle},
        },
        path::{Component, Path, PathBuf, Prefix},
        ptr,
    };
    #[cfg(feature = "mutation")]
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
    };
    use windows_sys::Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{FILE_OPEN, NtCreateFile},
    };
    #[cfg(feature = "mutation")]
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ADD_FILE, FILE_ATTRIBUTE_READONLY, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_INFO_EX, FILE_TRAVERSE, FileDispositionInfoEx, SetFileInformationByHandle,
    };
    use windows_sys::Win32::{
        Foundation::{
            GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE,
            RtlNtStatusToDosError, STATUS_REPARSE_POINT_ENCOUNTERED, UNICODE_STRING,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_ENCRYPTED,
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, FileCaseSensitiveInfo,
            FileIdInfo, GetDriveTypeW, GetFileInformationByHandle, GetFileInformationByHandleEx,
            GetVolumeInformationByHandleW, GetVolumeInformationW,
            GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, OPEN_EXISTING, SYNCHRONIZE,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            WindowsProgramming::{DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_REMOVABLE, DRIVE_UNKNOWN},
        },
    };

    #[derive(Debug, Default)]
    pub struct WindowsPlatform;

    #[derive(Debug, Clone)]
    pub struct VolumePathDiagnostics {
        pub input_path: String,
        pub absolute_path: String,
        pub canonical_path: String,
        pub prefix_kind: String,
        pub volume_root: String,
        pub dos_root: String,
        pub win32_root: String,
        pub win32_volume_path: String,
        pub utf16_len: usize,
        pub get_volume_path_name: String,
        pub get_volume_information: String,
        pub get_drive_type: String,
        pub filesystem_name: Option<String>,
        pub volume_identity: Option<String>,
        pub case_sensitive: Option<bool>,
        pub last_error: Option<u32>,
        pub error_87: bool,
        pub inspect_error: Option<String>,
        pub win32_api_trace: Vec<String>,
    }

    fn os_error_code_from_message(message: &str) -> Option<u32> {
        for marker in ["GetLastError=", "NTSTATUS=0x", "(os error "] {
            if let Some(index) = message.find(marker) {
                let rest = &message[index + marker.len()..];
                if marker == "NTSTATUS=0x" {
                    let hex: String = rest
                        .chars()
                        .take_while(|ch| ch.is_ascii_hexdigit())
                        .collect();
                    if let Ok(status) = u32::from_str_radix(&hex, 16)
                        && status == 0xC000_000D
                    {
                        return Some(87);
                    }
                    continue;
                }
                let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
                if let Ok(code) = digits.parse::<u32>() {
                    return Some(code);
                }
            }
        }
        None
    }

    impl WindowsPlatform {
        fn last_win32_error_code() -> Option<u32> {
            std::io::Error::last_os_error()
                .raw_os_error()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|code| *code != 0)
        }

        fn prefix_kind_label(path: &Path) -> String {
            match path.components().next() {
                Some(Component::Prefix(component)) => format!("{:?}", component.kind()),
                other => format!("{other:?}"),
            }
        }

        fn drive_type_label(drive_type: u32) -> &'static str {
            match drive_type {
                DRIVE_UNKNOWN => "DRIVE_UNKNOWN",
                DRIVE_NO_ROOT_DIR => "DRIVE_NO_ROOT_DIR",
                DRIVE_REMOVABLE => "DRIVE_REMOVABLE",
                DRIVE_FIXED => "DRIVE_FIXED",
                4 => "DRIVE_REMOTE",
                5 => "DRIVE_CDROM",
                6 => "DRIVE_RAMDISK",
                _ => "DRIVE_OTHER",
            }
        }

        fn probe_volume_path_name(path: &Path) -> (String, Option<u32>, Vec<u16>) {
            let path_wide = Self::wide_null(path.as_os_str());
            let mut buffer = vec![0_u16; 512];
            // SAFETY: path_wide is NUL-terminated; buffer is writable for the
            // character count passed to GetVolumePathNameW.
            let ok = unsafe {
                GetVolumePathNameW(
                    path_wide.as_ptr(),
                    buffer.as_mut_ptr(),
                    u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                )
            };
            if ok == 0 {
                let code = Self::last_win32_error_code();
                return (
                    format!(
                        "FAIL GetLastError={}",
                        code.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
                    ),
                    code,
                    Vec::new(),
                );
            }
            let wide = Self::ensure_trailing_backslash_wide(buffer);
            let utf16_len = wide
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(wide.len());
            (
                format!("OK {}", String::from_utf16_lossy(&wide[..utf16_len])),
                None,
                wide,
            )
        }

        fn probe_volume_information(mount_wide: &[u16]) -> (String, Option<String>, Option<u32>) {
            if mount_wide.is_empty() {
                return ("NOT RUN (no mount point)".to_owned(), None, None);
            }
            let mut volume_name = [0_u16; 128];
            let mut filesystem_name = [0_u16; 32];
            let mut serial = 0_u32;
            let mut maximum_component = 0_u32;
            let mut flags = 0_u32;
            // SAFETY: mount_wide is a NUL-terminated mount point; output buffers
            // match the character counts passed in. Diagnostic-only; no handle.
            let by_mount = unsafe {
                GetVolumeInformationW(
                    mount_wide.as_ptr(),
                    volume_name.as_mut_ptr(),
                    u32::try_from(volume_name.len()).unwrap_or(u32::MAX),
                    ptr::addr_of_mut!(serial),
                    ptr::addr_of_mut!(maximum_component),
                    ptr::addr_of_mut!(flags),
                    filesystem_name.as_mut_ptr(),
                    u32::try_from(filesystem_name.len()).unwrap_or(u32::MAX),
                )
            };
            if by_mount == 0 {
                let code = Self::last_win32_error_code();
                return (
                    format!(
                        "FAIL GetVolumeInformationW GetLastError={}",
                        code.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
                    ),
                    None,
                    code,
                );
            }
            let filesystem_length = filesystem_name
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(filesystem_name.len());
            let filesystem = String::from_utf16_lossy(&filesystem_name[..filesystem_length]);
            (
                format!("OK serial={serial:08x} filesystem={filesystem}"),
                Some(filesystem),
                None,
            )
        }

        fn probe_drive_type(dos_root: &str) -> (String, Option<u32>) {
            if dos_root.is_empty() {
                return ("NOT RUN (no DOS root)".to_owned(), None);
            }
            let dos_wide = Self::wide_null(OsStr::new(dos_root));
            // SAFETY: dos_wide is NUL-terminated `X:\`.
            let drive_type = unsafe { GetDriveTypeW(dos_wide.as_ptr()) };
            (
                format!("OK {drive_type} {}", Self::drive_type_label(drive_type)),
                None,
            )
        }

        fn win32_probe_line(fields: [&str; 9]) -> String {
            let [
                api,
                path,
                handle_type,
                access,
                share,
                disposition,
                flags,
                information_class,
                result,
            ] = fields;
            format!(
                "{api} path={path} handle={handle_type} access={access} share={share} disposition={disposition} flags={flags} information_class={information_class} result={result}"
            )
        }

        fn win32_result_from_bool(ok: i32) -> String {
            if ok != 0 {
                "OK".to_owned()
            } else {
                match Self::last_win32_error_code() {
                    Some(code) => format!("FAIL GetLastError={code}"),
                    None => "FAIL GetLastError=unknown".to_owned(),
                }
            }
        }

        fn probe_create_file_w(
            path: &Path,
            handle_type: &str,
            flags: u32,
            flags_label: &str,
        ) -> (Option<File>, String) {
            let path_text = path.display().to_string();
            let wide = Self::wide_null(path.as_os_str());
            // SAFETY: wide is NUL-terminated; this is a diagnostic open of an
            // existing path and does not follow an unrelated handle.
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    flags,
                    ptr::null_mut(),
                )
            };
            let result = if handle == INVALID_HANDLE_VALUE {
                Self::win32_result_from_bool(0)
            } else {
                "OK".to_owned()
            };
            let line = Self::win32_probe_line([
                "CreateFileW",
                &path_text,
                handle_type,
                "FILE_READ_ATTRIBUTES|SYNCHRONIZE",
                "FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE",
                "OPEN_EXISTING",
                flags_label,
                "-",
                &result,
            ]);
            if handle == INVALID_HANDLE_VALUE {
                (None, line)
            } else {
                // SAFETY: ownership of the successful CreateFileW handle transfers once.
                (Some(unsafe { File::from_raw_handle(handle as _) }), line)
            }
        }

        fn probe_handle_information(
            handle: HANDLE,
            path: &str,
            handle_type: &str,
            information_class_name: &str,
            class: windows_sys::Win32::Storage::FileSystem::FILE_INFO_BY_HANDLE_CLASS,
            buffer: *mut c_void,
            size: u32,
        ) -> String {
            // SAFETY: `buffer` is writable storage sized for `size` and `handle`
            // is live. GetLastError is read only when this call fails.
            let ok = unsafe { GetFileInformationByHandleEx(handle, class, buffer, size) };
            Self::win32_probe_line([
                "GetFileInformationByHandleEx",
                path,
                handle_type,
                "-",
                "-",
                "-",
                "-",
                information_class_name,
                &Self::win32_result_from_bool(ok),
            ])
        }

        fn probe_basic_by_handle(handle: HANDLE, path: &str, handle_type: &str) -> String {
            let mut basic = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: `basic` is writable storage; GetLastError is read only if
            // this call fails.
            let ok = unsafe { GetFileInformationByHandle(handle, ptr::addr_of_mut!(basic)) };
            Self::win32_probe_line([
                "GetFileInformationByHandle",
                path,
                handle_type,
                "-",
                "-",
                "-",
                "-",
                "-",
                &Self::win32_result_from_bool(ok),
            ])
        }

        fn instrument_inspect_chain(path: &Path) -> Vec<String> {
            let mut trace = Vec::new();
            let Ok((root, _)) = Self::drive_root_and_components(path) else {
                trace.push("drive_root_and_components: FAIL".to_owned());
                return trace;
            };
            let root_text = root.display().to_string();
            let target_text = path.display().to_string();

            let (legacy_root, legacy_line) = Self::probe_create_file_w(
                &root,
                "volume-root",
                FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL,
                "FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_OPEN_NO_RECALL",
            );
            trace.push(legacy_line);
            if let Some(file) = legacy_root {
                let handle = file.as_raw_handle() as HANDLE;
                let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
                let tag_size =
                    u32::try_from(mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>()).unwrap_or(u32::MAX);
                trace.push(Self::probe_handle_information(
                    handle,
                    &root_text,
                    "volume-root",
                    "FileAttributeTagInfo",
                    FileAttributeTagInfo,
                    ptr::addr_of_mut!(tag).cast::<c_void>(),
                    tag_size,
                ));
                trace.push(Self::probe_basic_by_handle(
                    handle,
                    &root_text,
                    "volume-root",
                ));
                let mut id = FILE_ID_INFO {
                    VolumeSerialNumber: 0,
                    FileId: unsafe { mem::zeroed() },
                };
                let id_size = u32::try_from(mem::size_of::<FILE_ID_INFO>()).unwrap_or(u32::MAX);
                trace.push(Self::probe_handle_information(
                    handle,
                    &root_text,
                    "volume-root",
                    "FileIdInfo",
                    FileIdInfo,
                    ptr::addr_of_mut!(id).cast::<c_void>(),
                    id_size,
                ));
                let mut case_info = FILE_CASE_SENSITIVE_INFO { Flags: 0 };
                let case_size =
                    u32::try_from(mem::size_of::<FILE_CASE_SENSITIVE_INFO>()).unwrap_or(u32::MAX);
                trace.push(Self::probe_handle_information(
                    handle,
                    &root_text,
                    "volume-root",
                    "FileCaseSensitiveInfo",
                    FileCaseSensitiveInfo,
                    ptr::addr_of_mut!(case_info).cast::<c_void>(),
                    case_size,
                ));
            }

            let (production_root, production_line) = Self::probe_create_file_w(
                &root,
                "volume-root",
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL,
                "FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_NO_RECALL",
            );
            trace.push(production_line);
            drop(production_root);
            trace.push(
                "GetFileInformationByHandleEx volume-root FileAttributeTagInfo: SKIPPED \
                 (unsupported on volume-root handles; do not fail inspect)"
                    .to_owned(),
            );

            match Self::open_anchored(
                path,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                true,
            ) {
                Ok(target) => {
                    let handle = target.as_raw_handle() as HANDLE;
                    trace.push(Self::win32_probe_line([
                        "open_anchored",
                        &target_text,
                        "directory-or-file",
                        "FILE_READ_ATTRIBUTES",
                        "FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE",
                        "OPEN_EXISTING",
                        "volume-root without FILE_FLAG_OPEN_REPARSE_POINT; children OBJ_DONT_REPARSE",
                        "-",
                        "OK",
                    ]));
                    let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
                    let tag_size = u32::try_from(mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>())
                        .unwrap_or(u32::MAX);
                    trace.push(Self::probe_handle_information(
                        handle,
                        &target_text,
                        "directory-or-file",
                        "FileAttributeTagInfo",
                        FileAttributeTagInfo,
                        ptr::addr_of_mut!(tag).cast::<c_void>(),
                        tag_size,
                    ));
                    let mut id = FILE_ID_INFO {
                        VolumeSerialNumber: 0,
                        FileId: unsafe { mem::zeroed() },
                    };
                    let id_size = u32::try_from(mem::size_of::<FILE_ID_INFO>()).unwrap_or(u32::MAX);
                    trace.push(Self::probe_handle_information(
                        handle,
                        &target_text,
                        "directory-or-file",
                        "FileIdInfo",
                        FileIdInfo,
                        ptr::addr_of_mut!(id).cast::<c_void>(),
                        id_size,
                    ));
                    let mut case_info = FILE_CASE_SENSITIVE_INFO { Flags: 0 };
                    let case_size = u32::try_from(mem::size_of::<FILE_CASE_SENSITIVE_INFO>())
                        .unwrap_or(u32::MAX);
                    trace.push(Self::probe_handle_information(
                        handle,
                        &target_text,
                        "directory-or-file",
                        "FileCaseSensitiveInfo",
                        FileCaseSensitiveInfo,
                        ptr::addr_of_mut!(case_info).cast::<c_void>(),
                        case_size,
                    ));
                    trace.push(Self::probe_basic_by_handle(
                        handle,
                        &target_text,
                        "directory-or-file",
                    ));
                }
                Err(error) => {
                    trace.push(format!(
                        "open_anchored path={target_text} handle=directory-or-file result=FAIL {error}"
                    ));
                }
            }
            trace
        }

        #[must_use]
        pub fn volume_path_diagnostics(path: &Path) -> VolumePathDiagnostics {
            let input_path = path.display().to_string();
            let absolute_path = fs::canonicalize(path)
                .or_else(|_| path.canonicalize())
                .unwrap_or_else(|_| path.to_path_buf());
            let canonical_path = fs::canonicalize(path)
                .map(|value| value.display().to_string())
                .unwrap_or_else(|error| format!("<canonicalize failed: {error}>"));
            let inspected = Path::new(&absolute_path);
            let prefix_kind = Self::prefix_kind_label(inspected);
            let mut last_error = None;

            let (volume_root, win32_root) = match Self::drive_root_and_components(inspected) {
                Ok((root, _)) => {
                    let formatted = root.display().to_string();
                    (formatted.clone(), formatted)
                }
                Err(error) => (format!("<root error: {error}>"), String::new()),
            };
            let dos_root = Self::drive_letter(inspected)
                .ok()
                .and_then(|letter| format_win32_drive_root(letter, false))
                .unwrap_or_default();

            let (get_volume_path_name, path_error, mount_wide) =
                Self::probe_volume_path_name(inspected);
            last_error = last_error.or(path_error);
            let utf16_len = mount_wide
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(mount_wide.len());
            let win32_volume_path = if mount_wide.is_empty() {
                String::new()
            } else {
                String::from_utf16_lossy(&mount_wide[..utf16_len])
            };

            let information_wide = if mount_wide.is_empty() {
                Self::wide_null(OsStr::new(&dos_root))
            } else {
                mount_wide
            };
            let (get_volume_information, filesystem_from_api, info_error) =
                Self::probe_volume_information(&information_wide);
            last_error = last_error.or(info_error);

            let (get_drive_type, drive_error) = Self::probe_drive_type(&dos_root);
            last_error = last_error.or(drive_error);

            let win32_api_trace = Self::instrument_inspect_chain(inspected);
            let inspected_volume = WindowsPlatform.inspect_volume(inspected);
            let inspect_error = inspected_volume.as_ref().err().map(ToString::to_string);
            // Capture the failing API's error from the inspect/open_anchored
            // message. Do not call GetLastError here — later probes overwrite it.
            let inspect_code = inspect_error
                .as_deref()
                .and_then(os_error_code_from_message);
            last_error = inspect_code.or(last_error);
            let inspect_error_87 = inspect_error.as_deref().is_some_and(|value| {
                value.contains("87")
                    || value.contains("INVALID_PARAMETER")
                    || value.to_ascii_uppercase().contains("C000000D")
            });
            let error_87 = inspect_error_87;

            match inspected_volume {
                Ok(volume) => VolumePathDiagnostics {
                    input_path,
                    absolute_path: absolute_path.display().to_string(),
                    canonical_path,
                    prefix_kind,
                    volume_root,
                    dos_root,
                    win32_root,
                    win32_volume_path,
                    utf16_len,
                    get_volume_path_name,
                    get_volume_information,
                    get_drive_type,
                    filesystem_name: volume.filesystem_type.or(filesystem_from_api),
                    volume_identity: Some(volume.stable_identifier),
                    case_sensitive: Some(volume.case_sensitive),
                    last_error,
                    error_87,
                    inspect_error: None,
                    win32_api_trace,
                },
                Err(_) => VolumePathDiagnostics {
                    input_path,
                    absolute_path: absolute_path.display().to_string(),
                    canonical_path,
                    prefix_kind,
                    volume_root,
                    dos_root,
                    win32_root,
                    win32_volume_path,
                    utf16_len,
                    get_volume_path_name,
                    get_volume_information,
                    get_drive_type,
                    filesystem_name: filesystem_from_api,
                    volume_identity: None,
                    case_sensitive: None,
                    last_error,
                    error_87,
                    inspect_error,
                    win32_api_trace,
                },
            }
        }

        fn wide_null(value: &OsStr) -> Vec<u16> {
            value.encode_wide().chain(Some(0)).collect()
        }

        fn native_path(path: &Path) -> NativePath {
            let mut bytes = Vec::new();
            for unit in path.as_os_str().encode_wide() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            NativePath {
                encoding: PathEncoding::WindowsUtf16Le,
                bytes,
            }
        }

        fn validate_component(name: &OsStr) -> Result<(), PlatformError> {
            let units = name.encode_wide().collect::<Vec<_>>();
            if units.is_empty()
                || units
                    .iter()
                    .any(|unit| *unit == 0 || *unit == u16::from(b':'))
                || units
                    .last()
                    .is_some_and(|unit| matches!(*unit, 0x20 | 0x2e))
            {
                return Err(PlatformError::PathPolicyRefusal);
            }
            let text = name.to_string_lossy();
            let stem = text
                .split_once('.')
                .map_or(text.as_ref(), |(stem, _)| stem)
                .trim_end_matches([' ', '.'])
                .to_ascii_uppercase();
            if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
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
            {
                return Err(PlatformError::PathPolicyRefusal);
            }
            Ok(())
        }

        fn drive_root_and_components(
            path: &Path,
        ) -> Result<(PathBuf, Vec<OsString>), PlatformError> {
            let mut components = path.components();
            let prefix = match components.next() {
                Some(Component::Prefix(prefix))
                    if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) =>
                {
                    prefix
                }
                _ => {
                    return Err(PlatformError::Unsupported(
                        "only absolute local drive paths are supported".to_owned(),
                    ));
                }
            };
            if !matches!(components.next(), Some(Component::RootDir)) {
                return Err(PlatformError::OutsideRoot);
            }
            let root = Self::win32_drive_root(prefix.kind())?;
            let mut names = Vec::new();
            for component in components {
                match component {
                    Component::Normal(name) => {
                        Self::validate_component(name)?;
                        names.push(name.to_os_string());
                    }
                    _ => return Err(PlatformError::OutsideRoot),
                }
            }
            Ok((root, names))
        }

        fn win32_drive_root(prefix: Prefix<'_>) -> Result<PathBuf, PlatformError> {
            let formatted = match prefix {
                Prefix::VerbatimDisk(letter) => format_win32_drive_root(letter, true),
                Prefix::Disk(letter) => format_win32_drive_root(letter, false),
                _ => None,
            };
            formatted.map(PathBuf::from).ok_or_else(|| {
                PlatformError::Unsupported(
                    "only absolute local drive paths are supported".to_owned(),
                )
            })
        }

        fn drive_letter(path: &Path) -> Result<u8, PlatformError> {
            match path.components().next() {
                Some(Component::Prefix(prefix)) => match prefix.kind() {
                    Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Ok(letter),
                    _ => Err(PlatformError::Unsupported(
                        "only absolute local drive paths are supported".to_owned(),
                    )),
                },
                _ => Err(PlatformError::Unsupported(
                    "only absolute local drive paths are supported".to_owned(),
                )),
            }
        }

        fn dos_drive_root_wide(path: &Path) -> Result<Vec<u16>, PlatformError> {
            let letter = Self::drive_letter(path)?;
            let root = format_win32_drive_root(letter, false)
                .ok_or_else(|| PlatformError::Unsupported("invalid drive letter".to_owned()))?;
            Ok(Self::wide_null(OsStr::new(&root)))
        }

        fn ensure_trailing_backslash_wide(mut units: Vec<u16>) -> Vec<u16> {
            while units.last() == Some(&0) {
                units.pop();
            }
            while units.last() == Some(&u16::from(b'\\')) {
                units.pop();
            }
            units.push(u16::from(b'\\'));
            units.push(0);
            units
        }

        fn win32_mount_point_wide(path: &Path) -> Result<Vec<u16>, PlatformError> {
            let path_wide = Self::wide_null(path.as_os_str());
            let mut buffer = vec![0_u16; 512];
            // SAFETY: path_wide is NUL-terminated; buffer is writable for the
            // character count passed to GetVolumePathNameW.
            let ok = unsafe {
                GetVolumePathNameW(
                    path_wide.as_ptr(),
                    buffer.as_mut_ptr(),
                    u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                )
            };
            if ok != 0 {
                return Ok(Self::ensure_trailing_backslash_wide(buffer));
            }
            let (root, _) = Self::drive_root_and_components(path)?;
            Ok(Self::ensure_trailing_backslash_wide(Self::wide_null(
                root.as_os_str(),
            )))
        }

        fn reject_unsafe_handle(handle: HANDLE) -> Result<(), PlatformError> {
            let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
            let size = u32::try_from(mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>())
                .map_err(|_| PlatformError::Unsupported("attribute info overflow".to_owned()))?;
            // SAFETY: `info` is valid writable storage for the documented query.
            let result = unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileAttributeTagInfo,
                    ptr::addr_of_mut!(info).cast::<c_void>(),
                    size,
                )
            };
            if result != 0 {
                return Self::reject_attribute_bits(info.FileAttributes, info.ReparseTag);
            }
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            // FileAttributeTagInfo is unsupported on some directory handles
            // (ERROR_INVALID_PARAMETER / 87). Fall back to basic attributes.
            // Volume-root handles must not reach this function — those classes
            // are unsupported and would fail the entire inspect.
            if code != 87 {
                return Err(PlatformError::from_windows_code(
                    u32::try_from(code).unwrap_or(u32::MAX),
                    false,
                ));
            }
            let mut basic = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: `basic` is valid writable storage for the documented query.
            let basic_ok = unsafe { GetFileInformationByHandle(handle, ptr::addr_of_mut!(basic)) };
            if basic_ok == 0 {
                let code = Self::last_win32_error_code().unwrap_or(u32::MAX);
                return Err(Self::win32_api_error(
                    "GetFileInformationByHandle",
                    "-",
                    "present",
                    "-",
                    0,
                    0,
                    0,
                    0,
                    0,
                    code,
                ));
            }
            Self::reject_attribute_bits(basic.dwFileAttributes, 0)
        }

        fn reject_attribute_bits(attributes: u32, reparse_tag: u32) -> Result<(), PlatformError> {
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || reparse_tag != 0 {
                return Err(PlatformError::ReparsePoint);
            }
            if attributes
                & (FILE_ATTRIBUTE_OFFLINE
                    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                    | FILE_ATTRIBUTE_RECALL_ON_OPEN)
                != 0
            {
                return Err(PlatformError::CloudPlaceholder);
            }
            Ok(())
        }

        fn last_windows_error(mutation_outcome_uncertain: bool) -> PlatformError {
            let error = std::io::Error::last_os_error();
            let code = error
                .raw_os_error()
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(u32::MAX);
            PlatformError::from_windows_code(code, mutation_outcome_uncertain)
        }

        fn win32_api_error(
            api: &str,
            path: &str,
            root_directory: &str,
            object_name: &str,
            access: u32,
            share: u32,
            disposition: u32,
            options: u32,
            attributes: u32,
            code: u32,
        ) -> PlatformError {
            let classified = PlatformError::from_windows_code(code, false);
            match classified {
                PlatformError::Io(_) => PlatformError::Io(std::io::Error::other(format!(
                    "{api} path={path} RootDirectory={root_directory} ObjectName={object_name} \
                     access=0x{access:08X} share=0x{share:08X} disposition=0x{disposition:08X} \
                     options=0x{options:08X} attributes=0x{attributes:08X} GetLastError={code} \
                     (os error {code})"
                ))),
                other => other,
            }
        }

        fn ntcreatefile_error(
            path: &str,
            object_name: &str,
            access: u32,
            share: u32,
            disposition: u32,
            options: u32,
            attributes: u32,
            status: i32,
        ) -> PlatformError {
            Self::ntstatus_error(
                "NtCreateFile",
                path,
                object_name,
                access,
                share,
                disposition,
                options,
                attributes,
                status,
            )
        }

        fn ntstatus_error(
            api: &str,
            path: &str,
            object_name: &str,
            access: u32,
            share: u32,
            disposition: u32,
            options: u32,
            attributes: u32,
            status: i32,
        ) -> PlatformError {
            if status == STATUS_REPARSE_POINT_ENCOUNTERED {
                return PlatformError::ReparsePoint;
            }
            // SAFETY: RtlNtStatusToDosError is the documented conversion of the
            // NTSTATUS captured immediately above; no other Win32 call runs first.
            let code = unsafe { RtlNtStatusToDosError(status) };
            let classified = PlatformError::from_windows_code(code, false);
            match classified {
                PlatformError::Io(_) => PlatformError::Io(std::io::Error::other(format!(
                    "{api} path={path} RootDirectory=present ObjectName={object_name} \
                     access=0x{access:08X} share=0x{share:08X} disposition=0x{disposition:08X} \
                     options=0x{options:08X} attributes=0x{attributes:08X} \
                     NTSTATUS=0x{status:08X} GetLastError={code} (os error {code})"
                ))),
                other => other,
            }
        }

        #[inline(never)]
        fn open_relative(
            parent: &File,
            name: &OsStr,
            desired_access: u32,
            share_access: u32,
            directory: bool,
            disposition: u32,
        ) -> Result<File, PlatformError> {
            Self::validate_component(name)?;
            let object_name = name.to_string_lossy();
            if !crate::relative_object_name_is_legal(&object_name) {
                return Err(PlatformError::OutsideRoot);
            }
            let mut name_wide = name.encode_wide().collect::<Vec<_>>();
            let name_bytes = name_wide
                .len()
                .checked_mul(mem::size_of::<u16>())
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| {
                    PlatformError::Unsupported("path component is too long".to_owned())
                })?;
            let unicode = UNICODE_STRING {
                Length: name_bytes,
                MaximumLength: name_bytes,
                Buffer: name_wide.as_mut_ptr(),
            };
            let object_attributes = OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE;
            let attributes = OBJECT_ATTRIBUTES {
                Length: u32::try_from(mem::size_of::<OBJECT_ATTRIBUTES>()).unwrap_or(u32::MAX),
                RootDirectory: parent.as_raw_handle() as HANDLE,
                ObjectName: ptr::addr_of!(unicode),
                Attributes: object_attributes,
                SecurityDescriptor: ptr::null(),
                SecurityQualityOfService: ptr::null(),
            };
            let mut status_block = IO_STATUS_BLOCK::default();
            let mut handle: HANDLE = ptr::null_mut();
            let create_options = anchored_create_options(directory);
            let file_attributes = if disposition == FILE_OPEN {
                0
            } else {
                FILE_ATTRIBUTE_NORMAL
            };
            // SAFETY: every pointer references live stack storage for the call;
            // ObjectName is a single relative leaf; RootDirectory is the parent.
            let status = unsafe {
                NtCreateFile(
                    ptr::addr_of_mut!(handle),
                    desired_access | SYNCHRONIZE,
                    ptr::addr_of!(attributes),
                    ptr::addr_of_mut!(status_block),
                    ptr::null(),
                    file_attributes,
                    share_access,
                    disposition,
                    create_options,
                    ptr::null(),
                    0,
                )
            };
            if status < 0 {
                return Err(Self::ntcreatefile_error(
                    &object_name,
                    &object_name,
                    desired_access | SYNCHRONIZE,
                    share_access,
                    disposition,
                    create_options,
                    object_attributes,
                    status,
                ));
            }
            if handle.is_null() {
                let code = Self::last_win32_error_code().unwrap_or(u32::MAX);
                return Err(Self::win32_api_error(
                    "NtCreateFile",
                    &object_name,
                    "present",
                    &object_name,
                    desired_access | SYNCHRONIZE,
                    share_access,
                    disposition,
                    create_options,
                    object_attributes,
                    code,
                ));
            }
            // SAFETY: ownership of the successful NtCreateFile handle transfers
            // exactly once to File.
            let file = unsafe { File::from_raw_handle(handle as _) };
            Self::reject_unsafe_handle(file.as_raw_handle() as HANDLE)?;
            Ok(file)
        }

        #[inline(never)]
        fn open_anchored(
            path: &Path,
            desired_access: u32,
            final_share_access: u32,
            final_directory: bool,
        ) -> Result<File, PlatformError> {
            let (root, components) = Self::drive_root_and_components(path)?;
            let root_wide = Self::wide_null(root.as_os_str());
            // Volume-root handles are NT namespace anchors, not mutation
            // targets. FILE_FLAG_OPEN_REPARSE_POINT on `\\?\X:\` produces a
            // handle that rejects FileAttributeTagInfo / FileIdInfo /
            // GetFileInformationByHandle with ERROR_INVALID_PARAMETER (87) on
            // Windows 11 26100. Open with backup semantics only. Child opens
            // use a relative ObjectName, OBJ_DONT_REPARSE, FILE_OPEN_REPARSE_POINT,
            // and reject_unsafe_handle. Directory CreateOptions never include
            // FILE_OPEN_NO_RECALL (STATUS_INVALID_PARAMETER / 87).
            let root_handle = unsafe {
                CreateFileW(
                    root_wide.as_ptr(),
                    FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL,
                    ptr::null_mut(),
                )
            };
            if root_handle == INVALID_HANDLE_VALUE {
                let code = Self::last_win32_error_code().unwrap_or(u32::MAX);
                return Err(Self::win32_api_error(
                    "CreateFileW",
                    &root.display().to_string(),
                    "null",
                    "-",
                    FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL,
                    0,
                    code,
                ));
            }
            // SAFETY: ownership of the CreateFileW handle transfers once.
            let mut current = unsafe { File::from_raw_handle(root_handle as _) };
            for (index, component) in components.iter().enumerate() {
                let final_component = index + 1 == components.len();
                current = Self::open_relative(
                    &current,
                    component,
                    if final_component {
                        desired_access
                    } else {
                        FILE_READ_ATTRIBUTES
                    },
                    if final_component {
                        final_share_access
                    } else {
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
                    },
                    if final_component {
                        final_directory
                    } else {
                        true
                    },
                    FILE_OPEN,
                )?;
            }
            Ok(current)
        }

        fn query_volume_information(
            mount_wide: &[u16],
            handle: HANDLE,
        ) -> Result<(u32, [u16; 32]), PlatformError> {
            let mut volume_name = [0_u16; 128];
            let mut filesystem_name = [0_u16; 32];
            let mut serial = 0_u32;
            let mut maximum_component = 0_u32;
            let mut flags = 0_u32;
            // SAFETY: mount_wide is a NUL-terminated mount point with a trailing
            // backslash; output buffers match the character counts passed in.
            let by_mount = unsafe {
                GetVolumeInformationW(
                    mount_wide.as_ptr(),
                    volume_name.as_mut_ptr(),
                    u32::try_from(volume_name.len()).unwrap_or(u32::MAX),
                    ptr::addr_of_mut!(serial),
                    ptr::addr_of_mut!(maximum_component),
                    ptr::addr_of_mut!(flags),
                    filesystem_name.as_mut_ptr(),
                    u32::try_from(filesystem_name.len()).unwrap_or(u32::MAX),
                )
            };
            if by_mount != 0 {
                return Ok((serial, filesystem_name));
            }
            // GetVolumeInformationByHandleW returns ERROR_INVALID_PARAMETER (87)
            // on some Windows 11 handles opened with FILE_FLAG_OPEN_REPARSE_POINT.
            // Keep it only as a last resort after a legal mount-point query.
            let by_handle = unsafe {
                GetVolumeInformationByHandleW(
                    handle,
                    volume_name.as_mut_ptr(),
                    u32::try_from(volume_name.len()).unwrap_or(u32::MAX),
                    ptr::addr_of_mut!(serial),
                    ptr::addr_of_mut!(maximum_component),
                    ptr::addr_of_mut!(flags),
                    filesystem_name.as_mut_ptr(),
                    u32::try_from(filesystem_name.len()).unwrap_or(u32::MAX),
                )
            };
            if by_handle == 0 {
                return Err(Self::last_windows_error(false));
            }
            Ok((serial, filesystem_name))
        }

        #[inline(never)]
        fn volume_from_handle(
            path: &Path,
            handle: HANDLE,
        ) -> Result<VolumeIdentity, PlatformError> {
            let mount_wide = Self::win32_mount_point_wide(path)?;
            let (serial, filesystem_name) =
                match Self::query_volume_information(&mount_wide, handle) {
                    Ok(value) => value,
                    Err(error) => {
                        let dos = Self::dos_drive_root_wide(path)?;
                        Self::query_volume_information(&dos, handle).map_err(|_| error)?
                    }
                };
            let filesystem_length = filesystem_name
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(filesystem_name.len());
            let mut volume_guid = [0_u16; 64];
            // SAFETY: mount_wide is a legal NUL-terminated mount point.
            let guid_result = unsafe {
                GetVolumeNameForVolumeMountPointW(
                    mount_wide.as_ptr(),
                    volume_guid.as_mut_ptr(),
                    u32::try_from(volume_guid.len()).unwrap_or(u32::MAX),
                )
            };
            let guid_length = volume_guid
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(volume_guid.len());
            // GetDriveTypeW often rejects \\?\X:\ (DRIVE_NO_ROOT_DIR). The DOS
            // root X:\ is the documented form.
            let dos_wide = Self::dos_drive_root_wide(path)?;
            // SAFETY: dos_wide is NUL-terminated `X:\`.
            let mut drive_type = unsafe { GetDriveTypeW(dos_wide.as_ptr()) };
            if drive_type == DRIVE_UNKNOWN || drive_type == DRIVE_NO_ROOT_DIR {
                drive_type = unsafe { GetDriveTypeW(mount_wide.as_ptr()) };
            }
            Ok(VolumeIdentity {
                platform: PlatformKind::Windows,
                stable_identifier: if guid_result != 0 {
                    String::from_utf16_lossy(&volume_guid[..guid_length])
                } else {
                    format!("volume-serial:{serial:08x}")
                },
                filesystem_type: Some(String::from_utf16_lossy(
                    &filesystem_name[..filesystem_length],
                )),
                case_sensitive: Self::case_sensitive_from_handle(handle),
                removable: drive_type == DRIVE_REMOVABLE,
                local: drive_type == DRIVE_FIXED || drive_type == DRIVE_REMOVABLE,
            })
        }

        fn case_sensitive_from_handle(handle: HANDLE) -> bool {
            let mut info = FILE_CASE_SENSITIVE_INFO { Flags: 0 };
            let size = u32::try_from(mem::size_of::<FILE_CASE_SENSITIVE_INFO>()).unwrap_or(0);
            // SAFETY: `info` is writable storage for FileCaseSensitiveInfo.
            // Unsupported classes (including volume-root handles) return 87;
            // NTFS default is case-insensitive, so treat that as false.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileCaseSensitiveInfo,
                    ptr::addr_of_mut!(info).cast::<c_void>(),
                    size,
                )
            };
            ok != 0 && info.Flags & 0x1 != 0
        }

        fn identity(
            path: &Path,
            metadata: &fs::Metadata,
        ) -> Result<NativeFileIdentity, PlatformError> {
            let file = Self::open_anchored(
                path,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                metadata.is_dir(),
            )?;
            Self::identity_from_open_file(path, metadata, &file)
        }

        fn identity_from_open_file(
            path: &Path,
            metadata: &fs::Metadata,
            file: &File,
        ) -> Result<NativeFileIdentity, PlatformError> {
            let parent = path.parent().ok_or_else(|| {
                PlatformError::Unsupported("the volume root is not a file entry".to_owned())
            })?;
            let parent_file = Self::open_anchored(
                parent,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                true,
            )?;
            let (serial, object_key, link_count) =
                Self::identity_from_handle(file.as_raw_handle() as HANDLE)?;
            let (parent_serial, parent_key, _) =
                Self::identity_from_handle(parent_file.as_raw_handle() as HANDLE)?;
            if serial != parent_serial {
                return Err(PlatformError::Precondition(
                    "parent and file are on different volumes".to_owned(),
                ));
            }
            let leaf = path.file_name().ok_or_else(|| {
                PlatformError::Unsupported("file entry has no leaf name".to_owned())
            })?;
            let attributes = metadata.file_attributes();

            Ok(NativeFileIdentity {
                volume: Self::volume_from_handle(path, file.as_raw_handle() as HANDLE)?,
                object_key: object_key.to_vec(),
                parent_key: parent_key.to_vec(),
                leaf_name: Self::native_path(Path::new(leaf)),
                link_count,
                reparse_tag: (attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(1),
            })
        }

        fn reject_unsafe_attributes(metadata: &fs::Metadata) -> Result<(), PlatformError> {
            let attributes = metadata.file_attributes();
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(PlatformError::ReparsePoint);
            }
            if attributes
                & (FILE_ATTRIBUTE_OFFLINE
                    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                    | FILE_ATTRIBUTE_RECALL_ON_OPEN)
                != 0
            {
                return Err(PlatformError::CloudPlaceholder);
            }
            Ok(())
        }

        fn metadata_ns(value: u64) -> Option<i128> {
            (value != 0).then_some(i128::from(value) * 100)
        }

        fn inspection_error(error: PlatformError) -> PlatformError {
            match error {
                PlatformError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    PlatformError::SourceMissing
                }
                PlatformError::Io(error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    PlatformError::PermissionDenied
                }
                other => other,
            }
        }

        fn targeted_path(root: &Path, relative_path: &Path) -> Result<PathBuf, PlatformError> {
            if relative_path.as_os_str().is_empty()
                || relative_path.is_absolute()
                || relative_path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(PlatformError::OutsideRoot);
            }
            Ok(root.join(relative_path))
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

        fn identity_from_handle(handle: HANDLE) -> Result<(u64, [u8; 16], u32), PlatformError> {
            let mut info = FILE_ID_INFO {
                VolumeSerialNumber: 0,
                FileId: unsafe { mem::zeroed() },
            };
            let size = u32::try_from(mem::size_of::<FILE_ID_INFO>())
                .map_err(|_| PlatformError::Unsupported("FILE_ID_INFO size overflow".to_owned()))?;
            // SAFETY: `info` points to writable storage of exactly `size` bytes and
            // `handle` is a live file handle owned by the caller.
            let result = unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileIdInfo,
                    ptr::addr_of_mut!(info).cast::<c_void>(),
                    size,
                )
            };
            if result == 0 {
                return Err(Self::last_windows_error(false));
            }
            let mut legacy: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
            // SAFETY: `legacy` is valid writable storage and `handle` remains live.
            let legacy_result =
                unsafe { GetFileInformationByHandle(handle, ptr::addr_of_mut!(legacy)) };
            if legacy_result == 0 {
                return Err(Self::last_windows_error(false));
            }
            Ok((
                info.VolumeSerialNumber,
                info.FileId.Identifier,
                legacy.nNumberOfLinks,
            ))
        }

        #[cfg(feature = "mutation")]
        fn parent_and_leaf(path: &Path) -> Result<(File, OsString), PlatformError> {
            Self::parent_and_leaf_with_access(path, FILE_READ_ATTRIBUTES)
        }

        #[cfg(feature = "mutation")]
        fn destination_parent_and_leaf(path: &Path) -> Result<(File, OsString), PlatformError> {
            // NtSetInformationFile(FileRenameInformation) checks FILE_ADD_FILE
            // on the RootDirectory handle. FILE_READ_ATTRIBUTES alone is not
            // enough for a relative-leaf rename.
            Self::parent_and_leaf_with_access(
                path,
                FILE_ADD_FILE | FILE_TRAVERSE | FILE_READ_ATTRIBUTES,
            )
        }

        #[cfg(feature = "mutation")]
        fn parent_and_leaf_with_access(
            path: &Path,
            desired_access: u32,
        ) -> Result<(File, OsString), PlatformError> {
            let parent = path.parent().ok_or(PlatformError::PathPolicyRefusal)?;
            let leaf = path
                .file_name()
                .ok_or(PlatformError::PathPolicyRefusal)?
                .to_os_string();
            let parent = Self::open_anchored(
                parent,
                desired_access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                true,
            )?;
            Ok((parent, leaf))
        }

        #[cfg(feature = "mutation")]
        fn identity_from_held_parent(
            path_for_volume: &Path,
            metadata: &fs::Metadata,
            file: &File,
            parent: &File,
            leaf: &OsStr,
        ) -> Result<NativeFileIdentity, PlatformError> {
            let (serial, object_key, link_count) =
                Self::identity_from_handle(file.as_raw_handle() as HANDLE)?;
            let (parent_serial, parent_key, _) =
                Self::identity_from_handle(parent.as_raw_handle() as HANDLE)?;
            if serial != parent_serial {
                return Err(PlatformError::Precondition(
                    "parent and source are on different native volumes".to_owned(),
                ));
            }
            let attributes = metadata.file_attributes();
            Ok(NativeFileIdentity {
                volume: Self::volume_from_handle(path_for_volume, file.as_raw_handle() as HANDLE)?,
                object_key: object_key.to_vec(),
                parent_key: parent_key.to_vec(),
                leaf_name: Self::native_path(Path::new(leaf)),
                link_count,
                reparse_tag: (attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(1),
            })
        }

        #[cfg(feature = "mutation")]
        fn exact_identity_matches(
            expected: &NativeFileIdentity,
            observed: &NativeFileIdentity,
        ) -> bool {
            expected.volume == observed.volume
                && expected.object_key == observed.object_key
                && expected.parent_key == observed.parent_key
                && expected.leaf_name == observed.leaf_name
                && expected.link_count == 1
                && observed.link_count == 1
                && expected.reparse_tag.is_none()
                && observed.reparse_tag.is_none()
        }

        #[cfg(feature = "mutation")]
        fn destination_absent_from_parent(
            parent: &File,
            leaf: &OsStr,
        ) -> Result<(), PlatformError> {
            match Self::open_relative(
                parent,
                leaf,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
                FILE_OPEN,
            ) {
                Ok(_) => Err(PlatformError::DestinationExists),
                Err(PlatformError::SourceMissing) => Ok(()),
                Err(error) => Err(error),
            }
        }

        #[cfg(feature = "mutation")]
        fn set_rename_no_replace(
            file: &File,
            destination_parent: &File,
            leaf: &OsStr,
        ) -> Result<(), PlatformError> {
            let object_name = leaf.to_string_lossy();
            if !crate::relative_object_name_is_legal(&object_name) {
                return Err(PlatformError::OutsideRoot);
            }
            let mut destination_wide: Vec<u16> = leaf.encode_wide().collect();
            destination_wide.push(0);
            let name_bytes = destination_wide
                .len()
                .checked_sub(1)
                .and_then(|units| units.checked_mul(mem::size_of::<u16>()))
                .ok_or_else(|| PlatformError::Unsupported("destination is too long".to_owned()))?;
            let file_name_offset = mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
            let total_size = file_name_offset
                .checked_add(destination_wide.len().saturating_mul(mem::size_of::<u16>()))
                .ok_or_else(|| PlatformError::Unsupported("rename buffer overflow".to_owned()))?;
            let name_length = u32::try_from(name_bytes)
                .map_err(|_| PlatformError::Unsupported("destination is too long".to_owned()))?;
            let words = total_size.div_ceil(mem::size_of::<u64>());
            let mut storage = vec![0_u64; words];
            let rename_info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();

            // SAFETY: `storage` is 8-byte aligned and large enough for the
            // FILE_RENAME_INFORMATION header plus a NUL-terminated relative
            // leaf. ReplaceIfExists stays 0 — no overwrite.
            unsafe {
                (*rename_info).Anonymous.ReplaceIfExists = false;
                (*rename_info).RootDirectory = destination_parent.as_raw_handle() as HANDLE;
                (*rename_info).FileNameLength = name_length;
                ptr::copy_nonoverlapping(
                    destination_wide.as_ptr(),
                    ptr::addr_of_mut!((*rename_info).FileName).cast::<u16>(),
                    destination_wide.len(),
                );
            }
            let size = u32::try_from(total_size)
                .map_err(|_| PlatformError::Unsupported("rename buffer overflow".to_owned()))?;
            let mut status_block = IO_STATUS_BLOCK::default();
            // SAFETY: handle and rename buffer remain valid for the call.
            // NtSetInformationFile(FileRenameInformation) accepts RootDirectory
            // + one relative leaf. The Win32 SetFileInformationByHandle wrapper
            // does not — that combination is ERROR 87 on Windows 11 26100.
            let status = unsafe {
                NtSetInformationFile(
                    file.as_raw_handle() as HANDLE,
                    ptr::addr_of_mut!(status_block),
                    rename_info.cast::<c_void>(),
                    size,
                    FileRenameInformation,
                )
            };
            if status < 0 {
                return Err(Self::ntstatus_error(
                    "NtSetInformationFile(FileRenameInformation)",
                    &object_name,
                    &object_name,
                    DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    FILE_SHARE_READ,
                    FILE_OPEN,
                    0,
                    0,
                    status,
                ));
            }
            Ok(())
        }
    }

    impl ReadOnlyPlatform for WindowsPlatform {
        #[inline(never)]
        fn inspect_volume(&self, root: &Path) -> Result<VolumeIdentity, PlatformError> {
            let metadata = fs::symlink_metadata(root)?;
            Self::reject_unsafe_attributes(&metadata)?;
            let root_handle = Self::open_anchored(
                root,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                metadata.is_dir(),
            )?;
            Self::volume_from_handle(root, root_handle.as_raw_handle() as HANDLE)
        }

        fn enumerate_regular_files(
            &self,
            root: &Path,
            max_entries: usize,
            is_cancelled: &dyn Fn() -> bool,
            on_progress: &mut dyn FnMut(EnumerationProgress),
        ) -> Result<ReadOnlyEnumeration, PlatformError> {
            let metadata = fs::symlink_metadata(root)?;
            Self::reject_unsafe_attributes(&metadata)?;
            if !metadata.is_dir() {
                return Err(PlatformError::Unsupported(
                    "registered root must be a directory".to_owned(),
                ));
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
                let _directory_guard = match Self::open_anchored(
                    &directory,
                    FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    true,
                ) {
                    Ok(handle) => handle,
                    Err(error) => {
                        output.issues.push(EnumerationIssue {
                            path: directory,
                            error,
                            is_directory: true,
                        });
                        output.progress.errors = output.progress.errors.saturating_add(1);
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
                        on_progress(output.progress);
                        continue;
                    }
                };
                let candidates = match fs::read_dir(&directory) {
                    Ok(candidates) => candidates,
                    Err(error) => {
                        output.issues.push(EnumerationIssue {
                            path: directory,
                            error: PlatformError::Io(error),
                            is_directory: true,
                        });
                        output.progress.errors = output.progress.errors.saturating_add(1);
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
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
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
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
                    if let Err(error) = Self::reject_unsafe_attributes(&metadata) {
                        output.issues.push(EnumerationIssue {
                            path,
                            error,
                            is_directory: metadata.is_dir(),
                        });
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
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
                        output.progress.skipped_items =
                            output.progress.skipped_items.saturating_add(1);
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
                    let identity = match Self::identity(&path, &metadata) {
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
                    let attributes = metadata.file_attributes();
                    let relative_path = Self::native_path(relative);
                    output.progress.files_discovered =
                        output.progress.files_discovered.saturating_add(1);
                    output.progress.bytes_discovered = output
                        .progress
                        .bytes_discovered
                        .saturating_add(metadata.file_size());
                    output.files.push(ReadOnlyEntry {
                        absolute_path: path,
                        relative_path,
                        identity,
                        byte_size: metadata.file_size(),
                        modified_at_ns: Self::metadata_ns(metadata.last_write_time()),
                        created_at_ns: Self::metadata_ns(metadata.creation_time()),
                        accessed_at_ns: Self::metadata_ns(metadata.last_access_time()),
                        attributes: attributes.into(),
                        read_only: metadata.permissions().readonly(),
                        hidden: attributes & 0x2 != 0,
                        cloud_placeholder: false,
                        encrypted: attributes & FILE_ATTRIBUTE_ENCRYPTED != 0,
                    });
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
            let root_metadata =
                fs::symlink_metadata(root).map_err(|error| Self::inspection_error(error.into()))?;
            Self::reject_unsafe_attributes(&root_metadata).map_err(Self::inspection_error)?;
            if !root_metadata.is_dir() {
                return Err(PlatformError::Unsupported(
                    "registered root must be a directory".to_owned(),
                ));
            }
            let _root_guard = Self::open_anchored(
                root,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                true,
            )
            .map_err(Self::inspection_error)?;
            let target = Self::targeted_path(root, relative_path)?;
            let file = Self::open_anchored(
                &target,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )
            .map_err(Self::inspection_error)?;
            let metadata = file
                .metadata()
                .map_err(|error| Self::inspection_error(error.into()))?;
            Self::reject_unsafe_attributes(&metadata).map_err(Self::inspection_error)?;
            if !metadata.is_file() {
                return Err(PlatformError::Unsupported(
                    "only regular files are analyzable".to_owned(),
                ));
            }
            let attributes = metadata.file_attributes();
            Ok(ReadOnlyEntry {
                absolute_path: target.clone(),
                relative_path: Self::native_path(relative_path),
                identity: Self::identity_from_open_file(&target, &metadata, &file)
                    .map_err(Self::inspection_error)?,
                byte_size: metadata.file_size(),
                modified_at_ns: Self::metadata_ns(metadata.last_write_time()),
                created_at_ns: Self::metadata_ns(metadata.creation_time()),
                accessed_at_ns: Self::metadata_ns(metadata.last_access_time()),
                attributes: attributes.into(),
                read_only: metadata.permissions().readonly(),
                hidden: attributes & 0x2 != 0,
                cloud_placeholder: false,
                encrypted: attributes & FILE_ATTRIBUTE_ENCRYPTED != 0,
            })
        }

        fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
            let mut file = Self::open_anchored(
                path,
                GENERIC_READ | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )?;
            let metadata = file.metadata()?;
            if metadata.len() > max_bytes {
                return Err(PlatformError::Unsupported(format!(
                    "file exceeds the {max_bytes}-byte analysis budget"
                )));
            }
            let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
            file.by_ref()
                .take(max_bytes.saturating_add(1))
                .read_to_end(&mut bytes)?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
                return Err(PlatformError::Unsupported(
                    "file changed while being read or exceeds its budget".to_owned(),
                ));
            }
            Ok(bytes)
        }

        fn read_prefix(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
            let file = Self::open_anchored(
                path,
                GENERIC_READ | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )?;
            let mut bytes = Vec::with_capacity(max_bytes);
            file.take(u64::try_from(max_bytes).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)?;
            Ok(bytes)
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
            // symlink_metadata is the reparse-point gate. `?` would wrap
            // ERROR_FILE_NOT_FOUND as Io via thiserror; recovery needs
            // SourceMissing so a committed move is not an opaque I/O failure.
            let metadata =
                fs::symlink_metadata(path).map_err(|error| Self::inspection_error(error.into()))?;
            Self::reject_unsafe_attributes(&metadata).map_err(Self::inspection_error)?;
            if !metadata.is_file() {
                return Err(PlatformError::Unsupported(
                    "only regular files can be fingerprinted".to_owned(),
                ));
            }
            let mut file = Self::open_anchored(
                path,
                GENERIC_READ | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )
            .map_err(Self::inspection_error)?;
            let metadata = file.metadata()?;
            Self::reject_unsafe_attributes(&metadata)?;
            let before_identity = Self::identity_from_open_file(path, &metadata, &file)?;
            if max_bytes > MAX_EXECUTION_FINGERPRINT_BYTES {
                return Err(PlatformError::VerificationLimitExceeded {
                    limit_bytes: MAX_EXECUTION_FINGERPRINT_BYTES,
                });
            }
            if metadata.file_size() > max_bytes {
                return Err(PlatformError::VerificationLimitExceeded {
                    limit_bytes: max_bytes,
                });
            }
            let content_digest = if include_content_digest {
                Some(Self::hash_open_file(
                    &mut file,
                    metadata.file_size(),
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
                    total_bytes: metadata.file_size(),
                });
                None
            };
            let after = file.metadata()?;
            if after.file_size() != metadata.file_size()
                || after.last_write_time() != metadata.last_write_time()
            {
                return Err(PlatformError::Precondition(
                    "source changed while it was being fingerprinted".to_owned(),
                ));
            }
            let after_identity = Self::identity_from_open_file(path, &after, &file)?;
            if before_identity.volume.stable_identifier != after_identity.volume.stable_identifier
                || before_identity.object_key != after_identity.object_key
                || before_identity.parent_key != after_identity.parent_key
                || before_identity.leaf_name != after_identity.leaf_name
                || before_identity.link_count != after_identity.link_count
                || after_identity.reparse_tag.is_some()
            {
                return Err(PlatformError::Precondition(
                    "source identity changed while it was being fingerprinted".to_owned(),
                ));
            }
            Ok(FileFingerprint {
                native_identity: after_identity,
                byte_size: after.file_size(),
                modified_at_ns: Self::metadata_ns(after.last_write_time()),
                created_at_ns: Self::metadata_ns(after.creation_time()),
                attributes: after.file_attributes().into(),
                quick_digest: None,
                content_digest,
            })
        }
    }

    #[cfg(feature = "mutation")]
    impl SafeFileOperations for WindowsPlatform {
        fn validate_destination_absent(&self, path: &Path) -> Result<(), PlatformError> {
            let (parent, leaf) = Self::parent_and_leaf(path)?;
            Self::destination_absent_from_parent(&parent, &leaf)
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
            if request.expected_identity.link_count != 1
                || request.expected_identity.reparse_tag.is_some()
                || !matches!(
                    request.expected_identity.volume.platform,
                    PlatformKind::Windows
                )
                || !request.expected_identity.volume.local
                || request.expected_identity.volume.removable
                || !request
                    .expected_identity
                    .volume
                    .filesystem_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("NTFS"))
            {
                return Err(PlatformError::Precondition(
                    "source identity is not a single-link local NTFS file".to_owned(),
                ));
            }
            let (source_parent, source_leaf) = Self::parent_and_leaf(&request.source)?;
            let (destination_parent, destination_leaf) =
                Self::destination_parent_and_leaf(&request.destination)?;
            let mut file = Self::open_relative(
                &source_parent,
                &source_leaf,
                DELETE | FILE_READ_ATTRIBUTES | GENERIC_READ,
                FILE_SHARE_READ,
                false,
                FILE_OPEN,
            )?;
            Self::reject_unsafe_handle(file.as_raw_handle() as HANDLE)?;
            Self::destination_absent_from_parent(&destination_parent, &destination_leaf)?;

            let destination_volume = Self::volume_from_handle(
                &request.destination,
                destination_parent.as_raw_handle() as HANDLE,
            )?;
            if destination_volume.stable_identifier
                != request.expected_identity.volume.stable_identifier
                || !destination_volume.local
                || destination_volume.removable
                || !destination_volume
                    .filesystem_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("NTFS"))
            {
                return Err(PlatformError::Precondition(
                    "destination parent volume is not the approved local NTFS volume".to_owned(),
                ));
            }
            let metadata = file.metadata()?;
            Self::reject_unsafe_attributes(&metadata)?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 {
                return Err(PlatformError::PermissionDenied);
            }
            let observed_identity = Self::identity_from_held_parent(
                &request.source,
                &metadata,
                &file,
                &source_parent,
                &source_leaf,
            )?;
            if !Self::exact_identity_matches(&request.expected_identity, &observed_identity) {
                return Err(PlatformError::Precondition(
                    "source native identity changed".to_owned(),
                ));
            }
            if metadata.file_size() != request.expected_byte_size
                || Self::metadata_ns(metadata.last_write_time()) != request.expected_modified_at_ns
                || u64::from(metadata.file_attributes()) != request.expected_attributes
            {
                return Err(PlatformError::Precondition(
                    "source size or modified time changed".to_owned(),
                ));
            }
            let observed_digest = Self::hash_open_file(
                &mut file,
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
            let after_hash = file.metadata()?;
            Self::reject_unsafe_attributes(&after_hash)?;
            let after_hash_identity = Self::identity_from_held_parent(
                &request.source,
                &after_hash,
                &file,
                &source_parent,
                &source_leaf,
            )?;
            if !Self::exact_identity_matches(&request.expected_identity, &after_hash_identity)
                || after_hash.file_size() != request.expected_byte_size
                || Self::metadata_ns(after_hash.last_write_time())
                    != request.expected_modified_at_ns
                || u64::from(after_hash.file_attributes()) != request.expected_attributes
            {
                return Err(PlatformError::Precondition(
                    "source changed during final native verification".to_owned(),
                ));
            }

            // This is the only mutation boundary. Destination absence was
            // advisory; the kernel no-replace call remains authoritative.
            Self::set_rename_no_replace(&file, &destination_parent, &destination_leaf)?;

            match Self::destination_absent_from_parent(&source_parent, &source_leaf) {
                Ok(()) => {}
                Err(_) => return Err(PlatformError::AmbiguousMutationOutcome),
            }
            let destination_file = Self::open_relative(
                &destination_parent,
                &destination_leaf,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
                FILE_OPEN,
            )
            .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
            let metadata = destination_file.metadata()?;
            Self::reject_unsafe_attributes(&metadata)
                .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
            let observed_identity = Self::identity_from_held_parent(
                &request.destination,
                &metadata,
                &destination_file,
                &destination_parent,
                &destination_leaf,
            )
            .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
            let (_, held_object_key, _) =
                Self::identity_from_handle(file.as_raw_handle() as HANDLE)
                    .map_err(|_| PlatformError::AmbiguousMutationOutcome)?;
            if observed_identity.volume.stable_identifier
                != request.expected_identity.volume.stable_identifier
                || observed_identity.object_key != request.expected_identity.object_key
                || observed_identity.object_key.as_slice() != held_object_key
                || observed_identity.parent_key
                    != Self::identity_from_handle(destination_parent.as_raw_handle() as HANDLE)
                        .map_err(|_| PlatformError::AmbiguousMutationOutcome)?
                        .1
                || observed_identity.link_count != 1
                || observed_identity.reparse_tag.is_some()
                || observed_identity.leaf_name != Self::native_path(Path::new(&destination_leaf))
                || metadata.file_size() != request.expected_byte_size
            {
                return Err(PlatformError::AmbiguousMutationOutcome);
            }
            Ok(RenameOutcome { observed_identity })
        }

        fn create_directory_no_replace(&self, path: &Path) -> Result<(), PlatformError> {
            let parent = path
                .parent()
                .ok_or_else(|| PlatformError::Unsupported("directory has no parent".to_owned()))?;
            let leaf = path
                .file_name()
                .ok_or_else(|| PlatformError::Unsupported("directory has no leaf".to_owned()))?;
            let parent = Self::open_anchored(parent, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, true)?;
            let _created = Self::open_relative(
                &parent,
                leaf,
                FILE_READ_ATTRIBUTES | DELETE,
                FILE_SHARE_READ,
                true,
                FILE_CREATE,
            )?;
            Ok(())
        }

        fn remove_directory_if_empty(&self, path: &Path) -> Result<(), PlatformError> {
            let directory =
                Self::open_anchored(path, DELETE | FILE_READ_ATTRIBUTES, FILE_SHARE_READ, true)?;
            let disposition = FILE_DISPOSITION_INFO_EX {
                Flags: FILE_DISPOSITION_FLAG_DELETE,
            };
            let size = u32::try_from(mem::size_of::<FILE_DISPOSITION_INFO_EX>())
                .map_err(|_| PlatformError::Unsupported("disposition overflow".to_owned()))?;
            // SAFETY: `directory` is an anchored handle and `disposition` is valid
            // fixed-size input. Windows rejects non-empty directories.
            let result = unsafe {
                SetFileInformationByHandle(
                    directory.as_raw_handle() as HANDLE,
                    FileDispositionInfoEx,
                    ptr::addr_of!(disposition).cast::<c_void>(),
                    size,
                )
            };
            if result == 0 {
                return Err(Self::last_windows_error(false));
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn long_verbatim_paths_are_prepared_component_by_component() {
            let path = (0..40).fold(PathBuf::from(r"\\?\C:\"), |path, index| {
                path.join(format!("component-{index:02}"))
            });
            let (root, components) = WindowsPlatform::drive_root_and_components(&path)
                .unwrap_or_else(|error| panic!("verbatim path should prepare: {error}"));
            assert_eq!(root, PathBuf::from(r"\\?\C:\"));
            assert_eq!(components.len(), 40);
            assert!(crate::is_legal_win32_mount_point(&root.to_string_lossy()));
        }

        #[test]
        fn github_runner_verbatim_temp_has_legal_win32_root() {
            let path = Path::new(r"\\?\D:\a\_temp\zemo-windows-qualification");
            let (root, names) = WindowsPlatform::drive_root_and_components(path)
                .unwrap_or_else(|error| panic!("runner temp should prepare: {error}"));
            assert_eq!(root, PathBuf::from(r"\\?\D:\"));
            assert_eq!(
                names
                    .iter()
                    .map(|name| name.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                ["a", "_temp", "zemo-windows-qualification"]
            );
            assert!(crate::is_legal_win32_mount_point(&root.to_string_lossy()));
            assert!(!root.to_string_lossy().ends_with(r"\\"));
        }

        #[test]
        fn dos_drive_temp_has_legal_win32_root() {
            let path = Path::new(r"D:\a\_temp\zemo-windows-qualification");
            let (root, _) = WindowsPlatform::drive_root_and_components(path)
                .unwrap_or_else(|error| panic!("DOS temp should prepare: {error}"));
            assert_eq!(root, PathBuf::from(r"D:\"));
            assert!(crate::is_legal_win32_mount_point(&root.to_string_lossy()));
        }

        #[test]
        fn github_runner_components_are_relative_object_names() {
            for path in [
                Path::new(r"D:\a\_temp\zemo-windows-qualification\zemo-windows-qualification-diag"),
                Path::new(
                    r"\\?\D:\a\_temp\zemo-windows-qualification\zemo-windows-qualification-diag",
                ),
            ] {
                let (root, names) = WindowsPlatform::drive_root_and_components(path)
                    .unwrap_or_else(|error| panic!("{path:?}: {error}"));
                assert!(crate::is_legal_win32_mount_point(&root.to_string_lossy()));
                for name in &names {
                    assert!(
                        crate::relative_object_name_is_legal(&name.to_string_lossy()),
                        "ObjectName must be relative to RootDirectory, got {name:?}"
                    );
                }
            }
        }

        #[test]
        fn directory_create_options_used_by_open_relative_are_legal() {
            let directory = crate::anchored_create_options(true);
            let file = crate::anchored_create_options(false);
            assert!(crate::directory_create_options_are_legal(directory));
            assert_eq!(directory & crate::FILE_OPEN_NO_RECALL, 0);
            assert_eq!(file & crate::FILE_DIRECTORY_FILE, 0);
            assert_eq!(
                file & crate::FILE_OPEN_NO_RECALL,
                crate::FILE_OPEN_NO_RECALL
            );
        }

        #[cfg(all(feature = "mutation", target_pointer_width = "64"))]
        #[test]
        fn nt_rename_information_layout_is_x64_wdk() {
            assert_eq!(
                mem::offset_of!(FILE_RENAME_INFORMATION, RootDirectory),
                8,
                "RootDirectory must be 8-byte aligned on x64"
            );
            assert_eq!(mem::offset_of!(FILE_RENAME_INFORMATION, FileNameLength), 16);
            assert_eq!(mem::align_of::<FILE_RENAME_INFORMATION>(), 8);
            assert!(crate::rename_flags_are_no_replace(0));
        }

        #[test]
        fn open_anchored_inspects_temp_child_directory_and_file_on_dos_and_verbatim() {
            let temporary = tempfile::Builder::new()
                .prefix("zemo-windows-qualification-")
                .tempdir()
                .unwrap_or_else(|error| panic!("temp root: {error}"));
            let nested = temporary
                .path()
                .join("a")
                .join("_temp")
                .join("zemo-windows-qualification")
                .join("zemo-windows-qualification-diag");
            fs::create_dir_all(&nested).unwrap_or_else(|error| panic!("nested root: {error}"));
            let child_dir = nested.join("child-dir");
            fs::create_dir(&child_dir).unwrap_or_else(|error| panic!("child dir: {error}"));
            let child_file = child_dir.join("child-file.txt");
            fs::write(&child_file, b"anchored-identity")
                .unwrap_or_else(|error| panic!("child file: {error}"));

            let volume = WindowsPlatform
                .inspect_volume(&nested)
                .unwrap_or_else(|error| panic!("inspect_volume: {error}"));
            assert!(volume.local);
            assert!(
                volume
                    .filesystem_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("NTFS")),
                "expected NTFS, got {:?}",
                volume.filesystem_type
            );

            let directory = WindowsPlatform::open_anchored(
                &child_dir,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                true,
            )
            .unwrap_or_else(|error| panic!("open_anchored directory: {error}"));
            let file = WindowsPlatform::open_anchored(
                &child_file,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )
            .unwrap_or_else(|error| panic!("open_anchored file: {error}"));
            let (serial, file_id, links) =
                WindowsPlatform::identity_from_handle(file.as_raw_handle() as HANDLE)
                    .unwrap_or_else(|error| panic!("FileIdInfo: {error}"));
            assert_ne!(serial, 0, "volume serial must be present");
            assert_ne!(file_id, [0_u8; 16], "file id must be present");
            assert_eq!(links, 1);
            drop(directory);
            drop(file);

            let fingerprint = WindowsPlatform
                .fingerprint(&child_file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
                .unwrap_or_else(|error| panic!("fingerprint: {error}"));
            assert_eq!(fingerprint.native_identity.object_key.as_slice(), &file_id);

            let verbatim = fs::canonicalize(&nested)
                .unwrap_or_else(|error| panic!("canonicalize nested: {error}"));
            assert!(
                verbatim.to_string_lossy().starts_with(r"\\?\"),
                "canonical path should be verbatim: {verbatim:?}"
            );
            WindowsPlatform
                .inspect_volume(&verbatim)
                .unwrap_or_else(|error| panic!("verbatim inspect_volume: {error}"));
            WindowsPlatform::open_anchored(
                &verbatim.join("child-dir").join("child-file.txt"),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                false,
            )
            .unwrap_or_else(|error| panic!("verbatim open_anchored file: {error}"));

            let linked = child_dir.join("linked.txt");
            if std::os::windows::fs::symlink_file(&child_file, &linked).is_ok() {
                assert!(matches!(
                    WindowsPlatform::open_anchored(
                        &linked,
                        FILE_READ_ATTRIBUTES,
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        false,
                    ),
                    Err(PlatformError::ReparsePoint)
                ));
            }
        }

        #[test]
        fn prefix_kind_and_win32_root_for_dos_verbatim_mixed_and_unicode() {
            for (path, verbatim, expected_root, expected_leaf) in [
                (r"D:\folder\file.txt", false, r"D:\", "file.txt"),
                (r"\\?\D:\folder\file.txt", true, r"\\?\D:\", "file.txt"),
                (r"d:\Folder\File.txt", false, r"D:\", "File.txt"),
                (
                    r"D:\dossier\facture-été.txt",
                    false,
                    r"D:\",
                    "facture-été.txt",
                ),
                (r"D:\inbox\facture-🎉.txt", false, r"D:\", "facture-🎉.txt"),
            ] {
                let parsed = Path::new(path);
                let prefix = match parsed.components().next() {
                    Some(Component::Prefix(component)) => component.kind(),
                    other => panic!("{path}: expected Prefix, got {other:?}"),
                };
                assert_eq!(
                    matches!(prefix, Prefix::VerbatimDisk(_)),
                    verbatim,
                    "{path}"
                );
                assert_eq!(matches!(prefix, Prefix::Disk(_)), !verbatim, "{path}");
                let (root, names) = WindowsPlatform::drive_root_and_components(parsed)
                    .unwrap_or_else(|error| panic!("{path}: {error}"));
                assert_eq!(root, PathBuf::from(expected_root), "{path}");
                assert!(crate::is_legal_win32_mount_point(&root.to_string_lossy()));
                assert!(!root.to_string_lossy().ends_with(r"\\"), "{path}");
                assert_eq!(
                    names.last().map(|name| name.to_string_lossy().into_owned()),
                    Some(expected_leaf.to_owned()),
                    "{path}"
                );
            }
        }

        #[test]
        fn native_path_preparation_rejects_ads_devices_and_traversal() {
            for path in [
                Path::new(r"C:\safe\document.txt:stream"),
                Path::new(r"C:\safe\CON.txt"),
                Path::new(r"C:\safe\name. "),
                Path::new(r"C:\safe\..\escape.txt"),
            ] {
                assert!(matches!(
                    WindowsPlatform::drive_root_and_components(path),
                    Err(PlatformError::PathPolicyRefusal | PlatformError::OutsideRoot)
                ));
            }
        }

        #[cfg(feature = "mutation")]
        mod mutation {
            use super::*;
            use std::{
                fs::OpenOptions,
                os::windows::fs::{OpenOptionsExt, symlink_dir, symlink_file},
            };
            use tempfile::TempDir;

            struct MutationSandbox {
                _temporary: TempDir,
                root: PathBuf,
            }

            impl MutationSandbox {
                fn new() -> Self {
                    let temporary = tempfile::tempdir()
                        .unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
                    let root = temporary
                        .path()
                        .canonicalize()
                        .unwrap_or_else(|error| panic!("sandbox should canonicalize: {error}"));
                    let temporary_root = std::env::temp_dir()
                        .canonicalize()
                        .unwrap_or_else(|error| panic!("temp root should canonicalize: {error}"));
                    assert!(root.starts_with(&temporary_root));
                    Self {
                        _temporary: temporary,
                        root,
                    }
                }

                fn path(&self, relative: &str) -> PathBuf {
                    let path = self.root.join(relative);
                    assert!(path.starts_with(&self.root));
                    path
                }
            }

            fn request(source: PathBuf, destination: PathBuf) -> RenameRequest {
                let fingerprint = WindowsPlatform
                    .fingerprint(&source, true, domain::MAX_EXECUTION_VERIFICATION_BYTES)
                    .unwrap_or_else(|error| panic!("source should fingerprint: {error}"));
                RenameRequest {
                    source,
                    destination,
                    expected_identity: fingerprint.native_identity,
                    expected_byte_size: fingerprint.byte_size,
                    expected_modified_at_ns: fingerprint.modified_at_ns,
                    expected_attributes: fingerprint.attributes,
                    expected_content_digest: fingerprint
                        .content_digest
                        .unwrap_or_else(|| panic!("digest should be present")),
                    maximum_hash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
                }
            }

            #[test]
            fn no_replace_refuses_existing_and_case_insensitive_destinations() {
                for destination_name in ["occupied.txt", "SOURCE.txt"] {
                    let sandbox = MutationSandbox::new();
                    let source = sandbox.path("source.txt");
                    let destination = sandbox.path(destination_name);
                    fs::write(&source, b"source")
                        .unwrap_or_else(|error| panic!("source should be written: {error}"));
                    if destination_name != "SOURCE.txt" {
                        fs::write(&destination, b"occupied").unwrap_or_else(|error| {
                            panic!("destination should be written: {error}")
                        });
                    }
                    let result = WindowsPlatform
                        .rename_same_volume_no_replace(&request(source.clone(), destination));
                    assert!(matches!(result, Err(PlatformError::DestinationExists)));
                    assert_eq!(
                        fs::read(source)
                            .unwrap_or_else(|error| panic!("source should remain: {error}")),
                        b"source"
                    );
                }
            }

            #[test]
            fn sharing_violation_is_structured_before_mutation() {
                let sandbox = MutationSandbox::new();
                let source = sandbox.path("locked.txt");
                let destination = sandbox.path("moved.txt");
                fs::write(&source, b"locked")
                    .unwrap_or_else(|error| panic!("source should be written: {error}"));
                let request = request(source.clone(), destination);
                let _lock = OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&source)
                    .unwrap_or_else(|error| panic!("exclusive lock should open: {error}"));

                assert!(matches!(
                    WindowsPlatform.rename_same_volume_no_replace(&request),
                    Err(PlatformError::SharingViolation | PlatformError::LockViolation)
                ));
                assert!(source.is_file());
            }

            #[test]
            fn read_only_refusal_never_changes_attributes() {
                let sandbox = MutationSandbox::new();
                let source = sandbox.path("read-only.txt");
                let destination = sandbox.path("moved.txt");
                fs::write(&source, b"read only")
                    .unwrap_or_else(|error| panic!("source should be written: {error}"));
                let mut permissions = fs::metadata(&source)
                    .unwrap_or_else(|error| panic!("metadata should load: {error}"))
                    .permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&source, permissions)
                    .unwrap_or_else(|error| panic!("read-only bit should be set: {error}"));
                let request = request(source.clone(), destination);
                let before = fs::metadata(&source)
                    .unwrap_or_else(|error| panic!("metadata should load: {error}"))
                    .file_attributes();

                assert!(matches!(
                    WindowsPlatform.rename_same_volume_no_replace(&request),
                    Err(PlatformError::PermissionDenied)
                ));
                let after = fs::metadata(&source)
                    .unwrap_or_else(|error| panic!("metadata should load: {error}"))
                    .file_attributes();
                assert_eq!(before, after);
            }

            #[test]
            fn reparse_leaf_is_refused_without_touching_target() {
                let sandbox = MutationSandbox::new();
                let target = sandbox.path("target.txt");
                let linked = sandbox.path("linked.txt");
                fs::write(&target, b"target")
                    .unwrap_or_else(|error| panic!("target should be written: {error}"));
                if symlink_file(&target, &linked).is_err() {
                    // Creating symlinks can require Developer Mode. Native
                    // qualification exercises this path in an enabled sandbox.
                    return;
                }
                assert!(matches!(
                    WindowsPlatform.fingerprint(
                        &linked,
                        true,
                        domain::MAX_EXECUTION_VERIFICATION_BYTES
                    ),
                    Err(PlatformError::ReparsePoint)
                ));
                assert_eq!(
                    fs::read(target)
                        .unwrap_or_else(|error| panic!("target should remain: {error}")),
                    b"target"
                );
            }

            #[test]
            fn reparse_directory_ancestor_is_refused_without_touching_target() {
                let sandbox = MutationSandbox::new();
                let target_directory = sandbox.path("target-directory");
                let linked_directory = sandbox.path("linked-directory");
                fs::create_dir(&target_directory)
                    .unwrap_or_else(|error| panic!("target directory should be created: {error}"));
                let target = target_directory.join("target.txt");
                fs::write(&target, b"target")
                    .unwrap_or_else(|error| panic!("target should be written: {error}"));
                if symlink_dir(&target_directory, &linked_directory).is_err() {
                    return;
                }

                assert!(matches!(
                    WindowsPlatform.fingerprint(
                        &linked_directory.join("target.txt"),
                        true,
                        domain::MAX_EXECUTION_VERIFICATION_BYTES
                    ),
                    Err(PlatformError::ReparsePoint)
                ));
                assert_eq!(
                    fs::read(target)
                        .unwrap_or_else(|error| panic!("target should remain: {error}")),
                    b"target"
                );
            }
        }
    }
}

#[cfg(windows)]
pub use windows::{VolumePathDiagnostics, WindowsPlatform};

#[cfg(not(windows))]
#[derive(Debug, Default)]
pub struct WindowsPlatform;

#[cfg(not(windows))]
impl platform::ReadOnlyPlatform for WindowsPlatform {
    fn inspect_volume(
        &self,
        _root: &std::path::Path,
    ) -> Result<domain::VolumeIdentity, platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }

    fn enumerate_regular_files(
        &self,
        _root: &std::path::Path,
        _max_entries: usize,
        _is_cancelled: &dyn Fn() -> bool,
        _on_progress: &mut dyn FnMut(platform::EnumerationProgress),
    ) -> Result<platform::ReadOnlyEnumeration, platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }

    fn read_bounded(
        &self,
        _path: &std::path::Path,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }

    fn read_prefix(
        &self,
        _path: &std::path::Path,
        _max_bytes: usize,
    ) -> Result<Vec<u8>, platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }

    fn fingerprint(
        &self,
        _path: &std::path::Path,
        _include_content_digest: bool,
        _max_bytes: u64,
    ) -> Result<domain::FileFingerprint, platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }
}

#[cfg(all(not(windows), feature = "mutation"))]
impl platform::SafeFileOperations for WindowsPlatform {
    fn rename_same_volume_no_replace(
        &self,
        _request: &platform::RenameRequest,
    ) -> Result<platform::RenameOutcome, platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }

    fn create_directory_no_replace(
        &self,
        _path: &std::path::Path,
    ) -> Result<(), platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }

    fn remove_directory_if_empty(
        &self,
        _path: &std::path::Path,
    ) -> Result<(), platform::PlatformError> {
        Err(platform::PlatformError::Unsupported(
            "Windows adapter is unavailable on this platform".to_owned(),
        ))
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    use platform::ReadOnlyPlatform;

    #[test]
    fn windows_adapter_always_exposes_read_only_capability() {
        fn assert_read_only<T: ReadOnlyPlatform>() {}
        assert_read_only::<WindowsPlatform>();
    }

    #[cfg(not(feature = "mutation"))]
    #[test]
    fn default_build_has_no_mutation_capability() {
        const {
            assert!(!MUTATION_CAPABILITY_COMPILED);
        }
    }
}
