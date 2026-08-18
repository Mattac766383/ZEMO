//! Consumer one-click organization policy.
//!
//! Classifies personal files into a shallow, understandable folder tree.
//! Programs, system components, and desktop shortcuts stay in place.

use std::path::{Component, Path};

/// Well-known user-content root for destination shaping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsumerRootKind {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Videos,
    Music,
    #[default]
    Unknown,
}

impl ConsumerRootKind {
    #[must_use]
    pub fn from_path_and_label(absolute_path: &str, display_label: &str) -> Self {
        let normalized = absolute_path.replace('\\', "/").to_lowercase();
        let last = normalized
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("");
        let label = display_label.to_lowercase();
        let haystack = format!("{last} {label}");
        if matches_any(&haystack, &["desktop", "bureau"]) {
            Self::Desktop
        } else if matches_any(&haystack, &["downloads", "téléchargements", "telechargements"])
        {
            Self::Downloads
        } else if matches_any(&haystack, &["documents"]) {
            Self::Documents
        } else if matches_any(&haystack, &["pictures", "images", "photos"]) {
            Self::Pictures
        } else if matches_any(&haystack, &["movies", "videos", "vidéos"]) {
            Self::Videos
        } else if matches_any(&haystack, &["music", "musique"]) {
            Self::Music
        } else {
            Self::Unknown
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Downloads => "downloads",
            Self::Documents => "documents",
            Self::Pictures => "pictures",
            Self::Videos => "videos",
            Self::Music => "music",
            Self::Unknown => "unknown",
        }
    }
}

/// Top-level consumer category shown in the simple preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerCategory {
    Documents,
    Images,
    Videos,
    Archives,
    Installers,
    Review,
    Shortcuts,
    LeaveInPlace,
}

impl ConsumerCategory {
    #[must_use]
    pub const fn preview_label(self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Images => "Images",
            Self::Videos => "Vidéos",
            Self::Archives => "Archives",
            Self::Installers => "Installateurs",
            Self::Review => "À vérifier",
            Self::Shortcuts => "Raccourcis",
            Self::LeaveInPlace => "Laissés en place",
        }
    }
}

/// Decision for one file under the consumer policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerDecision {
    pub category: ConsumerCategory,
    pub destination: Vec<String>,
    pub leave_in_place: bool,
    pub needs_review: bool,
    pub reason_code: &'static str,
    pub explanation: &'static str,
}

const REVIEW_FOLDER: &str = "À vérifier";
const INSTALLERS_FOLDER: &str = "Installateurs";
const DOCUMENTS_FOLDER: &str = "Documents";
const IMAGES_FOLDER: &str = "Images";
const VIDEOS_FOLDER: &str = "Vidéos";
const ARCHIVES_FOLDER: &str = "Archives";

