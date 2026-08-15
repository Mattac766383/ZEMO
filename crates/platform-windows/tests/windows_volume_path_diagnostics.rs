#![cfg(windows)]

//! Qualification-only volume/path diagnostics. Not used by production UI.

use platform_windows::WindowsPlatform;
use std::fs;

#[test]
fn prints_volume_path_identity_for_qualification_temp() {
    let input = std::env::temp_dir().join("zemo-windows-qualification-diag");
    fs::create_dir_all(&input).unwrap_or_else(|error| panic!("diag sandbox: {error}"));
    let diagnostics = WindowsPlatform::volume_path_diagnostics(&input);
    eprintln!("input path: {}", diagnostics.input_path);
    eprintln!("absolute path: {}", diagnostics.absolute_path);
    eprintln!("canonical path: {}", diagnostics.canonical_path);
    eprintln!("prefix kind: {}", diagnostics.prefix_kind);
    eprintln!("volume root: {}", diagnostics.volume_root);
    eprintln!("DOS root: {}", diagnostics.dos_root);
    eprintln!("Win32 root: {}", diagnostics.win32_root);
    eprintln!(
        "Win32 volume path passed to API: {}",
        diagnostics.win32_volume_path
    );
    eprintln!("UTF-16 representation length: {}", diagnostics.utf16_len);
    eprintln!("GetVolumePathNameW: {}", diagnostics.get_volume_path_name);
    eprintln!(
        "GetVolumeInformationW: {}",
        diagnostics.get_volume_information
    );
    eprintln!("GetDriveTypeW: {}", diagnostics.get_drive_type);
    eprintln!("filesystem name: {:?}", diagnostics.filesystem_name);
    eprintln!("volume serial/identity: {:?}", diagnostics.volume_identity);
    eprintln!("case-sensitivity result: {:?}", diagnostics.case_sensitive);
    eprintln!("GetLastError: {:?}", diagnostics.last_error);
    eprintln!("ERROR 87 present: {}", diagnostics.error_87);
    if let Some(error) = &diagnostics.inspect_error {
        eprintln!("inspect error: {error}");
    }
    eprintln!("Win32 API trace:");
    for line in &diagnostics.win32_api_trace {
        eprintln!("  {line}");
    }
    assert!(
        !diagnostics.error_87,
        "ERROR_INVALID_PARAMETER (87) still present: last_error={:?} inspect={:?}",
        diagnostics.last_error, diagnostics.inspect_error
    );
    assert!(
        diagnostics.inspect_error.is_none(),
        "volume inspect failed: {:?}",
        diagnostics.inspect_error
    );
    assert!(
        diagnostics
            .filesystem_name
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("NTFS")),
        "expected NTFS, observed {:?}",
        diagnostics.filesystem_name
    );
    assert!(
        platform_windows::is_legal_win32_mount_point(&diagnostics.win32_volume_path)
            || platform_windows::is_legal_win32_mount_point(&diagnostics.win32_root)
            || platform_windows::is_legal_win32_mount_point(&diagnostics.dos_root),
        "Win32 mount point must have exactly one trailing backslash: {:?} / {:?} / {:?}",
        diagnostics.win32_volume_path,
        diagnostics.win32_root,
        diagnostics.dos_root
    );
    assert!(
        diagnostics.dos_root.ends_with('\\') && !diagnostics.dos_root.ends_with("\\\\"),
        "DOS root must be X:\\ with exactly one trailing slash: {:?}",
        diagnostics.dos_root
    );
}
