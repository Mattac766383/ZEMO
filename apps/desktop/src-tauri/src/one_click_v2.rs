use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, io, path::{Path, PathBuf}};

const REVIEW: &str = "À vérifier";

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
    pub root: String,
    pub files_seen: u64,
    pub proposed_moves: Vec<OneClickMove>,
    pub skipped: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OneClickPlan {
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

pub fn build_plan(roots: &[PathBuf]) -> OneClickPlan {
    let mut results = Vec::new();
    let mut files_seen = 0_u64;
    let mut proposed = 0_u64;
    for root in roots {
        let result = plan_root(root);
        files_seen += result.files_seen;
        proposed += result.proposed_moves.len() as u64;
        results.push(result);
    }
    OneClickPlan { roots: results, files_seen, proposed_moves: proposed }
}

fn plan_root(root: &Path) -> OneClickRootResult {
    let mut result = OneClickRootResult {
        root: root.to_string_lossy().into_owned(), files_seen: 0, proposed_moves: Vec::new(), skipped: 0, errors: Vec::new(),
    };
    let read = match fs::read_dir(root) {
        Ok(read) => read,
        Err(error) => { result.errors.push(format!("READ_DIR: {error}")); return result; }
    };
    let mut reserved = HashSet::new();
    for entry in read {
        let entry = match entry { Ok(v) => v, Err(e) => { result.errors.push(format!("ENTRY: {e}")); continue; } };
        let path = entry.path();
        let ty = match entry.file_type() { Ok(v) => v, Err(e) => { result.errors.push(format!("FILE_TYPE: {e}")); continue; } };
        if !ty.is_file() { result.skipped += 1; continue; }
        result.files_seen += 1;
        let name = entry.file_name().to_string_lossy().into_owned();
        if protected_name(&name) { result.skipped += 1; continue; }
        let (category, reason) = classify(&name);
        if category.is_empty() { result.skipped += 1; continue; }
        let destination = collision_safe_destination(root, category, &name, &mut reserved);
        if destination == path { result.skipped += 1; continue; }
        result.proposed_moves.push(OneClickMove {
            source: path.to_string_lossy().into_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category: category.to_owned(), reason: reason.to_owned(),
        });
    }
    result
}

fn classify(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_lowercase();
    let ext = Path::new(name).extension().and_then(|v| v.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "lnk" | "url" | "webloc" | "desktop" | "alias" | "app" | "exe" | "com" | "scr" | "cpl" | "dll" | "sys" | "dylib" | "so" | "bundle" | "framework") { return ("", "protected"); }
    if matches!(ext.as_str(), "dmg" | "pkg" | "msi") || (ext == "exe" && (lower.contains("setup") || lower.contains("install"))) { return ("Installateurs", "installer"); }
    if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tif" | "tiff" | "bmp" | "svg") { return ("Images", "image"); }
    if matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v") { return ("Vidéos", "video"); }
    if matches!(ext.as_str(), "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz") { return ("Archives", "archive"); }
    if matches!(ext.as_str(), "pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp") {
        if ["facture", "invoice", "contrat", "assurance", "releve", "relevé", "impot", "impôt", "cerfa", "bank"].iter().any(|k| lower.contains(k)) { return ("Documents/Administratif", "document_admin"); }
        if ["cours", "devoir", "school", "etude", "étude", "universite", "université"].iter().any(|k| lower.contains(k)) { return ("Documents/Études", "document_study"); }
        if ["client", "projet", "travail", "meeting", "reunion", "réunion"].iter().any(|k| lower.contains(k)) { return ("Documents/Travail", "document_work"); }
        return ("Documents/Personnel", "document");
    }
    (REVIEW, "unknown_loose_file")
}

fn protected_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with('.') || matches!(lower.as_str(), "desktop.ini" | "thumbs.db" | ".ds_store")
}

