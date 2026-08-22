from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    if old not in text:
        raise SystemExit(f"marker missing in {path}: {old[:100]!r}")
    p.write_text(text.replace(old, new, 1))


# Persistence module export for bounded-memory consumer scan writes.
p = Path("crates/persistence/src/lib.rs")
s = p.read_text()
if "mod consumer_scan;" not in s:
    s = s.replace("mod ann_chunks;\n", "mod ann_chunks;\nmod consumer_scan;\n", 1)
if "pub use consumer_scan::*;" not in s:
    s = s.replace("pub use ann_chunks::{\n", "pub use consumer_scan::*;\npub use ann_chunks::{\n", 1)
p.write_text(s)

# Scanner: stream directory entries and persist in bounded batches.
p = Path("crates/application/src/scanner.rs")
s = p.read_text()
s = s.replace(
    "Database, DuplicateGroupInput, DuplicateGroupRecord, InventorySort, MonitoringRootStatus,\n",
    "ConsumerScanFinalization, Database, DuplicateGroupInput, DuplicateGroupRecord, InventorySort, MonitoringRootStatus,\n",
    1,
)
s = s.replace(
    "const CONSUMER_PROGRESS_EMIT_EVERY: u64 = 128;\n",
    "const CONSUMER_PROGRESS_EMIT_EVERY: u64 = 128;\nconst CONSUMER_SCAN_BATCH_SIZE: usize = 256;\n",
    1,
)
start_marker = "        let paths = top_level_regular_paths(&root.absolute_path_native, is_cancelled)?;\n"
end_marker = "\n    fn persist_catalog_output(\n"
start = s.find(start_marker)
end = s.find(end_marker, start)
if start < 0 or end < 0:
    raise SystemExit("consumer scan region markers missing")
