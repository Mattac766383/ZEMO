#![cfg(windows)]

//! Native Windows path-identity matrix. No mutation outside the qualification temp.

use platform::{MAX_EXECUTION_FINGERPRINT_BYTES, PlatformError, ReadOnlyPlatform};
use platform_windows::WindowsPlatform;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::Builder;

fn qualification_root() -> PathBuf {
    let root = std::env::temp_dir()
        .join("zemo-windows-qualification")
        .join("path-matrix");
    fs::create_dir_all(&root).unwrap_or_else(|error| panic!("path-matrix root: {error}"));
    root
}

fn assert_open_inspect_fingerprint(root: &Path, child: &Path, file: &Path) {
    let volume = WindowsPlatform
        .inspect_volume(root)
        .unwrap_or_else(|error| panic!("inspect root {root:?}: {error}"));
    assert!(volume.local, "root must be local: {root:?}");
    assert!(
        volume
            .filesystem_type
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("NTFS")),
        "root must be NTFS: {root:?} {:?}",
        volume.filesystem_type
    );

    let child_volume = WindowsPlatform
        .inspect_volume(child)
        .unwrap_or_else(|error| panic!("inspect child {child:?}: {error}"));
    assert_eq!(
        child_volume.stable_identifier, volume.stable_identifier,
        "child directory must stay on the same volume"
    );

    let fingerprint = WindowsPlatform
        .fingerprint(file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("fingerprint {file:?}: {error}"));
    assert_eq!(fingerprint.native_identity.object_key.len(), 16);
    assert_ne!(
        fingerprint.native_identity.object_key.as_slice(),
        [0_u8; 16]
    );
    assert_eq!(
        fingerprint.native_identity.volume.stable_identifier,
        volume.stable_identifier
    );
}