fn collision_safe_destination(root: &Path, category: &str, name: &str, reserved: &mut HashSet<PathBuf>) -> PathBuf {
    let base = root.join(category).join(name);
    if !base.exists() && reserved.insert(base.clone()) { return base; }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|v| v.to_str()).unwrap_or("fichier");
    let ext = p.extension().and_then(|v| v.to_str());
    for n in 2..10_000_u32 {
        let candidate_name = match ext { Some(ext) => format!("{stem} ({n}).{ext}"), None => format!("{stem} ({n})") };
        let candidate = root.join(category).join(candidate_name);
        if !candidate.exists() && reserved.insert(candidate.clone()) { return candidate; }
    }
    base
}

pub fn apply_plan(plan: &OneClickPlan) -> OneClickApplyResult {
    let mut out = OneClickApplyResult { applied: Vec::new(), skipped: 0, errors: Vec::new() };
    for root in &plan.roots {
        let root_path = PathBuf::from(&root.root);
        let canonical_root = match fs::canonicalize(&root_path) { Ok(v) => v, Err(e) => { out.errors.push(format!("ROOT: {e}")); continue; } };
        for mv in &root.proposed_moves {
            let source = PathBuf::from(&mv.source);
            let destination = PathBuf::from(&mv.destination);
            if !source.is_file() { out.skipped += 1; continue; }
            if !source.starts_with(&root_path) || !destination.starts_with(&root_path) { out.errors.push("SAFETY: path escaped root".to_owned()); continue; }
            if destination.exists() { out.skipped += 1; continue; }
            let parent = match destination.parent() { Some(v) => v, None => { out.skipped += 1; continue; } };
            if let Err(e) = fs::create_dir_all(parent) { out.errors.push(format!("MKDIR: {e}")); continue; }
            let canonical_parent = match fs::canonicalize(parent) { Ok(v) => v, Err(e) => { out.errors.push(format!("PARENT: {e}")); continue; } };
            if !canonical_parent.starts_with(&canonical_root) { out.errors.push("SAFETY: destination escaped canonical root".to_owned()); continue; }
            match fs::rename(&source, &destination) {
                Ok(()) => out.applied.push(AppliedMove { source: mv.source.clone(), destination: mv.destination.clone() }),
                Err(e) => out.errors.push(format!("MOVE {}: {e}", source.file_name().and_then(|v| v.to_str()).unwrap_or("file"))),
            }
        }
    }
    out
}

pub fn undo(applied: &[AppliedMove]) -> OneClickApplyResult {
    let mut out = OneClickApplyResult { applied: Vec::new(), skipped: 0, errors: Vec::new() };
    for mv in applied.iter().rev() {
        let source = PathBuf::from(&mv.destination);
        let destination = PathBuf::from(&mv.source);
        if !source.is_file() || destination.exists() { out.skipped += 1; continue; }
        match fs::rename(&source, &destination) {
            Ok(()) => out.applied.push(AppliedMove { source: mv.destination.clone(), destination: mv.source.clone() }),
            Err(e) => out.errors.push(format!("UNDO: {e}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("zemo-one-click-v2-{id}"));
        fs::create_dir_all(&root).unwrap();
        let names = ["facture-2026.pdf", "photo.jpg", "clip.mp4", "backup.zip", "mystere.xyz", "raccourci.webloc"];
        for i in 0..120 { let name = names[i % names.len()]; fs::write(root.join(format!("{i:03}-{name}")), b"fixture").unwrap(); }
        root
    }

    #[test]
    fn realistic_fixture_plan_apply_undo() {
        let root = fixture();
        let before = fs::read_dir(&root).unwrap().filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_file())).count();
        assert_eq!(before, 120);
        let plan = build_plan(std::slice::from_ref(&root));
        assert_eq!(plan.files_seen, 120);
        assert!(plan.proposed_moves >= 100);
        let applied = apply_plan(&plan);
        assert!(applied.errors.is_empty(), "{:?}", applied.errors);
        assert!(applied.applied.len() >= 100);
        let loose_after = fs::read_dir(&root).unwrap().filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_file())).count();
        assert!(loose_after < 30);
        let undone = undo(&applied.applied);
        assert!(undone.errors.is_empty(), "{:?}", undone.errors);
        let restored = fs::read_dir(&root).unwrap().filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_file())).count();
        assert_eq!(restored, 120);
        let _ = fs::remove_dir_all(root);
    }
}
