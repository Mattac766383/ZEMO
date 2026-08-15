use crate::{ApplicationError, monitoring::MonitoringRuntime};
use catalog::{
    CatalogScanner, HashingStatus, ReadabilityStatus, ScanItemStatus, ScanPhase, ScanPolicy,
    ScanProgress,
};
use domain::{DisplayLabel, ScanId, WorkspaceId};
use extraction::{ContentExtractionEngine, LocalExtractionEngine};
use knowledge::{DeterministicSemanticProvider, SemanticProvider};
use persistence::{
    Database, DuplicateGroupInput, DuplicateGroupRecord, InventorySort, RootRecord,
    ScanCompletionInput, ScanFileInput, ScanFileRecord, ScanIssueInput, ScanIssueRecord,
    ScanRecord, WorkspaceRecord,
};
use platform::ReadOnlyPlatform;
use search::{
    AnnIndexMeta, LocalEmbeddingProvider, OnnxLocalEmbeddingProvider, PersistentAnnIndex,
    UnavailableEmbeddingProvider,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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

    /// Production constructor. Uses a real ONNX local embedding provider when
    /// `model_root` is set; otherwise embeddings stay unavailable (lexical fallback).
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
        self.register_root_for_monitoring(&root)?;
        Ok(root)
    }

    #[inline(never)]
    pub fn scan_workspace(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> Result<ScanRecord, ApplicationError> {
        let root = self.database.active_root(workspace_id)?;
        let scan_id = ScanId::new();
        self.database.begin_scan(workspace_id, root.id, scan_id)?;
        let output = match self.scanner.scan_with_id_and_control(
            scan_id,
            workspace_id,
            root.id,
            &root.absolute_path_native,
            ScanPolicy::default(),
            is_cancelled,
            on_progress,
        ) {
            Ok(output) => output,
            Err(error) => {
                let _ = self.database.fail_scan(scan_id, "catalog_failed");
                return Err(ApplicationError::Catalog(error));
            }
        };

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