/// Infer a consumer decision from filename, relative path, and optional semantic type.
#[must_use]
pub fn decide_consumer_organization(
    source_relative_path: &str,
    source_name: &str,
    root_kind: ConsumerRootKind,
    document_type: Option<&str>,
) -> ConsumerDecision {
    let extension = extension_of(source_name);
    let name_lower = source_name.to_ascii_lowercase();

    if is_program_or_system(source_relative_path, source_name, extension.as_deref()) {
        return leave_in_place(
            "program_protected",
            "Les programmes et composants système restent en place.",
        );
    }

    if is_desktop_shortcut(root_kind, extension.as_deref(), source_relative_path) {
        return leave_in_place(
            "desktop_shortcut",
            "Les raccourcis du Bureau restent en place pour ne pas casser vos lancements.",
        );
    }

    if is_downloaded_installer(&name_lower, extension.as_deref()) {
        return ConsumerDecision {
            category: ConsumerCategory::Installers,
            destination: vec![INSTALLERS_FOLDER.to_owned()],
            leave_in_place: false,
            needs_review: false,
            reason_code: "installer",
            explanation: "Fichier d’installation téléchargé, rangé à part des applications.",
        };
    }

    if is_unknown_executable(extension.as_deref()) {
        return leave_in_place(
            "uncertain_program",
            "Ce fichier ressemble à un programme. ZEMO l’a laissé en place.",
        );
    }

    if let Some(decision) = classify_media_or_archive(
        root_kind,
        extension.as_deref(),
        &name_lower,
        document_type,
    ) {
        return maybe_preserve_existing(source_relative_path, decision);
    }

    if is_document_extension(extension.as_deref()) || is_document_type(document_type) {
        let destination = document_destination(root_kind, &name_lower, document_type);
        return maybe_preserve_existing(
            source_relative_path,
            ConsumerDecision {
                category: ConsumerCategory::Documents,
                destination,
                leave_in_place: false,
                needs_review: false,
                reason_code: "document",
                explanation: "Document personnel classé dans un dossier simple.",
            },
        );
    }

    if is_loose_file(source_relative_path) {
        return ConsumerDecision {
            category: ConsumerCategory::Review,
            destination: vec![REVIEW_FOLDER.to_owned()],
            leave_in_place: false,
            // Apply must still move the file so Desktop becomes visibly cleaner.
            // Uncertainty is expressed by the destination folder, not by blocking apply.
            needs_review: false,
            reason_code: "uncertain",
            explanation: "Type incertain : proposé dans À vérifier plutôt que deviné.",
        };
    }

    leave_in_place(
        "uncertain_nested",
        "Fichier déjà dans un dossier, laissé en place par prudence.",
    )
}

/// Map a proposed destination to the simple preview category.
#[must_use]
pub fn category_from_destination(destination: &[String], leave_in_place: bool) -> ConsumerCategory {
    if leave_in_place {
        return ConsumerCategory::LeaveInPlace;
    }
    let head = destination.first().map(String::as_str).unwrap_or("");
    match head {
        DOCUMENTS_FOLDER | "Travail" | "Administratif" | "Études" | "Etudes" | "Personnel" => {
            ConsumerCategory::Documents
        }
        IMAGES_FOLDER | "Photos" | "Captures d’écran" | "Captures d'écran" | "Images téléchargées" => {
            ConsumerCategory::Images
        }
        VIDEOS_FOLDER => ConsumerCategory::Videos,
        ARCHIVES_FOLDER => ConsumerCategory::Archives,
        INSTALLERS_FOLDER => ConsumerCategory::Installers,
        REVIEW_FOLDER | "TO_REVIEW" => ConsumerCategory::Review,
        "Raccourcis" => ConsumerCategory::Shortcuts,
        _ => ConsumerCategory::Review,
    }
}

#[must_use]
pub fn is_program_or_system(relative_path: &str, filename: &str, extension: Option<&str>) -> bool {
    let path = Path::new(relative_path);
    if path.components().any(|component| match component {
        Component::Normal(value) => {
            let name = value.to_string_lossy();
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".app")
                || lower.ends_with(".framework")
                || matches!(
                    lower.as_str(),
                    "program files"
                        | "program files (x86)"
                        | "windows"
                        | "system32"
                        | "syswow64"
                        | "applications"
                        | "library"
                        | "system"
                        | "appdata"
                        | "node_modules"
                        | "contents"
                )
        }
        _ => false,
    }) {
        return true;
    }

    let name = filename.to_ascii_lowercase();
    if name.contains(".app/") || name.ends_with(".app") {
        return true;
    }

    matches!(
        extension.map(str::to_ascii_lowercase).as_deref(),
        Some("dll" | "sys" | "dylib" | "so" | "bundle" | "framework")
    )
}

