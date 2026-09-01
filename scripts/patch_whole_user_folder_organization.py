from pathlib import Path
import json
import re

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def write(path, text):
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, got {count}")
    return text.replace(old, new, 1)


def replace_all_checked(text, old, new, minimum, label):
    count = text.count(old)
    if count < minimum:
        raise RuntimeError(f"{label}: expected at least {minimum} matches, got {count}")
    return text.replace(old, new)


# 1) Consumer policy: loose project manifests are user content, not protected junk.
path = "crates/organizer/src/consumer.rs"
text = read(path)
anchor = '''    if is_program_or_system(source_relative_path, source_name, extension.as_deref()) {\n        return leave_in_place(\n            "program_protected",\n            "Les programmes et composants système restent en place.",\n        );\n    }\n'''
insert = '''    if is_project_manifest(&name_lower) && is_loose_file(source_relative_path) {\n        return ConsumerDecision {\n            category: ConsumerCategory::Documents,\n            destination: vec![\n                "Développement".to_owned(),\n                "Fichiers projet".to_owned(),\n            ],\n            leave_in_place: false,\n            needs_review: false,\n            reason_code: "project_manifest",\n            explanation: "Fichier de projet classé dans Développement.",\n        };\n    }\n\n''' + anchor
text = replace_once(text, anchor, insert, "consumer loose project manifest")
write(path, text)


