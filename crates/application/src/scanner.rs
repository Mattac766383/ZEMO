use crate::{ApplicationError, monitoring::MonitoringRuntime};
use catalog::{
    CatalogScanner, HashingStatus, ReadabilityStatus, ScanItemStatus, ScanPhase, ScanPolicy,
    ScanProgress,
};
use domain::{
    DisplayLabel, FileFingerprint, FileId, FileKind, FileObservation, FileVersionId,
    NativeFileIdentity, NativePath, PathEncoding, ScanId, VolumeIdentity, WorkspaceId,
};
use extraction::{ContentExtractionEngine, LocalExtractionEngine};
use knowledge::{DeterministicSemanticProvider, SemanticProvider};
use persistence::{
    ConsumerScanFinalization, Database, DuplicateGroupInput, DuplicateGroupRecord, InventorySort,
    MonitoringRootStatus, RootMonitoringConfiguration, RootRecord, ScanCompletionInput,
    ScanFileInput, ScanFileRecord, ScanIssueInput, ScanIssueRecord, ScanRecord, WorkspaceRecord,
};
use platform::{PlatformError, ReadOnlyEntry, ReadOnlyPlatform};
use search::{
    AnnIndexMeta, LocalEmbeddingProvider, OnnxLocalEmbeddingProvider, PersistentAnnIndex,
    UnavailableEmbeddingProvider,
};
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const CONSUMER_PROGRESS_EMIT_EVERY: u64 = 128;
const CONSUMER_SCAN_BATCH_SIZE: usize = 256;
const DEFAULT_MONITORING_SIZE_THRESHOLD_BYTES: u64 = 512 * 1_024 * 1_024;
const DEFAULT_MONITORING_STARTUP_ENTRY_LIMIT: u32 = 100_000;

/// Generated dependency/cache folders are not useful user documents and can
/// explode an otherwise small personal-folder scan. Hidden folders are already
/// skipped separately.
const DEEP_INDEX_EXCLUDED_DIRECTORY_NAMES: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".git",
    ".svn",
    ".hg",
];

/// Scanner-only application boundary. It deliberately owns no filesystem
/// mutation capability, parser, model gateway, or network client.
pub struct ScannerApplicationService {
    pub(crate) database: Arc<Database>,
    pub(crate) read_only_platform: Arc<dyn ReadOnlyPlatform>,
    pub(crate) content_engine: Arc<dyn ContentExtractionEngine>,
    pub(crate) semantic_provider: Arc<dyn SemanticProvider>,
    pub(crate) embedding_provider: Arc<dyn LocalEmbeddingProvider>,
    pub(crate) ann_root: Option<PathBuf>,
    pub(crate) ann_indexes: Mutex<HashMap<String, Arc<PersistentAnnIndex>>>,
    pub(crate) monitoring: MonitoringRuntime,
    scanner: CatalogScanner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannerSystemStatus {
    pub local_first: bool,
    pub read_only_scan: bool,
    pub network_disabled: bool,
    pub version: String,
}

impl std::fmt::Debug for ScannerApplicationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScannerApplicationService")
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

impl ScannerApplicationService {
    #[must_use]
    pub fn new(database: Arc<Database>, read_only_platform: Arc<dyn ReadOnlyPlatform>) -> Self {
        Self::new_with_model_root(database, read_only_platform, None)
    }

    #[must_use]
    pub fn new_with_model_root(
        database: Arc<Database>,
        read_only_platform: Arc<dyn ReadOnlyPlatform>,
        model_root: Option<PathBuf>,
    ) -> Self {
        Self::new_with_content_engine(
            database,
            read_only_platform,
            default_content_engine(),
            model_root,
        )
    }

    #[must_use]
    pub fn new_with_content_engine(
        database: Arc<Database>,
        read_only_platform: Arc<dyn ReadOnlyPlatform>,
        content_engine: Arc<dyn ContentExtractionEngine>,
        model_root: Option<PathBuf>,
    ) -> Self {
        Self::new_with_engines(
            database,
            read_only_platform,
            content_engine,
            default_semantic_provider(),
            model_root,
        )
    }

    #[must_use]
    pub fn new_with_engines(
        database: Arc<Database>,
        read_only_platform: Arc<dyn ReadOnlyPlatform>,
        content_engine: Arc<dyn ContentExtractionEngine>,
        semantic_provider: Arc<dyn SemanticProvider>,
        model_root: Option<PathBuf>,
    ) -> Self {
        let ann_root = model_root.as_ref().map(|root| root.join("ann"));
        let embedding_provider = production_embedding_provider(model_root);
        Self::new_with_all_engines_and_ann(
            database,
            read_only_platform,
            content_engine,
            semantic_provider,
            embedding_provider,
            ann_root,
        )
    }