new_tail = r'''        let mut files = Vec::with_capacity(CONSUMER_SCAN_BATCH_SIZE);
        let mut issues = Vec::with_capacity(CONSUMER_SCAN_BATCH_SIZE);
        let mut persisted_files = 0_u64;
        let mut issue_count = 0_u64;
        let mut cancelled = false;

        progress.phase = ScanPhase::Inspecting;
        on_progress(progress);

        let entries = fs::read_dir(&root.absolute_path_native).map_err(ApplicationError::Io)?;
        for entry_result in entries {
            if is_cancelled() {
                cancelled = true;
                break;
            }

            let entry = match entry_result {
                Ok(entry) => entry,
                Err(error) => {
                    progress.errors = progress.errors.saturating_add(1);
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                    issues.push(ScanIssueInput {
                        relative_path: ".".to_owned(),
                        code: "directory_entry_unreadable".to_owned(),
                        message: error.to_string(),
                        is_directory: false,
                        is_error: true,
                        skipped: true,
                    });
                    if issues.len() >= CONSUMER_SCAN_BATCH_SIZE {
                        let (file_count, issues_written) = flush_consumer_batch(
                            self.database.as_ref(),
                            workspace_id,
                            scan_id,
                            &mut files,
                            &mut issues,
                        )?;
                        persisted_files = persisted_files.saturating_add(file_count);
                        issue_count = issue_count.saturating_add(issues_written);
                    }
                    continue;
                }
            };

            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(error) => {
                    progress.errors = progress.errors.saturating_add(1);
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                    issues.push(ScanIssueInput {
                        relative_path: name.to_string_lossy().into_owned(),
                        code: "metadata_unavailable".to_owned(),
                        message: error.to_string(),
                        is_directory: false,
                        is_error: true,
                        skipped: true,
                    });
                    continue;
                }
            };
            if file_type.is_dir() {
                progress.directories_discovered = progress.directories_discovered.saturating_add(1);
                continue;
            }
            if file_type.is_symlink() {
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                issues.push(ScanIssueInput {
                    relative_path: name.to_string_lossy().into_owned(),
                    code: "reparse_point".to_owned(),
                    message: "symbolic links and aliases are intentionally left in place".to_owned(),
                    is_directory: false,
                    is_error: false,
                    skipped: true,
                });
                continue;
            }
            if !file_type.is_file() {
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                continue;
            }

            let relative_path = PathBuf::from(name);
            match inspect_consumer_metadata(
                self.read_only_platform.as_ref(),
                &root.absolute_path_native,
                &relative_path,
                &volume,
            ) {
                Ok(entry) => {
                    if entry.hidden {
                        progress.skipped_items = progress.skipped_items.saturating_add(1);
                        continue;
                    }
                    progress.files_discovered = progress.files_discovered.saturating_add(1);
                    progress.bytes_discovered =
                        progress.bytes_discovered.saturating_add(entry.byte_size);
                    if entry.cloud_placeholder {
                        issues.push(ScanIssueInput {
                            relative_path: relative_path.to_string_lossy().into_owned(),
                            code: "cloud_placeholder".to_owned(),
                            message: "cloud placeholder left in place; content was not hydrated"
                                .to_owned(),
                            is_directory: false,
                            is_error: false,
                            skipped: true,
                        });
                        progress.skipped_items = progress.skipped_items.saturating_add(1);
                    } else {
                        files.push(metadata_scan_file_input(
                            workspace_id,
                            root.id,
                            scan_id,
                            entry,
                        )?);
                    }
                }
                Err(error) => {
                    let issue = scan_issue_for_platform_error(&relative_path, &error);
                    if issue.is_error {
                        progress.errors = progress.errors.saturating_add(1);
                    }
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                    issues.push(issue);
                }
            }

            if files.len() + issues.len() >= CONSUMER_SCAN_BATCH_SIZE {
                let (file_count, issues_written) = flush_consumer_batch(
                    self.database.as_ref(),
                    workspace_id,
                    scan_id,
                    &mut files,
                    &mut issues,
                )?;
                persisted_files = persisted_files.saturating_add(file_count);
                issue_count = issue_count.saturating_add(issues_written);
            }
            progress.files_indexed = persisted_files
                .saturating_add(u64::try_from(files.len()).unwrap_or(u64::MAX));
            if progress.files_discovered % CONSUMER_PROGRESS_EMIT_EVERY == 0 {
                on_progress(progress);
            }
        }

        let (file_count, issues_written) = flush_consumer_batch(
            self.database.as_ref(),
            workspace_id,
            scan_id,
            &mut files,
            &mut issues,
        )?;
        persisted_files = persisted_files.saturating_add(file_count);
        issue_count = issue_count.saturating_add(issues_written);
        progress.files_indexed = persisted_files;
        on_progress(progress);

        progress.phase = if cancelled {
            ScanPhase::Cancelled
        } else {
            ScanPhase::Persisting
        };
        on_progress(progress);

        let scan = self
            .database
            .finalize_consumer_scan(&ConsumerScanFinalization {
                scan_id,
                status: if cancelled {
                    "cancelled".to_owned()
                } else {
                    "completed".to_owned()
                },
                files_discovered: progress.files_discovered,
                files_indexed: persisted_files,
                directories_discovered: progress.directories_discovered,
                bytes_discovered: progress.bytes_discovered,
                errors: progress.errors,
                skipped_items: progress.skipped_items,
                issue_count,
                truncated: false,
            })
            .map_err(ApplicationError::Persistence)?;
        progress.files_indexed = scan.indexed_count;
        progress.phase = if cancelled {
            ScanPhase::Cancelled
        } else {
            ScanPhase::Completed
        };
        on_progress(progress);
        Ok(scan)
    }
'''
s = s[:start] + new_tail + s[end:]
# Remove old all-paths-in-memory helper if still present.
helper_start = s.find("fn top_level_regular_paths(\n")
if helper_start >= 0:
    helper_end = s.find("\n#[inline(never)]\nfn default_content_engine", helper_start)
    if helper_end < 0:
        raise SystemExit("top_level_regular_paths end marker missing")
    s = s[:helper_start] + s[helper_end + 1:]
# Add bounded-batch flush helper.
insert_marker = "fn metadata_scan_file_input(\n"
insert_at = s.find(insert_marker)
if insert_at < 0:
    raise SystemExit("metadata_scan_file_input marker missing")
helper = r'''fn flush_consumer_batch(
    database: &Database,
    workspace_id: WorkspaceId,
    scan_id: ScanId,
    files: &mut Vec<ScanFileInput>,
    issues: &mut Vec<ScanIssueInput>,
) -> Result<(u64, u64), ApplicationError> {
    if files.is_empty() && issues.is_empty() {
        return Ok((0, 0));
    }
    let file_count = u64::try_from(files.len()).unwrap_or(u64::MAX);
    let issue_count = u64::try_from(issues.len()).unwrap_or(u64::MAX);
    database
        .append_consumer_scan_batch(workspace_id, scan_id, files, issues)
        .map_err(ApplicationError::Persistence)?;
    files.clear();
    issues.clear();
    Ok((file_count, issue_count))
}

'''
s = s[:insert_at] + helper + s[insert_at:]
p.write_text(s)