# 2) Organizer: bundle every safe top-level user folder, preserving its subtree.
path = "crates/organizer/src/organization.rs"
text = read(path)
text = replace_all_checked(
    text,
    '''        let mut policy = VirtualPathPolicy {\n            maximum_depth: request.base.preferences.maximum_depth.clamp(2, 8),\n            ..VirtualPathPolicy::default()\n        };\n        policy.maximum_depth = policy.maximum_depth.min(8);''',
    '''        let mut policy = VirtualPathPolicy {\n            maximum_depth: if request.base.consumer_mode {\n                32\n            } else {\n                request.base.preferences.maximum_depth.clamp(2, 8)\n            },\n            ..VirtualPathPolicy::default()\n        };\n        policy.maximum_depth = policy.maximum_depth.min(32);''',
    1,
    "incremental consumer depth",
)
text = replace_all_checked(
    text,
    '''        let mut policy = VirtualPathPolicy {\n            maximum_depth: request.preferences.maximum_depth.clamp(2, 8),\n            ..VirtualPathPolicy::default()\n        };\n        policy.maximum_depth = policy.maximum_depth.min(8);''',
    '''        let mut policy = VirtualPathPolicy {\n            maximum_depth: if request.consumer_mode {\n                32\n            } else {\n                request.preferences.maximum_depth.clamp(2, 8)\n            },\n            ..VirtualPathPolicy::default()\n        };\n        policy.maximum_depth = policy.maximum_depth.min(32);''',
    1,
    "full consumer depth",
)
full_anchor = '''            apply_minimum_group_policy(\n                &mut drafts,\n                request.preferences.minimum_group_size.clamp(1, 20),\n            );\n\n            progress.phase = ProposalBuildPhase::DetectingConflicts;'''
full_new = '''            apply_minimum_group_policy(\n                &mut drafts,\n                request.preferences.minimum_group_size.clamp(1, 20),\n            );\n            if request.consumer_mode {\n                apply_consumer_folder_bundle_policy(\n                    &mut drafts,\n                    &request.inputs,\n                    request.consumer_root_kind,\n                    policy,\n                );\n            }\n\n            progress.phase = ProposalBuildPhase::DetectingConflicts;'''
text = replace_once(text, full_anchor, full_new, "full folder bundle hook")
inc_anchor = '''        apply_minimum_group_policy(\n            &mut drafts,\n            request.base.preferences.minimum_group_size.clamp(1, 20),\n        );\n\n        progress.phase = ProposalBuildPhase::DetectingConflicts;'''
inc_new = '''        apply_minimum_group_policy(\n            &mut drafts,\n            request.base.preferences.minimum_group_size.clamp(1, 20),\n        );\n        if request.base.consumer_mode {\n            apply_consumer_folder_bundle_policy(\n                &mut drafts,\n                &request.base.inputs,\n                request.base.consumer_root_kind,\n                policy,\n            );\n        }\n\n        progress.phase = ProposalBuildPhase::DetectingConflicts;'''
text = replace_once(text, inc_anchor, inc_new, "incremental folder bundle hook")
helper_anchor = '''fn draft_from_previous_operation(operation: &OrganizationProposalOperation) -> DraftOperation {'''
helper = r'''#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumerFolderBundleKind {
    Development,
    Work,
    Images,
    Videos,
    General,
}

#[derive(Debug, Default)]
struct ConsumerFolderBundleStats {
    files: usize,
    manifests: usize,
    code_files: usize,
    images: usize,
    videos: usize,
    work_signals: usize,
}

fn apply_consumer_folder_bundle_policy(
    drafts: &mut [DraftOperation],
    inputs: &[OrganizationSourceInput],
    root_kind: ConsumerRootKind,
    policy: VirtualPathPolicy,
) {
    let mut stats = HashMap::<String, ConsumerFolderBundleStats>::new();
    let mut canonical_names = HashMap::<String, String>::new();
    let by_file = inputs
        .iter()
        .map(|input| (input.file_id, input))
        .collect::<HashMap<_, _>>();

    for input in inputs {
        let components = normalized_source_components(&input.source_relative_path);
        if components.len() < 2 {
            continue;
        }
        let top = &components[0];
        if consumer_bundle_top_level_is_protected(top) {
            continue;
        }
        let key = top.to_ascii_lowercase();
        canonical_names.entry(key.clone()).or_insert_with(|| top.clone());
        let entry = stats.entry(key).or_default();
        entry.files = entry.files.saturating_add(1);
        let lower_name = input.source_name.to_ascii_lowercase();
        if is_bundle_project_manifest(&lower_name) {
            entry.manifests = entry.manifests.saturating_add(1);
        }
        if bundle_extension_is_code(&lower_name) {
            entry.code_files = entry.code_files.saturating_add(1);
        }
        if bundle_extension_is_image(&lower_name) {
            entry.images = entry.images.saturating_add(1);
        }
        if bundle_extension_is_video(&lower_name) {
            entry.videos = entry.videos.saturating_add(1);
        }
        if folder_name_looks_like_work(top)
            || input
                .context
                .as_ref()
                .is_some_and(|signal| signal.value.to_ascii_lowercase().contains("work"))
            || lower_name.contains("devis")
            || lower_name.contains("chantier")
            || lower_name.contains("client")
        {
            entry.work_signals = entry.work_signals.saturating_add(1);
        }
    }

    let kinds = stats
        .iter()
        .map(|(key, value)| {
            let kind = if value.manifests > 0 || value.code_files >= 2 {
                ConsumerFolderBundleKind::Development
            } else if value.images > 0 && value.images.saturating_mul(100) >= value.files.saturating_mul(70) {
                ConsumerFolderBundleKind::Images
            } else if value.videos > 0 && value.videos.saturating_mul(100) >= value.files.saturating_mul(70) {
                ConsumerFolderBundleKind::Videos
            } else if value.work_signals > 0 {
                ConsumerFolderBundleKind::Work
            } else {
                ConsumerFolderBundleKind::General
            };
            (key.clone(), kind)
        })
        .collect::<HashMap<_, _>>();

    for draft in drafts {
        if draft.operation.user_override || draft.operation.stale {
            continue;
        }
        let Some(input) = by_file.get(&draft.operation.file_id).copied() else {
            continue;
        };
        let components = normalized_source_components(&input.source_relative_path);
        if components.len() < 2 {
            continue;
        }
        let top_key = components[0].to_ascii_lowercase();
        let Some(kind) = kinds.get(&top_key).copied() else {
            continue;
        };
        let top = canonical_names
            .get(&top_key)
            .cloned()
            .unwrap_or_else(|| components[0].clone());
        let mut destination = consumer_bundle_base(root_kind, kind, &top);
        // Preserve the complete parent subtree below the top-level folder.
        // The filename itself is kept separately, exactly as today.
        destination.extend(components[1..components.len() - 1].iter().cloned());
        if destination.len() > policy.maximum_depth {
            // Deep developer trees remain safe and deterministic instead of
            // being flattened into collisions. The path-length gate below is
            // still authoritative.
            destination.truncate(policy.maximum_depth);
        }
        let (machine_destination, machine_name, changed, valid) =
            policy.fit_machine_path(&destination, &input.source_name);
        if !valid {
            draft.operation.conflict_state = ProposalConflictState::PathTooLong;
            draft.operation.needs_review = true;
            continue;
        }
        draft.operation.proposed_destination = destination;
        draft.operation.machine_destination = machine_destination;
        draft.operation.proposed_name = input.source_name.clone();
        draft.operation.machine_name = machine_name;
        draft.operation.operation_kind = ProposalOperationKind::MoveProposal;
        draft.operation.confidence_score = match kind {
            ConsumerFolderBundleKind::Development | ConsumerFolderBundleKind::Work => 0.98,
            ConsumerFolderBundleKind::Images | ConsumerFolderBundleKind::Videos => 0.97,
            ConsumerFolderBundleKind::General => 0.90,
        };
        draft.operation.confidence_level = ProposalConfidenceLevel::High;
        // Ambiguous folders are deliberately moved under À vérifier, so this
        // relocation itself is safe to apply without silently guessing a final taxonomy.
        draft.operation.needs_review = false;
        draft.operation.conflict_state = ProposalConflictState::None;
        draft.operation.semantic_context = "consumer_folder_bundle".to_owned();
        draft.operation.reasons.push(OrganizationReason {
            code: "consumer_folder_bundle".to_owned(),
            explanation: "Le dossier est conservé comme un bloc cohérent et rangé avec toute son arborescence.".to_owned(),
            evidence_references: vec![format!("folder:{top}")],
        });
        draft.operation.disruption_score = if changed { 0.12 } else { 0.08 };
        draft.operation.proposed_path_length = policy.path_length_utf16(
            &draft.operation.proposed_destination,
            &draft.operation.proposed_name,
        );
        draft.operation.proposed_depth = draft.operation.proposed_destination.len();
        draft.optional_tail = vec![false; draft.operation.proposed_destination.len()];
    }
}

fn normalized_source_components(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|component| !component.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn consumer_bundle_top_level_is_protected(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('.')
        || matches!(
            lower.as_str(),
            "documents"
                | "images"
                | "pictures"
                | "photos"
                | "vidéos"
                | "videos"
                | "archives"
                | "installateurs"
                | "à vérifier"
                | "a verifier"
                | "développement"
                | "developpement"
                | "node_modules"
                | "applications"
                | "library"
                | "system"
                | "windows"
                | "program files"
                | "program files (x86)"
        )
}

fn is_bundle_project_manifest(name: &str) -> bool {
    matches!(
        name,
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "cargo.toml"
            | "cargo.lock"
            | "pyproject.toml"
            | "requirements.txt"
            | "poetry.lock"
            | "go.mod"
            | "go.sum"
            | "pom.xml"
            | "composer.json"
            | "composer.lock"
    )
}

fn bundle_extension_is_code(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "rs" | "py" | "go" | "java"
                | "c" | "h" | "cpp" | "hpp" | "cs" | "php" | "rb" | "swift" | "kt" | "kts"
                | "vue" | "svelte" | "html" | "css" | "scss"
        )
    })
}

fn bundle_extension_is_image(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(ext, "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "avif" | "svg")
    })
}

fn bundle_extension_is_video(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(ext, "mp4" | "mov" | "m4v" | "mkv" | "avi" | "webm")
    })
}

fn folder_name_looks_like_work(name: &str) -> bool {
    let value = name.to_lowercase();
    [
        "maquette",
        "portfolio",
        "projet",
        "project",
        "client",
        "chantier",
        "site",
        "web",
        "coaching",
        "lea",
        "témoignage",
        "temoignage",
        "etanche",
        "psps",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn consumer_bundle_base(
    root_kind: ConsumerRootKind,
    kind: ConsumerFolderBundleKind,
    top: &str,
) -> Vec<String> {
    match kind {
        ConsumerFolderBundleKind::Development => vec![
            "Développement".to_owned(),
            "Projets".to_owned(),
            top.to_owned(),
        ],
        ConsumerFolderBundleKind::Work => {
            let mut value = if root_kind == ConsumerRootKind::Documents {
                vec!["Travail".to_owned(), "Projets".to_owned()]
            } else {
                vec!["Documents".to_owned(), "Travail".to_owned(), "Projets".to_owned()]
            };
            value.push(top.to_owned());
            value
        }
        ConsumerFolderBundleKind::Images => {
            let mut value = if root_kind == ConsumerRootKind::Pictures {
                vec!["Albums".to_owned()]
            } else {
                vec!["Images".to_owned(), "Albums".to_owned()]
            };
            value.push(top.to_owned());
            value
        }
        ConsumerFolderBundleKind::Videos => {
            let mut value = if root_kind == ConsumerRootKind::Videos {
                vec!["Collections".to_owned()]
            } else {
                vec!["Vidéos".to_owned(), "Collections".to_owned()]
            };
            value.push(top.to_owned());
            value
        }
        ConsumerFolderBundleKind::General => vec![
            "À vérifier".to_owned(),
            "Dossiers".to_owned(),
            top.to_owned(),
        ],
    }
}

'''
text = replace_once(text, helper_anchor, helper + helper_anchor, "folder bundle helpers")
write(path, text)