    #[must_use]
    pub fn new_with_all_engines(
        database: Arc<Database>,
        read_only_platform: Arc<dyn ReadOnlyPlatform>,
        content_engine: Arc<dyn ContentExtractionEngine>,
        semantic_provider: Arc<dyn SemanticProvider>,
        embedding_provider: Arc<dyn LocalEmbeddingProvider>,
    ) -> Self {
        Self::new_with_all_engines_and_ann(
            database,
            read_only_platform,
            content_engine,
            semantic_provider,
            embedding_provider,
            None,
        )
    }

    #[must_use]
    pub fn new_with_all_engines_and_ann(
        database: Arc<Database>,
        read_only_platform: Arc<dyn ReadOnlyPlatform>,
        content_engine: Arc<dyn ContentExtractionEngine>,
        semantic_provider: Arc<dyn SemanticProvider>,
        embedding_provider: Arc<dyn LocalEmbeddingProvider>,
        ann_root: Option<PathBuf>,
    ) -> Self {
        Self {
            database,
            scanner: CatalogScanner::new(read_only_platform.clone()),
            read_only_platform,
            content_engine,
            semantic_provider,
            embedding_provider,
            ann_root,
            ann_indexes: Mutex::new(HashMap::new()),
            monitoring: MonitoringRuntime::default(),
        }
    }

    pub(crate) fn ann_index_for(
        &self,
        workspace_id: WorkspaceId,
    ) -> Option<Arc<PersistentAnnIndex>> {
        let root = self.ann_root.as_ref()?;
        let key = workspace_id.to_string();
        let mut guard = self.ann_indexes.lock().ok()?;
        if let Some(existing) = guard.get(&key) {
            return Some(existing.clone());
        }
        let descriptor = self.embedding_provider.descriptor();
        let expected = AnnIndexMeta::for_provider(
            &descriptor.provider_id,
            &descriptor.version,
            descriptor.dimensions,
        );
        let index = PersistentAnnIndex::open_with_expected(root, &key, expected).ok()?;
        let shared = Arc::new(index);
        guard.insert(key, shared.clone());
        Some(shared)
    }

    #[must_use]
    pub fn embedding_provider(&self) -> Arc<dyn LocalEmbeddingProvider> {
        self.embedding_provider.clone()
    }

    #[must_use]
    pub fn system_status(&self) -> ScannerSystemStatus {
        ScannerSystemStatus {
            local_first: true,
            read_only_scan: true,
            network_disabled: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    pub fn create_workspace(&self, name: &str) -> Result<WorkspaceRecord, ApplicationError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(ApplicationError::InvalidWorkspaceName);
        }
        let workspace = self
            .database
            .create_workspace(name)
            .map_err(ApplicationError::Persistence)?;
        self.database.set_current_workspace(workspace.id)?;
        self.database
            .ensure_workspace_monitoring_state(workspace.id)?;
        Ok(workspace)
    }

    #[inline(never)]
    pub fn register_root(
        &self,
        workspace_id: WorkspaceId,
        absolute_path: &Path,
    ) -> Result<RootRecord, ApplicationError> {
        self.database.workspace(workspace_id)?;
        let volume = self.read_only_platform.inspect_volume(absolute_path)?;

        if let Some(existing) = self
            .database
            .list_roots(workspace_id)?
            .into_iter()
            .find(|root| same_registered_path(&root.absolute_path_native, absolute_path))
        {
            self.database.set_current_workspace(workspace_id)?;
            self.database.set_current_root(workspace_id, existing.id)?;
            self.ensure_root_monitoring_metadata(workspace_id, existing.id)?;
            return Ok(existing);
        }

        let display_value = absolute_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| absolute_path.to_string_lossy().into_owned());
        let display_label =
            DisplayLabel::new(display_value).map_err(|_| ApplicationError::InvalidWorkspaceName)?;
        let root = self
            .database
            .register_root(
                workspace_id,
                domain::RootId::new(),
                absolute_path,
                display_label.as_str(),
                &volume,
            )
            .map_err(ApplicationError::Persistence)?;

        self.database.set_current_workspace(workspace_id)?;
        self.database.set_current_root(workspace_id, root.id)?;
        self.ensure_root_monitoring_metadata(workspace_id, root.id)?;
        Ok(root)
    }

