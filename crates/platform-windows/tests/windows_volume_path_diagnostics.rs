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

#[test]
fn open_anchored_child_directory_and_file_on_runner_temp_forms() {
    use platform::{MAX_EXECUTION_FINGERPRINT_BYTES, ReadOnlyPlatform};
    use std::path::Path;

    let nested = std::env::temp_dir()
        .join("zemo-windows-qualification")
        .join("zemo-windows-qualification-diag")
        .join("open-anchored-child");
    fs::create_dir_all(&nested).unwrap_or_else(|error| panic!("diag nested: {error}"));
    let child_dir = nested.join("child-dir");
    fs::create_dir_all(&child_dir).unwrap_or_else(|error| panic!("child dir: {error}"));
    let child_file = child_dir.join("child-file.txt");
    fs::write(&child_file, b"open-anchored-identity")
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

    let fingerprint = WindowsPlatform
        .fingerprint(&child_file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("open_anchored file fingerprint: {error}"));
    assert_eq!(fingerprint.byte_size, 22);
    assert_eq!(fingerprint.native_identity.object_key.len(), 16);
    assert_ne!(
        fingerprint.native_identity.object_key.as_slice(),
        [0_u8; 16].as_slice()
    );

    let directory_volume = WindowsPlatform
        .inspect_volume(&child_dir)
        .unwrap_or_else(|error| panic!("open_anchored child directory: {error}"));
    assert_eq!(directory_volume.stable_identifier, volume.stable_identifier);

    let verbatim =
        fs::canonicalize(&nested).unwrap_or_else(|error| panic!("canonicalize: {error}"));
    assert!(
        verbatim.to_string_lossy().contains(":\\")
            || verbatim.to_string_lossy().starts_with(r"\\?\"),
        "runner path should be DOS or verbatim: {verbatim:?}"
    );
    WindowsPlatform
        .inspect_volume(&verbatim)
        .unwrap_or_else(|error| panic!("verbatim inspect: {error}"));
    WindowsPlatform
        .fingerprint(
            &verbatim.join("child-dir").join("child-file.txt"),
            true,
            MAX_EXECUTION_FINGERPRINT_BYTES,
        )
        .unwrap_or_else(|error| panic!("verbatim fingerprint: {error}"));

    let dos_display = nested.display().to_string();
    if !dos_display.starts_with(r"\\?\") {
        WindowsPlatform
            .inspect_volume(Path::new(&dos_display))
            .unwrap_or_else(|error| panic!("DOS inspect: {error}"));
    }

    let linked = child_dir.join("linked.txt");
    if std::os::windows::fs::symlink_file(&child_file, &linked).is_ok() {
        assert!(
            matches!(
                WindowsPlatform.fingerprint(&linked, true, MAX_EXECUTION_FINGERPRINT_BYTES),
                Err(platform::PlatformError::ReparsePoint)
            ),
            "reparse traversal must remain rejected"
        );
    }
}
