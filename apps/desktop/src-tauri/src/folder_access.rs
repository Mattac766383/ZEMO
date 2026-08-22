//! Probe and authorize standard personal folders without exposing OS internals.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

pub const ACCESS_ACCESSIBLE: &str = "accessible";
pub const ACCESS_AUTHORIZATION_REQUIRED: &str = "authorization_required";
pub const ACCESS_MISSING: &str = "missing";
pub const ACCESS_UNSUPPORTED: &str = "unsupported";
pub const ACCESS_LOCKED: &str = "locked";
pub const ACCESS_PERMISSION_DENIED: &str = "permission_denied";
pub const ACCESS_TEMPORARILY_UNAVAILABLE: &str = "temporarily_unavailable";
pub const ACCESS_UNEXPECTED: &str = "unexpected_error";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserContentKind {
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Movies,
    Music,
}

impl UserContentKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Documents => "documents",
            Self::Downloads => "downloads",
            Self::Pictures => "pictures",
            Self::Movies => "movies",
            Self::Music => "music",
        }
    }

    #[must_use]
    pub fn display_label_fr(self) -> &'static str {
        match self {
            Self::Desktop => "Bureau",
            Self::Documents => "Documents",
            Self::Downloads => "Téléchargements",
            Self::Pictures => "Images",
            Self::Movies => "Vidéos",
            Self::Music => "Musique",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "desktop" => Some(Self::Desktop),
            "documents" => Some(Self::Documents),
            "downloads" => Some(Self::Downloads),
            "pictures" => Some(Self::Pictures),
            "movies" => Some(Self::Movies),
            "music" => Some(Self::Music),
            _ => None,
        }
    }

    #[must_use]
    pub fn all() -> [Self; 6] {
        [
            Self::Desktop,
            Self::Documents,
            Self::Downloads,
            Self::Pictures,
            Self::Movies,
            Self::Music,
        ]
    }

    #[must_use]
    pub fn recommended(self) -> bool {
        !matches!(self, Self::Music)
    }

    /// Resolve the real OS path. Never uses localized Finder/Explorer labels.
    #[must_use]
    pub fn resolve_native_path(self) -> Option<PathBuf> {
        let raw = match self {
            Self::Desktop => dirs::desktop_dir(),
            Self::Documents => dirs::document_dir(),
            Self::Downloads => dirs::download_dir(),
            Self::Pictures => dirs::picture_dir(),
            Self::Movies => dirs::video_dir(),
            Self::Music => dirs::audio_dir(),
        }?;
        Some(resolve_user_content_path(&raw))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderAccessProbe {
    pub logical_name: String,
    pub kind: String,
    pub display_label: String,
    pub resolved_path: String,
    pub exists: bool,
    pub is_dir: bool,
    pub readable: bool,
    pub writable: bool,
    pub recommended: bool,
    pub raw_os_error: Option<i32>,
    pub platform_error: Option<String>,
    pub access_state: String,
    pub human_status: String,
    pub canonical_path: String,
    pub failed_stage: Option<String>,
    pub error_kind: Option<String>,
    pub inspect_result: Option<String>,
    pub technical_details: String,
}

impl FolderAccessProbe {
    #[must_use]
    pub fn can_scan(&self) -> bool {
        self.access_state == ACCESS_ACCESSIBLE && self.resolved_path_buf().is_some()
    }

    #[must_use]
    pub fn resolved_path_buf(&self) -> Option<PathBuf> {
        if self.resolved_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(&self.resolved_path))
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BookmarkFile {
    folders: BTreeMap<String, BookmarkEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BookmarkEntry {
    absolute_path: String,
}

#[must_use]
pub fn human_status_for(state: &str, label: &str) -> String {
    match state {
        ACCESS_ACCESSIBLE => format!("✓ {label}"),
        ACCESS_AUTHORIZATION_REQUIRED => format!("{label} — Autorisation nécessaire"),
        ACCESS_MISSING => format!("{label} — Indisponible"),
        ACCESS_UNSUPPORTED => format!("{label} — Non pris en charge"),
        ACCESS_LOCKED => format!("{label} — Utilisé par une autre application"),
        ACCESS_PERMISSION_DENIED => format!("{label} — Accès refusé"),
        ACCESS_TEMPORARILY_UNAVAILABLE => format!("{label} — Pas disponible localement"),
        _ => format!("{label} — Impossible à analyser"),
    }
}

#[must_use]
pub fn human_message_for(state: &str) -> &'static str {
    match state {
        ACCESS_ACCESSIBLE => "ZEMO peut analyser ce dossier.",
        ACCESS_AUTHORIZATION_REQUIRED => {
            "ZEMO a besoin de votre autorisation pour accéder à ce dossier."
        }
        ACCESS_MISSING => "Ce dossier est introuvable.",
        ACCESS_UNSUPPORTED => "Ce dossier ne peut pas être rangé.",
        ACCESS_LOCKED => "Ce dossier est utilisé par une autre application.",
        ACCESS_PERMISSION_DENIED => "ZEMO n’a pas accès à ce dossier.",
        ACCESS_TEMPORARILY_UNAVAILABLE => "Ce fichier n’est pas disponible localement.",
        _ => "ZEMO n’a pas pu analyser ce dossier.",
    }
}

/// Follow a short user-content symlink chain when the target stays in the home/iCloud area.
#[must_use]
pub fn resolve_user_content_path(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..3 {
        if is_forbidden_path(&current) {
            return path.to_path_buf();
        }
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            return current;
        };
        if !metadata.file_type().is_symlink() {
            return current;
        }
        let Ok(target) = fs::read_link(&current) else {
            return current;
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            current
                .parent()
                .map(|parent| parent.join(&target))
                .unwrap_or(target)
        };
        if is_forbidden_path(&resolved) || !is_safe_user_content_target(&resolved) {
            return path.to_path_buf();
        }
        current = resolved;
    }
    current
}

#[must_use]
pub fn is_forbidden_path(path: &Path) -> bool {
    let normalized = normalize_path(path);
    if normalized == "/" || matches!(normalized.as_str(), "c:" | "c:/" | "c:\\") {
        return true;
    }
    forbidden_markers().iter().any(|marker| {
        normalized == *marker
            || normalized.starts_with(&format!("{marker}/"))
            || normalized.starts_with(&format!("{marker}\\"))
    })
}

fn forbidden_markers() -> &'static [&'static str] {
    &[
        "/system",
        "/library",
        "/applications",
        "/private/var/db",
        "/usr",
        "/bin",
        "/sbin",
        "/opt/homebrew",
        "c:/windows",
        "c:/windows/system32",
        "c:/program files",
        "c:/program files (x86)",
        "c:/programdata",
        "/windows",
        "/program files",
        "/program files (x86)",
        "/programdata",
    ]
}