# 3) Execution domain + persistence: add journaled remove-empty-directory operation.
path = "crates/domain/src/execution.rs"
text = read(path)
text = replace_once(
    text,
    '''pub enum ExecutionOperationKind {\n    CreateDirectory,\n    Move,''',
    '''pub enum ExecutionOperationKind {\n    CreateDirectory,\n    RemoveDirectoryIfEmpty,\n    Move,''',
    "execution kind enum",
)
text = replace_once(
    text,
    '''            Self::CreateDirectory => "create_directory",\n            Self::Move => "move",''',
    '''            Self::CreateDirectory => "create_directory",\n            Self::RemoveDirectoryIfEmpty => "remove_directory_if_empty",\n            Self::Move => "move",''',
    "execution kind database name",
)
write(path, text)

path = "crates/persistence/src/execution.rs"
text = read(path)
text = replace_all_checked(
    text,
    "'create_directory', 'internal_stage'",
    "'create_directory', 'remove_directory_if_empty', 'internal_stage'",
    2,
    "execution summary excludes directory maintenance",
)
text = replace_once(
    text,
    '''        "create_directory" => Ok(ExecutionOperationKind::CreateDirectory),\n        "move" => Ok(ExecutionOperationKind::Move),''',
    '''        "create_directory" => Ok(ExecutionOperationKind::CreateDirectory),\n        "remove_directory_if_empty" => Ok(ExecutionOperationKind::RemoveDirectoryIfEmpty),\n        "move" => Ok(ExecutionOperationKind::Move),''',
    "parse execution cleanup kind",
)
write(path, text)


