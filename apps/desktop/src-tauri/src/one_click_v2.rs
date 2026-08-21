use crate::folder_access::{probe_kind, UserContentKind, ACCESS_ACCESSIBLE};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const REVIEW: &str = "À vérifier";
const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OneClickMove {
    pub source: String,
    pub destination: String,
    pub category: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OneClickRootResult {
    pub kind: String,
    pub display_label: String,
    pub root: String,
    pub files_seen: u64,
    pub proposed_moves: Vec<OneClickMove>,
    pub skipped: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OneClickPlan {
    pub plan_id: String,
    pub roots: Vec<OneClickRootResult>,
    pub files_seen: u64,
    pub proposed_moves: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppliedMove {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OneClickApplyResult {
    pub applied: Vec<AppliedMove>,
    pub skipped: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalPayload {
    version: u32,
    plan_id: String,
    created_at_unix_ms: u128,
    moves: Vec<AppliedMove>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEnvelope {
    payload: JournalPayload,
    mac: String,
}

#[derive(Clone)]
pub struct OneClickV2State {
    inner: Arc<OneClickV2Inner>,
}

struct OneClickV2Inner {
    current_plan: Mutex<Option<OneClickPlan>>,
    journal_path: PathBuf,
    journal_key: [u8; 32],
}

impl OneClickV2State {
    #[must_use]
    pub fn new(journal_path: PathBuf, journal_key: [u8; 32]) -> Self {
        Self {
            inner: Arc::new(OneClickV2Inner {
                current_plan: Mutex::new(None),
                journal_path,
                journal_key,
            }),
        }
    }

    pub fn build_recommended_plan(&self) -> Result<OneClickPlan, String> {
        let mut roots = Vec::new();
        let mut files_seen = 0_u64;
        let mut proposed_moves = 0_u64;

        for kind in UserContentKind::all().into_iter().filter(|kind| kind.recommended()) {
            let probe = probe_kind(kind, None);
            if probe.access_state != ACCESS_ACCESSIBLE {
                roots.push(OneClickRootResult {
                    kind: kind.as_str().to_owned(),
                    display_label: kind.display_label_fr().to_owned(),
                    root: probe.resolved_path,
                    files_seen: 0,
                    proposed_moves: Vec::new(),
                    skipped: 0,
                    errors: vec![format!(
                        "ACCESS {}: {}",
                        probe.access_state, probe.human_status
                    )],
                });
                continue;
            }
            let Some(root) = probe.resolved_path_buf() else {
                continue;
            };
            let result = plan_root(&root, kind.as_str(), kind.display_label_fr());
            files_seen = files_seen.saturating_add(result.files_seen);
            proposed_moves = proposed_moves.saturating_add(result.proposed_moves.len() as u64);
            roots.push(result);
        }

        let plan = OneClickPlan {
            plan_id: new_plan_id(),
            roots,
            files_seen,
            proposed_moves,
        };
        let mut guard = self
            .inner
            .current_plan
            .lock()
            .map_err(|_| "one-click plan state is unavailable".to_owned())?;
        *guard = Some(plan.clone());
        Ok(plan)
    }

    pub fn apply_current_plan(&self) -> Result<OneClickApplyResult, String> {
        let plan = self
            .inner
            .current_plan
            .lock()
            .map_err(|_| "one-click plan state is unavailable".to_owned())?
            .clone()
            .ok_or_else(|| "no one-click plan is ready".to_owned())?;
        let result = apply_plan(&plan);
        if !result.applied.is_empty() {
            self.write_journal(&plan.plan_id, &result.applied)?;
        }
        Ok(result)
    }

    pub fn undo_last(&self) -> Result<OneClickApplyResult, String> {
        let payload = self.read_journal()?;
        let result = undo(&payload.moves);
        if result.errors.is_empty() && result.skipped == 0 {
            match fs::remove_file(&self.inner.journal_path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("failed to clear undo journal: {error}")),
            }
        }
        Ok(result)
    }

    fn write_journal(&self, plan_id: &str, moves: &[AppliedMove]) -> Result<(), String> {
        if let Some(parent) = self.inner.journal_path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("journal directory: {error}"))?;
        }
        let payload = JournalPayload {
            version: JOURNAL_VERSION,
            plan_id: plan_id.to_owned(),
            created_at_unix_ms: now_unix_ms(),
            moves: moves.to_vec(),
        };
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|error| format!("journal serialization: {error}"))?;
        let mac = blake3::keyed_hash(&self.inner.journal_key, &payload_bytes)
            .to_hex()
            .to_string();
        let envelope = JournalEnvelope { payload, mac };
        let encoded = serde_json::to_vec(&envelope)
            .map_err(|error| format!("journal serialization: {error}"))?;
        let temporary = self.inner.journal_path.with_extension("tmp");
        fs::write(&temporary, encoded).map_err(|error| format!("journal write: {error}"))?;
        fs::rename(&temporary, &self.inner.journal_path)
            .map_err(|error| format!("journal commit: {error}"))?;
        Ok(())
    }

    fn read_journal(&self) -> Result<JournalPayload, String> {
        let encoded = fs::read(&self.inner.journal_path)
            .map_err(|error| format!("undo journal unavailable: {error}"))?;
        let envelope: JournalEnvelope = serde_json::from_slice(&encoded)
            .map_err(|error| format!("undo journal invalid: {error}"))?;
        if envelope.payload.version != JOURNAL_VERSION {
            return Err("undo journal version is unsupported".to_owned());
        }
        let payload_bytes = serde_json::to_vec(&envelope.payload)
            .map_err(|error| format!("undo journal invalid: {error}"))?;
        let expected = blake3::keyed_hash(&self.inner.journal_key, &payload_bytes)
            .to_hex()
            .to_string();
        if expected != envelope.mac {
            return Err("undo journal authentication failed".to_owned());
        }
        Ok(envelope.payload)
    }
}

pub fn build_plan(roots: &[PathBuf]) -> OneClickPlan {
    let mut results = Vec::new();
    let mut files_seen = 0_u64;
    let mut proposed = 0_u64;
    for root in roots {
        let result = plan_root(root, "fixture", "Fixture");
        files_seen = files_seen.saturating_add(result.files_seen);
        proposed = proposed.saturating_add(result.proposed_moves.len() as u64);
        results.push(result);
    }
    OneClickPlan {
        plan_id: new_plan_id(),
        roots: results,
        files_seen,
        proposed_moves: proposed,
    }
}

fn plan_root(root: &Path, kind: &str, display_label: &str) -> OneClickRootResult {
    let mut result = OneClickRootResult {
        kind: kind.to_owned(),
        display_label: display_label.to_owned(),
        root: root.to_string_lossy().into_owned(),
        files_seen: 0,
        proposed_moves: Vec::new(),
        skipped: 0,
        errors: Vec::new(),
    };
    let read = match fs::read_dir(root) {
        Ok(read) => read,
        Err(error) => {
            result.errors.push(format!("READ_DIR: {error}"));
            return result;
        }
    };
    let mut reserved = HashSet::new();
    for entry in read {
        let entry = match entry {
            Ok(value) => value,
            Err(error) => {
                result.errors.push(format!("ENTRY: {error}"));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(error) => {
                result.errors.push(format!("FILE_TYPE: {error}"));
                continue;
            }
        };
        if !file_type.is_file() || file_type.is_symlink() {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        }
        result.files_seen = result.files_seen.saturating_add(1);
        let name = entry.file_name().to_string_lossy().into_owned();
        if protected_name(&name) {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        }
        let (category, reason) = classify(&name);
        if category.is_empty() {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        }
        let destination = collision_safe_destination(root, category, &name, &mut reserved);
        if destination == path {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        }
        result.proposed_moves.push(OneClickMove {
            source: path.to_string_lossy().into_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category: category.to_owned(),
            reason: reason.to_owned(),
        });
    }
    result
}

fn classify(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_lowercase();
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if matches!(extension.as_str(), "dmg" | "pkg" | "msi")
        || (extension == "exe"
            && (lower.contains("setup")
                || lower.contains("install")
                || lower.contains("installer")))
    {
        return ("Installateurs", "installer");
    }
    if matches!(
        extension.as_str(),
        "lnk"
            | "url"
            | "webloc"
            | "desktop"
            | "alias"
            | "app"
            | "exe"
            | "com"
            | "scr"
            | "cpl"
            | "dll"
            | "sys"
            | "dylib"
            | "so"
            | "bundle"
            | "framework"
            | "sh"
            | "bash"
            | "zsh"
            | "command"
            | "bat"
            | "cmd"
            | "ps1"
            | "jar"
    ) {
        return ("", "protected_program_or_shortcut");
    }
    if matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tif" | "tiff" | "bmp" | "svg"
    ) {
        return ("Images", "image");
    }
    if matches!(
        extension.as_str(),
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v"
    ) {
        return ("Vidéos", "video");
    }
    if matches!(
        extension.as_str(),
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz"
    ) {
        return ("Archives", "archive");
    }
    if matches!(
        extension.as_str(),
        "pdf"
            | "doc"
            | "docx"
            | "odt"
            | "rtf"
            | "txt"
            | "md"
            | "xls"
            | "xlsx"
            | "ods"
            | "csv"
            | "ppt"
            | "pptx"
            | "odp"
    ) {
        if [
            "facture",
            "invoice",
            "contrat",
            "assurance",
            "releve",
            "relevé",
            "impot",
            "impôt",
            "cerfa",
            "bank",
        ]
        .iter()
        .any(|keyword| lower.contains(keyword))
        {
            return ("Documents/Administratif", "document_admin");
        }
        if [
            "cours",
            "devoir",
            "school",
            "etude",
            "étude",
            "universite",
            "université",
        ]
        .iter()
        .any(|keyword| lower.contains(keyword))
        {
            return ("Documents/Études", "document_study");
        }
        if [
            "client",
            "projet",
            "travail",
            "meeting",
            "reunion",
            "réunion",
        ]
        .iter()
        .any(|keyword| lower.contains(keyword))
        {
            return ("Documents/Travail", "document_work");
        }
        return ("Documents/Personnel", "document");
    }
    (REVIEW, "unknown_loose_file")
}

fn protected_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with('.') || matches!(lower.as_str(), "desktop.ini" | "thumbs.db" | ".ds_store")
}

fn collision_safe_destination(
    root: &Path,
    category: &str,
    name: &str,
    reserved: &mut HashSet<PathBuf>,
) -> PathBuf {
    let base = root.join(category).join(name);
    if !base.exists() && reserved.insert(base.clone()) {
        return base;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("fichier");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 2..10_000_u32 {
        let candidate_name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = root.join(category).join(candidate_name);
        if !candidate.exists() && reserved.insert(candidate.clone()) {
            return candidate;
        }
    }
    base
}

pub fn apply_plan(plan: &OneClickPlan) -> OneClickApplyResult {
    let mut output = OneClickApplyResult {
        applied: Vec::new(),
        skipped: 0,
        errors: Vec::new(),
    };
    for root in &plan.roots {
        if root.root.is_empty() {
            continue;
        }
        let root_path = PathBuf::from(&root.root);
        let canonical_root = match fs::canonicalize(&root_path) {
            Ok(value) => value,
            Err(error) => {
                output.errors.push(format!("ROOT: {error}"));
                continue;
            }
        };
        for movement in &root.proposed_moves {
            let source = PathBuf::from(&movement.source);
            let destination = PathBuf::from(&movement.destination);
            let metadata = match fs::symlink_metadata(&source) {
                Ok(value) => value,
                Err(_) => {
                    output.skipped = output.skipped.saturating_add(1);
                    continue;
                }
            };
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            }
            let Some(source_parent) = source.parent() else {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            };
            let canonical_source_parent = match fs::canonicalize(source_parent) {
                Ok(value) => value,
                Err(error) => {
                    output.errors.push(format!("SOURCE_PARENT: {error}"));
                    continue;
                }
            };
            if canonical_source_parent != canonical_root {
                output.errors.push("SAFETY: source is no longer a top-level root file".to_owned());
                continue;
            }
            if !destination.starts_with(&root_path) {
                output.errors.push("SAFETY: destination escaped root".to_owned());
                continue;
            }
            let Some(filename) = source.file_name().and_then(|value| value.to_str()) else {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            };
            let (current_category, _) = classify(filename);
            if current_category != movement.category || current_category.is_empty() {
                output.errors.push("DRIFT: file classification changed since preview".to_owned());
                continue;
            }
            if destination.exists() {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            }
            let Some(parent) = destination.parent() else {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            };
            if let Err(error) = fs::create_dir_all(parent) {
                output.errors.push(format!("MKDIR: {error}"));
                continue;
            }
            let canonical_parent = match fs::canonicalize(parent) {
                Ok(value) => value,
                Err(error) => {
                    output.errors.push(format!("PARENT: {error}"));
                    continue;
                }
            };
            if !canonical_parent.starts_with(&canonical_root) {
                output
                    .errors
                    .push("SAFETY: destination escaped canonical root".to_owned());
                continue;
            }
            match fs::rename(&source, &destination) {
                Ok(()) => output.applied.push(AppliedMove {
                    source: movement.source.clone(),
                    destination: movement.destination.clone(),
                }),
                Err(error) => output.errors.push(format!(
                    "MOVE {}: {error}",
                    source
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("file")
                )),
            }
        }
    }
    output
}

pub fn undo(applied: &[AppliedMove]) -> OneClickApplyResult {
    let mut output = OneClickApplyResult {
        applied: Vec::new(),
        skipped: 0,
        errors: Vec::new(),
    };
    for movement in applied.iter().rev() {
        let source = PathBuf::from(&movement.destination);
        let destination = PathBuf::from(&movement.source);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(value) => value,
            Err(_) => {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            }
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || destination.exists() {
            output.skipped = output.skipped.saturating_add(1);
            continue;
        }
        match fs::rename(&source, &destination) {
            Ok(()) => output.applied.push(AppliedMove {
                source: movement.destination.clone(),
                destination: movement.source.clone(),
            }),
            Err(error) => output.errors.push(format!("UNDO: {error}")),
        }
    }
    output
}

fn new_plan_id() -> String {
    let mut random = [0_u8; 8];
    let _ = getrandom::fill(&mut random);
    format!("{}-{}", now_unix_ms(), hex(&random))
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("zemo-one-click-v2-{}", new_plan_id()));
        fs::create_dir_all(&root).unwrap();
        let names = [
            "facture-2026.pdf",
            "photo.jpg",
            "clip.mp4",
            "backup.zip",
            "mystere.xyz",
            "raccourci.webloc",
        ];
        for index in 0..120 {
            let name = names[index % names.len()];
            fs::write(root.join(format!("{index:03}-{name}")), b"fixture").unwrap();
        }
        root
    }

    #[test]
    fn realistic_fixture_plan_apply_undo() {
        let root = fixture();
        let before = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count();
        assert_eq!(before, 120);
        let plan = build_plan(std::slice::from_ref(&root));
        assert_eq!(plan.files_seen, 120);
        assert!(plan.proposed_moves >= 100);
        let applied = apply_plan(&plan);
        assert!(applied.errors.is_empty(), "{:?}", applied.errors);
        assert!(applied.applied.len() >= 100);
        let loose_after = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count();
        assert!(loose_after < 30);
        let undone = undo(&applied.applied);
        assert!(undone.errors.is_empty(), "{:?}", undone.errors);
        let restored = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count();
        assert_eq!(restored, 120);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installer_exe_is_moved_but_arbitrary_exe_is_protected() {
        assert_eq!(classify("setup-product.exe").0, "Installateurs");
        assert_eq!(classify("game.exe").0, "");
    }
}