fn is_safe_user_content_target(path: &Path) -> bool {
    if is_forbidden_path(path) {
        return false;
    }
    let normalized = normalize_path(path);
    if normalized.contains("/library/mobile documents") || normalized.contains("icloud") {
        return true;
    }
    if let Some(home) = dirs::home_dir() {
        let home_norm = normalize_path(&home);
        if normalized.starts_with(&home_norm) {
            return !normalized.contains("/library/") || normalized.contains("mobile documents");
        }
    }
    // OneDrive redirected known folders often live outside the default profile tree
    // but are still user content (e.g. D:\OneDrive\Desktop).
    normalized.contains("onedrive")
        || normalized.contains("/users/")
        || normalized.contains("/home/")
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

#[must_use]
pub fn classify_io_error(error: &io::Error) -> &'static str {
    match error.kind() {
        ErrorKind::NotFound => ACCESS_MISSING,
        ErrorKind::PermissionDenied => {
            if cfg!(windows) {
                ACCESS_PERMISSION_DENIED
            } else {
                ACCESS_AUTHORIZATION_REQUIRED
            }
        }
        ErrorKind::WouldBlock => ACCESS_LOCKED,
        _ => {
            let classified = classify_os_code(error.raw_os_error());
            if classified == ACCESS_UNEXPECTED && cfg!(target_os = "macos") {
                ACCESS_AUTHORIZATION_REQUIRED
            } else {
                classified
            }
        }
    }
}

fn classify_os_code(code: Option<i32>) -> &'static str {
    if cfg!(windows) {
        return match code {
            Some(5) => ACCESS_PERMISSION_DENIED,
            Some(32 | 33) => ACCESS_LOCKED,
            Some(21 | 53 | 64 | 433) => ACCESS_TEMPORARILY_UNAVAILABLE,
            _ => ACCESS_UNEXPECTED,
        };
    }
    match code {
        Some(1 | 13) => ACCESS_AUTHORIZATION_REQUIRED, // EPERM / EACCES
        Some(16 | 11) => ACCESS_LOCKED,                // EBUSY / EAGAIN
        Some(375 | 376 | 377) => ACCESS_TEMPORARILY_UNAVAILABLE,
        _ => ACCESS_UNEXPECTED,
    }
}