#[must_use]
pub fn is_downloaded_installer(filename_lower: &str, extension: Option<&str>) -> bool {
    let ext = extension.map(str::to_ascii_lowercase);
    let installer_name = filename_lower.contains("setup")
        || filename_lower.contains("install")
        || filename_lower.contains("installer")
        || filename_lower.starts_with("setup")
        || filename_lower.contains("-setup")
        || filename_lower.contains("_setup");
    match ext.as_deref() {
        Some("dmg" | "pkg") => true,
        Some("msi") if installer_name || !looks_system_msi(filename_lower) => true,
        Some("exe") if installer_name => true,
        _ => false,
    }
}

fn looks_system_msi(filename_lower: &str) -> bool {
    filename_lower.contains("kb")
        || filename_lower.contains("hotfix")
        || filename_lower.contains("update")
        || filename_lower.starts_with("windows")
}

fn is_unknown_executable(extension: Option<&str>) -> bool {
    matches!(
        extension.map(str::to_ascii_lowercase).as_deref(),
        Some("exe" | "com" | "scr" | "cpl")
    )
}

fn is_desktop_shortcut(
    _root_kind: ConsumerRootKind,
    extension: Option<&str>,
    relative_path: &str,
) -> bool {
    if !is_loose_file(relative_path) {
        return false;
    }
    matches!(
        extension.map(str::to_ascii_lowercase).as_deref(),
        Some("lnk" | "url" | "webloc" | "desktop" | "alias")
    )
}

fn classify_media_or_archive(
    root_kind: ConsumerRootKind,
    extension: Option<&str>,
    name_lower: &str,
    document_type: Option<&str>,
) -> Option<ConsumerDecision> {
    if is_image_extension(extension) || matches!(document_type, Some("photo" | "image")) {
        let screenshot = name_lower.contains("screenshot")
            || name_lower.contains("capture")
            || name_lower.contains("screen shot")
            || name_lower.starts_with("capture d");
        let downloaded = root_kind == ConsumerRootKind::Downloads && !screenshot;
        let (folder, explanation) = if screenshot {
            ("Captures d’écran", "Capture d’écran classée dans Images.")
        } else if downloaded {
            (
                "Images téléchargées",
                "Image téléchargée classée dans Images.",
            )
        } else {
            ("Photos", "Photo classée dans Images.")
        };
        return Some(ConsumerDecision {
            category: ConsumerCategory::Images,
            destination: image_destination(root_kind, folder),
            leave_in_place: false,
            needs_review: false,
            reason_code: "image",
            explanation,
        });
    }

    if is_video_extension(extension) || matches!(document_type, Some("video")) {
        return Some(ConsumerDecision {
            category: ConsumerCategory::Videos,
            destination: video_destination(root_kind),
            leave_in_place: false,
            needs_review: false,
            reason_code: "video",
            explanation: "Vidéo classée dans Vidéos.",
        });
    }

    if is_archive_extension(extension) || matches!(document_type, Some("archive")) {
        return Some(ConsumerDecision {
            category: ConsumerCategory::Archives,
            destination: archive_destination(root_kind),
            leave_in_place: false,
            needs_review: false,
            reason_code: "archive",
            explanation: "Archive classée dans Archives.",
        });
    }

    None
}

fn document_destination(
    root_kind: ConsumerRootKind,
    name_lower: &str,
    document_type: Option<&str>,
) -> Vec<String> {
    let leaf = document_leaf(name_lower, document_type);
    match root_kind {
        ConsumerRootKind::Documents => vec![leaf.to_owned()],
        ConsumerRootKind::Pictures | ConsumerRootKind::Videos | ConsumerRootKind::Music => {
            vec![DOCUMENTS_FOLDER.to_owned(), leaf.to_owned()]
        }
        _ => vec![DOCUMENTS_FOLDER.to_owned(), leaf.to_owned()],
    }
}