    fn ensure_root_monitoring_metadata(
        &self,
        workspace_id: WorkspaceId,
        root_id: domain::RootId,
    ) -> Result<(), ApplicationError> {
        let state = self
            .database
            .ensure_workspace_monitoring_state(workspace_id)?;
        let already_configured = self
            .database
            .list_monitored_roots(workspace_id)?
            .into_iter()
            .any(|root| root.root_id == root_id);
        if already_configured {
            return Ok(());
        }
        self.database.configure_root_monitoring(
            root_id,
            RootMonitoringConfiguration {
                enabled: true,
                status: if state.paused {
                    MonitoringRootStatus::Paused
                } else {
                    MonitoringRootStatus::Starting
                },
                size_threshold_bytes: DEFAULT_MONITORING_SIZE_THRESHOLD_BYTES,
                startup_entry_limit: DEFAULT_MONITORING_STARTUP_ENTRY_LIMIT,
            },
        )?;
        self.database
            .mark_startup_reconciliation_pending(workspace_id)?;
        Ok(())
    }

    /// Standard personal folders use a metadata-only *deep* index. That gives
    /// search / relationships / the mental map visibility into existing
    /// subfolders without opening file contents during discovery. The separate
    /// `scan_workspace_consumer` method below deliberately remains top-level
    /// only for the destructive One-Click organization contract.
    #[inline(never)]
    pub fn scan_workspace(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanRecord, ApplicationError> {
        let root = self
            .database
            .restore_current_root(workspace_id)?
            .ok_or(ApplicationError::NotFound)?;
        if is_standard_personal_root(&root.absolute_path_native) {
            return self.scan_workspace_deep_consumer(workspace_id, is_cancelled, on_progress);
        }

        let scan_id = ScanId::new();
        self.database.begin_scan(workspace_id, root.id, scan_id)?;
        let output = self.scanner.scan_with_id_and_control(
            scan_id,
            workspace_id,
            root.id,
            &root.absolute_path_native,
            ScanPolicy::default(),
            is_cancelled,
            on_progress,
        );
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                let _ = self.database.fail_scan(scan_id, "catalog_failed");
                return Err(ApplicationError::Catalog(error));
            }
        };
        self.persist_catalog_output(workspace_id, &root, scan_id, output, on_progress)
    }

    /// One-Click organization discovery contract: metadata-only and top-level
    /// only. Existing user folder trees are intentionally left out of this
    /// specific scan so One-Click never decides to reorganize an already
    /// arranged subtree.
    #[inline(never)]
    pub fn scan_workspace_consumer(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanRecord, ApplicationError> {
        let root = self
            .database
            .restore_current_root(workspace_id)?
            .ok_or(ApplicationError::NotFound)?;
        let scan_id = ScanId::new();
        self.database.begin_scan(workspace_id, root.id, scan_id)?;
        let volume = self
            .read_only_platform
            .inspect_volume(&root.absolute_path_native)?;

        let mut progress = initial_consumer_progress(scan_id);
        on_progress(progress);

        let mut files = Vec::with_capacity(CONSUMER_SCAN_BATCH_SIZE);
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
                    flush_if_needed(
                        self.database.as_ref(),
                        workspace_id,
                        scan_id,
                        &mut files,
                        &mut issues,
                        &mut persisted_files,
                        &mut issue_count,
                    )?;
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
                    flush_if_needed(
                        self.database.as_ref(),
                        workspace_id,
                        scan_id,
                        &mut files,
                        &mut issues,
                        &mut persisted_files,
                        &mut issue_count,
                    )?;
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
                    message: "symbolic links and aliases are intentionally left in place"
                        .to_owned(),
                    is_directory: false,
                    is_error: false,
                    skipped: true,
                });
                flush_if_needed(
                    self.database.as_ref(),
                    workspace_id,
                    scan_id,
                    &mut files,
                    &mut issues,
                    &mut persisted_files,
                    &mut issue_count,
                )?;
                continue;
            }
            if !file_type.is_file() {
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                continue;
            }

            let relative_path = PathBuf::from(name);
            inspect_and_queue_consumer_file(
                self.read_only_platform.as_ref(),
                workspace_id,
                root.id,
                scan_id,
                &root.absolute_path_native,
                &relative_path,
                &volume,
                &mut files,
                &mut issues,
                &mut progress,
            )?;

            flush_if_needed(
                self.database.as_ref(),
                workspace_id,
                scan_id,
                &mut files,
                &mut issues,
                &mut persisted_files,
                &mut issue_count,
            )?;
            progress.files_indexed = persisted_files
                .saturating_add(u64::try_from(files.len()).unwrap_or(u64::MAX));
            if progress.files_discovered % CONSUMER_PROGRESS_EMIT_EVERY == 0 {
                on_progress(progress);
            }
        }

        finalize_consumer_scan(
            self.database.as_ref(),
            workspace_id,
            scan_id,
            &mut files,
            &mut issues,
            persisted_files,
            issue_count,
            progress,
            cancelled,
            on_progress,
        )
    }

    /// Deep metadata-only index for standard personal folders. It is iterative
    /// rather than recursively calling itself so an adversarial/deep directory
    /// tree cannot overflow the process stack. Symlinks/junctions are never
    /// followed, unreadable nested folders become issues, and scanning continues.
    #[inline(never)]
    pub fn scan_workspace_deep_consumer(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanRecord, ApplicationError> {
        let root = self
            .database
            .restore_current_root(workspace_id)?
            .ok_or(ApplicationError::NotFound)?;
        let scan_id = ScanId::new();
        self.database.begin_scan(workspace_id, root.id, scan_id)?;
        let volume = self
            .read_only_platform
            .inspect_volume(&root.absolute_path_native)?;

        let mut progress = initial_consumer_progress(scan_id);
        on_progress(progress);

        let mut files = Vec::with_capacity(CONSUMER_SCAN_BATCH_SIZE);
        let mut issues = Vec::with_capacity(CONSUMER_SCAN_BATCH_SIZE);
        let mut persisted_files = 0_u64;
        let mut issue_count = 0_u64;
        let mut cancelled = false;
        let mut entries_seen = 0_u64;
        let mut pending_directories = vec![PathBuf::new()];

        progress.phase = ScanPhase::Inspecting;
        on_progress(progress);

        while let Some(relative_directory) = pending_directories.pop() {
            if is_cancelled() {
                cancelled = true;
                break;
            }

            let absolute_directory = if relative_directory.as_os_str().is_empty() {
                root.absolute_path_native.clone()
            } else {
                root.absolute_path_native.join(&relative_directory)
            };

            let entries = match fs::read_dir(&absolute_directory) {
                Ok(entries) => entries,
                Err(error) => {
                    if relative_directory.as_os_str().is_empty() {
                        let _ = self.database.fail_scan(scan_id, "root_unreadable");
                        return Err(ApplicationError::Io(error));
                    }
                    progress.errors = progress.errors.saturating_add(1);
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                    issues.push(ScanIssueInput {
                        relative_path: display_relative_path(&relative_directory),
                        code: "directory_unreadable".to_owned(),
                        message: error.to_string(),
                        is_directory: true,
                        is_error: true,
                        skipped: true,
                    });
                    flush_if_needed(
                        self.database.as_ref(),
                        workspace_id,
                        scan_id,
                        &mut files,
                        &mut issues,
                        &mut persisted_files,
                        &mut issue_count,
                    )?;
                    continue;
                }
            };

            for entry_result in entries {
                if is_cancelled() {
                    cancelled = true;
                    break;
                }
                entries_seen = entries_seen.saturating_add(1);

                let entry = match entry_result {
                    Ok(entry) => entry,
                    Err(error) => {
                        progress.errors = progress.errors.saturating_add(1);
                        progress.skipped_items = progress.skipped_items.saturating_add(1);
                        issues.push(ScanIssueInput {
                            relative_path: display_relative_path(&relative_directory),
                            code: "directory_entry_unreadable".to_owned(),
                            message: error.to_string(),
                            is_directory: false,
                            is_error: true,
                            skipped: true,
                        });
                        flush_if_needed(
                            self.database.as_ref(),
                            workspace_id,
                            scan_id,
                            &mut files,
                            &mut issues,
                            &mut persisted_files,
                            &mut issue_count,
                        )?;
                        continue;
                    }
                };

                let name = entry.file_name();
                let relative_path = relative_directory.join(&name);

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
                            relative_path: display_relative_path(&relative_path),
                            code: "metadata_unavailable".to_owned(),
                            message: error.to_string(),
                            is_directory: false,
                            is_error: true,
                            skipped: true,
                        });
                        flush_if_needed(
                            self.database.as_ref(),
                            workspace_id,
                            scan_id,
                            &mut files,
                            &mut issues,
                            &mut persisted_files,
                            &mut issue_count,
                        )?;
                        continue;
                    }
                };

                if file_type.is_symlink() {
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                    issues.push(ScanIssueInput {
                        relative_path: display_relative_path(&relative_path),
                        code: "reparse_point".to_owned(),
                        message: "symbolic links, aliases and junctions are intentionally not followed"
                            .to_owned(),
                        is_directory: false,
                        is_error: false,
                        skipped: true,
                    });
                } else if file_type.is_dir() {
                    progress.directories_discovered =
                        progress.directories_discovered.saturating_add(1);
                    if should_skip_deep_index_directory(&name) {
                        progress.skipped_items = progress.skipped_items.saturating_add(1);
                        issues.push(ScanIssueInput {
                            relative_path: display_relative_path(&relative_path),
                            code: "generated_directory_excluded".to_owned(),
                            message: "generated dependency/cache directory excluded from deep indexing"
                                .to_owned(),
                            is_directory: true,
                            is_error: false,
                            skipped: true,
                        });
                    } else {
                        pending_directories.push(relative_path);
                    }
                } else if file_type.is_file() {
                    inspect_and_queue_consumer_file(
                        self.read_only_platform.as_ref(),
                        workspace_id,
                        root.id,
                        scan_id,
                        &root.absolute_path_native,
                        &relative_path,
                        &volume,
                        &mut files,
                        &mut issues,
                        &mut progress,
                    )?;
                } else {
                    progress.skipped_items = progress.skipped_items.saturating_add(1);
                }

                flush_if_needed(
                    self.database.as_ref(),
                    workspace_id,
                    scan_id,
                    &mut files,
                    &mut issues,
                    &mut persisted_files,
                    &mut issue_count,
                )?;
                progress.files_indexed = persisted_files
                    .saturating_add(u64::try_from(files.len()).unwrap_or(u64::MAX));
                if entries_seen % CONSUMER_PROGRESS_EMIT_EVERY == 0 {
                    on_progress(progress);
                }
            }

            if cancelled {
                break;
            }
        }

        finalize_consumer_scan(
            self.database.as_ref(),
            workspace_id,
            scan_id,
            &mut files,
            &mut issues,
            persisted_files,
            issue_count,
            progress,
            cancelled,
            on_progress,
        )
    }

    fn persist_catalog_output(
        &self,
        workspace_id: WorkspaceId,
        root: &RootRecord,
        scan_id: ScanId,
        output: catalog::ScanOutput,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanRecord, ApplicationError> {
        let files = output
            .files
            .iter()
            .map(|file| ScanFileInput {
                observation: file.observation.clone(),
                extension: file.extension.clone(),
                accessed_at_ns: file.accessed_at_ns,
                readability_status: readability_status(file.readability_status).to_owned(),
                scan_status: scan_item_status(file.scan_status).to_owned(),
                hashing_status: hashing_status(file.hashing_status).to_owned(),
                error_code: file.error.map(|kind| kind.code().to_owned()),
            })
            .collect();
        let issues = output
            .issues
            .iter()
            .map(|issue| ScanIssueInput {
                relative_path: issue.relative_path.clone(),
                code: issue.kind.code().to_owned(),
                message: issue.message.clone(),
                is_directory: issue.is_directory,
                is_error: issue.kind.is_error(),
                skipped: issue.skipped,
            })
            .collect();
        let duplicate_groups = output
            .duplicate_groups
            .iter()
            .map(|group| DuplicateGroupInput {
                digest: group.key.clone(),
                byte_size: group.byte_size,
                members: group.members.clone(),
            })
            .collect();
        let completion = ScanCompletionInput {
            scan_id,
            workspace_id,
            root_id: root.id,
            status: if output.cancelled {
                "cancelled".to_owned()
            } else {
                "completed".to_owned()
            },
            files_discovered: output.progress.files_discovered,
            directories_discovered: output.progress.directories_discovered,
            bytes_discovered: output.progress.bytes_discovered,
            files_hashed: output.progress.files_hashed,
            errors: output.progress.errors,
            skipped_items: output.progress.skipped_items,
            truncated: output.truncated,
            files,
            issues,
            duplicate_groups,
        };
        let persisted = match self.database.complete_scan(&completion) {
            Ok(persisted) => persisted,
            Err(error) => {
                let _ = self.database.fail_scan(scan_id, "persistence_failed");
                return Err(ApplicationError::Persistence(error));
            }
        };
        let mut final_progress = output.progress;
        final_progress.files_indexed = persisted.scan.indexed_count;
        final_progress.phase = if output.cancelled {
            ScanPhase::Cancelled
        } else {
            ScanPhase::Completed
        };
        on_progress(final_progress);
        Ok(persisted.scan)
    }

    pub fn scan_files(
        &self,
        scan_id: ScanId,
        sort: InventorySort,
        descending: bool,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ScanFileRecord>, ApplicationError> {
        self.database
            .scan_files(scan_id, sort, descending, limit, offset)
            .map_err(ApplicationError::Persistence)
    }

    pub fn scan_duplicate_groups(
        &self,
        scan_id: ScanId,
    ) -> Result<Vec<DuplicateGroupRecord>, ApplicationError> {
        self.database
            .scan_duplicate_groups(scan_id)
            .map_err(ApplicationError::Persistence)
    }

    pub fn scan_issues(&self, scan_id: ScanId) -> Result<Vec<ScanIssueRecord>, ApplicationError> {
        self.database
            .scan_issues(scan_id)
            .map_err(ApplicationError::Persistence)
    }
}

const fn initial_consumer_progress(scan_id: ScanId) -> ScanProgress {
    ScanProgress {
        scan_id,
        phase: ScanPhase::Discovering,
        files_discovered: 0,
        files_indexed: 0,
        directories_discovered: 1,
        bytes_discovered: 0,
        files_hashed: 0,
        errors: 0,
        skipped_items: 0,
        duplicate_groups: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect_and_queue_consumer_file(
    platform: &dyn ReadOnlyPlatform,
    workspace_id: WorkspaceId,
    root_id: domain::RootId,
    scan_id: ScanId,
    root: &Path,
    relative_path: &Path,
    volume: &VolumeIdentity,
    files: &mut Vec<ScanFileInput>,
    issues: &mut Vec<ScanIssueInput>,
    progress: &mut ScanProgress,
) -> Result<(), ApplicationError> {
    match inspect_consumer_metadata(platform, root, relative_path, volume) {
        Ok(entry) => {
            if entry.hidden {
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                return Ok(());
            }
            progress.files_discovered = progress.files_discovered.saturating_add(1);
            progress.bytes_discovered = progress.bytes_discovered.saturating_add(entry.byte_size);
            if entry.cloud_placeholder {
                issues.push(ScanIssueInput {
                    relative_path: display_relative_path(relative_path),
                    code: "cloud_placeholder".to_owned(),
                    message: "cloud placeholder left in place; content was not hydrated".to_owned(),
                    is_directory: false,
                    is_error: false,
                    skipped: true,
                });
                progress.skipped_items = progress.skipped_items.saturating_add(1);
            } else {
                files.push(metadata_scan_file_input(
                    workspace_id,
                    root_id,
                    scan_id,
                    entry,
                )?);
            }
        }
        Err(error) => {
            let issue = scan_issue_for_platform_error(relative_path, &error);
            if issue.is_error {
                progress.errors = progress.errors.saturating_add(1);
            }
            progress.skipped_items = progress.skipped_items.saturating_add(1);
            issues.push(issue);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finalize_consumer_scan(
    database: &Database,
    workspace_id: WorkspaceId,
    scan_id: ScanId,
    files: &mut Vec<ScanFileInput>,
    issues: &mut Vec<ScanIssueInput>,
    mut persisted_files: u64,
    mut issue_count: u64,
    mut progress: ScanProgress,
    cancelled: bool,
    on_progress: &mut dyn FnMut(ScanProgress),
) -> Result<ScanRecord, ApplicationError> {
    let (file_count, issues_written) =
        flush_consumer_batch(database, workspace_id, scan_id, files, issues)?;
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

    let scan = database
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

#[allow(clippy::too_many_arguments)]
fn flush_if_needed(
    database: &Database,
    workspace_id: WorkspaceId,
    scan_id: ScanId,
    files: &mut Vec<ScanFileInput>,
    issues: &mut Vec<ScanIssueInput>,
    persisted_files: &mut u64,
    issue_count: &mut u64,
) -> Result<(), ApplicationError> {
    if files.len() + issues.len() < CONSUMER_SCAN_BATCH_SIZE {
        return Ok(());
    }
    let (file_count, issues_written) =
        flush_consumer_batch(database, workspace_id, scan_id, files, issues)?;
    *persisted_files = (*persisted_files).saturating_add(file_count);
    *issue_count = (*issue_count).saturating_add(issues_written);
    Ok(())
}

fn flush_consumer_batch(
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

fn metadata_scan_file_input(
    workspace_id: WorkspaceId,
    root_id: domain::RootId,
    scan_id: ScanId,
    entry: ReadOnlyEntry,
) -> Result<ScanFileInput, ApplicationError> {
    let label = entry
        .absolute_path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fichier".to_owned());
    let display_label =
        DisplayLabel::new(label).map_err(|_| ApplicationError::InvalidWorkspaceName)?;
    let extension = entry
        .absolute_path
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase());
    let observation = FileObservation {
        file_id: FileId::new(),
        version_id: FileVersionId::new(),
        workspace_id,
        root_id,
        scan_id,
        relative_path: entry.relative_path,
        display_label,
        kind: FileKind::Regular,
        detected_mime: metadata_mime_from_extension(&entry.absolute_path),
        fingerprint: FileFingerprint {
            native_identity: entry.identity,
            byte_size: entry.byte_size,
            modified_at_ns: entry.modified_at_ns,
            created_at_ns: entry.created_at_ns,
            attributes: entry.attributes,
            quick_digest: None,
            content_digest: None,
        },
        read_only: entry.read_only,
        hidden: entry.hidden,
        cloud_placeholder: entry.cloud_placeholder,
        encrypted: entry.encrypted,
    };
    Ok(ScanFileInput {
        observation,
        extension,
        accessed_at_ns: entry.accessed_at_ns,
        readability_status: "not_checked".to_owned(),
        scan_status: "indexed".to_owned(),
        hashing_status: "not_candidate".to_owned(),
        error_code: None,
    })
}

#[cfg(target_os = "macos")]
fn inspect_consumer_metadata(
    _platform: &dyn ReadOnlyPlatform,
    root: &Path,
    relative_path: &Path,
    volume: &VolumeIdentity,
) -> Result<ReadOnlyEntry, PlatformError> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PlatformError::OutsideRoot);
    }

    let root_metadata = fs::symlink_metadata(root).map_err(metadata_io_error)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(PlatformError::ReparsePoint);
    }

    let target = root.join(relative_path);
    let metadata = fs::symlink_metadata(&target).map_err(metadata_io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(PlatformError::ReparsePoint);
    }
    if !metadata.is_file() {
        return Err(PlatformError::Unsupported(
            "only regular files are analyzable".to_owned(),
        ));
    }

    let parent = target.parent().ok_or(PlatformError::OutsideRoot)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(metadata_io_error)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(PlatformError::ReparsePoint);
    }

    let leaf_bytes = target
        .file_name()
        .ok_or(PlatformError::OutsideRoot)?
        .as_bytes()
        .to_vec();
    let hidden = leaf_bytes.first() == Some(&b'.');

    Ok(ReadOnlyEntry {
        absolute_path: target,
        relative_path: NativePath {
            encoding: PathEncoding::UnixBytes,
            bytes: relative_path.as_os_str().as_bytes().to_vec(),
        },
        identity: NativeFileIdentity {
            volume: volume.clone(),
            object_key: metadata.ino().to_le_bytes().to_vec(),
            parent_key: parent_metadata.ino().to_le_bytes().to_vec(),
            leaf_name: NativePath {
                encoding: PathEncoding::UnixBytes,
                bytes: leaf_bytes,
            },
            link_count: u32::try_from(metadata.nlink()).unwrap_or(u32::MAX),
            reparse_tag: None,
        },
        byte_size: metadata.len(),
        modified_at_ns: metadata_time_ns(metadata.modified()),
        created_at_ns: metadata_time_ns(metadata.created()),
        accessed_at_ns: metadata_time_ns(metadata.accessed()),
        attributes: u64::from(metadata.mode()),
        read_only: metadata.permissions().readonly(),
        hidden,
        cloud_placeholder: false,
        encrypted: false,
    })
}

#[cfg(not(target_os = "macos"))]
fn inspect_consumer_metadata(
    platform: &dyn ReadOnlyPlatform,
    root: &Path,
    relative_path: &Path,
    _volume: &VolumeIdentity,
) -> Result<ReadOnlyEntry, PlatformError> {
    platform.inspect_regular_file(root, relative_path)
}

#[cfg(target_os = "macos")]
fn metadata_io_error(error: std::io::Error) -> PlatformError {
    match error.kind() {
        std::io::ErrorKind::NotFound => PlatformError::SourceMissing,
        std::io::ErrorKind::PermissionDenied => PlatformError::PermissionDenied,
        _ => PlatformError::Io(error),
    }
}

fn metadata_time_ns(value: std::io::Result<SystemTime>) -> Option<i128> {
    let duration = value.ok()?.duration_since(UNIX_EPOCH).ok()?;
    i128::try_from(duration.as_nanos()).ok()
}

fn display_relative_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn should_skip_deep_index_directory(name: &OsStr) -> bool {
    let normalized = name.to_string_lossy().to_ascii_lowercase();
    DEEP_INDEX_EXCLUDED_DIRECTORY_NAMES
        .iter()
        .any(|excluded| normalized == *excluded)
}

fn scan_issue_for_platform_error(relative_path: &Path, error: &PlatformError) -> ScanIssueInput {
    let (code, is_error, skipped) = match error {
        PlatformError::ReparsePoint
        | PlatformError::PathPolicyRefusal
        | PlatformError::OutsideRoot => ("reparse_point", false, true),
        PlatformError::CloudPlaceholder => ("cloud_placeholder", false, true),
        PlatformError::PermissionDenied => ("permission_denied", true, true),
        PlatformError::SourceMissing => ("source_missing", false, true),
        PlatformError::SharingViolation | PlatformError::LockViolation => ("locked", false, true),
        PlatformError::Cancelled => ("cancelled", false, true),
        PlatformError::Unsupported(_) | PlatformError::VerificationLimitExceeded { .. } => {
            ("unsupported", false, true)
        }
        PlatformError::Io(_)
        | PlatformError::Precondition(_)
        | PlatformError::DestinationExists
        | PlatformError::DiskFull
        | PlatformError::AmbiguousMutationOutcome
        | PlatformError::SecretStore(_) => ("io", true, true),
    };
    ScanIssueInput {
        relative_path: relative_path.to_string_lossy().into_owned(),
        code: code.to_owned(),
        message: error.to_string(),
        is_directory: false,
        is_error,
        skipped,
    }
}

fn metadata_mime_from_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let mime = match extension.as_str() {
        "txt" | "log" | "md" | "rtf" | "ini" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" | "jsx" | "py" | "java" | "rs" | "go" | "swift" | "sql" | "c"
        | "h" | "cpp" | "hpp" | "cs" | "sh" | "ps1" => "text/plain",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "odt" => "application/vnd.oasis.opendocument.text",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" | "heif" => "image/heic",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        _ => return None,
    };
    Some(mime.to_owned())
}

fn same_registered_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn is_standard_personal_root(path: &Path) -> bool {
    let Some(name) = path
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
    else {
        return false;
    };
    matches!(
        name.as_str(),
        "desktop"
            | "bureau"
            | "documents"
            | "downloads"
            | "téléchargements"
            | "telechargements"
            | "pictures"
            | "images"
            | "movies"
            | "videos"
            | "vidéos"
            | "music"
            | "musique"
    )
}

#[inline(never)]
fn default_content_engine() -> Arc<dyn ContentExtractionEngine> {
    Arc::new(LocalExtractionEngine::local_default())
}

#[inline(never)]
fn default_semantic_provider() -> Arc<dyn SemanticProvider> {
    Arc::new(DeterministicSemanticProvider::default())
}

#[inline(never)]
fn production_embedding_provider(model_root: Option<PathBuf>) -> Arc<dyn LocalEmbeddingProvider> {
    match model_root {
        Some(root) => match OnnxLocalEmbeddingProvider::new(root) {
            Ok(provider) => Arc::new(provider),
            Err(_) => Arc::new(UnavailableEmbeddingProvider),
        },
        None => Arc::new(UnavailableEmbeddingProvider),
    }
}

const fn readability_status(status: ReadabilityStatus) -> &'static str {
    match status {
        ReadabilityStatus::Readable => "readable",
        ReadabilityStatus::Unreadable => "unreadable",
        ReadabilityStatus::NotChecked => "not_checked",
    }
}