# 4) Authenticated protocol primitive.
path = "crates/ipc-contracts/src/executor_v2/model.rs"
text = read(path)
text = replace_once(
    text,
    '''    CreateDirectory {\n        destination_relative_path: String,\n    },\n    SameVolumeMove {''',
    '''    CreateDirectory {\n        destination_relative_path: String,\n    },\n    RemoveDirectoryIfEmpty {\n        source_relative_path: String,\n    },\n    SameVolumeMove {''',
    "protocol cleanup primitive enum",
)
text = replace_once(
    text,
    '''            Self::CreateDirectory {\n                destination_relative_path,\n            } => validate_relative_path(destination_relative_path),\n            Self::SameVolumeMove {''',
    '''            Self::CreateDirectory {\n                destination_relative_path,\n            } => validate_relative_path(destination_relative_path),\n            Self::RemoveDirectoryIfEmpty {\n                source_relative_path,\n            } => validate_relative_path(source_relative_path),\n            Self::SameVolumeMove {''',
    "protocol cleanup validation",
)
create_manifest = '''        domain::ExecutionOperationKind::CreateDirectory => {\n            if operation.source_relative_path.is_some()\n                || operation.live_fingerprint.is_some()\n                || operation.directory_existed_before != Some(false)\n            {\n                return Err(ValidationError::InvalidField("create-directory manifest"));\n            }\n            OperationPrimitiveManifest::CreateDirectory {\n                destination_relative_path: operation.destination_relative_path.clone(),\n            }\n        }'''
cleanup_manifest = create_manifest + '''\n        domain::ExecutionOperationKind::RemoveDirectoryIfEmpty => {\n            let source_relative_path = operation\n                .source_relative_path\n                .clone()\n                .ok_or(ValidationError::InvalidField("cleanup source directory"))?;\n            if operation.live_fingerprint.is_some()\n                || operation.directory_existed_before != Some(true)\n            {\n                return Err(ValidationError::InvalidField("remove-empty-directory manifest"));\n            }\n            OperationPrimitiveManifest::RemoveDirectoryIfEmpty {\n                source_relative_path,\n            }\n        }'''
text = replace_once(text, create_manifest, cleanup_manifest, "execution detail cleanup manifest")
text = replace_all_checked(
    text,
    '''                domain::ExecutionOperationKind::CreateDirectory => unreachable!(),''',
    '''                domain::ExecutionOperationKind::CreateDirectory\n                | domain::ExecutionOperationKind::RemoveDirectoryIfEmpty => unreachable!(),''',
    1,
    "nested manifest unreachable arms",
)
write(path, text)


# 5) Native worker: cleanup forward, recreate on rollback.
path = "workers/operation-executor/src/lib.rs"
text = read(path)
anchor = '''            (\n                OperationPrimitiveManifest::CreateDirectory {\n                    destination_relative_path,\n                },\n                OperationDirection::Rollback,\n            ) => self.remove_directory(&root, destination_relative_path),\n            (\n                OperationPrimitiveManifest::SameVolumeMove {'''
replacement = '''            (\n                OperationPrimitiveManifest::CreateDirectory {\n                    destination_relative_path,\n                },\n                OperationDirection::Rollback,\n            ) => self.remove_directory(&root, destination_relative_path),\n            (\n                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {\n                    source_relative_path,\n                },\n                OperationDirection::Forward,\n            ) => self.remove_directory(&root, source_relative_path),\n            (\n                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {\n                    source_relative_path,\n                },\n                OperationDirection::Rollback,\n            ) => self.create_directory(&root, source_relative_path),\n            (\n                OperationPrimitiveManifest::SameVolumeMove {'''
text = replace_once(text, anchor, replacement, "native worker cleanup dispatch")
write(path, text)


