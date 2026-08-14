//! Sandbox containment guards for Windows qualification fixtures.
//! Runs on all hosts for the static policy checks; Windows hosts also probe
//! native temporary roots.

#[cfg(windows)]
use std::path::PathBuf;
use std::path::{Component, Path};
use tempfile::Builder;

const FORBIDDEN_PROFILE_DIRS: &[&str] = &["Documents", "Desktop", "Downloads"];

fn assert_not_profile_corpus(path: &Path) {
    for component in path.components() {
        if let Component::Normal(name) = component {
            for forbidden in FORBIDDEN_PROFILE_DIRS {
                assert_ne!(
                    name, *forbidden,
                    "qualification path must not enter {forbidden}: {path:?}"
                );
            }
        }
    }
}

#[test]
fn m15_sandbox_prefix_stays_under_process_temp_and_outside_profile_dirs() {
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|error| panic!("temp root: {error}"));
    let sandbox = Builder::new()
        .prefix("supremacy-m15-sandbox-")
        .tempdir()
        .unwrap_or_else(|error| panic!("sandbox: {error}"));
    let root = sandbox
        .path()
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize: {error}"));
    assert!(
        root.starts_with(&temporary_root),
        "sandbox escaped process temp: {root:?} vs {temporary_root:?}"
    );
    assert_not_profile_corpus(&root);

    let nested = root.join("nested").join("file.txt");
    assert!(nested.starts_with(&root));
    assert_not_profile_corpus(&nested);
}

#[test]
fn forbidden_profile_directory_names_are_explicitly_listed() {
    // Keep the policy list honest for the harness static check.
    assert_eq!(
        FORBIDDEN_PROFILE_DIRS,
        &["Documents", "Desktop", "Downloads"]
    );
}

#[cfg(windows)]
#[test]
fn windows_temp_sandbox_rejects_system_root_candidates() {
    let sandbox = Builder::new()
        .prefix("supremacy-m15-sandbox-")
        .tempdir()
        .unwrap_or_else(|error| panic!("sandbox: {error}"));
    let root = sandbox
        .path()
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize: {error}"));
    let system =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
    assert!(
        !root.starts_with(&system),
        "sandbox must not be under SystemRoot"
    );
    for forbidden in ["Program Files", "Program Files (x86)", "Windows"] {
        assert!(
            !root
                .components()
                .any(|component| component.as_os_str() == forbidden),
            "sandbox must not use system directory {forbidden}"
        );
    }
}