#[must_use]
pub fn probe_kind(kind: UserContentKind, store_dir: Option<&Path>) -> FolderAccessProbe {
    let override_path = store_dir.and_then(|dir| load_bookmark(dir, kind.as_str()));
    let resolved = override_path.or_else(|| kind.resolve_native_path());
    probe_resolved(kind, resolved)
}

fn probe_resolved(kind: UserContentKind, resolved: Option<PathBuf>) -> FolderAccessProbe {
    let label = kind.display_label_fr();
    let Some(path) = resolved else {
        return FolderAccessProbe {
            logical_name: kind.as_str().to_owned(),
            kind: kind.as_str().to_owned(),
            display_label: label.to_owned(),
            resolved_path: String::new(),
            exists: false,
            is_dir: false,
            readable: false,
            writable: false,
            recommended: kind.recommended(),
            raw_os_error: None,
            platform_error: Some("unresolved_known_folder".to_owned()),
            access_state: ACCESS_MISSING.to_owned(),
            human_status: human_status_for(ACCESS_MISSING, label),
            canonical_path: String::new(),
            failed_stage: Some("resolve_native_path".to_owned()),
            error_kind: Some("unresolved_known_folder".to_owned()),
            inspect_result: None,
            technical_details: technical_details(
                kind.as_str(),
                "",
                "",
                Some("resolve_native_path"),
                None,
                Some("unresolved_known_folder"),
                None,
                ACCESS_MISSING,
            ),
        };
    };

    if path.parent().is_none() || is_forbidden_path(&path) {
        return finished_probe(
            kind,
            &path,
            false,
            false,
            false,
            false,
            None,
            Some("protected_system_or_program_path".to_owned()),
            Some("forbidden_path".to_owned()),
            Some("protected_system_or_program_path".to_owned()),
            ACCESS_UNSUPPORTED,
        );
    }

    let meta = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) => {
            let state = classify_io_error(&error);
            return finished_probe(
                kind,
                &path,
                false,
                false,
                false,
                false,
                error.raw_os_error(),
                Some(platform_error_code(&error)),
                Some("symlink_metadata".to_owned()),
                Some(format!("{:?}", error.kind())),
                state,
            );
        }
    };

    let exists = true;
    let is_dir = meta.is_dir() || meta.file_type().is_symlink();
    if !meta.is_dir() && !meta.file_type().is_symlink() {
        return finished_probe(
            kind,
            &path,
            exists,
            false,
            false,
            false,
            None,
            Some("not_a_directory".to_owned()),
            Some("symlink_metadata".to_owned()),
            Some("not_a_directory".to_owned()),
            ACCESS_MISSING,
        );
    }

    let read = fs::read_dir(&path);
    let (readable, raw_os_error, platform_error, read_state, failed_stage, error_kind) = match read
    {
        Ok(_) => (true, None, None, ACCESS_ACCESSIBLE, None, None),
        Err(error) => (
            false,
            error.raw_os_error(),
            Some(platform_error_code(&error)),
            classify_io_error(&error),
            Some("read_dir".to_owned()),
            Some(format!("{:?}", error.kind())),
        ),
    };
    let writable = readable && !meta.permissions().readonly();
    let access_state = if exists && !readable && read_state == ACCESS_UNEXPECTED {
        ACCESS_AUTHORIZATION_REQUIRED
    } else {
        read_state
    };

    finished_probe(
        kind,
        &path,
        exists,
        is_dir,
        readable,
        writable,
        raw_os_error,
        platform_error,
        failed_stage,
        error_kind,
        access_state,
    )
}

#[allow(clippy::too_many_arguments)]
fn finished_probe(
    kind: UserContentKind,
    path: &Path,
    exists: bool,
    is_dir: bool,
    readable: bool,
    writable: bool,
    raw_os_error: Option<i32>,
    platform_error: Option<String>,
    failed_stage: Option<String>,
    error_kind: Option<String>,
    access_state: &str,
) -> FolderAccessProbe {
    let canonical_path = fs::canonicalize(path)
        .ok()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let probe = FolderAccessProbe {
        logical_name: kind.as_str().to_owned(),
        kind: kind.as_str().to_owned(),
        display_label: kind.display_label_fr().to_owned(),
        resolved_path: path.to_string_lossy().into_owned(),
        exists,
        is_dir,
        readable,
        writable,
        recommended: kind.recommended(),
        raw_os_error,
        platform_error: platform_error.clone(),
        access_state: access_state.to_owned(),
        human_status: human_status_for(access_state, kind.display_label_fr()),
        canonical_path: canonical_path.clone(),
        failed_stage: failed_stage.clone(),
        error_kind: error_kind.clone(),
        inspect_result: None,
        technical_details: technical_details(
            kind.as_str(),
            &path.to_string_lossy(),
            &canonical_path,
            failed_stage.as_deref(),
            raw_os_error,
            error_kind.as_deref(),
            platform_error.as_deref(),
            access_state,
        ),
    };
    log_probe(&probe);
    probe
}