#[test]
fn dos_and_verbatim_temp_tree_share_stable_identity() {
    let root = qualification_root().join("D-temp-root");
    let child = root.join("child");
    let file = child.join("file.txt");
    fs::create_dir_all(&child).unwrap_or_else(|error| panic!("child: {error}"));
    fs::write(&file, b"path-matrix").unwrap_or_else(|error| panic!("file: {error}"));

    assert_open_inspect_fingerprint(&root, &child, &file);

    let verbatim_root =
        fs::canonicalize(&root).unwrap_or_else(|error| panic!("canonicalize root: {error}"));
    assert!(
        verbatim_root.to_string_lossy().starts_with(r"\\?\"),
        "canonical form should be verbatim: {verbatim_root:?}"
    );
    let verbatim_child = verbatim_root.join("child");
    let verbatim_file = verbatim_child.join("file.txt");
    assert_open_inspect_fingerprint(&verbatim_root, &verbatim_child, &verbatim_file);

    let dos = WindowsPlatform
        .fingerprint(&file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("dos fingerprint: {error}"));
    let verbatim = WindowsPlatform
        .fingerprint(&verbatim_file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("verbatim fingerprint: {error}"));
    assert_eq!(
        dos.native_identity.object_key,
        verbatim.native_identity.object_key
    );
    assert_eq!(
        dos.native_identity.volume.stable_identifier,
        verbatim.native_identity.volume.stable_identifier
    );
}

#[test]
fn unicode_accented_and_emoji_leaves_keep_identity() {
    let root = qualification_root().join("unicode");
    fs::create_dir_all(&root).unwrap_or_else(|error| panic!("unicode root: {error}"));
    for name in ["facture-été.txt", "facture-🎉.txt", "客户-фактура.txt"] {
        let path = root.join(name);
        fs::write(&path, name.as_bytes()).unwrap_or_else(|error| panic!("{name}: {error}"));
        let fingerprint = WindowsPlatform
            .fingerprint(&path, true, MAX_EXECUTION_FINGERPRINT_BYTES)
            .unwrap_or_else(|error| panic!("fingerprint {name}: {error}"));
        assert_eq!(
            fingerprint.byte_size,
            u64::try_from(name.len()).unwrap_or(0)
        );
        assert!(
            !fingerprint.native_identity.leaf_name.bytes.is_empty(),
            "leaf bytes must be present for {name}"
        );
    }
}

#[test]
fn long_path_over_win32_max_path_is_addressable() {
    let root = qualification_root().join("long");
    let mut current = root.clone();
    for index in 0..20 {
        current.push(format!("component-{index:02}-xxxxxxxxxxxxxxxxxxxx"));
    }
    fs::create_dir_all(&current).unwrap_or_else(|error| panic!("long dir: {error}"));
    let file = current.join("leaf.txt");
    fs::write(&file, b"long-path").unwrap_or_else(|error| panic!("long file: {error}"));
    let displayed = file.display().to_string();
    assert!(
        displayed.len() > 260
            || file
                .canonicalize()
                .map(|p| p.display().to_string().len() > 260)
                .unwrap_or(true),
        "fixture must exceed MAX_PATH: {displayed}"
    );
    WindowsPlatform
        .inspect_volume(&current)
        .unwrap_or_else(|error| panic!("long inspect: {error}"));
    let fingerprint = WindowsPlatform
        .fingerprint(&file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("long fingerprint: {error}"));
    assert_eq!(fingerprint.byte_size, 9);
}

#[test]
fn case_only_path_difference_is_the_same_ntfs_object() {
    let root = qualification_root().join("case");
    fs::create_dir_all(&root).unwrap_or_else(|error| panic!("case root: {error}"));
    let lower = root.join("foo.txt");
    fs::write(&lower, b"case").unwrap_or_else(|error| panic!("foo.txt: {error}"));
    let upper = root.join("FOO.TXT");
    let lower_id = WindowsPlatform
        .fingerprint(&lower, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("lower: {error}"));
    let upper_id = WindowsPlatform
        .fingerprint(&upper, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("upper: {error}"));
    assert_eq!(
        lower_id.native_identity.object_key,
        upper_id.native_identity.object_key
    );
}

#[test]
fn file_id_info_is_stable_across_dos_and_verbatim_opens() {
    let temporary = Builder::new()
        .prefix("supremacy-m15-sandbox-matrix-")
        .tempdir()
        .unwrap_or_else(|error| panic!("sandbox: {error}"));
    let file = temporary.path().join("id.txt");
    fs::write(&file, b"id").unwrap_or_else(|error| panic!("id file: {error}"));
    let first = WindowsPlatform
        .fingerprint(&file, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("first: {error}"));
    let canonical = fs::canonicalize(&file).unwrap_or_else(|error| panic!("canonical: {error}"));
    let second = WindowsPlatform
        .fingerprint(&canonical, true, MAX_EXECUTION_FINGERPRINT_BYTES)
        .unwrap_or_else(|error| panic!("second: {error}"));
    assert_eq!(
        first.native_identity.object_key,
        second.native_identity.object_key
    );
}

#[test]
fn missing_file_fingerprint_is_source_missing() {
    let path = qualification_root().join("definitely-absent-fingerprint.txt");
    let _ = fs::remove_file(&path);
    assert!(
        !path.exists(),
        "fixture must be absent before fingerprint: {path:?}"
    );
    let error = WindowsPlatform.fingerprint(&path, true, MAX_EXECUTION_FINGERPRINT_BYTES);
    assert!(
        matches!(error, Err(PlatformError::SourceMissing)),
        "absent path must be SourceMissing for restart reconciliation, got {error:?}"
    );
}

#[test]
fn reparse_leaf_is_refused_when_the_host_can_create_one() {
    let root = qualification_root().join("reparse");
    fs::create_dir_all(&root).unwrap_or_else(|error| panic!("reparse root: {error}"));
    let target = root.join("target.txt");
    let link = root.join("link.txt");
    fs::write(&target, b"target").unwrap_or_else(|error| panic!("target: {error}"));
    if std::os::windows::fs::symlink_file(&target, &link).is_err() {
        return;
    }
    assert!(matches!(
        WindowsPlatform.fingerprint(&link, true, MAX_EXECUTION_FINGERPRINT_BYTES),
        Err(PlatformError::ReparsePoint)
    ));
}