fn document_leaf(name_lower: &str, document_type: Option<&str>) -> &'static str {
    if matches!(
        document_type,
        Some("invoice" | "tax_document" | "insurance_document" | "bank_statement" | "receipt")
    ) || contains_any(
        name_lower,
        &[
            "invoice",
            "facture",
            "tax",
            "impot",
            "impôt",
            "bank",
            "releve",
            "relevé",
            "admin",
            "contrat",
            "contract",
            "assurance",
            "cerfa",
        ],
    ) {
        return "Administratif";
    }
    if contains_any(
        name_lower,
        &["school", "cours", "etude", "étude", "homework", "devoir", "université", "universite"],
    ) {
        return "Études";
    }
    if contains_any(
        name_lower,
        &["work", "travail", "meeting", "reunion", "réunion", "projet", "client"],
    ) {
        return "Travail";
    }
    "Personnel"
}

fn image_destination(root_kind: ConsumerRootKind, leaf: &str) -> Vec<String> {
    match root_kind {
        ConsumerRootKind::Pictures => vec![leaf.to_owned()],
        _ => vec![IMAGES_FOLDER.to_owned(), leaf.to_owned()],
    }
}

fn video_destination(root_kind: ConsumerRootKind) -> Vec<String> {
    match root_kind {
        ConsumerRootKind::Videos => Vec::new(),
        _ => vec![VIDEOS_FOLDER.to_owned()],
    }
}

fn archive_destination(_root_kind: ConsumerRootKind) -> Vec<String> {
    vec![ARCHIVES_FOLDER.to_owned()]
}

fn maybe_preserve_existing(
    source_relative_path: &str,
    decision: ConsumerDecision,
) -> ConsumerDecision {
    if decision.leave_in_place || decision.destination.is_empty() {
        return decision;
    }
    if is_loose_file(source_relative_path) {
        return decision;
    }
    let current = source_parent_components(source_relative_path);
    if current.is_empty() {
        return decision;
    }
    if current
        .first()
        .is_some_and(|head| destination_compatible(head, &decision.destination))
    {
        return ConsumerDecision {
            leave_in_place: true,
            destination: current,
            reason_code: "already_organized",
            explanation: "Le fichier est déjà dans un dossier compréhensible.",
            ..decision
        };
    }
    decision
}

fn destination_compatible(current_head: &str, destination: &[String]) -> bool {
    destination
        .first()
        .is_some_and(|expected| expected.eq_ignore_ascii_case(current_head))
        || matches!(
            current_head,
            DOCUMENTS_FOLDER
                | IMAGES_FOLDER
                | VIDEOS_FOLDER
                | ARCHIVES_FOLDER
                | INSTALLERS_FOLDER
                | REVIEW_FOLDER
                | "Travail"
                | "Administratif"
                | "Études"
                | "Personnel"
                | "Photos"
        )
}

fn leave_in_place(reason_code: &'static str, explanation: &'static str) -> ConsumerDecision {
    ConsumerDecision {
        category: ConsumerCategory::LeaveInPlace,
        destination: Vec::new(),
        leave_in_place: true,
        needs_review: false,
        reason_code,
        explanation,
    }
}

fn is_loose_file(relative_path: &str) -> bool {
    source_parent_components(relative_path).is_empty()
}

fn source_parent_components(relative_path: &str) -> Vec<String> {
    let normalized = relative_path.replace('\\', "/");
    let mut parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    parts.pop();
    parts
}

fn extension_of(filename: &str) -> Option<String> {
    Path::new(filename)
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
}

fn is_image_extension(extension: Option<&str>) -> bool {
    matches!(
        extension,
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "bmp" | "tif" | "tiff")
    )
}

fn is_video_extension(extension: Option<&str>) -> bool {
    matches!(
        extension,
        Some("mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" | "wmv")
    )
}

fn is_archive_extension(extension: Option<&str>) -> bool {
    matches!(
        extension,
        Some("zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2")
    )
}

fn is_document_extension(extension: Option<&str>) -> bool {
    matches!(
        extension,
        Some(
            "pdf"
                | "doc"
                | "docx"
                | "txt"
                | "rtf"
                | "odt"
                | "pages"
                | "xls"
                | "xlsx"
                | "csv"
                | "ppt"
                | "pptx"
                | "md"
        )
    )
}