# 6) Sandbox executor parity.
path = "crates/application/tests/support/mod.rs"
text = read(path)
anchor = '''            (\n                OperationPrimitiveManifest::CreateDirectory {\n                    destination_relative_path,\n                },\n                OperationDirection::Rollback,\n            ) => operations.remove_directory_if_empty(&self.root.join(destination_relative_path)),\n            (primitive, OperationDirection::Forward) => {'''
replacement = '''            (\n                OperationPrimitiveManifest::CreateDirectory {\n                    destination_relative_path,\n                },\n                OperationDirection::Rollback,\n            ) => operations.remove_directory_if_empty(&self.root.join(destination_relative_path)),\n            (\n                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {\n                    source_relative_path,\n                },\n                OperationDirection::Forward,\n            ) => operations.remove_directory_if_empty(&self.root.join(source_relative_path)),\n            (\n                OperationPrimitiveManifest::RemoveDirectoryIfEmpty {\n                    source_relative_path,\n                },\n                OperationDirection::Rollback,\n            ) => operations.create_directory_no_replace(&self.root.join(source_relative_path)),\n            (primitive, OperationDirection::Forward) => {'''
text = replace_once(text, anchor, replacement, "sandbox cleanup dispatch")
text = replace_once(
    text,
    '''        OperationPrimitiveManifest::CreateDirectory { .. } => Err(\n            ApprovedExecutorError::Ambiguous("expected a file primitive".to_owned()),\n        ),''',
    '''        OperationPrimitiveManifest::CreateDirectory { .. }\n        | OperationPrimitiveManifest::RemoveDirectoryIfEmpty { .. } => Err(\n            ApprovedExecutorError::Ambiguous("expected a file primitive".to_owned()),\n        ),''',
    "sandbox file primitive cleanup exclusion",
)
write(path, text)


# 7) Application planner/state machine: add only provably-empty-after-plan source directories.
path = "crates/application/src/execution.rs"
text = read(path)
prep_anchor = '''        let (mut directories, user_directory_count) = self.plan_directories(\n            execution_id,\n            &canonical_root,\n            &all_destinations,\n            &user_destinations,\n        )?;\n        directories.extend(executable);\n        directories.extend(\n            candidates'''
prep_new = '''        let (mut directories, user_directory_count) = self.plan_directories(\n            execution_id,\n            &canonical_root,\n            &all_destinations,\n            &user_destinations,\n        )?;\n        let source_cleanup = self.plan_source_directory_cleanup(\n            execution_id,\n            &canonical_root,\n            &executable,\n            &user_destinations,\n        )?;\n        directories.extend(executable);\n        directories.extend(source_cleanup);\n        directories.extend(\n            candidates'''
text = replace_once(text, prep_anchor, prep_new, "source cleanup planner hook")

method_anchor = '''    fn unique_staging_path(\n        &self,'''
cleanup_methods = r'''    fn plan_source_directory_cleanup(
        &self,
        execution_id: ExecutionId,
        root: &Path,
        executable: &[ExecutionOperation],
        user_destinations: &[String],
    ) -> Result<Vec<ExecutionOperation>, ApplicationError> {
        let planned_sources = executable
            .iter()
            .filter(|operation| {
                operation.proposal_operation_id.is_some()
                    && operation.kind != ExecutionOperationKind::InternalStage
            })
            .filter_map(|operation| {
                operation
                    .original_source_relative_path
                    .as_deref()
                    .or(operation.source_relative_path.as_deref())
            })
            .map(normalize_relative_string)
            .collect::<HashSet<_>>();
        if planned_sources.is_empty() {
            return Ok(Vec::new());
        }
        let destination_paths = user_destinations
            .iter()
            .map(|value| normalize_relative_string(value))
            .collect::<Vec<_>>();
        let mut top_levels = planned_sources
            .iter()
            .filter_map(|source| source.split('/').next())
            .filter(|top| !top.is_empty() && !source_cleanup_top_level_is_protected(top))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        let mut removable = Vec::<String>::new();
        top_levels.retain(|top| {
            let mut discovered = Vec::new();
            let safe = collect_fully_planned_directory_tree(
                root,
                top,
                &planned_sources,
                &destination_paths,
                &mut discovered,
            )
            .unwrap_or(false);
            if safe {
                removable.extend(discovered);
            }
            safe
        });
        removable.sort_by(|left, right| {
            let left_depth = left.split('/').count();
            let right_depth = right.split('/').count();
            right_depth.cmp(&left_depth).then_with(|| left.cmp(right))
        });
        removable.dedup();
        Ok(removable
            .into_iter()
            .map(|relative| ExecutionOperation {
                id: OperationStepId::new(),
                execution_id,
                proposal_operation_id: None,
                kind: ExecutionOperationKind::RemoveDirectoryIfEmpty,
                source_relative_path: Some(relative.clone()),
                destination_relative_path: relative.clone(),
                original_source_relative_path: Some(relative),
                expected_source_hash: None,
                expected_source_size: None,
                expected_source_modified_at: None,
                live_fingerprint: None,
                post_fingerprint: None,
                preconditions: vec![
                    "source_directory_is_unlinked".to_owned(),
                    "source_directory_is_empty_after_planned_moves".to_owned(),
                ],
                dependencies: Vec::new(),
                sequence: 0,
                status: ExecutionOperationStatus::PreflightOk,
                directory_existed_before: Some(true),
                reason: Some(
                    "Remove an emptied source folder so Ranger cleans the visible folder tree."
                        .to_owned(),
                ),
                error_code: None,
                error_message: None,
                started_at: None,
                completed_at: None,
                rolled_back_at: None,
            })
            .collect())
    }

'''
text = replace_once(text, method_anchor, cleanup_methods + method_anchor, "cleanup planner methods")

