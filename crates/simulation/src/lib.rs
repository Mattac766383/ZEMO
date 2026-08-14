//! Pure virtual-filesystem simulation.

use domain::{
    ConflictSeverity, FileFingerprint, NativePath, PathEncoding, ProposalAction, ProposalItemId,
    ProposalRevision, ProposalSimulation, ReviewReason, ReviewState, RootId, SimulationConflict,
    SimulationDiff,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationFile {
    pub item_id: ProposalItemId,
    pub root_id: RootId,
    pub source_path: NativePath,
    pub source_display: String,
    pub fingerprint: FileFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedMove {
    pub item_id: ProposalItemId,
    pub source_root_id: RootId,
    pub source_path: NativePath,
    pub destination_root_id: RootId,
    pub destination_path: NativePath,
    pub fingerprint: FileFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedDirectory {
    pub root_id: RootId,
    pub path: NativePath,
    pub display_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationOutcome {
    pub simulation: ProposalSimulation,
    pub directories: Vec<PlannedDirectory>,
    pub moves: Vec<PlannedMove>,
}

#[derive(Debug, thiserror::Error)]
pub enum SimulationError {
    #[error("proposal serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("proposal item has no corresponding snapshot file")]
    MissingSnapshot,
}

#[derive(Debug, Default)]
pub struct VirtualFileSystem {
    occupied_paths: HashMap<RootId, HashSet<String>>,
    existing_directories: HashMap<RootId, HashSet<String>>,
}

impl VirtualFileSystem {
    pub fn with_occupied_paths<I>(paths: I) -> Self
    where
        I: IntoIterator<Item = (RootId, String)>,
    {
        let mut occupied_paths: HashMap<RootId, HashSet<String>> = HashMap::new();
        for (root, path) in paths {
            occupied_paths
                .entry(root)
                .or_default()
                .insert(case_key(&path));
        }
        Self {
            occupied_paths,
            existing_directories: HashMap::new(),
        }
    }

    pub fn with_snapshot<F, D>(files: F, directories: D) -> Self
    where
        F: IntoIterator<Item = (RootId, String)>,
        D: IntoIterator<Item = (RootId, String)>,
    {
        let mut filesystem = Self::with_occupied_paths(files);
        for (root, path) in directories {
            filesystem
                .existing_directories
                .entry(root)
                .or_default()
                .insert(case_key(&path));
        }
        filesystem
    }

    pub fn simulate(
        &self,
        proposal: &ProposalRevision,
        snapshot_files: &[SimulationFile],
        now_unix_ms: i64,
    ) -> Result<SimulationOutcome, SimulationError> {
        let proposal_digest = *blake3::hash(&serde_json::to_vec(proposal)?).as_bytes();
        let files = snapshot_files
            .iter()
            .map(|file| (file.item_id, file))
            .collect::<HashMap<_, _>>();
        let mut occupied = self.occupied_paths.clone();
        let mut conflicts = Vec::new();
        let mut diffs = Vec::new();
        let mut directories = Vec::new();
        let mut planned_directories: HashMap<RootId, HashSet<String>> = HashMap::new();
        let mut moves = Vec::new();

        for item in &proposal.items {
            if matches!(item.review_state, ReviewState::Rejected) {
                continue;
            }
            let source = files
                .get(&item.id)
                .copied()
                .ok_or(SimulationError::MissingSnapshot)?;
            if matches!(item.review_state, ReviewState::Blocked | ReviewState::Stale) {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: ReviewReason::SourceChanged,
                    severity: ConflictSeverity::Blocker,
                    message: "La source est bloquée ou périmée.".to_owned(),
                });
                continue;
            }

            let destination = match &item.action {
                ProposalAction::Keep => continue,
                ProposalAction::Move { destination }
                | ProposalAction::PlaceInReview { destination } => destination,
            };
            let components = destination
                .folder_components
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(destination.file_name.as_str()))
                .collect::<Vec<_>>();
            if let Some(reason) = components
                .iter()
                .find_map(|value| validate_component(value))
            {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason,
                    severity: ConflictSeverity::Blocker,
                    message: "La destination contient un composant Windows non sûr.".to_owned(),
                });
                continue;
            }
            let destination_display = components.join("\\");
            if destination_display.encode_utf16().count() > 240 {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: ReviewReason::InvalidPath,
                    severity: ConflictSeverity::Blocker,
                    message: "La destination dépasse la limite produit de 240 unités UTF-16."
                        .to_owned(),
                });
                continue;
            }

            let root_paths = occupied.entry(destination.root_id).or_default();
            if !root_paths.insert(case_key(&destination_display)) {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: ReviewReason::DestinationConflict,
                    severity: ConflictSeverity::Blocker,
                    message: "Une entrée existe déjà à cette destination.".to_owned(),
                });
                continue;
            }
            if source.root_id != destination.root_id {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: ReviewReason::CrossVolume,
                    severity: ConflictSeverity::Blocker,
                    message: "Le MVP n’autorise pas les transferts entre racines ou volumes."
                        .to_owned(),
                });
                continue;
            }
            let volume = &source.fingerprint.native_identity.volume;
            if !volume.local || volume.removable {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: if volume.removable {
                        ReviewReason::RemovableVolume
                    } else {
                        ReviewReason::NonLocalVolume
                    },
                    severity: ConflictSeverity::Blocker,
                    message: "Apply est limité aux volumes locaux fixes.".to_owned(),
                });
                continue;
            }
            if volume.platform == domain::PlatformKind::Windows
                && !volume
                    .filesystem_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("NTFS"))
            {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: ReviewReason::NonNtfsVolume,
                    severity: ConflictSeverity::Blocker,
                    message: "Apply Windows est limité aux volumes NTFS.".to_owned(),
                });
                continue;
            }
            if !source.fingerprint.stable_for_apply() {
                let reason = if source.fingerprint.native_identity.link_count != 1 {
                    ReviewReason::HardLink
                } else if source.fingerprint.native_identity.reparse_tag.is_some() {
                    ReviewReason::ReparsePoint
                } else {
                    ReviewReason::SourceChanged
                };
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason,
                    severity: ConflictSeverity::Blocker,
                    message: "La source ne possède pas une identité stable pour Apply.".to_owned(),
                });
                continue;
            }
            if item.review_state == ReviewState::ToReview {
                conflicts.push(SimulationConflict {
                    item_id: item.id,
                    reason: ReviewReason::LowConfidence,
                    severity: ConflictSeverity::Warning,
                    message: "Une décision explicite est requise avant le scellement.".to_owned(),
                });
            }

            for index in 1..=destination.folder_components.len() {
                let display_path = destination.folder_components[..index].join("\\");
                let key = case_key(&display_path);
                let exists = self
                    .existing_directories
                    .get(&destination.root_id)
                    .is_some_and(|values| values.contains(&key));
                let already_planned = !planned_directories
                    .entry(destination.root_id)
                    .or_default()
                    .insert(key);
                if !exists && !already_planned {
                    directories.push(PlannedDirectory {
                        root_id: destination.root_id,
                        path: encode_native_path(&display_path, source.source_path.encoding),
                        display_path,
                    });
                }
            }

            let destination_path =
                encode_native_path(&destination_display, source.source_path.encoding);
            diffs.push(SimulationDiff {
                item_id: item.id,
                display_label: source.source_display.clone(),
                before_label: Some(source.source_display.clone()),
                after_label: Some(destination_display),
                summary: "Déplacement intra-racine proposé, sans écrasement.".to_owned(),
            });
            moves.push(PlannedMove {
                item_id: item.id,
                source_root_id: source.root_id,
                source_path: source.source_path.clone(),
                destination_root_id: destination.root_id,
                destination_path,
                fingerprint: source.fingerprint.clone(),
            });
        }

        Ok(SimulationOutcome {
            simulation: ProposalSimulation {
                proposal_id: proposal.id,
                proposal_digest,
                diffs,
                conflicts,
                simulated_at_unix_ms: now_unix_ms,
            },
            directories,
            moves,
        })
    }
}

