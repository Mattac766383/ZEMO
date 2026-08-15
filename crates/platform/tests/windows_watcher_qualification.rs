#![cfg(windows)]

//! Native Windows monitoring / watcher qualification.
//! Uses temporary sandboxes only; does not alter monitoring business policy.

use platform::{ChangeMonitor, ChangeScope, LocalChangeMonitor, LocalEventKind, ReadOnlyPlatform};
use platform_windows::WindowsPlatform;
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use tempfile::{Builder, TempDir};

const FORBIDDEN_PROFILE_DIRS: &[&str] = &["Documents", "Desktop", "Downloads"];

fn m15_sandbox() -> TempDir {
    let dir = Builder::new()
        .prefix("supremacy-m15-sandbox-watch-")
        .tempdir()
        .unwrap_or_else(|error| panic!("watch sandbox: {error}"));
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|error| panic!("temp root: {error}"));
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize: {error}"));
    assert!(root.starts_with(&temporary_root));
    for forbidden in FORBIDDEN_PROFILE_DIRS {
        assert!(
            !root
                .components()
                .any(|component| component.as_os_str() == *forbidden),
            "watcher sandbox must not use {forbidden}"
        );
    }
    let volume = WindowsPlatform
        .inspect_volume(&root)
        .unwrap_or_else(|error| panic!("volume inspect: {error}"));
    assert!(
        volume.local,
        "monitoring qualification requires a local volume"
    );
    assert!(
        !volume.removable,
        "monitoring qualification requires a non-removable volume"
    );
    dir
}

fn scoped(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    assert!(path.starts_with(root) && path != root);
    path
}

fn wait_for_hint<F>(monitor: &LocalChangeMonitor, timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut(&[platform::ChangeHint]) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let hints = monitor
            .drain_hints()
            .unwrap_or_else(|error| panic!("drain: {error}"));
        if predicate(&hints) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

fn started_monitor(root: &Path) -> LocalChangeMonitor {
    let monitor = LocalChangeMonitor::default();
    monitor
        .start(root)
        .unwrap_or_else(|error| panic!("start watcher: {error}"));
    monitor
}

fn canonical_root(sandbox: &TempDir) -> PathBuf {
    sandbox
        .path()
        .canonicalize()
        .unwrap_or_else(|error| panic!("root: {error}"))
}

#[test]
fn windows_watcher_observes_create() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    fs::create_dir_all(scoped(&root, "nested"))
        .unwrap_or_else(|error| panic!("nested dir: {error}"));
    let monitor = started_monitor(&root);
    let created = scoped(&root, "nested/created.txt");
    fs::write(&created, b"one").unwrap_or_else(|error| panic!("create: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                hint.kind == LocalEventKind::Created
                    && hint
                        .path_after
                        .as_ref()
                        .is_some_and(|path| path.ends_with("created.txt"))
            })
        }),
        "create event missing"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop watcher: {error}"));
}

#[test]
fn windows_watcher_observes_modify() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let path = scoped(&root, "modified.txt");
    fs::write(&path, b"one").unwrap_or_else(|error| panic!("seed: {error}"));
    let monitor = started_monitor(&root);
    fs::write(&path, b"two").unwrap_or_else(|error| panic!("modify: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                matches!(
                    hint.kind,
                    LocalEventKind::Modified | LocalEventKind::Created
                ) && hint
                    .path_after
                    .as_ref()
                    .is_some_and(|observed| observed.ends_with("modified.txt"))
            })
        }),
        "modify event missing"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop watcher: {error}"));
}

#[test]
fn windows_watcher_observes_rename() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let created = scoped(&root, "created.txt");
    fs::write(&created, b"one").unwrap_or_else(|error| panic!("seed: {error}"));
    let monitor = started_monitor(&root);
    let renamed = scoped(&root, "renamed.txt");
    fs::rename(&created, &renamed).unwrap_or_else(|error| panic!("rename: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                hint.kind == LocalEventKind::Moved
                    || hint
                        .path_after
                        .as_ref()
                        .is_some_and(|path| path.ends_with("renamed.txt"))
                    || hint
                        .path_before
                        .as_ref()
                        .is_some_and(|path| path.ends_with("created.txt"))
            })
        }),
        "rename/move event missing"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop watcher: {error}"));
}

#[test]
fn windows_watcher_observes_delete() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let path = scoped(&root, "deleted.txt");
    fs::write(&path, b"one").unwrap_or_else(|error| panic!("seed: {error}"));
    let monitor = started_monitor(&root);
    fs::remove_file(&path).unwrap_or_else(|error| panic!("delete: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                hint.kind == LocalEventKind::Removed
                    || hint
                        .path_before
                        .as_ref()
                        .is_some_and(|observed| observed.ends_with("deleted.txt"))
            })
        }),
        "delete event missing"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop watcher: {error}"));
}

#[test]
fn windows_watcher_observes_directory_rename() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let dir_a = scoped(&root, "folder-a");
    let dir_b = scoped(&root, "folder-b");
    fs::create_dir(&dir_a).unwrap_or_else(|error| panic!("dir create: {error}"));
    let monitor = started_monitor(&root);
    let _ = wait_for_hint(&monitor, Duration::from_secs(1), |_| true);
    fs::rename(&dir_a, &dir_b).unwrap_or_else(|error| panic!("dir rename: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                hint.scope == ChangeScope::Directory
                    || hint
                        .path_after
                        .as_ref()
                        .is_some_and(|path| path.ends_with("folder-b"))
                    || hint
                        .path_before
                        .as_ref()
                        .is_some_and(|path| path.ends_with("folder-a"))
            })
        }),
        "directory rename event missing"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop watcher: {error}"));
}

#[test]
fn windows_watcher_survives_burst() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let monitor = started_monitor(&root);
    for index in 0..40 {
        let path = scoped(&root, &format!("burst-{index}.txt"));
        fs::write(&path, format!("burst-{index}"))
            .unwrap_or_else(|error| panic!("burst write: {error}"));
    }
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(5), |hints| !hints.is_empty()),
        "burst should produce watcher hints"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop watcher: {error}"));
}

#[test]
fn windows_watcher_survives_restart() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let monitor = started_monitor(&root);
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop before restart: {error}"));
    monitor
        .start(&root)
        .unwrap_or_else(|error| panic!("restart watcher: {error}"));
    let recovery = scoped(&root, "after-restart.txt");
    fs::write(&recovery, b"recovered").unwrap_or_else(|error| panic!("recovery write: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                hint.path_after
                    .as_ref()
                    .is_some_and(|path| path.ends_with("after-restart.txt"))
            })
        }),
        "watcher should observe events after restart"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("final stop: {error}"));
}

#[test]
fn windows_watcher_handles_unicode_path_encoding() {
    let sandbox = m15_sandbox();
    let root = canonical_root(&sandbox);
    let monitor = started_monitor(&root);
    let path = scoped(&root, "café-文档.txt");
    fs::write(&path, "été").unwrap_or_else(|error| panic!("unicode write: {error}"));
    assert!(
        wait_for_hint(&monitor, Duration::from_secs(3), |hints| {
            hints.iter().any(|hint| {
                hint.path_after.as_ref().is_some_and(|observed| {
                    observed
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().contains("café"))
                })
            })
        }),
        "unicode path encoding should survive watcher events"
    );
    monitor
        .stop()
        .unwrap_or_else(|error| panic!("stop: {error}"));
}