const fn scan_item_status(status: ScanItemStatus) -> &'static str {
    match status {
        ScanItemStatus::Indexed => "indexed",
        ScanItemStatus::IndexedWithErrors => "indexed_with_errors",
    }
}

const fn hashing_status(status: HashingStatus) -> &'static str {
    match status {
        HashingStatus::NotCandidate => "not_candidate",
        HashingStatus::Hashed => "hashed",
        HashingStatus::Failed => "failed",
        HashingStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_extended_consumer_mime_types() {
        assert_eq!(
            metadata_mime_from_extension(Path::new("report.odt")).as_deref(),
            Some("application/vnd.oasis.opendocument.text")
        );
        assert_eq!(
            metadata_mime_from_extension(Path::new("clip.webm")).as_deref(),
            Some("video/webm")
        );
        assert_eq!(
            metadata_mime_from_extension(Path::new("photo.tiff")).as_deref(),
            Some("image/tiff")
        );
    }

    #[test]
    fn excludes_generated_dependency_directories_case_insensitively() {
        assert!(should_skip_deep_index_directory(OsStr::new("node_modules")));
        assert!(should_skip_deep_index_directory(OsStr::new("NODE_MODULES")));
        assert!(should_skip_deep_index_directory(OsStr::new("__pycache__")));
        assert!(!should_skip_deep_index_directory(OsStr::new("Clients")));
    }

    #[test]
    fn music_is_a_standard_personal_root() {
        assert!(is_standard_personal_root(Path::new("Music")));
        assert!(is_standard_personal_root(Path::new("Musique")));
    }
}