#[allow(clippy::too_many_arguments)]
pub fn technical_details(
    folder: &str,
    path: &str,
    canonical: &str,
    stage: Option<&str>,
    errno: Option<i32>,
    error_kind: Option<&str>,
    platform_error: Option<&str>,
    access_state: &str,
) -> String {
    format!(
        "Folder: {folder}\nPath: {path}\nCanonical: {canonical}\nStage: {}\nerrno: {}\nErrorKind: {}\nPlatformError: {}\nAccessState: {access_state}",
        stage.unwrap_or("ok"),
        errno
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        error_kind.unwrap_or("none"),
        platform_error.unwrap_or("none"),
    )
}

pub fn with_inspect_outcome(
    mut probe: FolderAccessProbe,
    inspect_result: Result<String, (Option<i32>, String, String, &'static str)>,
) -> FolderAccessProbe {
    match inspect_result {
        Ok(detail) => {
            probe.inspect_result = Some(detail);
            if probe.failed_stage.is_none() {
                probe.failed_stage = None;
            }
        }
        Err((raw_os_error, error_kind, platform_error, state)) => {
            probe.inspect_result = Some(platform_error.clone());
            probe.failed_stage = Some("inspect_volume".to_owned());
            probe.error_kind = Some(error_kind);
            probe.raw_os_error = raw_os_error.or(probe.raw_os_error);
            probe.platform_error = Some(platform_error);
            probe.readable = false;
            probe.access_state = state.to_owned();
            probe.human_status = human_status_for(state, &probe.display_label);
        }
    }
    probe.technical_details = technical_details(
        &probe.logical_name,
        &probe.resolved_path,
        &probe.canonical_path,
        probe.failed_stage.as_deref(),
        probe.raw_os_error,
        probe.error_kind.as_deref(),
        probe.platform_error.as_deref(),
        &probe.access_state,
    );
    if let Some(inspect) = probe.inspect_result.as_ref() {
        probe
            .technical_details
            .push_str(&format!("\nInspect: {inspect}"));
    }
    log_probe(&probe);
    probe
}

fn platform_error_code(error: &io::Error) -> String {
    format!("{:?}", error.kind())
}

fn log_probe(probe: &FolderAccessProbe) {
    eprintln!(
        "ZEMO folder access: logical_name={} resolved_path={} canonical={} exists={} is_dir={} readable={} writable_if_needed={} raw_os_error={:?} platform_error={:?} failed_stage={:?} error_kind={:?} inspect_result={:?} access_state={}",
        probe.logical_name,
        probe.resolved_path,
        probe.canonical_path,
        probe.exists,
        probe.is_dir,
        probe.readable,
        probe.writable,
        probe.raw_os_error,
        probe.platform_error,
        probe.failed_stage,
        probe.error_kind,
        probe.inspect_result,
        probe.access_state
    );
}

#[must_use]
pub fn probe_recommended(store_dir: Option<&Path>) -> Vec<FolderAccessProbe> {
    UserContentKind::all()
        .into_iter()
        .filter(|kind| kind.recommended())
        .map(|kind| probe_kind(kind, store_dir))
        .collect()
}

pub fn persist_authorized_path(store_dir: &Path, kind: &str, path: &Path) -> io::Result<()> {
    fs::create_dir_all(store_dir)?;
    let file = store_dir.join("folder-access.json");
    let mut store = load_store(&file);
    store.folders.insert(
        kind.to_owned(),
        BookmarkEntry {
            absolute_path: path.to_string_lossy().into_owned(),
        },
    );
    fs::write(file, serde_json::to_vec_pretty(&store).unwrap_or_default())
}

fn load_bookmark(store_dir: &Path, kind: &str) -> Option<PathBuf> {
    let store = load_store(&store_dir.join("folder-access.json"));
    store
        .folders
        .get(kind)
        .map(|entry| PathBuf::from(&entry.absolute_path))
        .filter(|path| path.exists() && !is_forbidden_path(path))
}

fn load_store(file: &Path) -> BookmarkFile {
    fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// After the user picked a folder, accept it only if it is still personal content.
#[must_use]
pub fn accept_authorized_selection(kind: UserContentKind, selected: &Path) -> Option<PathBuf> {
    let resolved = resolve_user_content_path(selected);
    if is_forbidden_path(&resolved) || resolved.parent().is_none() {
        return None;
    }
    if !resolved.is_dir() {
        return None;
    }
    let expected = kind.resolve_native_path();
    if let Some(expected) = expected {
        if same_dir(&resolved, &expected) {
            return Some(resolved);
        }
        // User chose another personal folder instead of the blocked known folder.
        if is_safe_user_content_target(&resolved) {
            return Some(resolved);
        }
        return None;
    }
    is_safe_user_content_target(&resolved).then_some(resolved)
}

fn same_dir(left: &Path, right: &Path) -> bool {
    let left_c = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right_c = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left_c == right_c || normalize_path(&left_c) == normalize_path(&right_c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_system_and_program_paths() {
        assert!(is_forbidden_path(Path::new("/System/Library")));
        assert!(is_forbidden_path(Path::new("/Applications/Notes.app")));
        assert!(is_forbidden_path(Path::new(r"C:\Windows\System32")));
        assert!(is_forbidden_path(Path::new(r"C:\Program Files\App")));
        assert!(!is_forbidden_path(Path::new("/Users/ada/Desktop")));
        assert!(!is_forbidden_path(Path::new(r"C:\Users\ada\Documents")));
    }

    #[test]
    fn classifies_permission_without_os_jargon() {
        let error = io::Error::from(ErrorKind::PermissionDenied);
        let state = classify_io_error(&error);
        if cfg!(windows) {
            assert_eq!(state, ACCESS_PERMISSION_DENIED);
        } else {
            assert_eq!(state, ACCESS_AUTHORIZATION_REQUIRED);
        }
        assert!(!human_message_for(state).contains("EACCES"));
        assert!(!human_message_for(state).contains("ACCESS_DENIED"));
    }

    #[test]
    fn persists_and_reuses_authorized_personal_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("Desktop");
        fs::create_dir(&folder).expect("personal folder");
        persist_authorized_path(dir.path(), "desktop", &folder).expect("persist");
        let loaded = load_bookmark(dir.path(), "desktop").expect("bookmark");
        assert_eq!(loaded, folder);
    }

    #[test]
    fn live_standard_folders_resolve_native_paths() {
        for kind in UserContentKind::all()
            .into_iter()
            .filter(|item| item.recommended())
        {
            let probe = probe_kind(kind, None);
            eprintln!(
                "ZEMO live probe: logical_name={} resolved_path={} exists={} is_dir={} readable={} writable_if_needed={} raw_os_error={:?} platform_error={:?} access_state={}",
                probe.logical_name,
                probe.resolved_path,
                probe.exists,
                probe.is_dir,
                probe.readable,
                probe.writable,
                probe.raw_os_error,
                probe.platform_error,
                probe.access_state
            );
            let path_lower = probe.resolved_path.replace('\\', "/").to_ascii_lowercase();
            assert!(
                !path_lower.ends_with("/bureau")
                    && !path_lower.contains("/téléchargements")
                    && !path_lower.contains("/telechargements"),
                "must resolve native paths, not localized labels: {}",
                probe.resolved_path
            );
            if let Some(expected) = kind.resolve_native_path() {
                assert_eq!(Path::new(&probe.resolved_path), expected.as_path());
            }
        }
    }

    #[test]
    fn macos_uncategorized_access_failure_requires_authorization() {
        let error = io::Error::new(ErrorKind::Other, "Operation not permitted");
        if cfg!(target_os = "macos") {
            assert_eq!(classify_io_error(&error), ACCESS_AUTHORIZATION_REQUIRED);
        }
    }

    #[test]
    fn classifies_missing_folder() {
        let error = io::Error::from(ErrorKind::NotFound);
        assert_eq!(classify_io_error(&error), ACCESS_MISSING);
    }

    #[test]
    fn human_status_never_exposes_os_codes() {
        let text = human_status_for(ACCESS_AUTHORIZATION_REQUIRED, "Documents");
        assert!(text.contains("Autorisation nécessaire"));
        assert!(!text.to_ascii_lowercase().contains("eacces"));
        assert!(!text.to_ascii_lowercase().contains("tcc"));
        assert!(!text.contains("ACCESS_DENIED"));
    }

    #[test]
    fn recommended_scope_matches_product() {
        assert!(UserContentKind::Desktop.recommended());
        assert!(UserContentKind::Movies.recommended());
        assert!(!UserContentKind::Music.recommended());
    }
}
