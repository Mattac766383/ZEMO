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
    eprintln!("volume root: {}", diagnostics.volume_root);
    eprintln!(
        "Win32 volume path passed to API: {}",
        diagnostics.win32_volume_path
    );
    eprintln!("UTF-16 representation length: {}", diagnostics.utf16_len);
    eprintln!("filesystem name: {:?}", diagnostics.filesystem_name);
    eprintln!("volume serial/identity: {:?}", diagnostics.volume_identity);
    eprintln!("case-sensitivity result: {:?}", diagnostics.case_sensitive);
    if let Some(error) = &diagnostics.inspect_error {
        eprintln!("inspect error: {error}");
    }
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
            || platform_windows::is_legal_win32_mount_point(&diagnostics.volume_root),
        "Win32 mount point must have exactly one trailing backslash: {:?} / {:?}",
        diagnostics.win32_volume_path,
        diagnostics.volume_root
    );
}