# Revalidation / postcondition / rollback / recovery special cases.
old = '''    fn revalidate_operation(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<Option<FileFingerprint>, ApplicationError> {\n        let destination_relative = relative_path(&operation.destination_relative_path)?;'''
new = '''    fn revalidate_operation(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<Option<FileFingerprint>, ApplicationError> {\n        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {\n            let source_relative = relative_path(\n                operation\n                    .source_relative_path\n                    .as_deref()\n                    .ok_or(ApplicationError::InvalidExecution)?,\n            )?;\n            let source = root.join(source_relative);\n            let metadata = fs::symlink_metadata(&source)?;\n            if metadata.file_type().is_symlink() || !metadata.is_dir() {\n                return Err(ApplicationError::InvalidExecution);\n            }\n            let mut entries = fs::read_dir(&source)?;\n            if entries.next().transpose()?.is_some() {\n                return Err(ApplicationError::Operations(OperationsError::Platform(\n                    PlatformError::Precondition(\n                        "source directory is not empty after its planned moves".to_owned(),\n                    ),\n                )));\n            }\n            return Ok(None);\n        }\n        let destination_relative = relative_path(&operation.destination_relative_path)?;'''
text = replace_once(text, old, new, "cleanup forward revalidation")

old = '''    fn verify_postcondition(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<Option<FileFingerprint>, ApplicationError> {\n        let destination = root.join(relative_path(&operation.destination_relative_path)?);'''
new = '''    fn verify_postcondition(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<Option<FileFingerprint>, ApplicationError> {\n        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {\n            let source = root.join(relative_path(\n                operation\n                    .source_relative_path\n                    .as_deref()\n                    .ok_or(ApplicationError::InvalidExecution)?,\n            )?);\n            return match fs::symlink_metadata(source) {\n                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),\n                _ => Err(ApplicationError::InvalidExecution),\n            };\n        }\n        let destination = root.join(relative_path(&operation.destination_relative_path)?);'''
text = replace_once(text, old, new, "cleanup forward postcondition")

old = '''    fn revalidate_rollback(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<Option<FileFingerprint>, ApplicationError> {\n        let current_relative = relative_path(&operation.destination_relative_path)?;'''
new = '''    fn revalidate_rollback(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<Option<FileFingerprint>, ApplicationError> {\n        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {\n            let restore_relative = relative_path(\n                operation\n                    .source_relative_path\n                    .as_deref()\n                    .ok_or(ApplicationError::InvalidExecution)?,\n            )?;\n            self.policy\n                .resolve_absent_destination(root, &restore_relative, false)?;\n            return Ok(None);\n        }\n        let current_relative = relative_path(&operation.destination_relative_path)?;'''
text = replace_once(text, old, new, "cleanup rollback revalidation")

old = '''    fn verify_rollback_postcondition(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<(), ApplicationError> {\n        let prior_destination = root.join(relative_path(&operation.destination_relative_path)?);'''
new = '''    fn verify_rollback_postcondition(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<(), ApplicationError> {\n        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {\n            let restored = root.join(relative_path(\n                operation\n                    .source_relative_path\n                    .as_deref()\n                    .ok_or(ApplicationError::InvalidExecution)?,\n            )?);\n            let metadata = fs::symlink_metadata(restored)?;\n            if metadata.file_type().is_symlink() || !metadata.is_dir() {\n                return Err(ApplicationError::InvalidExecution);\n            }\n            return Ok(());\n        }\n        let prior_destination = root.join(relative_path(&operation.destination_relative_path)?);'''
text = replace_once(text, old, new, "cleanup rollback postcondition")