# Consumer policy: deeper deterministic document categories and protected project manifests.
p = Path("crates/organizer/src/consumer.rs")
s = p.read_text()
s = s.replace(
    "    if is_unknown_executable(extension.as_deref()) {\n        return leave_in_place(\n            \"uncertain_program\",\n            \"Ce fichier ressemble à un programme. ZEMO l’a laissé en place.\",\n        );\n    }\n\n",
    "    if is_unknown_executable(extension.as_deref()) {\n        return leave_in_place(\n            \"uncertain_program\",\n            \"Ce fichier ressemble à un programme. ZEMO l’a laissé en place.\",\n        );\n    }\n\n    if is_project_manifest(&name_lower) {\n        return leave_in_place(\n            \"project_manifest\",\n            \"Ce fichier peut définir un projet ou un outil de développement. ZEMO l’a laissé en place.\",\n        );\n    }\n\n",
    1,
)
old_doc = r'''fn document_destination(
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
'''
new_doc = r'''fn document_destination(
    root_kind: ConsumerRootKind,
    name_lower: &str,
    document_type: Option<&str>,
) -> Vec<String> {
    let leaf = document_leaf_segments(name_lower, document_type);
    match root_kind {
        ConsumerRootKind::Documents => leaf.into_iter().map(str::to_owned).collect(),
        ConsumerRootKind::Pictures | ConsumerRootKind::Videos | ConsumerRootKind::Music => {
            std::iter::once(DOCUMENTS_FOLDER.to_owned())
                .chain(leaf.into_iter().map(str::to_owned))
                .collect()
        }
        _ => std::iter::once(DOCUMENTS_FOLDER.to_owned())
            .chain(leaf.into_iter().map(str::to_owned))
            .collect(),
    }
}

fn document_leaf_segments(name_lower: &str, document_type: Option<&str>) -> Vec<&'static str> {
    if matches!(document_type, Some("invoice" | "receipt"))
        || contains_any(name_lower, &["invoice", "facture", "receipt", "reçu", "recu", "quittance"])
    {
        return vec!["Administratif", "Factures"];
    }
    if matches!(document_type, Some("bank_statement"))
        || contains_any(name_lower, &["bank", "banque", "releve", "relevé", "iban", "rib"])
    {
        return vec!["Administratif", "Banque"];
    }
    if matches!(document_type, Some("insurance_document"))
        || contains_any(name_lower, &["assurance", "insurance", "mutuelle"])
    {
        return vec!["Administratif", "Assurances"];
    }
    if matches!(document_type, Some("tax_document"))
        || contains_any(name_lower, &["tax", "impot", "impôt", "fiscal", "fisc"])
    {
        return vec!["Administratif", "Impôts"];
    }
    if matches!(document_type, Some("contract"))
        || contains_any(name_lower, &["contrat", "contract", "cerfa", "admin"])
    {
        return vec!["Administratif"];
    }
    if contains_any(
        name_lower,
        &["school", "cours", "etude", "étude", "homework", "devoir", "université", "universite"],
    ) {
        return vec!["Études"];
    }
    if contains_any(
        name_lower,
        &["work", "travail", "meeting", "reunion", "réunion", "projet", "client", "chantier", "devis"],
    ) {
        return vec!["Travail"];
    }
    vec!["Personnel"]
}
'''
if old_doc not in s:
    raise SystemExit("document classifier marker missing")
s = s.replace(old_doc, new_doc, 1)
# Project manifests that must never be torn away from a development root.
manifest_marker = "fn is_unknown_executable(extension: Option<&str>) -> bool {\n"
manifest_at = s.find(manifest_marker)
if manifest_at < 0:
    raise SystemExit("manifest insert marker missing")
manifest_fn = r'''fn is_project_manifest(filename_lower: &str) -> bool {
    matches!(
        filename_lower,
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
            | "build.gradle"
            | "build.gradle.kts"
            | "gradle.properties"
            | "composer.json"
            | "composer.lock"
    )
}

'''
s = s[:manifest_at] + manifest_fn + s[manifest_at:]
# Update existing unit expectation for the refined invoice folder.
s = s.replace(
    'assert_eq!(invoice.destination, ["Documents", "Administratif"]);',
    'assert_eq!(invoice.destination, ["Documents", "Administratif", "Factures"]);',
    1,
)
p.write_text(s)