#[must_use]
pub fn validate_component(component: &str) -> Option<ReviewReason> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.ends_with([' ', '.'])
        || component.chars().any(|character| {
            character < '\u{20}'
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        || component.encode_utf16().count() > 255
    {
        return Some(ReviewReason::InvalidPath);
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    reserved.then_some(ReviewReason::InvalidPath)
}

fn encode_native_path(value: &str, encoding: PathEncoding) -> NativePath {
    let bytes = match encoding {
        PathEncoding::UnixBytes => value.replace('\\', "/").into_bytes(),
        PathEncoding::WindowsUtf16Le => value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    };
    NativePath { encoding, bytes }
}

fn case_key(value: &str) -> String {
    value.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_and_traversal_names() {
        assert_eq!(
            validate_component("CON.txt"),
            Some(ReviewReason::InvalidPath)
        );
        assert_eq!(validate_component(".."), Some(ReviewReason::InvalidPath));
        assert_eq!(validate_component("Factures"), None);
    }

    #[test]
    fn collision_keys_are_case_insensitive() {
        let root = RootId::new();
        let filesystem =
            VirtualFileSystem::with_occupied_paths([(root, "Clients\\ACME.pdf".to_owned())]);
        assert!(
            filesystem
                .occupied_paths
                .get(&root)
                .is_some_and(|paths| paths.contains("clients\\acme.pdf"))
        );
    }
}