old = '''    fn observe_recovery(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<RecoveryObservation, ApplicationError> {\n        let destination = root.join(relative_path(&operation.destination_relative_path)?);'''
new = '''    fn observe_recovery(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<RecoveryObservation, ApplicationError> {\n        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {\n            let source = root.join(relative_path(\n                operation\n                    .source_relative_path\n                    .as_deref()\n                    .ok_or(ApplicationError::InvalidExecution)?,\n            )?);\n            return match fs::symlink_metadata(source) {\n                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {\n                    Ok(RecoveryObservation::Applied(None))\n                }\n                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {\n                    Ok(RecoveryObservation::NotStarted)\n                }\n                Ok(_) => Ok(RecoveryObservation::Ambiguous(\n                    "Cleanup source path contains an unexpected entry.".to_owned(),\n                )),\n                Err(error) => Err(error.into()),\n            };\n        }\n        let destination = root.join(relative_path(&operation.destination_relative_path)?);'''
text = replace_once(text, old, new, "cleanup recovery observation")

old = '''    fn observe_rollback_recovery(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<RecoveryObservation, ApplicationError> {\n        let current = root.join(relative_path(&operation.destination_relative_path)?);'''
new = '''    fn observe_rollback_recovery(\n        &self,\n        root: &Path,\n        operation: &ExecutionOperation,\n    ) -> Result<RecoveryObservation, ApplicationError> {\n        if operation.kind == ExecutionOperationKind::RemoveDirectoryIfEmpty {\n            let restored = root.join(relative_path(\n                operation\n                    .source_relative_path\n                    .as_deref()\n                    .ok_or(ApplicationError::InvalidExecution)?,\n            )?);\n            return match fs::symlink_metadata(restored) {\n                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {\n                    Ok(RecoveryObservation::Applied(None))\n                }\n                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {\n                    Ok(RecoveryObservation::NotStarted)\n                }\n                Ok(_) => Ok(RecoveryObservation::Ambiguous(\n                    "Rollback cleanup path contains an unexpected entry.".to_owned(),\n                )),\n                Err(error) => Err(error.into()),\n            };\n        }\n        let current = root.join(relative_path(&operation.destination_relative_path)?);'''
text = replace_once(text, old, new, "cleanup rollback recovery observation")

text = replace_once(
    text,
    '''            if operation.kind != ExecutionOperationKind::CreateDirectory\n                && operation.post_fingerprint.is_none()''',
    '''            if !matches!(\n                operation.kind,\n                ExecutionOperationKind::CreateDirectory\n                    | ExecutionOperationKind::RemoveDirectoryIfEmpty\n            ) && operation.post_fingerprint.is_none()''',
    "cleanup rollback authorization",
)
write(path, text)

# Add free helper functions near execution summary utilities.
text = read(path)
free_anchor = '''fn execution_summary(\n    proposal: &domain::OrganizationProposal,'''
free_helpers = r'''fn source_cleanup_top_level_is_protected(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('.')
        || matches!(
            lower.as_str(),
            "documents"
                | "images"
                | "pictures"
                | "photos"
                | "vidéos"
                | "videos"
                | "archives"
                | "installateurs"
                | "à vérifier"
                | "a verifier"
                | "développement"
                | "developpement"
                | "applications"
                | "library"
                | "system"
                | "windows"
                | "program files"
                | "program files (x86)"
        )
}

fn collect_fully_planned_directory_tree(
    root: &Path,
    relative_directory: &str,
    planned_sources: &HashSet<String>,
    destination_paths: &[String],
    removable: &mut Vec<String>,
) -> Result<bool, ApplicationError> {
    let normalized_directory = normalize_relative_string(relative_directory);
    if destination_paths.iter().any(|destination| {
        destination == &normalized_directory
            || destination.starts_with(&(normalized_directory.clone() + "/"))
    }) {
        return Ok(false);
    }
    let absolute = root.join(relative_path(&normalized_directory)?);
    let metadata = fs::symlink_metadata(&absolute)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }
    let mut children = fs::read_dir(&absolute)?
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let entry_path = entry.path();
        let entry_metadata = fs::symlink_metadata(&entry_path)?;
        if entry_metadata.file_type().is_symlink() {
            return Ok(false);
        }
        let child_relative = entry_path
            .strip_prefix(root)
            .map_err(|_| ApplicationError::InvalidExecution)?
            .to_string_lossy()
            .replace('\\', "/");
        if entry_metadata.is_dir() {
            if !collect_fully_planned_directory_tree(
                root,
                &child_relative,
                planned_sources,
                destination_paths,
                removable,
            )? {
                return Ok(false);
            }
        } else if entry_metadata.is_file() {
            if !planned_sources.contains(&normalize_relative_string(&child_relative)) {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    removable.push(normalized_directory);
    Ok(true)
}

'''
text = replace_once(text, free_anchor, free_helpers + free_anchor, "cleanup filesystem helpers")
write(path, text)