# Frontend: never claim success for a partial execution; verify Undo too.
p = Path("apps/desktop/src/App.tsx")
s = p.read_text()
old_apply = r'''        const completed = await startExecution(approved.session.id);
        const applied = completed.session.summary?.applied ?? 0;
        if (current.summary.proposedMoves > 0 && applied === 0) {
          throw new Error(
            `Apply returned zero physical moves for proposal ${current.id}; ZEMO will not report success.`,
          );
        }
        executionIds.push(completed.session.id);
        filesMoved += applied;'''
new_apply = r'''        const completed = await startExecution(approved.session.id);
        const summary = completed.session.summary;
        if (
          completed.session.status !== "COMPLETED" ||
          summary.failed > 0 ||
          summary.blocked > 0 ||
          summary.skipped > 0
        ) {
          throw new Error(
            `Apply incomplete for proposal ${current.id}: status=${completed.session.status}, applied=${summary.applied}, blocked=${summary.blocked}, skipped=${summary.skipped}, failed=${summary.failed}`,
          );
        }
        if (current.summary.proposedMoves > 0 && summary.applied === 0) {
          throw new Error(
            `Apply returned zero physical moves for proposal ${current.id}; ZEMO will not report success.`,
          );
        }
        executionIds.push(completed.session.id);
        filesMoved += current.summary.proposedMoves;'''
if old_apply not in s:
    raise SystemExit("App Apply marker missing")
s = s.replace(old_apply, new_apply, 1)
old_undo = r'''      for (const executionId of [...lastOrganize.executionIds].reverse()) {
        await rollbackExecution(executionId);
      }
      clearLastOrganizeResult();'''
new_undo = r'''      for (const executionId of [...lastOrganize.executionIds].reverse()) {
        const rolledBack = await rollbackExecution(executionId);
        if (
          rolledBack.session.status !== "ROLLED_BACK" ||
          rolledBack.session.summary.rollbackBlocked > 0 ||
          rolledBack.session.summary.rollbackFailed > 0
        ) {
          throw new Error(
            `Undo incomplete for execution ${executionId}: status=${rolledBack.session.status}, blocked=${rolledBack.session.summary.rollbackBlocked}, failed=${rolledBack.session.summary.rollbackFailed}`,
          );
        }
      }
      clearLastOrganizeResult();'''
if old_undo not in s:
    raise SystemExit("App Undo marker missing")
s = s.replace(old_undo, new_undo, 1)
p.write_text(s)

# UI wording: don't claim the whole computer is clean while existing folders are intentionally preserved.
p = Path("apps/desktop/src/OneClickOrganize.tsx")
s = p.read_text()
s = s.replace(
    '<h2 id="one-click-done-title">Votre ordinateur est rangé.</h2>',
    '<h2 id="one-click-done-title">Rangement appliqué.</h2>',
    1,
)
s = s.replace(
    '      <p>0 fichier écrasé</p>\n',
    '      <p>0 fichier écrasé</p>\n      <p className="one-click-note">Les applications, raccourcis et dossiers existants protégés sont laissés en place.</p>\n',
    1,
)
p.write_text(s)

# Scale acceptance: exercise more than ten thousand loose files.
p = Path("crates/application/tests/one_click_organize.rs")
s = p.read_text().replace("const FILE_COUNT: usize = 5_257;", "const FILE_COUNT: usize = 10_001;", 1)
p.write_text(s)

# Modernize the packaged macOS qualification harness to the actual product name.
p = Path("scripts/macos-apply-qualification/run.mjs")
s = p.read_text()
s = s.replace('"release/bundle/macos/Working Name.app"', '"release/bundle/macos/ZEMO.app"')
s = s.replace('"aarch64-apple-darwin/release/bundle/macos/Working Name.app"', '"aarch64-apple-darwin/release/bundle/macos/ZEMO.app"')
s = s.replace('throw new Error("release Working Name.app was not found after build");', 'throw new Error("release ZEMO.app was not found after build");')
s = s.replace('path.join(artifactsDirectory, "Working Name.app")', 'path.join(artifactsDirectory, "ZEMO.app")')
s = s.replace('`Working-Name-${packVersion}-arm64.dmg`', '`ZEMO-${packVersion}-arm64.dmg`')
s = s.replace('`${dmgChecksum}  Working-Name-${packVersion}-arm64.dmg\\n`', '`${dmgChecksum}  ZEMO-${packVersion}-arm64.dmg\\n`')
s = s.replace('path.join(appPath, "Contents/MacOS/Working Name")', 'path.join(appPath, "Contents/MacOS/ZEMO")')
p.write_text(s)

print("One-Click v3 patch applied")