fn is_document_type(document_type: Option<&str>) -> bool {
    matches!(
        document_type,
        Some(
            "invoice"
                | "quote"
                | "contract"
                | "receipt"
                | "tax_document"
                | "insurance_document"
                | "bank_statement"
                | "letter"
                | "note"
        )
    )
}

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(root: ConsumerRootKind, path: &str) -> ConsumerDecision {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        decide_consumer_organization(path, name, root, None)
    }

    #[test]
    fn desktop_personal_files_get_shallow_visible_folders() {
        let invoice = decide(ConsumerRootKind::Desktop, "invoice.pdf");
        assert_eq!(invoice.destination, ["Documents", "Administratif"]);
        assert!(!invoice.leave_in_place);

        let photo = decide(ConsumerRootKind::Desktop, "holiday.jpg");
        assert_eq!(photo.destination, ["Images", "Photos"]);

        let shot = decide(ConsumerRootKind::Desktop, "screenshot.png");
        assert_eq!(shot.destination, ["Images", "Captures d’écran"]);

        let school = decide(ConsumerRootKind::Desktop, "school.docx");
        assert_eq!(school.destination, ["Documents", "Études"]);

        let notes = decide(ConsumerRootKind::Desktop, "notes.txt");
        assert_eq!(notes.destination, ["Documents", "Personnel"]);

        let video = decide(ConsumerRootKind::Desktop, "video.mp4");
        assert_eq!(video.destination, ["Vidéos"]);

        let archive = decide(ConsumerRootKind::Desktop, "archive.zip");
        assert_eq!(archive.destination, ["Archives"]);
    }

    #[test]
    fn installers_move_programs_and_shortcuts_stay() {
        let setup = decide(ConsumerRootKind::Desktop, "setup.exe");
        assert_eq!(setup.destination, [INSTALLERS_FOLDER]);
        assert_eq!(setup.category, ConsumerCategory::Installers);

        let dmg = decide(ConsumerRootKind::Downloads, "Installer.dmg");
        assert_eq!(dmg.category, ConsumerCategory::Installers);

        let shortcut = decide(ConsumerRootKind::Desktop, "Chrome.lnk");
        assert!(shortcut.leave_in_place);
        let download_shortcut = decide(ConsumerRootKind::Downloads, "App.lnk");
        assert!(download_shortcut.leave_in_place);

        let dll = decide(ConsumerRootKind::Desktop, "library.dll");
        assert!(dll.leave_in_place);

        let app = decide(ConsumerRootKind::Desktop, "Notes.app/Contents/MacOS/Notes");
        assert!(app.leave_in_place);

        let unknown_exe = decide(ConsumerRootKind::Desktop, "chrome.exe");
        assert!(unknown_exe.leave_in_place);
    }

    #[test]
    fn uncertain_loose_files_go_to_review_folder() {
        let unknown = decide(ConsumerRootKind::Desktop, "unknown.xyz");
        assert_eq!(unknown.destination, [REVIEW_FOLDER]);
        assert!(!unknown.leave_in_place);
    }

    #[test]
    fn already_organized_files_stay() {
        let nested = decide(ConsumerRootKind::Documents, "Travail/notes.txt");
        assert!(nested.leave_in_place);
    }

    #[test]
    fn infer_root_kind_from_os_paths() {
        assert_eq!(
            ConsumerRootKind::from_path_and_label("/Users/ada/Desktop", "Bureau"),
            ConsumerRootKind::Desktop
        );
        assert_eq!(
            ConsumerRootKind::from_path_and_label(r"C:\Users\ada\Downloads", "Downloads"),
            ConsumerRootKind::Downloads
        );
        assert_eq!(
            ConsumerRootKind::from_path_and_label("/Users/ada/Movies", "Vidéos"),
            ConsumerRootKind::Videos
        );
    }
}