# 8) Documentation JSON schema for the new authenticated primitive.
path = "schemas/operation-executor-protocol-v2.schema.json"
data = json.loads(read(path))
one_of = data["$defs"]["operationPrimitive"]["oneOf"]
cleanup_schema = {
    "type": "object",
    "additionalProperties": False,
    "required": ["kind", "source_relative_path"],
    "properties": {
        "kind": {"const": "remove_directory_if_empty"},
        "source_relative_path": {"$ref": "#/$defs/relativePath"},
    },
}
if not any(
    item.get("properties", {}).get("kind", {}).get("const") == "remove_directory_if_empty"
    for item in one_of
    if isinstance(item, dict)
):
    one_of.insert(1, cleanup_schema)
write(path, json.dumps(data, ensure_ascii=False, indent=2) + "\n")


# 9) Real acceptance test: reproduce a cluttered Desktop with whole project folders.
path = "crates/application/tests/one_click_real_user_pipeline.rs"
text = read(path)
fixture_anchor = '''    sandbox.write(\n        "Desktop/Ancien dossier/Sous dossier/photo.jpg",\n        b"fake-jpeg-private-beta-fixture",\n    );\n\n    let initial = sandbox.snapshot();'''
fixture_new = '''    sandbox.write(\n        "Desktop/Ancien dossier/Sous dossier/photo.jpg",\n        b"fake-jpeg-private-beta-fixture",\n    );\n    sandbox.write(\n        "Desktop/portfolio/package.json",\n        br#"{\"name\":\"portfolio\",\"scripts\":{\"dev\":\"vite\"}}"#,\n    );\n    sandbox.write(\n        "Desktop/portfolio/src/index.js",\n        b"console.log('portfolio');",\n    );\n    sandbox.write(\n        "Desktop/lodash/package.json",\n        br#"{\"name\":\"lodash-local\"}"#,\n    );\n    sandbox.write(\n        "Desktop/lodash/fp/map.js",\n        b"export const map = () => {};",\n    );\n    sandbox.write(\n        "Desktop/maquette-experience-esport/index.html",\n        b"<html><body>maquette esport</body></html>",\n    );\n    sandbox.write(\n        "Desktop/maquette-experience-esport/assets/app.css",\n        b"body { margin: 0; }",\n    );\n\n    let initial = sandbox.snapshot();'''
text = replace_once(text, fixture_anchor, fixture_new, "real test project fixtures")
text = replace_once(
    text,
    '''    assert_eq!(scan.indexed_count, 3, "all nested fixture files must be indexed");''',
    '''    assert_eq!(scan.indexed_count, 9, "all nested fixture files must be indexed");''',
    "real test indexed count",
)
text = replace_once(
    text,
    '''    assert_eq!(proposal.summary.files_analyzed, 3);''',
    '''    assert_eq!(proposal.summary.files_analyzed, 9);''',
    "real test analyzed count",
)
text = replace_once(
    text,
    '''        proposal.summary.proposed_moves >= 3,''',
    '''        proposal.summary.proposed_moves >= 9,''',
    "real test move count",
)
assert_anchor = '''    assert!(!desktop.join("Ancien dossier/Sous dossier/photo.jpg").exists());\n    assert_eq!(\n        fs::read(&invoice_destination)'''
assert_new = '''    assert!(!desktop.join("Ancien dossier/Sous dossier/photo.jpg").exists());\n\n    let portfolio_destination = desktop\n        .join("Développement")\n        .join("Projets")\n        .join("portfolio");\n    let lodash_destination = desktop\n        .join("Développement")\n        .join("Projets")\n        .join("lodash");\n    let maquette_destination = desktop\n        .join("Développement")\n        .join("Projets")\n        .join("maquette-experience-esport");\n    assert!(portfolio_destination.join("package.json").is_file());\n    assert!(portfolio_destination.join("src/index.js").is_file());\n    assert!(lodash_destination.join("package.json").is_file());\n    assert!(lodash_destination.join("fp/map.js").is_file());\n    assert!(maquette_destination.join("index.html").is_file());\n    assert!(maquette_destination.join("assets/app.css").is_file());\n    assert!(\n        !desktop.join("portfolio").exists(),\n        "the old top-level project folder must be removed once empty"\n    );\n    assert!(\n        !desktop.join("lodash").exists(),\n        "package-like clutter must no longer remain on the Desktop"\n    );\n    assert!(\n        !desktop.join("maquette-experience-esport").exists(),\n        "work/project folder must move as one preserved tree"\n    );\n\n    assert_eq!(\n        fs::read(&invoice_destination)'''
text = replace_once(text, assert_anchor, assert_new, "real test folder cleanup assertions")
write(path, text)

print("whole-user folder organization patch applied")
