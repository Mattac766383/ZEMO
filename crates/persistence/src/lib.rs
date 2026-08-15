//! SQLCipher-backed persistence with a single serialized writer.

mod ann_chunks;
mod execution;
mod hybrid_search;
mod identity;
mod monitoring;
mod proposal;
mod records;
mod rules;
mod scale_fixture;

pub use ann_chunks::{
    AnnFileCandidate, AnnRebuildVector, AnnUpsertRecord, FileChunkReplaceResult,
    FileChunkReplacement,
};
pub use hybrid_search::FileEmbeddingReplacement;
pub use records::*;
pub use scale_fixture::{
    LargeScaleFixture, LargeScaleFixtureConfig, LargeScaleFixtureStats, M13_PROVIDER_ID,
    M13_PROVIDER_VERSION, database_file_size, open_scale_database,
};

use domain::{ArtifactId, FileObservation, NativePath, PathEncoding, RootId, ScanId, WorkspaceId};
use knowledge::{
    ConfidencePolicy, FieldStatus, SemanticAnalysis, SemanticEvidence, SemanticFieldType,
    SemanticReviewReason, SemanticValue,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, named_params, params};
use search::{
    EmbeddingSearchStatus, MatchSource, ModifiedFilter, SearchPage, SearchQuery, SearchResult,
    SearchSort, SearchTimings, safe_fts_query as safe_local_fts_query,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const INITIAL_MIGRATION: &str = include_str!("../../../migrations/0001_initial.sql");
const SAFE_SCANNER_MIGRATION: &str = include_str!("../../../migrations/0002_safe_scanner.sql");
const SAFE_EXTRACTION_MIGRATION: &str =
    include_str!("../../../migrations/0003_safe_content_extraction.sql");
const LOCAL_SEARCH_REVIEW_MIGRATION: &str =
    include_str!("../../../migrations/0004_local_search_review.sql");
const LOCAL_SEMANTIC_MIGRATION: &str =
    include_str!("../../../migrations/0005_local_semantic_understanding.sql");
const LOCAL_RELATIONSHIPS_MIGRATION: &str =
    include_str!("../../../migrations/0006_local_cross_file_relationships.sql");
const LOCAL_ORGANIZATION_MIGRATION: &str =
    include_str!("../../../migrations/0007_local_organization_proposals.sql");
const SAFETY_GATED_EXECUTION_MIGRATION: &str =
    include_str!("../../../migrations/0008_safety_gated_filesystem_application.sql");
const HYBRID_SEMANTIC_SEARCH_MIGRATION: &str =
    include_str!("../../../migrations/0012_hybrid_semantic_search.sql");
const CONTINUOUS_MONITORING_MIGRATION: &str =
    include_str!("../../../migrations/0009_continuous_monitoring.sql");
const LOCAL_RULES_LEARNING_MIGRATION: &str =
    include_str!("../../../migrations/0010_local_rules_learning.sql");
const EXECUTION_CONSENT_MIGRATION: &str =
    include_str!("../../../migrations/0011_execution_consent_attestation.sql");
const MONITORING_CORRECTNESS_MIGRATION: &str =
    include_str!("../../../migrations/0013_monitoring_correctness_hardening.sql");
const EXECUTION_SAFETY_POLICY_V2_MIGRATION: &str =
    include_str!("../../../migrations/0014_execution_safety_policy_v2.sql");
const CROSS_PROCESS_RECOVERY_MIGRATION: &str =
    include_str!("../../../migrations/0015_cross_process_recovery_hardening.sql");
const LOCAL_ANN_SEMANTIC_INDEX_MIGRATION: &str =
    include_str!("../../../migrations/0016_local_ann_semantic_index.sql");
const INCREMENTAL_ORGANIZATION_PROPOSALS_MIGRATION: &str =
    include_str!("../../../migrations/0017_incremental_organization_proposals.sql");
const LOCAL_PRINCIPAL_ID: &str = "01900000-0000-7000-8000-000000000001";
const BUILTIN_PROCESSOR_ID: &str = "01900000-0000-7000-8000-000000000002";

#[derive(Debug)]
pub struct DatabaseKey([u8; 32]);

impl DatabaseKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn generate() -> Self {
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let mut bytes = [0_u8; 32];
        bytes[..16].copy_from_slice(first.as_bytes());
        bytes[16..].copy_from_slice(second.as_bytes());
        Self(bytes)
    }

    #[must_use]
    pub fn expose_for_secret_store(&self) -> [u8; 32] {
        self.0
    }
}

impl Drop for DatabaseKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub struct Database {
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Database")
            .field("connection", &"<encrypted single-writer connection>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("database failure: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("database writer mutex was poisoned")]
    WriterPoisoned,
    #[error("database schema version {0} is newer than this application")]
    UnsupportedSchema(i64),
    #[error("database key is invalid or SQLCipher is unavailable")]
    InvalidCipher,
    #[error("requested record does not exist")]
    NotFound,
    #[error("stored identifier is invalid: {0}")]
    InvalidIdentifier(#[from] uuid::Error),
    #[error("filesystem value cannot be represented safely")]
    InvalidNativePath,
    #[error("numeric value exceeds SQLite range")]
    NumericOverflow,
    #[error("semantic output failed bounded schema validation")]
    InvalidSemanticOutput,
    #[error("identity resolution input failed bounded validation")]
    InvalidIdentityInput,
    #[error("identity operation conflicts with confirmed or current state")]
    IdentityConflict,
    #[error("organization proposal failed bounded schema validation")]
    InvalidProposal,
    #[error("filesystem execution state failed bounded or integrity validation")]
    InvalidExecution,
    #[error("local rule or learning record failed bounded validation")]
    InvalidRule,
    #[error("local rule operation conflicts with current ordered state")]
    RuleConflict,
    #[error("monitoring state failed bounded or integrity validation")]
    InvalidMonitoringInput,
}

impl Database {
    pub fn open(path: &Path, key: &DatabaseKey) -> Result<Self, PersistenceError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        Self::initialize(connection, key)
    }

    pub fn open_in_memory(key: &DatabaseKey) -> Result<Self, PersistenceError> {
        Self::initialize(Connection::open_in_memory()?, key)
    }

    #[inline(never)]
    fn initialize(connection: Connection, key: &DatabaseKey) -> Result<Self, PersistenceError> {
        apply_cipher_key(&connection, key)?;
        apply_runtime_pragmas(&connection)?;
        let schema_version = read_user_schema_version(&connection)?;
        apply_schema_migrations(&connection, schema_version)?;
        repair_legacy_native_path_storage(&connection)?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;

        let database = Self {
            connection: Mutex::new(connection),
        };
        database.ensure_builtin_records()?;
        Ok(database)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, PersistenceError> {
        self.connection
            .lock()
            .map_err(|_| PersistenceError::WriterPoisoned)
    }

    fn ensure_builtin_records(&self) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT OR IGNORE INTO principals(id, kind, display_name)
             VALUES (?1, 'human', 'Utilisateur local')",
            [LOCAL_PRINCIPAL_ID],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO processors(
                id, name, kind, version, entrypoint, deterministic, capabilities_json
             ) VALUES (?1, 'extracteur local intégré', 'parser', '1', 'builtin', 1, '{\"network\":false}')",
            [BUILTIN_PROCESSOR_ID],
        )?;
        Ok(())
    }

    pub fn create_workspace(&self, name: &str) -> Result<WorkspaceRecord, PersistenceError> {
        let id = WorkspaceId::new();
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO workspaces(id, name, owner_principal_id) VALUES (?1, ?2, ?3)",
            params![id.to_string(), name, LOCAL_PRINCIPAL_ID],
        )?;
        let created_at = connection.query_row(
            "SELECT created_at FROM workspaces WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )?;
        Ok(WorkspaceRecord {
            id,
            name: name.to_owned(),
            created_at,
        })
    }

    pub fn workspace(&self, id: WorkspaceId) -> Result<WorkspaceRecord, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT name, created_at FROM workspaces WHERE id = ?1 AND archived_at IS NULL",
                [id.to_string()],
                |row| {
                    Ok(WorkspaceRecord {
                        id,
                        name: row.get(0)?,
                        created_at: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)
    }

    pub fn register_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        absolute_path: &Path,
        display_label: &str,
        volume: &domain::VolumeIdentity,
    ) -> Result<RootRecord, PersistenceError> {
        let absolute_path_native = monitoring::encode_native_path(absolute_path)?;
        let normalized_path_buf =
            monitoring::monitoring_path_key(absolute_path, volume.case_sensitive);
        let normalized_path_native = monitoring::encode_native_path(&normalized_path_buf)?;
        let absolute = safe_native_path_display(absolute_path, &absolute_path_native);
        let normalized = normalized_path_buf.to_str().map_or_else(
            || safe_native_path_display(&normalized_path_buf, &normalized_path_native),
            |value| value.replace('\\', "/").trim_start_matches('/').to_owned(),
        );
        let volume_id = Uuid::now_v7().to_string();
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO volumes(
                id, workspace_id, platform, stable_identifier, display_name,
                filesystem_type, case_sensitive, removable
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                volume_id,
                workspace_id.to_string(),
                platform_name(volume.platform),
                volume.stable_identifier,
                display_label,
                volume.filesystem_type,
                i64::from(volume.case_sensitive),
                i64::from(volume.removable),
            ],
        )?;
        let effective_volume_id: String = transaction.query_row(
            "SELECT id FROM volumes WHERE workspace_id = ?1 AND stable_identifier = ?2",
            params![workspace_id.to_string(), volume.stable_identifier],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO roots(
                id, workspace_id, volume_id, added_by_principal_id,
                absolute_path, normalized_path, display_name,
                absolute_path_native, normalized_path_native
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                effective_volume_id,
                LOCAL_PRINCIPAL_ID,
                absolute,
                normalized,
                display_label,
                absolute_path_native,
                normalized_path_native,
            ],
        )?;
        transaction.commit()?;
        Ok(RootRecord {
            id: root_id,
            workspace_id,
            display_label: display_label.to_owned(),
            absolute_path: absolute,
            absolute_path_native: absolute_path.to_path_buf(),
        })
    }

    pub fn active_root(&self, workspace_id: WorkspaceId) -> Result<RootRecord, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id, display_name, absolute_path, absolute_path_native
                 FROM roots
                 WHERE workspace_id = ?1 AND state = 'active'
                 ORDER BY created_at DESC LIMIT 1",
                [workspace_id.to_string()],
                |row| {
                    let id: String = row.get(0)?;
                    Ok((
                        id,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(
                    id,
                    display_label,
                    absolute_path,
                    absolute_path_native,
                )| -> Result<RootRecord, PersistenceError> {
                    Ok(RootRecord {
                        id: id.parse()?,
                        workspace_id,
                        display_label,
                        absolute_path,
                        absolute_path_native: monitoring::decode_native_path(
                            &absolute_path_native,
                        )?,
                    })
                },
            )
            .transpose()?
            .ok_or(PersistenceError::NotFound)
    }

    pub fn persist_scan(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        scan_id: ScanId,
        observations: &[FileObservation],
        issue_count: usize,
    ) -> Result<ScanRecord, PersistenceError> {
        self.persist_scan_detailed(workspace_id, root_id, scan_id, observations, issue_count)
            .map(|result| result.scan)
    }

    pub fn begin_scan(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        scan_id: ScanId,
    ) -> Result<(), PersistenceError> {
        self.begin_scan_with_kind(workspace_id, root_id, scan_id, ScanKind::Initial)
    }

    pub fn fail_scan(&self, scan_id: ScanId, code: &str) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        connection.execute(
            "UPDATE scans
             SET status = 'failed',
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error_text = ?2
             WHERE id = ?1 AND status = 'running'",
            params![scan_id.to_string(), code],
        )?;
        Ok(())
    }

    pub fn persist_scan_detailed(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        scan_id: ScanId,
        observations: &[FileObservation],
        issue_count: usize,
    ) -> Result<PersistedScan, PersistenceError> {
        self.begin_scan(workspace_id, root_id, scan_id)?;
        let files = observations
            .iter()
            .cloned()
            .map(|observation| ScanFileInput {
                observation,
                extension: None,
                accessed_at_ns: None,
                readability_status: "not_checked".to_owned(),
                scan_status: "indexed".to_owned(),
                hashing_status: "not_candidate".to_owned(),
                error_code: None,
            })
            .collect::<Vec<_>>();
        let issues = (0..issue_count)
            .map(|_| ScanIssueInput {
                relative_path: String::new(),
                code: "legacy_issue".to_owned(),
                message: "scanner issue".to_owned(),
                is_directory: false,
                is_error: false,
                skipped: true,
            })
            .collect();
        self.complete_scan(&ScanCompletionInput {
            scan_id,
            workspace_id,
            root_id,
            status: "completed".to_owned(),
            files_discovered: u64::try_from(observations.len()).unwrap_or(u64::MAX),
            directories_discovered: 0,
            bytes_discovered: observations
                .iter()
                .map(|observation| observation.fingerprint.byte_size)
                .fold(0_u64, u64::saturating_add),
            files_hashed: u64::try_from(
                observations
                    .iter()
                    .filter(|observation| observation.fingerprint.content_digest.is_some())
                    .count(),
            )
            .unwrap_or(u64::MAX),
            errors: 0,
            skipped_items: u64::try_from(issue_count).unwrap_or(u64::MAX),
            truncated: false,
            files,
            issues,
            duplicate_groups: Vec::new(),
        })
    }

    pub fn complete_scan(
        &self,
        input: &ScanCompletionInput,
    ) -> Result<PersistedScan, PersistenceError> {
        let database_status = if input.status == "cancelled" {
            "cancelled"
        } else {
            "completed"
        };
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let mut persisted_files = Vec::with_capacity(input.files.len());
        for file in &input.files {
            let persisted =
                persist_observation(&transaction, &file.observation, file.accessed_at_ns)?;
            transaction.execute(
                "INSERT INTO scan_file_statuses(
                    scan_id, file_version_id, extension, readability_status,
                    scan_status, hashing_status, error_code
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    input.scan_id.to_string(),
                    persisted.file_version_id,
                    file.extension,
                    file.readability_status,
                    file.scan_status,
                    file.hashing_status,
                    file.error_code,
                ],
            )?;
            upsert_scan_search_document(
                &transaction,
                input.workspace_id,
                &persisted,
                file.extension.as_deref(),
            )?;
            synchronize_scanner_review(
                &transaction,
                input.workspace_id,
                &persisted,
                &file.readability_status,
                file.error_code.as_deref(),
            )?;
            persisted_files.push(persisted);
        }

        for issue in &input.issues {
            transaction.execute(
                "INSERT INTO scan_issues(
                    id, scan_id, relative_path, code, severity, message, details_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    Uuid::now_v7().to_string(),
                    input.scan_id.to_string(),
                    issue.relative_path,
                    issue.code,
                    if issue.is_error { "error" } else { "warning" },
                    issue.message,
                    serde_json::json!({
                        "isDirectory": issue.is_directory,
                        "skipped": issue.skipped,
                    })
                    .to_string(),
                ],
            )?;
        }

        persist_duplicate_groups(&transaction, input, &persisted_files)?;
        transaction.execute(
            "INSERT INTO scan_metrics(
                scan_id, files_indexed, directories_discovered, bytes_discovered,
                files_hashed, error_count, skipped_count, duplicate_group_count, truncated
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                input.scan_id.to_string(),
                to_sql_integer(input.files.len())?,
                to_sql_u64(input.directories_discovered)?,
                to_sql_u64(input.bytes_discovered)?,
                to_sql_u64(input.files_hashed)?,
                to_sql_u64(input.errors)?,
                to_sql_u64(input.skipped_items)?,
                to_sql_integer(input.duplicate_groups.len())?,
                i64::from(input.truncated),
            ],
        )?;
        transaction.execute(
            "UPDATE scans
             SET status = ?2,
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 discovered_count = ?3,
                 changed_count = ?4,
                 issue_count = ?5
             WHERE id = ?1 AND status = 'running'",
            params![
                input.scan_id.to_string(),
                database_status,
                to_sql_u64(input.files_discovered)?,
                to_sql_integer(input.files.len())?,
                to_sql_integer(input.issues.len())?,
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        Ok(PersistedScan {
            scan: self.scan(input.scan_id)?,
            files: persisted_files,
        })
    }

    pub fn scan(&self, scan_id: ScanId) -> Result<ScanRecord, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT s.workspace_id, s.root_id, s.status, s.started_at,
                        s.discovered_count, COALESCE(sm.files_indexed, 0),
                        COALESCE(sm.directories_discovered, 0),
                        COALESCE(sm.bytes_discovered, 0),
                        COALESCE(sm.files_hashed, 0),
                        COALESCE(sm.error_count, 0),
                        COALESCE(sm.skipped_count, 0),
                        COALESCE(sm.duplicate_group_count, 0),
                        s.issue_count, COALESCE(sm.truncated, 0), s.completed_at
                 FROM scans s
                 LEFT JOIN scan_metrics sm ON sm.scan_id = s.id
                 WHERE s.id = ?1",
                [scan_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, i64>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, Option<String>>(14)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(
                    workspace_id,
                    root_id,
                    status,
                    started_at,
                    discovered_count,
                    indexed_count,
                    directory_count,
                    byte_count,
                    hashed_count,
                    error_count,
                    skipped_count,
                    duplicate_group_count,
                    issue_count,
                    truncated,
                    completed_at,
                )|
                 -> Result<ScanRecord, PersistenceError> {
                    let status = if status == "completed" && error_count > 0 {
                        "completed_with_errors".to_owned()
                    } else {
                        status
                    };
                    Ok(ScanRecord {
                        id: scan_id,
                        workspace_id: workspace_id.parse()?,
                        root_id: root_id.parse()?,
                        status,
                        started_at,
                        discovered_count: from_sql_u64(discovered_count)?,
                        indexed_count: from_sql_u64(indexed_count)?,
                        directory_count: from_sql_u64(directory_count)?,
                        byte_count: from_sql_u64(byte_count)?,
                        hashed_count: from_sql_u64(hashed_count)?,
                        error_count: from_sql_u64(error_count)?,
                        skipped_count: from_sql_u64(skipped_count)?,
                        duplicate_group_count: from_sql_u64(duplicate_group_count)?,
                        issue_count: from_sql_u64(issue_count)?,
                        truncated: truncated != 0,
                        completed_at,
                    })
                },
            )
            .transpose()?
            .ok_or(PersistenceError::NotFound)
    }

    pub fn scan_files(
        &self,
        scan_id: ScanId,
        sort: InventorySort,
        descending: bool,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ScanFileRecord>, PersistenceError> {
        let order_column = match sort {
            InventorySort::Filename => "fl.basename",
            InventorySort::FileType => "COALESCE(c.media_type, sfs.extension, '')",
            InventorySort::Size => "fv.byte_size",
            InventorySort::Modified => "COALESCE(fv.modified_at, '')",
            InventorySort::RelativePath => "fl.normalized_relative_path",
            InventorySort::Status => "sfs.scan_status",
        };
        let direction = if descending { "DESC" } else { "ASC" };
        let query = format!(
            "SELECT fv.id, fl.basename, c.media_type, sfs.extension, fv.byte_size,
                    fv.modified_at, fl.relative_path, sfs.scan_status,
                    sfs.hashing_status, sfs.readability_status
             FROM scan_file_statuses sfs
             JOIN file_versions fv ON fv.id = sfs.file_version_id
             JOIN file_locations fl ON fl.id = fv.location_id
             LEFT JOIN contents c ON c.id = fv.content_id
             WHERE sfs.scan_id = ?1
             ORDER BY {order_column} {direction}, fl.normalized_relative_path ASC
             LIMIT ?2 OFFSET ?3"
        );
        let connection = self.lock()?;
        let mut statement = connection.prepare(&query)?;
        let rows = statement.query_map(
            params![
                scan_id.to_string(),
                to_sql_integer(limit.min(1_000))?,
                to_sql_integer(offset)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )?;
        let mut files = Vec::new();
        for row in rows {
            let (
                id,
                filename,
                file_type,
                extension,
                byte_size,
                modified_at,
                relative_path,
                status,
                hashing_status,
                readability_status,
            ) = row?;
            files.push(ScanFileRecord {
                id,
                filename,
                file_type,
                extension,
                byte_size: from_sql_u64(byte_size)?,
                modified_at,
                relative_path,
                status,
                hashing_status,
                readable: readability_status == "readable",
            });
        }
        Ok(files)
    }

    pub fn scan_issues(&self, scan_id: ScanId) -> Result<Vec<ScanIssueRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT COALESCE(relative_path, ''), code, message,
                    COALESCE(json_extract(details_json, '$.isDirectory'), 0)
             FROM scan_issues
             WHERE scan_id = ?1
             ORDER BY severity DESC, relative_path, occurred_at",
        )?;
        let rows = statement.query_map([scan_id.to_string()], |row| {
            Ok(ScanIssueRecord {
                relative_path: row.get(0)?,
                category: row.get(1)?,
                message: row.get(2)?,
                is_directory: row.get::<_, i64>(3)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn scan_duplicate_groups(
        &self,
        scan_id: ScanId,
    ) -> Result<Vec<DuplicateGroupRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut group_statement = connection.prepare(
            "SELECT dg.id, dg.group_key, c.byte_size
             FROM scan_duplicate_groups sdg
             JOIN duplicate_groups dg ON dg.id = sdg.duplicate_group_id
             JOIN contents c ON c.id = dg.canonical_content_id
             WHERE sdg.scan_id = ?1 AND dg.method = 'exact_digest'
             ORDER BY c.byte_size DESC, dg.id",
        )?;
        let group_rows = group_statement.query_map([scan_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut groups = Vec::new();
        for group_row in group_rows {
            let (group_id, digest, byte_size) = group_row?;
            let mut member_statement = connection.prepare(
                "SELECT fv.id, fl.basename, fl.relative_path
                 FROM duplicate_group_members dgm
                 JOIN file_versions fv ON fv.id = dgm.file_version_id
                 JOIN file_locations fl ON fl.id = fv.location_id
                 WHERE dgm.duplicate_group_id = ?1
                   AND fv.observed_by_scan_id = ?2
                 ORDER BY fl.normalized_relative_path",
            )?;
            let members = member_statement
                .query_map(params![group_id, scan_id.to_string()], |row| {
                    Ok(DuplicateFileRecord {
                        id: row.get(0)?,
                        filename: row.get(1)?,
                        relative_path: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            groups.push(DuplicateGroupRecord {
                digest_hex: bytes_to_hex(&digest),
                byte_size: from_sql_u64(byte_size)?,
                files: members,
            });
        }
        Ok(groups)
    }

    pub fn begin_extraction_batch(
        &self,
        scan_id: ScanId,
    ) -> Result<ExtractionBatchRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (workspace_id, scan_status): (String, String) = transaction
            .query_row(
                "SELECT workspace_id, status FROM scans WHERE id = ?1",
                [scan_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        if scan_status != "completed" {
            return Err(PersistenceError::NotFound);
        }
        let queued: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM scan_file_statuses WHERE scan_id = ?1",
            [scan_id.to_string()],
            |row| row.get(0),
        )?;
        let batch_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO content_extraction_batches(
                id, workspace_id, scan_id, status, files_queued, started_at
             ) VALUES (
                ?1, ?2, ?3, 'running', ?4,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )",
            params![batch_id, workspace_id, scan_id.to_string(), queued],
        )?;
        transaction.execute(
            "INSERT INTO content_extraction_results(
                id, batch_id, scan_id, file_id, file_version_id, status,
                extension, detected_content_type
             )
             SELECT ?1 || ':' || fv.id, ?1, ?2, fv.file_id, fv.id, 'pending',
                    sfs.extension, c.media_type
             FROM scan_file_statuses sfs
             JOIN file_versions fv ON fv.id = sfs.file_version_id
             LEFT JOIN contents c ON c.id = fv.content_id
             WHERE sfs.scan_id = ?2",
            params![batch_id, scan_id.to_string()],
        )?;
        transaction.commit()?;
        drop(connection);
        self.extraction_batch(&batch_id)
    }

    pub fn extraction_candidates(
        &self,
        scan_id: ScanId,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExtractionCandidate>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT fv.file_id, fv.id, r.absolute_path, fl.relative_path, fl.basename,
                    sfs.extension, c.media_type, fv.byte_size, sfs.readability_status
             FROM scan_file_statuses sfs
             JOIN file_versions fv ON fv.id = sfs.file_version_id
             JOIN file_locations fl ON fl.id = fv.location_id
             JOIN roots r ON r.id = fl.root_id
             LEFT JOIN contents c ON c.id = fv.content_id
             WHERE sfs.scan_id = ?1
             ORDER BY fl.normalized_relative_path
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(
            params![
                scan_id.to_string(),
                to_sql_integer(limit.min(256))?,
                to_sql_integer(offset)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )?;
        let mut candidates = Vec::new();
        for row in rows {
            let (
                file_id,
                file_version_id,
                root_path,
                relative_path,
                filename,
                extension,
                declared_media_type,
                byte_size,
                readability,
            ) = row?;
            candidates.push(ExtractionCandidate {
                file_id,
                file_version_id,
                root_path,
                relative_path,
                filename,
                extension,
                declared_media_type,
                byte_size: from_sql_u64(byte_size)?,
                readable: readability == "readable",
            });
        }
        Ok(candidates)
    }

    pub fn mark_extraction_running(
        &self,
        batch_id: &str,
        file_version_id: &str,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE content_extraction_results
             SET status = 'running',
                 started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             WHERE batch_id = ?1 AND file_version_id = ?2 AND status = 'pending'",
            params![batch_id, file_version_id],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(PersistenceError::NotFound)
        }
    }

    pub fn store_extraction_result(
        &self,
        batch_id: &str,
        candidate: &ExtractionCandidate,
        result: &ExtractionResultInput,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE content_extraction_results
             SET status = ?3,
                 extractor_type = ?4,
                 extractor_version = ?5,
                 detected_content_type = ?6,
                 type_mismatch = ?7,
                 extracted_text = ?8,
                 character_count = ?9,
                 page_count = ?10,
                 sheet_count = ?11,
                 slide_count = ?12,
                 image_width = ?13,
                 image_height = ?14,
                 requires_ocr = ?15,
                 ocr_used = ?16,
                 ocr_confidence = ?17,
                 language_hint = ?18,
                 extraction_duration_ms = ?19,
                 truncated = ?20,
                 structured_metadata_json = ?21,
                 error_category = ?22,
                 error_message = ?23,
                 started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 extracted_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE batch_id = ?1
               AND file_version_id = ?2
               AND status IN ('pending', 'running')",
            params![
                batch_id,
                candidate.file_version_id,
                result.status,
                result.extractor_type,
                result.extractor_version,
                result.detected_content_type,
                i64::from(result.type_mismatch),
                result.extracted_text,
                to_sql_u64(result.character_count)?,
                result.page_count.map(i64::from),
                result.sheet_count.map(i64::from),
                result.slide_count.map(i64::from),
                result.image_width.map(i64::from),
                result.image_height.map(i64::from),
                i64::from(result.requires_ocr),
                i64::from(result.ocr_used),
                result.ocr_confidence.map(f64::from),
                result.language_hint,
                to_sql_u64(result.extraction_duration_ms)?,
                i64::from(result.truncated),
                result.structured_metadata_json,
                result.error_category,
                result.error_message,
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        let extraction_result_id: String = transaction.query_row(
            "SELECT id
             FROM content_extraction_results
             WHERE batch_id = ?1 AND file_version_id = ?2",
            params![batch_id, candidate.file_version_id],
            |row| row.get(0),
        )?;
        synchronize_search_extraction(
            &transaction,
            &candidate.file_id,
            &extraction_result_id,
            result,
        )?;
        synchronize_extraction_review(&transaction, candidate, &extraction_result_id, result)?;
        let success = i64::from(result.status == "success");
        let partial = i64::from(result.status == "partial");
        let unsupported = i64::from(result.status == "unsupported");
        let skipped = i64::from(result.status == "skipped");
        let failed = i64::from(result.status == "failed");
        transaction.execute(
            "UPDATE content_extraction_batches
             SET files_completed = files_completed + 1,
                 successful_count = successful_count + ?2,
                 partial_count = partial_count + ?3,
                 unsupported_count = unsupported_count + ?4,
                 skipped_count = skipped_count + ?5,
                 failed_count = failed_count + ?6,
                 ocr_processed_count = ocr_processed_count + ?7
             WHERE id = ?1 AND status = 'running'",
            params![
                batch_id,
                success,
                partial,
                unsupported,
                skipped,
                failed,
                i64::from(result.ocr_used),
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn finish_extraction_batch(
        &self,
        batch_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<ExtractionBatchRecord, PersistenceError> {
        if !matches!(status, "completed" | "cancelled" | "failed") {
            return Err(PersistenceError::NotFound);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let pending_status = if status == "cancelled" {
            "skipped"
        } else {
            "failed"
        };
        let pending_category = if status == "cancelled" {
            "cancelled"
        } else {
            "parser_failure"
        };
        let pending_message = if status == "cancelled" {
            "content extraction was cancelled before this file started"
        } else {
            "content extraction ended before this file completed"
        };
        let remaining = transaction.execute(
            "UPDATE content_extraction_results
             SET status = ?2,
                 error_category = ?3,
                 error_message = ?4,
                 started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                 extracted_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE batch_id = ?1 AND status IN ('pending', 'running')",
            params![batch_id, pending_status, pending_category, pending_message],
        )?;
        synchronize_unfinished_extractions(&transaction, batch_id, status)?;
        let remaining = to_sql_integer(remaining)?;
        let skipped_increment = if status == "cancelled" { remaining } else { 0 };
        let failed_increment = if status == "cancelled" { 0 } else { remaining };
        let changed = transaction.execute(
            "UPDATE content_extraction_batches
             SET status = ?2,
                 files_completed = files_completed + ?3,
                 skipped_count = skipped_count + ?4,
                 failed_count = failed_count + ?5,
                 error_message = ?6,
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND status = 'running'",
            params![
                batch_id,
                status,
                remaining,
                skipped_increment,
                failed_increment,
                error_message,
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        transaction.commit()?;
        drop(connection);
        self.extraction_batch(batch_id)
    }

    pub fn extraction_batch(
        &self,
        batch_id: &str,
    ) -> Result<ExtractionBatchRecord, PersistenceError> {
        let connection = self.lock()?;
        let row = connection
            .query_row(
                "SELECT workspace_id, scan_id, status, files_queued, files_completed,
                        successful_count, partial_count, unsupported_count, skipped_count,
                        failed_count, ocr_processed_count, started_at, completed_at
                 FROM content_extraction_batches
                 WHERE id = ?1",
                [batch_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        Ok(ExtractionBatchRecord {
            id: batch_id.to_owned(),
            workspace_id: row.0.parse()?,
            scan_id: row.1.parse()?,
            status: row.2,
            files_queued: from_sql_u64(row.3)?,
            files_completed: from_sql_u64(row.4)?,
            successful_count: from_sql_u64(row.5)?,
            partial_count: from_sql_u64(row.6)?,
            unsupported_count: from_sql_u64(row.7)?,
            skipped_count: from_sql_u64(row.8)?,
            failed_count: from_sql_u64(row.9)?,
            ocr_processed_count: from_sql_u64(row.10)?,
            started_at: row.11,
            completed_at: row.12,
        })
    }

    pub fn extraction_results(
        &self,
        batch_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExtractionDetailRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT cer.file_version_id, fl.basename, fl.relative_path, cer.extension,
                    cer.status, cer.extractor_type, cer.extractor_version,
                    cer.detected_content_type, cer.type_mismatch,
                    substr(cer.extracted_text, 1, 4000), cer.character_count,
                    cer.page_count, cer.sheet_count, cer.slide_count,
                    cer.image_width, cer.image_height, cer.requires_ocr, cer.ocr_used,
                    cer.ocr_confidence, cer.language_hint, cer.extraction_duration_ms,
                    cer.truncated, cer.structured_metadata_json, cer.error_category,
                    cer.error_message, cer.extracted_at
             FROM content_extraction_results cer
             JOIN file_versions fv ON fv.id = cer.file_version_id
             JOIN file_locations fl ON fl.id = fv.location_id
             WHERE cer.batch_id = ?1
             ORDER BY fl.normalized_relative_path
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(
            params![
                batch_id,
                to_sql_integer(limit.min(500))?,
                to_sql_integer(offset)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, Option<f64>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, i64>(20)?,
                    row.get::<_, i64>(21)?,
                    row.get::<_, String>(22)?,
                    row.get::<_, Option<String>>(23)?,
                    row.get::<_, Option<String>>(24)?,
                    row.get::<_, Option<String>>(25)?,
                ))
            },
        )?;
        let mut output = Vec::new();
        for row in rows {
            let row = row?;
            output.push(ExtractionDetailRecord {
                file_version_id: row.0,
                filename: row.1,
                relative_path: row.2,
                extension: row.3,
                status: row.4,
                extractor_type: row.5,
                extractor_version: row.6,
                detected_content_type: row.7,
                type_mismatch: row.8 != 0,
                text_preview: row.9,
                character_count: from_sql_u64(row.10)?,
                page_count: optional_u32(row.11)?,
                sheet_count: optional_u32(row.12)?,
                slide_count: optional_u32(row.13)?,
                image_width: optional_u32(row.14)?,
                image_height: optional_u32(row.15)?,
                requires_ocr: row.16 != 0,
                ocr_used: row.17 != 0,
                ocr_confidence: row.18.map(|value| value as f32),
                language_hint: row.19,
                extraction_duration_ms: from_sql_u64(row.20)?,
                truncated: row.21 != 0,
                structured_metadata: serde_json::from_str(&row.22)
                    .unwrap_or_else(|_| serde_json::json!({})),
                error_category: row.23,
                error_message: row.24,
                extracted_at: row.25,
            });
        }
        Ok(output)
    }

    pub fn begin_semantic_batch(
        &self,
        scan_id: ScanId,
    ) -> Result<SemanticAnalysisBatchRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id: String = transaction
            .query_row(
                "SELECT workspace_id FROM scans WHERE id = ?1",
                [scan_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let files_queued: i64 = transaction.query_row(
            "SELECT COUNT(*)
             FROM local_search_documents d
             JOIN content_extraction_results cer ON cer.id = d.extraction_result_id
             WHERE cer.scan_id = ?1
               AND cer.status NOT IN ('pending', 'running')",
            [scan_id.to_string()],
            |row| row.get(0),
        )?;
        let batch_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO semantic_analysis_batches(
                id, workspace_id, scan_id, status, files_queued, started_at
             ) VALUES (
                ?1, ?2, ?3, 'running', ?4,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )",
            params![batch_id, workspace_id, scan_id.to_string(), files_queued,],
        )?;
        transaction.commit()?;
        drop(connection);
        self.semantic_batch(&batch_id)
    }

    pub fn semantic_candidates(
        &self,
        scan_id: ScanId,
        limit: usize,
        offset: usize,
        max_input_chars: usize,
    ) -> Result<Vec<SemanticAnalysisCandidate>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT
                d.workspace_id, cer.scan_id, d.file_id, d.file_version_id, cer.id,
                d.filename, d.extension, cer.detected_content_type, cer.status,
                substr(cer.extracted_text, 1, ?2),
                cer.extractor_type, cer.extractor_version,
                cer.page_count, cer.sheet_count, cer.slide_count,
                cer.ocr_used, cer.ocr_confidence, cer.truncated, cer.language_hint
             FROM local_search_documents d
             JOIN content_extraction_results cer ON cer.id = d.extraction_result_id
             WHERE cer.scan_id = ?1
               AND cer.status NOT IN ('pending', 'running')
             ORDER BY d.id
             LIMIT ?3 OFFSET ?4",
        )?;
        let bounded_chars = max_input_chars.min(500_000).saturating_add(1);
        let rows = statement.query_map(
            params![
                scan_id.to_string(),
                to_sql_integer(bounded_chars)?,
                to_sql_integer(limit.min(64))?,
                to_sql_integer(offset)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, Option<f64>>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, Option<String>>(18)?,
                ))
            },
        )?;
        let mut output = Vec::new();
        for row in rows {
            let row = row?;
            output.push(SemanticAnalysisCandidate {
                workspace_id: row.0.parse()?,
                scan_id: row.1.parse()?,
                file_id: row.2,
                file_version_id: row.3,
                extraction_result_id: row.4,
                filename: row.5,
                extension: row.6,
                detected_content_type: row.7,
                extraction_status: row.8,
                extracted_text: row.9,
                extractor_type: row.10,
                extractor_version: row.11,
                page_count: optional_u32(row.12)?,
                sheet_count: optional_u32(row.13)?,
                slide_count: optional_u32(row.14)?,
                ocr_used: row.15 != 0,
                ocr_confidence: row.16.map(|value| value as f32),
                extraction_truncated: row.17 != 0,
                language_hint: row.18,
            });
        }
        Ok(output)
    }

    pub fn begin_semantic_analysis(
        &self,
        batch_id: &str,
        candidate: &SemanticAnalysisCandidate,
        analysis: &SemanticAnalysis,
        input_digest: &[u8; 32],
    ) -> Result<String, PersistenceError> {
        let connection = self.lock()?;
        let analysis_id = Uuid::now_v7().to_string();
        let input_characters = to_sql_integer(analysis.input_character_count)?;
        let changed = connection.execute(
            "INSERT INTO semantic_analyses(
                id, batch_id, workspace_id, scan_id, file_id, file_version_id,
                extraction_result_id, status, schema_version, analyzer_id,
                analyzer_version, provider_id, provider_version,
                processing_location, input_digest, input_character_count,
                analyzed_character_count, input_quality, input_quality_status,
                input_quality_reasons_json, duration_ms, is_current
             )
             SELECT
                ?1, b.id, b.workspace_id, b.scan_id, ?3, ?4, ?5,
                'running', ?6, ?7, ?8, ?9, ?10, 'local', ?11, ?12,
                0, 0.0, 'unusable', '[]', 0, 0
             FROM semantic_analysis_batches b
             WHERE b.id = ?2
               AND b.status = 'running'
               AND b.workspace_id = ?13
               AND b.scan_id = ?14",
            params![
                analysis_id,
                batch_id,
                candidate.file_id,
                candidate.file_version_id,
                candidate.extraction_result_id,
                i64::from(analysis.analyzer.schema_version),
                analysis.analyzer.analyzer_id,
                analysis.analyzer.analyzer_version,
                analysis.analyzer.provider_id,
                analysis.analyzer.provider_version,
                input_digest.as_slice(),
                input_characters,
                candidate.workspace_id.to_string(),
                candidate.scan_id.to_string(),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        Ok(analysis_id)
    }

    pub fn store_semantic_analysis(
        &self,
        analysis_id: &str,
        candidate: &SemanticAnalysisCandidate,
        analysis: &SemanticAnalysis,
    ) -> Result<(), PersistenceError> {
        analysis
            .validate(knowledge::SemanticLimits::default())
            .map_err(|_| PersistenceError::InvalidSemanticOutput)?;
        if !matches!(
            analysis.status,
            knowledge::SemanticStatus::Success
                | knowledge::SemanticStatus::Partial
                | knowledge::SemanticStatus::Unknown
        ) {
            return Err(PersistenceError::InvalidSemanticOutput);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let batch_id: String = transaction
            .query_row(
                "SELECT batch_id
                 FROM semantic_analyses
                 WHERE id = ?1
                   AND file_version_id = ?2
                   AND status = 'running'",
                params![analysis_id, candidate.file_version_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;

        for field in &analysis.fields {
            insert_semantic_field(&transaction, analysis_id, field)?;
        }
        for entity in &analysis.entities {
            insert_semantic_entity(&transaction, analysis_id, entity)?;
        }

        transaction.execute(
            "UPDATE semantic_analyses
             SET is_current = 0,
                 superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_version_id = ?1
               AND is_current = 1
               AND id <> ?2",
            params![candidate.file_version_id, analysis_id],
        )?;
        let quality_reasons = serde_json::to_string(
            &analysis
                .input_quality
                .reasons
                .iter()
                .map(|reason| reason.database_name())
                .collect::<Vec<_>>(),
        )
        .map_err(|_| PersistenceError::InvalidSemanticOutput)?;
        let changed = transaction.execute(
            "UPDATE semantic_analyses
             SET status = ?2,
                 input_character_count = ?3,
                 analyzed_character_count = ?4,
                 input_quality = ?5,
                 input_quality_status = ?6,
                 input_quality_reasons_json = ?7,
                 language = ?8,
                 duration_ms = ?9,
                 is_current = 1,
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND status = 'running'",
            params![
                analysis_id,
                analysis.status.database_name(),
                to_sql_integer(analysis.input_character_count)?,
                to_sql_integer(analysis.analyzed_character_count)?,
                f64::from(analysis.input_quality.score.value()),
                input_quality_status_name(analysis.input_quality.status),
                quality_reasons,
                analysis.language,
                to_sql_u64(analysis.duration_ms)?,
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }

        let (document_type, context, document_confidence) = semantic_projection_values(analysis);
        transaction.execute(
            "UPDATE local_search_documents
             SET semantic_document_type = ?2,
                 semantic_context = ?3,
                 semantic_status = ?4,
                 semantic_confidence = ?5,
                 metadata_text = trim(
                    COALESCE(extension, '') || ' ' ||
                    COALESCE(detected_type, '') || ' ' ||
                    COALESCE(?2, '') || ' ' ||
                    COALESCE(?3, '')
                 ),
                 indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_id = ?1 AND file_version_id = ?6",
            params![
                candidate.file_id,
                document_type,
                context,
                analysis.status.database_name(),
                document_confidence.map(f64::from),
                candidate.file_version_id,
            ],
        )?;
        synchronize_semantic_reviews(
            &transaction,
            candidate,
            analysis_id,
            &analysis.review_reasons,
        )?;

        let high_confidence = i64::from(
            analysis.review_reasons.is_empty()
                && document_confidence
                    .is_some_and(|confidence| confidence >= ConfidencePolicy::default().high),
        );
        let needs_review = i64::from(!analysis.review_reasons.is_empty());
        let unknown = i64::from(analysis.status == knowledge::SemanticStatus::Unknown);
        let partial = i64::from(analysis.status == knowledge::SemanticStatus::Partial);
        transaction.execute(
            "UPDATE semantic_analysis_batches
             SET files_completed = files_completed + 1,
                 high_confidence_count = high_confidence_count + ?2,
                 needs_review_count = needs_review_count + ?3,
                 unknown_count = unknown_count + ?4,
                 partial_count = partial_count + ?5
             WHERE id = ?1 AND status = 'running'",
            params![batch_id, high_confidence, needs_review, unknown, partial,],
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_semantic_failure(
        &self,
        batch_id: &str,
        candidate: &SemanticAnalysisCandidate,
        analyzer_id: &str,
        analyzer_version: &str,
        provider_id: &str,
        provider_version: &str,
        input_digest: &[u8; 32],
        error_message: &str,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let has_current: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM semantic_analyses
                WHERE file_version_id = ?1 AND is_current = 1
             )",
            [candidate.file_version_id.as_str()],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )?;
        let analysis_id = Uuid::now_v7().to_string();
        let inserted = transaction.execute(
            "INSERT INTO semantic_analyses(
                id, batch_id, workspace_id, scan_id, file_id, file_version_id,
                extraction_result_id, status, schema_version, analyzer_id,
                analyzer_version, provider_id, provider_version,
                processing_location, input_digest, input_character_count,
                analyzed_character_count, input_quality, input_quality_status,
                input_quality_reasons_json, duration_ms, is_current,
                completed_at, error_message
             )
             SELECT
                ?1, b.id, b.workspace_id, b.scan_id, ?3, ?4, ?5,
                'failed', 1, ?6, ?7, ?8, ?9, 'local', ?10, ?11,
                0, 0.0, 'unusable', '[\"provider_failure\"]', 0, ?12,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?13
             FROM semantic_analysis_batches b
             WHERE b.id = ?2 AND b.status = 'running'",
            params![
                analysis_id,
                batch_id,
                candidate.file_id,
                candidate.file_version_id,
                candidate.extraction_result_id,
                analyzer_id,
                analyzer_version,
                provider_id,
                provider_version,
                input_digest.as_slice(),
                to_sql_integer(candidate.extracted_text.chars().count())?,
                i64::from(!has_current),
                truncate_database_text(error_message, 512),
            ],
        )?;
        if inserted != 1 {
            return Err(PersistenceError::NotFound);
        }
        if !has_current {
            transaction.execute(
                "UPDATE local_search_documents
                 SET semantic_status = 'failed',
                     semantic_document_type = NULL,
                     semantic_context = NULL,
                     semantic_confidence = NULL,
                     indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE file_id = ?1 AND file_version_id = ?2",
                params![candidate.file_id, candidate.file_version_id],
            )?;
        }
        transaction.execute(
            "UPDATE semantic_analysis_batches
             SET files_completed = files_completed + 1,
                 failed_count = failed_count + 1
             WHERE id = ?1 AND status = 'running'",
            [batch_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn finish_semantic_batch(
        &self,
        batch_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<SemanticAnalysisBatchRecord, PersistenceError> {
        if !matches!(status, "completed" | "cancelled" | "failed") {
            return Err(PersistenceError::NotFound);
        }
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE semantic_analysis_batches
             SET status = ?2,
                 error_message = ?3,
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND status = 'running'",
            params![
                batch_id,
                status,
                error_message.map(|message| truncate_database_text(message, 512)),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        drop(connection);
        self.semantic_batch(batch_id)
    }

    pub fn semantic_batch(
        &self,
        batch_id: &str,
    ) -> Result<SemanticAnalysisBatchRecord, PersistenceError> {
        let connection = self.lock()?;
        semantic_batch_from_connection(&connection, batch_id)
    }

    pub fn analysis_candidates(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<AnalysisCandidate>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT f.id, fv.id, r.absolute_path, fl.relative_path, fl.basename,
                    c.media_type, c.byte_size, cd.digest
             FROM files f
             JOIN file_versions fv ON fv.file_id = f.id
             JOIN file_locations fl ON fl.id = fv.location_id
             JOIN roots r ON r.id = fl.root_id
             JOIN contents c ON c.id = fv.content_id
             LEFT JOIN content_digests cd ON cd.content_id = c.id AND cd.algorithm = 'blake3'
             WHERE f.workspace_id = ?1
               AND fl.valid_to_scan_id IS NULL
               AND fv.version_number = (
                   SELECT MAX(newer.version_number) FROM file_versions newer WHERE newer.file_id = f.id
               )
             ORDER BY fl.normalized_relative_path",
        )?;
        let rows = statement.query_map([workspace_id.to_string()], |row| {
            Ok(AnalysisCandidate {
                file_id: row.get(0)?,
                file_version_id: row.get(1)?,
                root_path: row.get(2)?,
                relative_path: row.get(3)?,
                display_label: row.get(4)?,
                media_type: row.get(5)?,
                byte_size: row.get::<_, i64>(6)?,
                content_digest: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn store_extraction(
        &self,
        workspace_id: WorkspaceId,
        candidate: &AnalysisCandidate,
        title: &str,
        full_text: &str,
        language: Option<&str>,
        method: &str,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let job_id = Uuid::now_v7().to_string();
        let artifact_id = ArtifactId::new().to_string();
        let extraction_id = Uuid::now_v7().to_string();
        let idempotency_key = format!(
            "extract:{}:{}",
            candidate.file_version_id,
            blake3::hash(full_text.as_bytes()).to_hex()
        );
        transaction.execute(
            "INSERT OR IGNORE INTO jobs(
                id, workspace_id, processor_id, file_version_id,
                requested_by_principal_id, status, idempotency_key,
                started_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'succeeded', ?6,
                       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                job_id,
                workspace_id.to_string(),
                BUILTIN_PROCESSOR_ID,
                candidate.file_version_id,
                LOCAL_PRINCIPAL_ID,
                idempotency_key,
            ],
        )?;
        let effective_job_id: String = transaction.query_row(
            "SELECT id FROM jobs WHERE workspace_id = ?1 AND idempotency_key = ?2",
            params![workspace_id.to_string(), idempotency_key],
            |row| row.get(0),
        )?;
        let digest = *blake3::hash(full_text.as_bytes()).as_bytes();
        transaction.execute(
            "INSERT OR IGNORE INTO artifacts(
                id, job_id, kind, media_type, payload, byte_size, digest_algorithm, digest
             ) VALUES (?1, ?2, 'text', 'text/plain; charset=utf-8', ?3, ?4, 'blake3', ?5)",
            params![
                artifact_id,
                effective_job_id,
                full_text.as_bytes(),
                i64::try_from(full_text.len()).map_err(|_| PersistenceError::NumericOverflow)?,
                digest.as_slice(),
            ],
        )?;
        let effective_artifact_id: String = transaction.query_row(
            "SELECT id FROM artifacts WHERE job_id = ?1 AND digest = ?2 LIMIT 1",
            params![effective_job_id, digest.as_slice()],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO provenance(
                id, artifact_id, source_file_version_id, relation, parameters_json
             ) VALUES (?1, ?2, ?3, 'extracted_from', '{\"network\":false}')",
            params![
                Uuid::now_v7().to_string(),
                effective_artifact_id,
                candidate.file_version_id,
            ],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO extractions(
                id, artifact_id, file_version_id, processor_id, method,
                language, title, full_text, character_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                extraction_id,
                effective_artifact_id,
                candidate.file_version_id,
                BUILTIN_PROCESSOR_ID,
                method,
                language,
                title,
                full_text,
                i64::try_from(full_text.chars().count())
                    .map_err(|_| PersistenceError::NumericOverflow)?,
            ],
        )?;
        let effective_extraction_id: String = transaction.query_row(
            "SELECT id FROM extractions WHERE artifact_id = ?1",
            [effective_artifact_id],
            |row| row.get(0),
        )?;

        for (sequence, (start, end, text)) in chunk_text(full_text, 2_000).into_iter().enumerate() {
            transaction.execute(
                "INSERT OR IGNORE INTO chunks(
                    id, extraction_id, sequence_number, strategy, text, start_offset, end_offset
                 ) VALUES (?1, ?2, ?3, 'paragraph', ?4, ?5, ?6)",
                params![
                    Uuid::now_v7().to_string(),
                    effective_extraction_id,
                    i64::try_from(sequence).map_err(|_| PersistenceError::NumericOverflow)?,
                    text,
                    i64::try_from(start).map_err(|_| PersistenceError::NumericOverflow)?,
                    i64::try_from(end).map_err(|_| PersistenceError::NumericOverflow)?,
                ],
            )?;
        }

        transaction.execute(
            "UPDATE search_documents SET is_current = 0 WHERE file_id = ?1 AND is_current = 1",
            [candidate.file_id.as_str()],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO search_documents(
                file_id, file_version_id, extraction_id, title, path, body, language
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                candidate.file_id,
                candidate.file_version_id,
                effective_extraction_id,
                title,
                candidate.display_label,
                full_text,
                language,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn search(
        &self,
        workspace_id: WorkspaceId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchRow>, PersistenceError> {
        let Some(fts_query) = safe_fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = i64::try_from(limit).map_err(|_| PersistenceError::NumericOverflow)?;
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sd.file_id, sd.file_version_id, fl.basename,
                    snippet(search_documents_fts, 2, '', '', ' … ', 24),
                    sd.body, bm25(search_documents_fts)
             FROM search_documents_fts
             JOIN search_documents sd ON sd.id = search_documents_fts.rowid
             JOIN files f ON f.id = sd.file_id
             JOIN file_locations fl ON fl.file_id = f.id AND fl.valid_to_scan_id IS NULL
             WHERE search_documents_fts MATCH ?1
               AND f.workspace_id = ?2
               AND sd.is_current = 1
             ORDER BY bm25(search_documents_fts)
             LIMIT ?3",
        )?;
        let rows =
            statement.query_map(params![fts_query, workspace_id.to_string(), limit], |row| {
                let raw_score: f64 = row.get(5)?;
                Ok(SearchRow {
                    file_id: row.get(0)?,
                    file_version_id: row.get(1)?,
                    display_label: row.get(2)?,
                    excerpt: row.get(3)?,
                    body: row.get(4)?,
                    score: 1.0 / (1.0 + raw_score.abs()),
                })
            })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn local_search(
        &self,
        workspace_id: WorkspaceId,
        query: SearchQuery,
    ) -> Result<SearchPage, PersistenceError> {
        const FILTERS: &str = "
            AND (:file_type IS NULL OR d.type_group = :file_type)
            AND (:extraction IS NULL OR d.extraction_status = :extraction)
            AND (:ocr IS NULL OR d.ocr_status = :ocr)
            AND (:minimum_size IS NULL OR d.byte_size >= :minimum_size)
            AND (:maximum_size IS NULL OR d.byte_size <= :maximum_size)
            AND (
                :modified = 'any'
                OR (
                    d.modified_at_native IS NOT NULL
                    AND (
                        (:modified = 'today' AND
                            CAST(d.modified_at_native AS INTEGER) >=
                            CAST(strftime('%s', 'now', 'start of day') AS INTEGER) * 1000000000)
                        OR (:modified = 'last_seven_days' AND
                            CAST(d.modified_at_native AS INTEGER) >=
                            CAST(strftime('%s', 'now', '-7 days') AS INTEGER) * 1000000000)
                        OR (:modified = 'last_thirty_days' AND
                            CAST(d.modified_at_native AS INTEGER) >=
                            CAST(strftime('%s', 'now', '-30 days') AS INTEGER) * 1000000000)
                        OR (:modified = 'this_year' AND
                            CAST(d.modified_at_native AS INTEGER) >=
                            CAST(strftime('%s', 'now', 'start of year') AS INTEGER) * 1000000000)
                    )
                )
            )";

        let query = query.bounded();
        let fts_query = safe_local_fts_query(&query.text);
        if fts_query.is_none() && !query.text.trim().is_empty() {
            return Ok(SearchPage {
                query: query.text,
                page: query.page,
                page_size: query.page_size,
                total: 0,
                has_more: false,
                results: Vec::new(),
                interpreted_query: Vec::new(),
                embeddings: EmbeddingSearchStatus::default(),
                timings: SearchTimings::default(),
            });
        }
        let file_type = query.filters.file_type.database_name();
        let extraction = query.filters.extraction.database_name();
        let ocr = query.filters.ocr.database_name();
        let modified = match query.filters.modified {
            ModifiedFilter::Any => "any",
            ModifiedFilter::Today => "today",
            ModifiedFilter::LastSevenDays => "last_seven_days",
            ModifiedFilter::LastThirtyDays => "last_thirty_days",
            ModifiedFilter::ThisYear => "this_year",
        };
        let minimum_size = query.filters.minimum_size.map(to_sql_u64).transpose()?;
        let maximum_size = query.filters.maximum_size.map(to_sql_u64).transpose()?;
        let page_size = to_sql_integer(query.page_size)?;
        let offset = query
            .page
            .checked_mul(query.page_size)
            .ok_or(PersistenceError::NumericOverflow)
            .and_then(to_sql_integer)?;
        let workspace = workspace_id.to_string();
        let connection = self.lock()?;

        let count_sql = if fts_query.is_some() {
            format!(
                "SELECT COUNT(*)
                 FROM local_search_fts
                 CROSS JOIN local_search_documents d ON d.id = local_search_fts.rowid
                 WHERE local_search_fts MATCH :fts
                   AND d.workspace_id = :workspace
                 {FILTERS}"
            )
        } else {
            format!(
                "SELECT COUNT(*)
                 FROM local_search_documents d
                 WHERE (:fts IS NULL OR 1 = 1)
                   AND d.workspace_id = :workspace
                 {FILTERS}"
            )
        };
        let total: i64 = connection.query_row(
            &count_sql,
            named_params! {
                ":fts": fts_query.as_deref(),
                ":workspace": workspace.as_str(),
                ":file_type": file_type,
                ":extraction": extraction,
                ":ocr": ocr,
                ":minimum_size": minimum_size,
                ":maximum_size": maximum_size,
                ":modified": modified,
            },
            |row| row.get(0),
        )?;

        let order = match query.sort {
            SearchSort::Relevance if fts_query.is_some() => {
                "raw_rank ASC, d.filename COLLATE NOCASE ASC, d.file_id ASC"
            }
            SearchSort::Relevance | SearchSort::Filename => {
                "d.filename COLLATE NOCASE ASC, d.relative_path COLLATE NOCASE ASC, d.file_id ASC"
            }
            SearchSort::Newest => {
                "(d.modified_at_native IS NULL) ASC, CAST(d.modified_at_native AS INTEGER) DESC, d.file_id ASC"
            }
            SearchSort::Oldest => {
                "(d.modified_at_native IS NULL) ASC, CAST(d.modified_at_native AS INTEGER) ASC, d.file_id ASC"
            }
            SearchSort::Size => "d.byte_size DESC, d.filename COLLATE NOCASE ASC, d.file_id ASC",
        };
        let select = if fts_query.is_some() {
            "SELECT
                d.file_id, d.filename, d.relative_path, d.detected_type, d.extension,
                d.byte_size, d.modified_at_native, d.extraction_status, d.ocr_status,
                EXISTS(
                    SELECT 1 FROM duplicate_group_members duplicate
                    WHERE duplicate.file_version_id = d.file_version_id
                ),
                bm25(local_search_fts, 8.0, 3.5, 1.0, 0.5) AS raw_rank,
                instr(highlight(local_search_fts, 0, char(31), char(30)), char(31)),
                instr(highlight(local_search_fts, 1, char(31), char(30)), char(31)),
                instr(highlight(local_search_fts, 3, char(31), char(30)), char(31)),
                snippet(local_search_fts, 2, '', '', ' … ', 28),
                d.metadata_text
             FROM local_search_fts
             CROSS JOIN local_search_documents d ON d.id = local_search_fts.rowid
             WHERE local_search_fts MATCH :fts
               AND d.workspace_id = :workspace"
        } else {
            "SELECT
                d.file_id, d.filename, d.relative_path, d.detected_type, d.extension,
                d.byte_size, d.modified_at_native, d.extraction_status, d.ocr_status,
                EXISTS(
                    SELECT 1 FROM duplicate_group_members duplicate
                    WHERE duplicate.file_version_id = d.file_version_id
                ),
                0.0 AS raw_rank, 1, 0, 0, '', d.metadata_text
             FROM local_search_documents d
             WHERE (:fts IS NULL OR 1 = 1)
               AND d.workspace_id = :workspace"
        };
        let sql = format!("{select} {FILTERS} ORDER BY {order} LIMIT :limit OFFSET :offset");
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(named_params! {
            ":fts": fts_query.as_deref(),
            ":workspace": workspace.as_str(),
            ":file_type": file_type,
            ":extraction": extraction,
            ":ocr": ocr,
            ":minimum_size": minimum_size,
            ":maximum_size": maximum_size,
            ":modified": modified,
            ":limit": page_size,
            ":offset": offset,
        })?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            let filename: String = row.get(1)?;
            let relative_path: String = row.get(2)?;
            let filename_match = row.get::<_, i64>(11)? != 0;
            let path_match = row.get::<_, i64>(12)? != 0;
            let metadata_match = row.get::<_, i64>(13)? != 0;
            let content_snippet: String = row.get(14)?;
            let metadata_text: String = row.get(15)?;
            let (match_source, snippet) = if filename_match {
                (MatchSource::Filename, filename.clone())
            } else if path_match {
                (MatchSource::Path, relative_path.clone())
            } else if !content_snippet.is_empty() {
                (MatchSource::Content, content_snippet)
            } else if metadata_match {
                (MatchSource::Metadata, metadata_text)
            } else {
                (MatchSource::Content, String::new())
            };
            let raw_rank: f64 = row.get(10)?;
            results.push(SearchResult {
                file_id: row.get(0)?,
                filename,
                relative_path,
                detected_type: row.get(3)?,
                extension: row.get(4)?,
                byte_size: from_sql_u64(row.get(5)?)?,
                modified_at: row.get(6)?,
                extraction_status: row.get(7)?,
                ocr_status: row.get(8)?,
                duplicate: row.get::<_, i64>(9)? != 0,
                match_source,
                relevance: if fts_query.is_some() {
                    1.0 / (1.0 + raw_rank.abs())
                } else {
                    0.0
                },
                snippet: bounded_chars(&snippet, 500),
                why_matched: vec!["correspondance lexicale locale".to_owned()],
            });
        }
        let total = from_sql_u64(total)?;
        let returned_through = u64::try_from(offset)
            .map_err(|_| PersistenceError::NumericOverflow)?
            .saturating_add(
                u64::try_from(results.len()).map_err(|_| PersistenceError::NumericOverflow)?,
            );
        Ok(SearchPage {
            query: query.text,
            page: query.page,
            page_size: query.page_size,
            total,
            has_more: returned_through < total,
            results,
            interpreted_query: Vec::new(),
            embeddings: EmbeddingSearchStatus::default(),
            timings: SearchTimings::default(),
        })
    }

    pub fn review_items(
        &self,
        workspace_id: WorkspaceId,
        status: ReviewStatusFilter,
        reason: ReviewReasonFilter,
        limit: usize,
        offset: usize,
    ) -> Result<ReviewPageRecord, PersistenceError> {
        let reason_filter = review_reason_filter_sql(reason);
        let status = status.database_name();
        let limit = limit.clamp(1, 100);
        let sql_filter = format!(
            "ri.workspace_id = :workspace
             AND (:status IS NULL OR ri.status = :status)
             AND ({reason_filter})"
        );
        let connection = self.lock()?;
        let total: i64 = connection.query_row(
            &format!("SELECT COUNT(*) FROM file_review_items ri WHERE {sql_filter}"),
            named_params! {
                ":workspace": workspace_id.to_string(),
                ":status": status,
            },
            |row| row.get(0),
        )?;
        let sql = format!(
            "SELECT
                ri.id, ri.file_id, fl.basename, fl.relative_path, ri.reason,
                ri.source_subsystem, ri.severity, ri.explanation, ri.technical_details,
                ri.status, ri.retry_available, ri.retry_count, cer.status,
                ri.created_at, ri.updated_at
             FROM file_review_items ri
             JOIN file_versions fv ON fv.id = ri.file_version_id
             JOIN file_locations fl ON fl.id = fv.location_id
             LEFT JOIN content_extraction_results cer ON cer.id = ri.extraction_result_id
             WHERE {sql_filter}
             ORDER BY
                CASE ri.severity WHEN 'error' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END,
                ri.updated_at DESC, fl.normalized_relative_path
             LIMIT :limit OFFSET :offset"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(named_params! {
            ":workspace": workspace_id.to_string(),
            ":status": status,
            ":limit": to_sql_integer(limit)?,
            ":offset": to_sql_integer(offset)?,
        })?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(review_item_from_row(row)?);
        }
        let total = from_sql_u64(total)?;
        let returned_through = u64::try_from(offset)
            .map_err(|_| PersistenceError::NumericOverflow)?
            .saturating_add(
                u64::try_from(items.len()).map_err(|_| PersistenceError::NumericOverflow)?,
            );
        Ok(ReviewPageRecord {
            total,
            limit,
            offset,
            has_more: returned_through < total,
            items,
        })
    }

    pub fn update_review_item(
        &self,
        review_id: &str,
        action: ReviewAction,
    ) -> Result<ReviewItemRecord, PersistenceError> {
        let connection = self.lock()?;
        let status = action.database_name();
        let changed = connection.execute(
            "UPDATE file_review_items
             SET status = ?2,
                 resolved_at = CASE
                     WHEN ?2 = 'resolved' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            params![review_id, status],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        review_item_by_id(&connection, review_id)
    }

    pub fn file_detail(&self, file_id: &str) -> Result<FileDetailRecord, PersistenceError> {
        let connection = self.lock()?;
        let row = connection
            .query_row(
                "SELECT
                    d.file_version_id, d.filename, d.relative_path, d.extension,
                    d.detected_type, d.byte_size, d.created_at_native,
                    d.modified_at_native, cd.digest,
                    EXISTS(
                        SELECT 1 FROM duplicate_group_members duplicate
                        WHERE duplicate.file_version_id = d.file_version_id
                    ),
                    d.extraction_status, cer.extractor_type, cer.extractor_version,
                    d.ocr_status,
                    CASE
                        WHEN d.extraction_status IN ('success', 'partial')
                        THEN substr(COALESCE(cer.extracted_text, ''), 1, 4000)
                        ELSE ''
                    END,
                    COALESCE(cer.character_count, 0)
                 FROM local_search_documents d
                 JOIN file_versions fv ON fv.id = d.file_version_id
                 LEFT JOIN content_digests cd
                    ON cd.content_id = fv.content_id AND cd.algorithm = 'blake3'
                 LEFT JOIN content_extraction_results cer ON cer.id = d.extraction_result_id
                 WHERE d.file_id = ?1",
                [file_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<Vec<u8>>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, i64>(15)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let review_items = review_items_for_file(&connection, file_id)?;
        let semantic_analysis = semantic_detail_for_file(&connection, file_id)?;
        let relationships = identity::relationships_for_file(&connection, file_id)?;
        Ok(FileDetailRecord {
            file_id: file_id.to_owned(),
            file_version_id: row.0,
            filename: row.1,
            relative_path: row.2,
            extension: row.3,
            detected_type: row.4,
            byte_size: from_sql_u64(row.5)?,
            created_at: row.6,
            modified_at: row.7,
            hash: row.8.as_deref().map(bytes_to_hex),
            duplicate: row.9 != 0,
            extraction_status: row.10,
            extractor_type: row.11,
            extractor_version: row.12,
            ocr_status: row.13,
            text_preview: row.14,
            character_count: from_sql_u64(row.15)?,
            review_items,
            semantic_analysis,
            relationships,
        })
    }

    pub fn store_semantic_correction(
        &self,
        file_id: &str,
        input: &SemanticCorrectionInput,
    ) -> Result<SemanticCorrectionRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let machine = transaction
            .query_row(
                "SELECT
                    sa.workspace_id, sa.id, sf.id, sf.value_kind,
                    sf.display_value, sf.normalized_value_json
                 FROM semantic_analyses sa
                 JOIN semantic_fields sf ON sf.analysis_id = sa.id
                 WHERE sa.file_id = ?1
                   AND sa.is_current = 1
                   AND sf.field_key = ?2
                   AND sf.is_primary = 1",
                params![file_id, input.field_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;

        let (value_kind, display_value, normalized_value_json) = if input.correction_state
            == "user_confirmed"
        {
            (
                machine.3.ok_or(PersistenceError::NotFound)?,
                machine.4.ok_or(PersistenceError::NotFound)?,
                machine.5,
            )
        } else if input.correction_state == "user_corrected" {
            if serde_json::from_str::<serde_json::Value>(&input.normalized_value_json).is_err() {
                return Err(PersistenceError::InvalidSemanticOutput);
            }
            (
                input.value_kind.clone(),
                input.display_value.clone(),
                input.normalized_value_json.clone(),
            )
        } else {
            return Err(PersistenceError::InvalidSemanticOutput);
        };

        transaction.execute(
            "UPDATE semantic_user_corrections
             SET active = 0,
                 superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_id = ?1 AND field_key = ?2 AND active = 1",
            params![file_id, input.field_key],
        )?;
        let correction_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO semantic_user_corrections(
                id, workspace_id, file_id, field_key, correction_state,
                source_analysis_id, source_field_id, value_kind,
                display_value, normalized_value_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                correction_id,
                machine.0,
                file_id,
                input.field_key,
                input.correction_state,
                machine.1,
                machine.2,
                value_kind,
                truncate_database_text(&display_value, 512),
                normalized_value_json,
            ],
        )?;
        resolve_review_for_correction(&transaction, file_id, &input.field_key)?;
        transaction.commit()?;
        drop(connection);
        let connection = self.lock()?;
        semantic_correction_by_id(&connection, &correction_id)
    }

    pub fn begin_review_retry(
        &self,
        review_id: &str,
    ) -> Result<ReviewRetryCandidate, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let row = transaction
            .query_row(
                "SELECT
                    ri.id, ri.workspace_id, ri.file_id, ri.file_version_id,
                    fv.observed_by_scan_id, r.absolute_path, fl.relative_path, fl.basename,
                    sfs.extension, c.media_type, fv.byte_size, sfs.readability_status
                 FROM file_review_items ri
                 JOIN file_versions fv ON fv.id = ri.file_version_id
                 JOIN file_locations fl ON fl.id = fv.location_id
                 JOIN roots r ON r.id = fl.root_id
                 LEFT JOIN scan_file_statuses sfs
                    ON sfs.scan_id = fv.observed_by_scan_id
                   AND sfs.file_version_id = fv.id
                 LEFT JOIN contents c ON c.id = fv.content_id
                 WHERE ri.id = ?1
                   AND ri.status = 'needs_review'
                   AND ri.retry_available = 1
                   AND ri.retry_count < 5",
                [review_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, Option<String>>(11)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let scan_id: ScanId = row.4.ok_or(PersistenceError::NotFound)?.parse()?;
        let batch_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO content_extraction_batches(
                id, workspace_id, scan_id, status, files_queued, started_at
             ) VALUES (
                ?1, ?2, ?3, 'running', 1,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )",
            params![batch_id, row.1, scan_id.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO content_extraction_results(
                id, batch_id, scan_id, file_id, file_version_id, status,
                extension, detected_content_type
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
            params![
                format!("{batch_id}:{}", row.3),
                batch_id,
                scan_id.to_string(),
                row.2,
                row.3,
                row.8,
                row.9,
            ],
        )?;
        transaction.execute(
            "UPDATE file_review_items
             SET retry_count = retry_count + 1,
                 last_retried_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 retry_available = CASE WHEN retry_count + 1 >= 5 THEN 0 ELSE retry_available END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            [review_id],
        )?;
        transaction.commit()?;
        Ok(ReviewRetryCandidate {
            review_id: row.0,
            batch_id,
            scan_id,
            candidate: ExtractionCandidate {
                file_id: row.2,
                file_version_id: row.3,
                root_path: row.5,
                relative_path: row.6,
                filename: row.7,
                extension: row.8,
                declared_media_type: row.9,
                byte_size: from_sql_u64(row.10)?,
                // An explicit retry re-checks current access through the same
                // scoped read-only platform instead of trusting stale scan state.
                readable: true,
            },
        })
    }

    pub fn local_search_integrity_check(&self) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO local_search_fts(local_search_fts) VALUES('integrity-check')",
            [],
        )?;
        Ok(())
    }

    pub fn foreign_key_violation_count(&self) -> Result<u64, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
        let mut rows = statement.query([])?;
        let mut count = 0_u64;
        while rows.next()?.is_some() {
            count = count.saturating_add(1);
        }
        Ok(count)
    }
}

fn schema_table_exists(
    connection: &Connection,
    table_name: &str,
) -> Result<bool, PersistenceError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table' AND name = ?1
             )",
            [table_name],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .map_err(PersistenceError::Sql)
}

fn apply_monitoring_migration_if_missing(
    connection: &Connection,
    original_schema_version: i64,
) -> Result<(), PersistenceError> {
    if schema_table_exists(connection, "workspace_monitoring_state")? {
        return Ok(());
    }

    // Early development catalogs could contain the former v9 hybrid-search
    // migration. Release v12 owns that schema now, leaving v9 unambiguous.
    connection.execute(
        "DELETE FROM schema_migrations
         WHERE version = 9 AND name = '0009_hybrid_semantic_search'",
        [],
    )?;
    connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
    connection.pragma_update(None, "user_version", original_schema_version)?;
    Ok(())
}

fn apply_hybrid_search_migration(connection: &Connection) -> Result<(), PersistenceError> {
    if schema_table_exists(connection, "local_embedding_models")? {
        connection.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, name)
             VALUES (12, '0012_hybrid_semantic_search')",
            [],
        )?;
        connection.pragma_update(None, "user_version", 12)?;
        return Ok(());
    }

    connection.execute_batch(HYBRID_SEMANTIC_SEARCH_MIGRATION)?;
    Ok(())
}

fn repair_legacy_native_path_storage(connection: &Connection) -> Result<(), PersistenceError> {
    let transaction = connection.unchecked_transaction()?;

    let file_locations = {
        let mut statement = transaction.prepare(
            "SELECT id, relative_path, normalized_relative_path
             FROM file_locations
             WHERE substr(relative_path_native, 1, 1) = X'00'
                OR substr(normalized_relative_path_native, 1, 1) = X'00'",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, relative, normalized) in file_locations {
        transaction.execute(
            "UPDATE file_locations
             SET relative_path_native = ?2,
                 normalized_relative_path_native = ?3
             WHERE id = ?1",
            params![
                id,
                native_text_storage_blob(&relative),
                native_text_storage_blob(&normalized)
            ],
        )?;
    }

    let watch_events = {
        let mut statement = transaction.prepare(
            "SELECT id, path_before, path_after
             FROM watch_events
             WHERE substr(path_before_native, 1, 1) = X'00'
                OR substr(path_after_native, 1, 1) = X'00'",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, before, after) in watch_events {
        transaction.execute(
            "UPDATE watch_events
             SET path_before_native = ?2,
                 path_after_native = ?3
             WHERE id = ?1",
            params![
                id,
                before.as_deref().map(native_text_storage_blob),
                after.as_deref().map(native_text_storage_blob)
            ],
        )?;
    }

    let monitoring_jobs = {
        let mut statement = transaction.prepare(
            "SELECT id, path_before, path_after, coalescing_path
             FROM monitoring_jobs
             WHERE substr(path_before_native, 1, 1) = X'00'
                OR substr(path_after_native, 1, 1) = X'00'
                OR substr(coalescing_path_native, 1, 1) = X'00'",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, before, after, coalescing) in monitoring_jobs {
        transaction.execute(
            "UPDATE monitoring_jobs
             SET path_before_native = ?2,
                 path_after_native = ?3,
                 coalescing_path_native = ?4
             WHERE id = ?1",
            params![
                id,
                before.as_deref().map(native_text_storage_blob),
                after.as_deref().map(native_text_storage_blob),
                coalescing.as_deref().map(native_text_storage_blob)
            ],
        )?;
    }

    transaction.commit()?;
    Ok(())
}

fn native_text_storage_blob(value: &str) -> Vec<u8> {
    #[cfg(unix)]
    {
        let mut output = Vec::with_capacity(value.len().saturating_add(1));
        output.push(1);
        output.extend_from_slice(value.as_bytes());
        output
    }
    #[cfg(windows)]
    {
        let mut output = Vec::with_capacity(value.len().saturating_mul(2).saturating_add(1));
        output.push(2);
        output.extend(value.encode_utf16().flat_map(u16::to_le_bytes));
        output
    }
    #[cfg(not(any(unix, windows)))]
    {
        let mut output = Vec::with_capacity(value.len().saturating_add(1));
        output.push(0);
        output.extend_from_slice(value.as_bytes());
        output
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisCandidate {
    pub file_id: String,
    pub file_version_id: String,
    pub root_path: String,
    pub relative_path: String,
    pub display_label: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub content_digest: Option<Vec<u8>>,
}

fn semantic_batch_from_connection(
    connection: &Connection,
    batch_id: &str,
) -> Result<SemanticAnalysisBatchRecord, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT workspace_id, scan_id, status, files_queued, files_completed,
                    high_confidence_count, needs_review_count, unknown_count,
                    partial_count, failed_count, started_at, completed_at
             FROM semantic_analysis_batches
             WHERE id = ?1",
            [batch_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    Ok(SemanticAnalysisBatchRecord {
        id: batch_id.to_owned(),
        workspace_id: row.0.parse()?,
        scan_id: row.1.parse()?,
        status: row.2,
        files_queued: from_sql_u64(row.3)?,
        files_completed: from_sql_u64(row.4)?,
        high_confidence_count: from_sql_u64(row.5)?,
        needs_review_count: from_sql_u64(row.6)?,
        unknown_count: from_sql_u64(row.7)?,
        partial_count: from_sql_u64(row.8)?,
        failed_count: from_sql_u64(row.9)?,
        started_at: row.10,
        completed_at: row.11,
    })
}

fn insert_semantic_field(
    transaction: &Transaction<'_>,
    analysis_id: &str,
    field: &knowledge::SemanticField,
) -> Result<(), PersistenceError> {
    let primary_id = Uuid::now_v7().to_string();
    insert_semantic_field_row(
        transaction,
        analysis_id,
        &primary_id,
        field.field_type,
        0,
        true,
        field.value.as_ref(),
        field.original_value.as_deref(),
        field.confidence.value(),
        field.status,
        field.source_method,
        &field.analyzer_version,
        &field.evidence,
    )?;

    let primary_key = field.value.as_ref().map(semantic_value_key);
    let mut rank = 1_usize;
    for candidate in &field.candidates {
        if primary_key
            .as_ref()
            .is_some_and(|key| *key == semantic_value_key(&candidate.value))
        {
            continue;
        }
        let candidate_id = Uuid::now_v7().to_string();
        let status = if candidate.ambiguous {
            FieldStatus::Ambiguous
        } else if candidate.confidence.value() >= ConfidencePolicy::default().high {
            FieldStatus::Confirmed
        } else if candidate.confidence.value() >= ConfidencePolicy::default().medium {
            FieldStatus::Inferred
        } else {
            FieldStatus::Unknown
        };
        insert_semantic_field_row(
            transaction,
            analysis_id,
            &candidate_id,
            field.field_type,
            rank,
            false,
            Some(&candidate.value),
            Some(&candidate.original_value),
            candidate.confidence.value(),
            status,
            candidate.source_method,
            &field.analyzer_version,
            &candidate.evidence,
        )?;
        rank = rank.saturating_add(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_semantic_field_row(
    transaction: &Transaction<'_>,
    analysis_id: &str,
    field_id: &str,
    field_type: SemanticFieldType,
    candidate_rank: usize,
    is_primary: bool,
    value: Option<&SemanticValue>,
    display_value: Option<&str>,
    confidence: f32,
    status: FieldStatus,
    source_method: knowledge::SourceMethod,
    analyzer_version: &str,
    evidence: &[SemanticEvidence],
) -> Result<(), PersistenceError> {
    let normalized =
        serde_json::to_string(&value).map_err(|_| PersistenceError::InvalidSemanticOutput)?;
    let effective_display = value
        .map(SemanticValue::display_value)
        .or_else(|| display_value.map(|value| truncate_database_text(value, 512)));
    transaction.execute(
        "INSERT INTO semantic_fields(
            id, analysis_id, field_key, candidate_rank, is_primary,
            value_kind, display_value, normalized_value_json, confidence,
            field_status, source_method, analyzer_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            field_id,
            analysis_id,
            field_type.database_name(),
            to_sql_integer(candidate_rank)?,
            i64::from(is_primary),
            value.map(SemanticValue::kind_name),
            effective_display,
            normalized,
            f64::from(confidence),
            status.database_name(),
            source_method.database_name(),
            analyzer_version,
        ],
    )?;
    for item in evidence.iter().take(8) {
        insert_semantic_evidence(transaction, analysis_id, Some(field_id), None, item)?;
    }
    Ok(())
}

fn insert_semantic_entity(
    transaction: &Transaction<'_>,
    analysis_id: &str,
    entity: &knowledge::SemanticEntity,
) -> Result<(), PersistenceError> {
    let entity_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO semantic_entities(
            id, analysis_id, candidate_key, entity_type, original_value,
            normalized_value, confidence, field_status, source_method,
            analyzer_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            entity_id,
            analysis_id,
            entity.candidate_key,
            entity.entity_type.database_name(),
            entity.original_value,
            entity.normalized_value,
            f64::from(entity.confidence.value()),
            entity.status.database_name(),
            entity.source_method.database_name(),
            entity.analyzer_version,
        ],
    )?;
    for item in entity.evidence.iter().take(8) {
        insert_semantic_evidence(transaction, analysis_id, None, Some(&entity_id), item)?;
    }
    Ok(())
}

fn insert_semantic_evidence(
    transaction: &Transaction<'_>,
    analysis_id: &str,
    field_id: Option<&str>,
    entity_id: Option<&str>,
    evidence: &SemanticEvidence,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT INTO semantic_evidence(
            id, analysis_id, field_id, entity_id, evidence_type, exact_text,
            start_offset, end_offset, page_number, sheet_name, slide_number,
            source_label, explanation, extraction_method, analyzer_version
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
         )",
        params![
            Uuid::now_v7().to_string(),
            analysis_id,
            field_id,
            entity_id,
            evidence.evidence_type.database_name(),
            truncate_database_text(&evidence.exact_text, 2_000),
            evidence.start_offset.map(to_sql_integer).transpose()?,
            evidence.end_offset.map(to_sql_integer).transpose()?,
            evidence.page_number.map(i64::from),
            evidence.sheet_name,
            evidence.slide_number.map(i64::from),
            truncate_database_text(&evidence.source_label, 256),
            truncate_database_text(&evidence.explanation, 512),
            truncate_database_text(&evidence.extraction_method, 128),
            evidence.analyzer_version,
        ],
    )?;
    Ok(())
}

fn semantic_value_key(value: &SemanticValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.display_value())
}

fn semantic_projection_values(
    analysis: &SemanticAnalysis,
) -> (Option<&'static str>, Option<&'static str>, Option<f32>) {
    let document = analysis
        .primary_field(SemanticFieldType::DocumentType)
        .and_then(|field| match field.value {
            Some(SemanticValue::DocumentType { value }) => {
                Some((value.database_name(), field.confidence.value()))
            }
            _ => None,
        });
    let context = analysis
        .primary_field(SemanticFieldType::Context)
        .and_then(|field| match field.value {
            Some(SemanticValue::Context { value }) => Some(value.database_name()),
            _ => None,
        });
    (
        document.map(|(document_type, _)| document_type),
        context,
        document.map(|(_, confidence)| confidence),
    )
}

fn synchronize_semantic_reviews(
    transaction: &Transaction<'_>,
    candidate: &SemanticAnalysisCandidate,
    analysis_id: &str,
    reasons: &[SemanticReviewReason],
) -> Result<(), PersistenceError> {
    let all_reasons = [
        SemanticReviewReason::SemanticAmbiguity,
        SemanticReviewReason::ConflictingFields,
        SemanticReviewReason::LowConfidenceDocumentType,
        SemanticReviewReason::LowConfidenceContext,
        SemanticReviewReason::MissingCriticalFields,
    ];
    for reason in all_reasons {
        if reasons.contains(&reason) {
            let (severity, explanation) = semantic_review_copy(reason);
            transaction.execute(
                "INSERT INTO file_review_items(
                    id, workspace_id, file_id, file_version_id,
                    extraction_result_id, reason, source_subsystem, severity,
                    explanation, technical_details, retry_available
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, 'semantic', ?7, ?8, ?9, 0
                 )
                 ON CONFLICT(file_version_id, reason) DO UPDATE SET
                    extraction_result_id = excluded.extraction_result_id,
                    source_subsystem = 'semantic',
                    severity = excluded.severity,
                    explanation = excluded.explanation,
                    technical_details = excluded.technical_details,
                    status = CASE
                        WHEN file_review_items.status = 'ignored' THEN 'ignored'
                        ELSE 'needs_review'
                    END,
                    resolved_at = NULL,
                    retry_available = 0,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![
                    Uuid::now_v7().to_string(),
                    candidate.workspace_id.to_string(),
                    candidate.file_id,
                    candidate.file_version_id,
                    candidate.extraction_result_id,
                    reason.database_name(),
                    severity,
                    explanation,
                    format!("semantic_analysis_id={analysis_id}"),
                ],
            )?;
        } else {
            transaction.execute(
                "UPDATE file_review_items
                 SET status = CASE WHEN status = 'ignored' THEN 'ignored' ELSE 'resolved' END,
                     resolved_at = CASE
                        WHEN status = 'ignored' THEN NULL
                        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     END,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE file_version_id = ?1
                   AND reason = ?2
                   AND source_subsystem = 'semantic'
                   AND status = 'needs_review'",
                params![candidate.file_version_id, reason.database_name()],
            )?;
        }
    }
    Ok(())
}

fn semantic_review_copy(reason: SemanticReviewReason) -> (&'static str, &'static str) {
    match reason {
        SemanticReviewReason::SemanticAmbiguity => (
            "warning",
            "Plusieurs interprétations sémantiques restent plausibles.",
        ),
        SemanticReviewReason::ConflictingFields => (
            "warning",
            "Le document contient des valeurs importantes contradictoires.",
        ),
        SemanticReviewReason::LowConfidenceDocumentType => (
            "information",
            "Le type du document ne peut pas être établi avec assez de certitude.",
        ),
        SemanticReviewReason::LowConfidenceContext => (
            "information",
            "Le contexte personnel ou professionnel reste incertain.",
        ),
        SemanticReviewReason::MissingCriticalFields => (
            "warning",
            "Des champs essentiels attendus dans ce type de document sont absents.",
        ),
    }
}

const fn input_quality_status_name(status: knowledge::InputQualityStatus) -> &'static str {
    match status {
        knowledge::InputQualityStatus::Good => "good",
        knowledge::InputQualityStatus::Degraded => "degraded",
        knowledge::InputQualityStatus::Poor => "poor",
        knowledge::InputQualityStatus::Unusable => "unusable",
    }
}

fn truncate_database_text(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn semantic_detail_for_file(
    connection: &Connection,
    file_id: &str,
) -> Result<Option<SemanticAnalysisDetailRecord>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT
                id, status, analyzer_id, analyzer_version, provider_id,
                provider_version, schema_version, input_quality,
                input_quality_status, input_quality_reasons_json,
                language, completed_at
             FROM semantic_analyses
             WHERE file_id = ?1 AND is_current = 1
             ORDER BY completed_at DESC, started_at DESC
             LIMIT 1",
            [file_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, f64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let fields = semantic_fields_for_analysis(connection, file_id, &row.0)?;
    let entities = semantic_entities_for_analysis(connection, &row.0)?;
    let input_quality_reasons =
        serde_json::from_str::<Vec<String>>(&row.9).unwrap_or_else(|_| Vec::new());
    Ok(Some(SemanticAnalysisDetailRecord {
        analysis_id: row.0,
        status: row.1,
        analyzer_id: row.2,
        analyzer_version: row.3,
        provider_id: row.4,
        provider_version: row.5,
        schema_version: u32::try_from(row.6).map_err(|_| PersistenceError::NumericOverflow)?,
        input_quality: row.7 as f32,
        input_quality_status: row.8,
        input_quality_reasons,
        language: row.10,
        analyzed_at: row.11,
        fields,
        entities,
    }))
}

fn semantic_fields_for_analysis(
    connection: &Connection,
    file_id: &str,
    analysis_id: &str,
) -> Result<Vec<SemanticFieldRecord>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            sf.id, sf.field_key, COALESCE(c.value_kind, sf.value_kind), sf.display_value,
            COALESCE(c.display_value, sf.display_value),
            COALESCE(c.normalized_value_json, sf.normalized_value_json),
            sf.confidence, sf.field_status, sf.source_method,
            sf.analyzer_version, c.correction_state
         FROM semantic_fields sf
         LEFT JOIN semantic_user_corrections c
            ON c.file_id = ?2
           AND c.field_key = sf.field_key
           AND c.active = 1
         WHERE sf.analysis_id = ?1 AND sf.is_primary = 1
         ORDER BY CASE sf.field_key
            WHEN 'document_type' THEN 0
            WHEN 'context' THEN 1
            WHEN 'supplier_candidate' THEN 2
            WHEN 'issuer' THEN 2
            WHEN 'customer_candidate' THEN 3
            WHEN 'issue_date' THEN 4
            WHEN 'total' THEN 5
            WHEN 'amount' THEN 5
            WHEN 'invoice_number' THEN 6
            WHEN 'quote_number' THEN 6
            ELSE 20
         END, sf.field_key",
    )?;
    let rows = statement.query_map(params![analysis_id, file_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, f64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, Option<String>>(10)?,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let row = row?;
        output.push(SemanticFieldRecord {
            evidence: semantic_evidence_for_target(connection, Some(&row.0), None)?,
            candidates: semantic_candidates_for_field(connection, analysis_id, &row.1)?,
            field_id: row.0,
            field_key: row.1,
            value_kind: row.2,
            display_value: row.4,
            machine_display_value: row.3,
            normalized_value: serde_json::from_str(&row.5).unwrap_or(serde_json::Value::Null),
            confidence: row.6 as f32,
            status: row.7,
            source_method: row.8,
            analyzer_version: row.9,
            value_source: if row.10.is_some() {
                "user".to_owned()
            } else {
                "machine".to_owned()
            },
            user_state: row.10,
        });
    }
    Ok(output)
}

fn semantic_candidates_for_field(
    connection: &Connection,
    analysis_id: &str,
    field_key: &str,
) -> Result<Vec<SemanticCandidateValueRecord>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT id, display_value, normalized_value_json, confidence,
                field_status, source_method
         FROM semantic_fields
         WHERE analysis_id = ?1
           AND field_key = ?2
           AND is_primary = 0
         ORDER BY candidate_rank
         LIMIT 8",
    )?;
    let rows = statement.query_map(params![analysis_id, field_key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let row = row?;
        output.push(SemanticCandidateValueRecord {
            display_value: row.1.unwrap_or_else(|| "Unknown".to_owned()),
            normalized_value: serde_json::from_str(&row.2).unwrap_or(serde_json::Value::Null),
            confidence: row.3 as f32,
            status: row.4,
            source_method: row.5,
            evidence: semantic_evidence_for_target(connection, Some(&row.0), None)?,
        });
    }
    Ok(output)
}

fn semantic_entities_for_analysis(
    connection: &Connection,
    analysis_id: &str,
) -> Result<Vec<SemanticEntityRecord>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT id, candidate_key, entity_type, original_value,
                normalized_value, confidence, field_status, source_method
         FROM semantic_entities
         WHERE analysis_id = ?1
         ORDER BY confidence DESC, entity_type, normalized_value
         LIMIT 128",
    )?;
    let rows = statement.query_map([analysis_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, f64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let row = row?;
        output.push(SemanticEntityRecord {
            evidence: semantic_evidence_for_target(connection, None, Some(&row.0))?,
            entity_id: row.0,
            candidate_key: row.1,
            entity_type: row.2,
            original_value: row.3,
            normalized_value: row.4,
            confidence: row.5 as f32,
            status: row.6,
            source_method: row.7,
        });
    }
    Ok(output)
}

fn semantic_evidence_for_target(
    connection: &Connection,
    field_id: Option<&str>,
    entity_id: Option<&str>,
) -> Result<Vec<SemanticEvidenceRecord>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            evidence_type, exact_text, start_offset, end_offset,
            page_number, sheet_name, slide_number, source_label,
            explanation, extraction_method, analyzer_version
         FROM semantic_evidence
         WHERE (?1 IS NOT NULL AND field_id = ?1)
            OR (?2 IS NOT NULL AND entity_id = ?2)
         ORDER BY created_at, id
         LIMIT 8",
    )?;
    let rows = statement.query_map(params![field_id, entity_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let row = row?;
        output.push(SemanticEvidenceRecord {
            evidence_type: row.0,
            exact_text: row.1,
            start_offset: row.2.map(from_sql_u64).transpose()?,
            end_offset: row.3.map(from_sql_u64).transpose()?,
            page_number: optional_u32(row.4)?,
            sheet_name: row.5,
            slide_number: optional_u32(row.6)?,
            source_label: row.7,
            explanation: row.8,
            extraction_method: row.9,
            analyzer_version: row.10,
        });
    }
    Ok(output)
}

fn semantic_correction_by_id(
    connection: &Connection,
    correction_id: &str,
) -> Result<SemanticCorrectionRecord, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT file_id, field_key, correction_state, value_kind,
                    display_value, normalized_value_json, created_at, updated_at
             FROM semantic_user_corrections
             WHERE id = ?1",
            [correction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    Ok(SemanticCorrectionRecord {
        correction_id: correction_id.to_owned(),
        file_id: row.0,
        field_key: row.1,
        correction_state: row.2,
        value_kind: row.3,
        display_value: row.4,
        normalized_value: serde_json::from_str(&row.5).unwrap_or(serde_json::Value::Null),
        created_at: row.6,
        updated_at: row.7,
    })
}

fn resolve_review_for_correction(
    transaction: &Transaction<'_>,
    file_id: &str,
    field_key: &str,
) -> Result<(), PersistenceError> {
    let reasons: &[&str] = match field_key {
        "document_type" => &[
            "low_confidence_document_type",
            "semantic_ambiguity",
            "conflicting_fields",
        ],
        "context" => &["low_confidence_context", "semantic_ambiguity"],
        _ => &["conflicting_fields", "missing_critical_fields"],
    };
    for reason in reasons {
        transaction.execute(
            "UPDATE file_review_items
             SET status = CASE WHEN status = 'ignored' THEN 'ignored' ELSE 'resolved' END,
                 resolved_at = CASE
                    WHEN status = 'ignored' THEN NULL
                    ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_id = ?1
               AND reason = ?2
               AND source_subsystem = 'semantic'
               AND status = 'needs_review'",
            params![file_id, reason],
        )?;
    }
    Ok(())
}

fn bounded_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn file_type_group(extension: Option<&str>) -> &'static str {
    match extension.unwrap_or_default().to_ascii_lowercase().as_str() {
        "pdf" => "pdf",
        "txt" | "md" | "log" | "json" | "xml" | "doc" | "docx" | "rtf" | "odt" => "documents",
        "csv" | "xls" | "xlsx" | "ods" => "spreadsheets",
        "ppt" | "pptx" | "odp" => "presentations",
        "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" | "gif" | "heic" => "images",
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" => "archives",
        _ => "other",
    }
}

fn upsert_scan_search_document(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
    persisted: &PersistedFile,
    extension: Option<&str>,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT INTO local_search_documents(
            workspace_id, file_id, file_version_id, filename, relative_path,
            extension, detected_type, type_group, metadata_text, byte_size,
            modified_at_native, created_at_native
         )
         SELECT
            ?1, fv.file_id, fv.id, fl.basename, fl.relative_path,
            ?5, c.media_type, ?6, trim(COALESCE(?5, '') || ' ' || COALESCE(c.media_type, '')),
            fv.byte_size, fv.modified_at, fv.created_at_native
         FROM file_versions fv
         JOIN file_locations fl ON fl.id = fv.location_id
         LEFT JOIN contents c ON c.id = fv.content_id
         WHERE fv.id = ?3 AND fv.file_id = ?2
         ON CONFLICT(file_id) DO UPDATE SET
            workspace_id = excluded.workspace_id,
            file_version_id = excluded.file_version_id,
            extraction_result_id = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.extraction_result_id
                ELSE NULL
            END,
            filename = excluded.filename,
            relative_path = excluded.relative_path,
            extension = excluded.extension,
            detected_type = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN COALESCE(local_search_documents.detected_type, excluded.detected_type)
                ELSE excluded.detected_type
            END,
            type_group = excluded.type_group,
            metadata_text = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN trim(
                    COALESCE(excluded.extension, '') || ' ' ||
                    COALESCE(local_search_documents.detected_type, excluded.detected_type, '')
                )
                ELSE excluded.metadata_text
            END,
            byte_size = excluded.byte_size,
            modified_at_native = excluded.modified_at_native,
            created_at_native = excluded.created_at_native,
            extraction_status = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.extraction_status
                ELSE NULL
            END,
            ocr_status = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.ocr_status
                ELSE NULL
            END,
            semantic_document_type = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.semantic_document_type
                ELSE NULL
            END,
            semantic_context = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.semantic_context
                ELSE NULL
            END,
            semantic_status = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.semantic_status
                ELSE NULL
            END,
            semantic_confidence = CASE
                WHEN local_search_documents.file_version_id = excluded.file_version_id
                THEN local_search_documents.semantic_confidence
                ELSE NULL
            END,
            indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            workspace_id.to_string(),
            persisted.file_id,
            persisted.file_version_id,
            persisted.location_id,
            extension,
            file_type_group(extension),
        ],
    )?;
    transaction.execute(
        "UPDATE semantic_analyses
         SET is_current = 0
         WHERE file_id = ?1
           AND file_version_id <> ?2
           AND is_current = 1",
        params![persisted.file_id, persisted.file_version_id],
    )?;
    transaction.execute(
        "DELETE FROM local_search_embeddings
         WHERE file_id = ?1 AND file_version_id <> ?2",
        params![persisted.file_id, persisted.file_version_id],
    )?;
    transaction.execute(
        "DELETE FROM local_search_embedding_state
         WHERE file_id = ?1 AND file_version_id <> ?2",
        params![persisted.file_id, persisted.file_version_id],
    )?;
    transaction.execute(
        "UPDATE file_review_items
         SET status = 'resolved',
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE file_id = ?1
           AND file_version_id <> ?2
           AND status = 'needs_review'",
        params![persisted.file_id, persisted.file_version_id],
    )?;
    Ok(())
}

fn synchronize_scanner_review(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
    persisted: &PersistedFile,
    readability_status: &str,
    error_code: Option<&str>,
) -> Result<(), PersistenceError> {
    if readability_status != "unreadable" {
        transaction.execute(
            "UPDATE file_review_items
             SET status = 'resolved',
                 resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_version_id = ?1
               AND source_subsystem = 'scanner'
               AND status = 'needs_review'",
            [persisted.file_version_id.as_str()],
        )?;
        return Ok(());
    }
    let permission_denied = error_code
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("permission");
    let reason = if permission_denied {
        "permission_denied"
    } else {
        "unreadable"
    };
    let explanation = if permission_denied {
        "L’application n’a pas l’autorisation de lire ce fichier."
    } else {
        "Ce fichier ne peut pas être lu de façon sûre."
    };
    transaction.execute(
        "INSERT INTO file_review_items(
            id, workspace_id, file_id, file_version_id, reason,
            source_subsystem, severity, explanation, technical_details,
            retry_available
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'scanner', 'error', ?6, ?7, 1)
         ON CONFLICT(file_version_id, reason) DO UPDATE SET
            source_subsystem = 'scanner',
            severity = excluded.severity,
            explanation = excluded.explanation,
            technical_details = excluded.technical_details,
            status = CASE
                WHEN file_review_items.status = 'ignored' THEN 'ignored'
                ELSE 'needs_review'
            END,
            retry_available = CASE
                WHEN file_review_items.retry_count >= 5 THEN 0
                ELSE 1
            END,
            resolved_at = NULL,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            Uuid::now_v7().to_string(),
            workspace_id.to_string(),
            persisted.file_id,
            persisted.file_version_id,
            reason,
            explanation,
            error_code,
        ],
    )?;
    Ok(())
}

fn ensure_search_document(
    transaction: &Transaction<'_>,
    file_id: &str,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT OR IGNORE INTO local_search_documents(
            workspace_id, file_id, file_version_id, filename, relative_path,
            extension, detected_type, type_group, metadata_text, byte_size,
            modified_at_native, created_at_native
         )
         SELECT
            f.workspace_id, f.id, fv.id, fl.basename, fl.relative_path,
            sfs.extension, c.media_type,
            CASE
                WHEN lower(COALESCE(sfs.extension, '')) = 'pdf' THEN 'pdf'
                WHEN lower(COALESCE(sfs.extension, '')) IN (
                    'txt', 'md', 'log', 'json', 'xml', 'doc', 'docx', 'rtf', 'odt'
                ) THEN 'documents'
                WHEN lower(COALESCE(sfs.extension, '')) IN ('csv', 'xls', 'xlsx', 'ods')
                    THEN 'spreadsheets'
                WHEN lower(COALESCE(sfs.extension, '')) IN ('ppt', 'pptx', 'odp')
                    THEN 'presentations'
                WHEN lower(COALESCE(sfs.extension, '')) IN (
                    'png', 'jpg', 'jpeg', 'webp', 'tif', 'tiff', 'bmp', 'gif', 'heic'
                ) THEN 'images'
                WHEN lower(COALESCE(sfs.extension, '')) IN (
                    'zip', '7z', 'rar', 'tar', 'gz', 'bz2', 'xz'
                ) THEN 'archives'
                ELSE 'other'
            END,
            trim(COALESCE(sfs.extension, '') || ' ' || COALESCE(c.media_type, '')),
            fv.byte_size, fv.modified_at, fv.created_at_native
         FROM files f
         JOIN file_versions fv ON fv.file_id = f.id
         JOIN file_locations fl ON fl.id = fv.location_id
         LEFT JOIN scan_file_statuses sfs
            ON sfs.scan_id = fv.observed_by_scan_id
           AND sfs.file_version_id = fv.id
         LEFT JOIN contents c ON c.id = fv.content_id
         WHERE f.id = ?1
           AND fv.version_number = (
               SELECT MAX(newer.version_number)
               FROM file_versions newer
               WHERE newer.file_id = f.id
           )",
        [file_id],
    )?;
    Ok(())
}

fn synchronize_search_extraction(
    transaction: &Transaction<'_>,
    file_id: &str,
    extraction_result_id: &str,
    result: &ExtractionResultInput,
) -> Result<(), PersistenceError> {
    if result.error_category.as_deref() == Some("cancelled") {
        return Ok(());
    }
    ensure_search_document(transaction, file_id)?;
    let ocr_status = if result.ocr_used {
        Some("used")
    } else if result.requires_ocr && result.error_category.as_deref() == Some("ocr_unavailable") {
        Some("unavailable")
    } else {
        Some("not_used")
    };
    transaction.execute(
        "UPDATE local_search_documents
         SET extraction_result_id = ?2,
             detected_type = ?3,
             metadata_text = trim(COALESCE(extension, '') || ' ' || COALESCE(?3, '')),
             extraction_status = ?4,
             ocr_status = ?5,
             indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE file_id = ?1
           AND (
               ?4 IN ('success', 'partial')
               OR extraction_result_id IS NULL
               OR extraction_status NOT IN ('success', 'partial')
           )",
        params![
            file_id,
            extraction_result_id,
            result.detected_content_type,
            result.status,
            ocr_status,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ReviewDescriptor {
    reason: &'static str,
    severity: &'static str,
    explanation: &'static str,
    retry_available: bool,
}

fn extraction_review_descriptor(
    candidate: &ExtractionCandidate,
    result: &ExtractionResultInput,
) -> Option<ReviewDescriptor> {
    if result.error_category.as_deref() == Some("cancelled") {
        return None;
    }
    let extension = candidate
        .extension
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if result.status == "unsupported"
        && (result.detected_content_type.starts_with("video/")
            || matches!(extension.as_str(), "mp4" | "mov" | "avi" | "mkv" | "webm"))
    {
        return None;
    }
    if result.type_mismatch || result.error_category.as_deref() == Some("type_mismatch") {
        return Some(ReviewDescriptor {
            reason: "type_mismatch",
            severity: "warning",
            explanation: "Le type réel de ce fichier ne correspond pas à son extension.",
            retry_available: false,
        });
    }
    let descriptor = match result.error_category.as_deref() {
        Some("unreadable") => ReviewDescriptor {
            reason: "unreadable",
            severity: "error",
            explanation: "Ce fichier ne peut pas être lu de façon sûre.",
            retry_available: true,
        },
        Some("encrypted_document") => ReviewDescriptor {
            reason: "encrypted",
            severity: "warning",
            explanation: "Ce document est chiffré et ne peut pas être analysé sans mot de passe.",
            retry_available: false,
        },
        Some("unsupported") => ReviewDescriptor {
            reason: "unsupported_format",
            severity: "warning",
            explanation: "Ce format de fichier n’est pas encore pris en charge.",
            retry_available: false,
        },
        Some("corrupt" | "archive_traversal" | "potential_archive_bomb") => ReviewDescriptor {
            reason: "corrupt",
            severity: "error",
            explanation: "Ce fichier semble endommagé ou présente une structure non sûre.",
            retry_available: false,
        },
        Some("too_large" | "too_many_pages" | "too_many_cells" | "too_many_entries") => {
            ReviewDescriptor {
                reason: "too_large",
                severity: "warning",
                explanation: "Ce fichier dépasse une limite de sécurité de l’analyse locale.",
                retry_available: false,
            }
        }
        Some("ocr_failed") => ReviewDescriptor {
            reason: "ocr_failed",
            severity: "warning",
            explanation: "La reconnaissance locale du texte n’a pas abouti.",
            retry_available: true,
        },
        Some("ocr_unavailable") => ReviewDescriptor {
            reason: "ocr_provider_unavailable",
            severity: "warning",
            explanation: "Ce document semble contenir du texte numérisé, mais la reconnaissance locale est indisponible.",
            retry_available: true,
        },
        Some("permission_denied") => ReviewDescriptor {
            reason: "permission_denied",
            severity: "error",
            explanation: "L’application n’a pas l’autorisation de lire ce fichier.",
            retry_available: true,
        },
        _ if result.status == "partial" => ReviewDescriptor {
            reason: "partial_extraction",
            severity: "warning",
            explanation: "Une partie seulement du contenu a pu être extraite.",
            retry_available: true,
        },
        _ if result.status == "unsupported" => ReviewDescriptor {
            reason: "unsupported_format",
            severity: "warning",
            explanation: "Ce format de fichier n’est pas encore pris en charge.",
            retry_available: false,
        },
        _ if result.status == "failed" => ReviewDescriptor {
            reason: "extraction_failed",
            severity: "error",
            explanation: "L’extraction locale n’a pas pu traiter ce fichier.",
            retry_available: true,
        },
        _ => return None,
    };
    Some(descriptor)
}

fn synchronize_extraction_review(
    transaction: &Transaction<'_>,
    candidate: &ExtractionCandidate,
    extraction_result_id: &str,
    result: &ExtractionResultInput,
) -> Result<(), PersistenceError> {
    if result.error_category.as_deref() == Some("cancelled") {
        return Ok(());
    }
    transaction.execute(
        "UPDATE file_review_items
         SET status = 'resolved',
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE file_version_id = ?1
           AND status = 'needs_review'",
        [candidate.file_version_id.as_str()],
    )?;
    let Some(descriptor) = extraction_review_descriptor(candidate, result) else {
        return Ok(());
    };
    let workspace_id: String = transaction.query_row(
        "SELECT workspace_id FROM files WHERE id = ?1",
        [candidate.file_id.as_str()],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO file_review_items(
            id, workspace_id, file_id, file_version_id, extraction_result_id,
            reason, source_subsystem, severity, explanation, technical_details,
            retry_available
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, 'extraction', ?7, ?8, ?9, ?10
         )
         ON CONFLICT(file_version_id, reason) DO UPDATE SET
            extraction_result_id = excluded.extraction_result_id,
            source_subsystem = 'extraction',
            severity = excluded.severity,
            explanation = excluded.explanation,
            technical_details = excluded.technical_details,
            status = CASE
                WHEN file_review_items.status = 'ignored' THEN 'ignored'
                ELSE 'needs_review'
            END,
            retry_available = CASE
                WHEN file_review_items.retry_count >= 5 THEN 0
                ELSE excluded.retry_available
            END,
            resolved_at = NULL,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            Uuid::now_v7().to_string(),
            workspace_id,
            candidate.file_id,
            candidate.file_version_id,
            extraction_result_id,
            descriptor.reason,
            descriptor.severity,
            descriptor.explanation,
            result.error_message,
            i64::from(descriptor.retry_available),
        ],
    )?;
    Ok(())
}

fn synchronize_unfinished_extractions(
    transaction: &Transaction<'_>,
    batch_id: &str,
    final_status: &str,
) -> Result<(), PersistenceError> {
    if final_status == "cancelled" {
        return Ok(());
    }
    transaction.execute(
        "UPDATE local_search_documents
         SET extraction_result_id = (
                 SELECT cer.id
                 FROM content_extraction_results cer
                 WHERE cer.batch_id = ?1
                   AND cer.file_version_id = local_search_documents.file_version_id
             ),
             extraction_status = 'failed',
             ocr_status = 'not_used',
             indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE EXISTS (
                 SELECT 1
                 FROM content_extraction_results cer
                 WHERE cer.batch_id = ?1
                   AND cer.file_version_id = local_search_documents.file_version_id
                   AND cer.error_category = 'parser_failure'
             )
           AND (
               extraction_result_id IS NULL
               OR extraction_status NOT IN ('success', 'partial')
           )",
        [batch_id],
    )?;
    transaction.execute(
        "INSERT INTO file_review_items(
            id, workspace_id, file_id, file_version_id, extraction_result_id,
            reason, source_subsystem, severity, explanation, technical_details,
            retry_available
         )
         SELECT
            lower(hex(randomblob(16))), f.workspace_id, cer.file_id,
            cer.file_version_id, cer.id, 'extraction_failed', 'extraction',
            'error', 'L’extraction locale n’a pas pu traiter ce fichier.',
            cer.error_message, 1
         FROM content_extraction_results cer
         JOIN files f ON f.id = cer.file_id
         WHERE cer.batch_id = ?1
           AND cer.error_category = 'parser_failure'
         ON CONFLICT(file_version_id, reason) DO UPDATE SET
            extraction_result_id = excluded.extraction_result_id,
            source_subsystem = 'extraction',
            severity = excluded.severity,
            explanation = excluded.explanation,
            technical_details = excluded.technical_details,
            status = CASE
                WHEN file_review_items.status = 'ignored' THEN 'ignored'
                ELSE 'needs_review'
            END,
            retry_available = CASE
                WHEN file_review_items.retry_count >= 5 THEN 0
                ELSE 1
            END,
            resolved_at = NULL,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        [batch_id],
    )?;
    Ok(())
}

fn review_reason_filter_sql(filter: ReviewReasonFilter) -> &'static str {
    match filter {
        ReviewReasonFilter::All => "1 = 1",
        ReviewReasonFilter::Ocr => "ri.reason IN ('ocr_failed', 'ocr_provider_unavailable')",
        ReviewReasonFilter::Unsupported => "ri.reason = 'unsupported_format'",
        ReviewReasonFilter::Permissions => "ri.reason IN ('permission_denied', 'unreadable')",
        ReviewReasonFilter::Partial => "ri.reason = 'partial_extraction'",
        ReviewReasonFilter::Corrupt => "ri.reason = 'corrupt'",
        ReviewReasonFilter::Semantic => "ri.source_subsystem = 'semantic'",
    }
}

fn review_item_from_row(row: &rusqlite::Row<'_>) -> Result<ReviewItemRecord, PersistenceError> {
    Ok(ReviewItemRecord {
        review_id: row.get(0)?,
        file_id: row.get(1)?,
        filename: row.get(2)?,
        relative_path: row.get(3)?,
        reason: row.get(4)?,
        source_subsystem: row.get(5)?,
        severity: row.get(6)?,
        explanation: row.get(7)?,
        technical_details: row.get(8)?,
        status: row.get(9)?,
        retry_available: row.get::<_, i64>(10)? != 0,
        retry_count: from_sql_u64(row.get(11)?)?,
        extraction_status: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn review_item_by_id(
    connection: &Connection,
    review_id: &str,
) -> Result<ReviewItemRecord, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            ri.id, ri.file_id, fl.basename, fl.relative_path, ri.reason,
            ri.source_subsystem, ri.severity, ri.explanation, ri.technical_details,
            ri.status, ri.retry_available, ri.retry_count, cer.status,
            ri.created_at, ri.updated_at
         FROM file_review_items ri
         JOIN file_versions fv ON fv.id = ri.file_version_id
         JOIN file_locations fl ON fl.id = fv.location_id
         LEFT JOIN content_extraction_results cer ON cer.id = ri.extraction_result_id
         WHERE ri.id = ?1",
    )?;
    let mut rows = statement.query([review_id])?;
    let row = rows.next()?.ok_or(PersistenceError::NotFound)?;
    review_item_from_row(row)
}

fn review_items_for_file(
    connection: &Connection,
    file_id: &str,
) -> Result<Vec<ReviewItemRecord>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            ri.id, ri.file_id, fl.basename, fl.relative_path, ri.reason,
            ri.source_subsystem, ri.severity, ri.explanation, ri.technical_details,
            ri.status, ri.retry_available, ri.retry_count, cer.status,
            ri.created_at, ri.updated_at
         FROM file_review_items ri
         JOIN file_versions fv ON fv.id = ri.file_version_id
         JOIN file_locations fl ON fl.id = fv.location_id
         LEFT JOIN content_extraction_results cer ON cer.id = ri.extraction_result_id
         WHERE ri.file_id = ?1
         ORDER BY
            CASE ri.status
                WHEN 'needs_review' THEN 0
                WHEN 'ignored' THEN 1
                ELSE 2
            END,
            ri.updated_at DESC",
    )?;
    let mut rows = statement.query([file_id])?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        output.push(review_item_from_row(row)?);
    }
    Ok(output)
}

#[inline(never)]
fn apply_runtime_pragmas(connection: &Connection) -> Result<(), PersistenceError> {
    connection.execute_batch(
        "
        PRAGMA cipher_memory_security = ON;
        PRAGMA foreign_keys = ON;
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = FULL;
        PRAGMA busy_timeout = 5000;
        PRAGMA wal_autocheckpoint = 1000;
        PRAGMA temp_store = MEMORY;
        PRAGMA trusted_schema = OFF;
        ",
    )?;
    let cipher_version = connection
        .query_row("PRAGMA cipher_version", [], |row| row.get::<_, String>(0))
        .optional()?;
    if cipher_version.as_deref().unwrap_or_default().is_empty() {
        return Err(PersistenceError::InvalidCipher);
    }
    Ok(())
}

#[inline(never)]
fn read_user_schema_version(connection: &Connection) -> Result<i64, PersistenceError> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(Into::into)
}

#[inline(never)]
fn apply_schema_migrations(
    connection: &Connection,
    schema_version: i64,
) -> Result<(), PersistenceError> {
    match schema_version {
        0 => {
            connection.execute_batch(INITIAL_MIGRATION)?;
            connection.execute_batch(SAFE_SCANNER_MIGRATION)?;
            connection.execute_batch(SAFE_EXTRACTION_MIGRATION)?;
            connection.execute_batch(LOCAL_SEARCH_REVIEW_MIGRATION)?;
            connection.execute_batch(LOCAL_SEMANTIC_MIGRATION)?;
            connection.execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)?;
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        1 => {
            connection.execute_batch(SAFE_SCANNER_MIGRATION)?;
            connection.execute_batch(SAFE_EXTRACTION_MIGRATION)?;
            connection.execute_batch(LOCAL_SEARCH_REVIEW_MIGRATION)?;
            connection.execute_batch(LOCAL_SEMANTIC_MIGRATION)?;
            connection.execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)?;
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        2 => {
            connection.execute_batch(SAFE_EXTRACTION_MIGRATION)?;
            connection.execute_batch(LOCAL_SEARCH_REVIEW_MIGRATION)?;
            connection.execute_batch(LOCAL_SEMANTIC_MIGRATION)?;
            connection.execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)?;
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        3 => {
            connection.execute_batch(LOCAL_SEARCH_REVIEW_MIGRATION)?;
            connection.execute_batch(LOCAL_SEMANTIC_MIGRATION)?;
            connection.execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)?;
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        4 => {
            connection.execute_batch(LOCAL_SEMANTIC_MIGRATION)?;
            connection.execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)?;
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        5 => {
            connection.execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)?;
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        6 => {
            connection.execute_batch(LOCAL_ORGANIZATION_MIGRATION)?;
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        7 => {
            connection.execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)?;
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        8 => {
            connection.execute_batch(CONTINUOUS_MONITORING_MIGRATION)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        9 => {
            apply_monitoring_migration_if_missing(connection, schema_version)?;
            connection.execute_batch(LOCAL_RULES_LEARNING_MIGRATION)?;
        }
        10 => {
            apply_monitoring_migration_if_missing(connection, schema_version)?;
        }
        11 => {
            apply_monitoring_migration_if_missing(connection, schema_version)?;
        }
        12..=17 => {}
        value => return Err(PersistenceError::UnsupportedSchema(value)),
    }
    if schema_version <= 10 {
        connection.execute_batch(EXECUTION_CONSENT_MIGRATION)?;
    }
    if schema_version <= 11 {
        apply_hybrid_search_migration(connection)?;
    }
    if schema_version <= 12 {
        connection.execute_batch(MONITORING_CORRECTNESS_MIGRATION)?;
    }
    if schema_version <= 13 {
        connection.execute_batch(EXECUTION_SAFETY_POLICY_V2_MIGRATION)?;
    }
    if schema_version <= 14 {
        connection.execute_batch(CROSS_PROCESS_RECOVERY_MIGRATION)?;
    }
    if schema_version <= 15 {
        connection.execute_batch(LOCAL_ANN_SEMANTIC_INDEX_MIGRATION)?;
    }
    if schema_version <= 16 {
        connection.execute_batch(INCREMENTAL_ORGANIZATION_PROPOSALS_MIGRATION)?;
    }
    Ok(())
}

#[inline(never)]
fn apply_cipher_key(connection: &Connection, key: &DatabaseKey) -> Result<(), PersistenceError> {
    let mut hex = Zeroizing::new(String::with_capacity(64));
    for byte in key.0 {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").map_err(|_| PersistenceError::InvalidCipher)?;
    }
    let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", hex.as_str()));
    connection.execute_batch(pragma.as_str())?;
    Ok(())
}

fn persist_duplicate_groups(
    transaction: &Transaction<'_>,
    input: &ScanCompletionInput,
    _persisted_files: &[PersistedFile],
) -> Result<(), PersistenceError> {
    let root_id = input.root_id.to_string();
    let mut statement = transaction.prepare(
        "SELECT digest.digest, version.content_id, version.id
         FROM file_locations AS location
         JOIN file_versions AS version ON version.id = (
            SELECT current_version.id
            FROM file_versions AS current_version
            WHERE current_version.location_id = location.id
            ORDER BY current_version.version_number DESC, current_version.id DESC
            LIMIT 1
         )
         JOIN content_digests AS digest
           ON digest.content_id = version.content_id
          AND digest.algorithm = 'blake3'
         WHERE location.root_id = ?1
           AND location.valid_to_scan_id IS NULL
         ORDER BY digest.digest, location.normalized_relative_path_native, version.id",
    )?;
    let rows = statement.query_map([root_id.as_str()], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut candidates = HashMap::<Vec<u8>, Vec<(String, String)>>::new();
    for row in rows {
        let (digest, content_id, version_id) = row?;
        candidates
            .entry(digest)
            .or_default()
            .push((content_id, version_id));
    }
    drop(statement);
    candidates.retain(|_, members| members.len() >= 2);

    let mut existing_statement = transaction.prepare(
        "SELECT id, group_key
         FROM duplicate_groups
         WHERE workspace_id = ?1
           AND root_id = ?2
           AND method = 'exact_digest'
           AND algorithm = 'blake3'",
    )?;
    let existing_rows = existing_statement.query_map(
        params![input.workspace_id.to_string(), root_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )?;
    let mut existing = existing_rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, digest)| (digest, id))
        .collect::<HashMap<_, _>>();
    drop(existing_statement);

    for (digest, members) in candidates {
        let canonical_content_id = members[0].0.clone();
        let group_id = transaction
            .query_row(
                "SELECT id FROM duplicate_groups
                 WHERE workspace_id = ?1 AND root_id = ?2
                   AND method = 'exact_digest'
                   AND algorithm = 'blake3' AND group_key = ?3",
                params![
                    input.workspace_id.to_string(),
                    root_id.as_str(),
                    digest.as_slice()
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        transaction.execute(
            "INSERT INTO duplicate_groups(
                id, workspace_id, root_id, canonical_content_id, method,
                algorithm, group_key, confidence
             ) VALUES (?1, ?2, ?3, ?4, 'exact_digest', 'blake3', ?5, 1.0)
             ON CONFLICT(workspace_id, root_id, method, algorithm, group_key)
             DO UPDATE SET
                canonical_content_id = excluded.canonical_content_id,
                confidence = excluded.confidence,
                generated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                group_id.as_str(),
                input.workspace_id.to_string(),
                root_id.as_str(),
                canonical_content_id,
                digest.as_slice(),
            ],
        )?;
        transaction.execute(
            "DELETE FROM duplicate_group_members WHERE duplicate_group_id = ?1",
            [group_id.as_str()],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO scan_duplicate_groups(scan_id, duplicate_group_id)
             VALUES (?1, ?2)",
            params![input.scan_id.to_string(), group_id.as_str()],
        )?;
        for (index, (content_id, version_id)) in members.iter().enumerate() {
            transaction.execute(
                "INSERT OR IGNORE INTO duplicate_group_members(
                    duplicate_group_id, content_id, file_version_id, distance, is_canonical
                 ) VALUES (?1, ?2, ?3, 0.0, ?4)",
                params![
                    group_id.as_str(),
                    content_id,
                    version_id,
                    i64::from(index == 0),
                ],
            )?;
        }
        existing.remove(&digest);
    }
    for obsolete_id in existing.into_values() {
        transaction.execute(
            "DELETE FROM duplicate_groups WHERE id = ?1 AND root_id = ?2",
            params![obsolete_id, root_id.as_str()],
        )?;
    }
    Ok(())
}

fn to_sql_integer(value: usize) -> Result<i64, PersistenceError> {
    i64::try_from(value).map_err(|_| PersistenceError::NumericOverflow)
}

fn to_sql_u64(value: u64) -> Result<i64, PersistenceError> {
    i64::try_from(value).map_err(|_| PersistenceError::NumericOverflow)
}

fn from_sql_u64(value: i64) -> Result<u64, PersistenceError> {
    u64::try_from(value).map_err(|_| PersistenceError::NumericOverflow)
}

fn optional_u32(value: Option<i64>) -> Result<Option<u32>, PersistenceError> {
    value
        .map(|value| u32::try_from(value).map_err(|_| PersistenceError::NumericOverflow))
        .transpose()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn persist_observation(
    transaction: &Transaction<'_>,
    observation: &FileObservation,
    accessed_at_ns: Option<i128>,
) -> Result<PersistedFile, PersistenceError> {
    let workspace_id = observation.workspace_id.to_string();
    let root_id = observation.root_id.to_string();
    let scan_id = observation.scan_id.to_string();
    let (volume_id, case_sensitive): (String, i64) = transaction.query_row(
        "SELECT root.volume_id, volume.case_sensitive
         FROM roots AS root
         JOIN volumes AS volume ON volume.id = root.volume_id
         WHERE root.id = ?1",
        [root_id.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let existing_file: Option<String> = transaction
        .query_row(
            "SELECT file_id FROM native_identities
             WHERE volume_id = ?1
               AND identity_kind IN ('windows_file_id', 'posix_inode')
               AND identity_key = ?2
               AND valid_to_scan_id IS NULL",
            params![
                volume_id,
                observation.fingerprint.native_identity.object_key,
            ],
            |row| row.get(0),
        )
        .optional()?;
    let file_id = existing_file.unwrap_or_else(|| observation.file_id.to_string());
    transaction.execute(
        "INSERT OR IGNORE INTO files(id, workspace_id, kind, lifecycle_state)
         VALUES (?1, ?2, 'regular', 'present')",
        params![file_id, workspace_id],
    )?;
    transaction.execute(
        "UPDATE files
         SET lifecycle_state = 'present',
             last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        [file_id.as_str()],
    )?;

    let identity_kind = match observation.fingerprint.native_identity.volume.platform {
        domain::PlatformKind::Windows => "windows_file_id",
        _ => "posix_inode",
    };
    let identity_id = transaction
        .query_row(
            "SELECT id FROM native_identities
             WHERE file_id = ?1 AND volume_id = ?2 AND identity_kind = ?3
               AND valid_to_scan_id IS NULL",
            params![file_id, volume_id, identity_kind],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    transaction.execute(
        "INSERT OR IGNORE INTO native_identities(
            id, file_id, volume_id, valid_from_scan_id, identity_kind, identity_key
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            identity_id,
            file_id,
            volume_id,
            scan_id,
            identity_kind,
            observation.fingerprint.native_identity.object_key,
        ],
    )?;

    let relative_path = native_path_to_internal_text(&observation.relative_path)?;
    let normalized_path = normalize_internal_path(&relative_path);
    let relative_path_native = native_path_storage_blob(&observation.relative_path)?;
    let normalized_path_native =
        normalized_native_path_storage_blob(&observation.relative_path, case_sensitive != 0)?;
    transaction.execute(
        "UPDATE file_locations
         SET valid_to_scan_id = ?1
         WHERE root_id = ?2 AND normalized_relative_path_native = ?3
           AND valid_to_scan_id IS NULL AND file_id <> ?4",
        params![scan_id, root_id, normalized_path_native, file_id],
    )?;
    transaction.execute(
        "UPDATE file_locations
         SET valid_to_scan_id = ?1
         WHERE file_id = ?2 AND valid_to_scan_id IS NULL
           AND (root_id <> ?3 OR normalized_relative_path_native <> ?4)",
        params![scan_id, file_id, root_id, normalized_path_native],
    )?;
    let location_id = transaction
        .query_row(
            "SELECT id FROM file_locations
             WHERE file_id = ?1 AND root_id = ?2
               AND normalized_relative_path_native = ?3
               AND valid_to_scan_id IS NULL",
            params![file_id, root_id, normalized_path_native],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    transaction.execute(
        "INSERT OR IGNORE INTO file_locations(
            id, file_id, root_id, valid_from_scan_id, relative_path,
            normalized_relative_path, basename, parent_normalized_path,
            relative_path_native, normalized_relative_path_native
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            location_id,
            file_id,
            root_id,
            scan_id,
            relative_path,
            normalized_path,
            observation.display_label.as_str(),
            parent_internal_path(&normalized_path),
            relative_path_native,
            normalized_path_native,
        ],
    )?;
    transaction.execute(
        "UPDATE file_locations
         SET last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        [location_id.as_str()],
    )?;

    let content_id = if let Some(digest) = observation.fingerprint.content_digest {
        transaction
            .query_row(
                "SELECT content_id FROM content_digests
                 WHERE algorithm = 'blake3' AND digest = ?1",
                [digest.as_slice()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| Uuid::now_v7().to_string())
    } else {
        Uuid::now_v7().to_string()
    };
    transaction.execute(
        "INSERT OR IGNORE INTO contents(
            id, workspace_id, byte_size, media_type, storage_kind
         ) VALUES (?1, ?2, ?3, ?4, 'filesystem')",
        params![
            content_id,
            workspace_id,
            i64::try_from(observation.fingerprint.byte_size)
                .map_err(|_| PersistenceError::NumericOverflow)?,
            observation.detected_mime,
        ],
    )?;
    if let Some(digest) = observation.fingerprint.content_digest {
        transaction.execute(
            "INSERT OR IGNORE INTO content_digests(id, content_id, algorithm, digest)
             VALUES (?1, ?2, 'blake3', ?3)",
            params![Uuid::now_v7().to_string(), content_id, digest.as_slice()],
        )?;
    }

    let version_number: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(version_number), 0) + 1 FROM file_versions WHERE file_id = ?1",
        [file_id.as_str()],
        |row| row.get(0),
    )?;
    let version_id = observation.version_id.to_string();
    transaction.execute(
        "INSERT INTO file_versions(
            id, file_id, content_id, native_identity_id, location_id,
            observed_by_scan_id, version_number, byte_size, modified_at,
            created_at_native, hidden, attributes_json, accessed_at_native
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            version_id,
            file_id,
            content_id,
            identity_id,
            location_id,
            scan_id,
            version_number,
            i64::try_from(observation.fingerprint.byte_size)
                .map_err(|_| PersistenceError::NumericOverflow)?,
            observation
                .fingerprint
                .modified_at_ns
                .map(|value| value.to_string()),
            observation
                .fingerprint
                .created_at_ns
                .map(|value| value.to_string()),
            i64::from(observation.hidden),
            serde_json::json!({
                "readOnly": observation.read_only,
                "cloudPlaceholder": observation.cloud_placeholder,
                "encrypted": observation.encrypted,
                "raw": observation.fingerprint.attributes
            })
            .to_string(),
            accessed_at_ns.map(|value| value.to_string()),
        ],
    )?;
    transaction.execute(
        "INSERT INTO scan_observations(
            id, scan_id, file_id, native_identity_id, location_id,
            file_version_id, outcome
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'changed')",
        params![
            Uuid::now_v7().to_string(),
            scan_id,
            file_id,
            identity_id,
            location_id,
            version_id,
        ],
    )?;
    Ok(PersistedFile {
        file_id,
        file_version_id: version_id,
        location_id,
        content_id,
    })
}

fn platform_name(platform: domain::PlatformKind) -> &'static str {
    match platform {
        domain::PlatformKind::Windows => "windows",
        domain::PlatformKind::MacOs => "macos",
        domain::PlatformKind::Linux => "linux",
        domain::PlatformKind::Other => "other",
    }
}

fn native_path_to_internal_text(path: &NativePath) -> Result<String, PersistenceError> {
    match path.encoding {
        PathEncoding::UnixBytes => Ok(String::from_utf8_lossy(&path.bytes).into_owned()),
        PathEncoding::WindowsUtf16Le => {
            let mut units = Vec::with_capacity(path.bytes.len() / 2);
            let chunks = path.bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return Err(PersistenceError::InvalidNativePath);
            }
            for pair in chunks {
                units.push(u16::from_le_bytes([pair[0], pair[1]]));
            }
            Ok(String::from_utf16_lossy(&units))
        }
    }
}

fn native_path_storage_blob(path: &NativePath) -> Result<Vec<u8>, PersistenceError> {
    if path.bytes.is_empty() || path.bytes.len() > 16_384 {
        return Err(PersistenceError::InvalidNativePath);
    }
    let prefix = match path.encoding {
        PathEncoding::UnixBytes => 1,
        PathEncoding::WindowsUtf16Le => 2,
    };
    let contains_nul = match path.encoding {
        PathEncoding::UnixBytes => path.bytes.contains(&0),
        PathEncoding::WindowsUtf16Le => {
            let chunks = path.bytes.chunks_exact(2);
            !chunks.remainder().is_empty()
                || chunks
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .any(|unit| unit == 0)
        }
    };
    if contains_nul {
        return Err(PersistenceError::InvalidNativePath);
    }
    let mut stored = Vec::with_capacity(path.bytes.len().saturating_add(1));
    stored.push(prefix);
    stored.extend_from_slice(&path.bytes);
    Ok(stored)
}

fn normalized_native_path_storage_blob(
    path: &NativePath,
    case_sensitive: bool,
) -> Result<Vec<u8>, PersistenceError> {
    let normalized = match path.encoding {
        PathEncoding::UnixBytes => {
            String::from_utf8(path.bytes.clone())
                .ok()
                .map(|value| NativePath {
                    encoding: PathEncoding::UnixBytes,
                    bytes: if case_sensitive {
                        value
                            .replace('\\', "/")
                            .trim_start_matches('/')
                            .as_bytes()
                            .to_vec()
                    } else {
                        normalize_internal_path(&value).into_bytes()
                    },
                })
        }
        PathEncoding::WindowsUtf16Le => {
            let chunks = path.bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                None
            } else {
                let units = chunks
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>();
                String::from_utf16(&units).ok().map(|value| NativePath {
                    encoding: PathEncoding::WindowsUtf16Le,
                    bytes: (if case_sensitive {
                        value.replace('\\', "/").trim_start_matches('/').to_owned()
                    } else {
                        normalize_internal_path(&value)
                    })
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
                })
            }
        }
    };
    native_path_storage_blob(normalized.as_ref().unwrap_or(path))
}

fn safe_native_path_display(path: &Path, encoded: &[u8]) -> String {
    path.to_str().map_or_else(
        || {
            let mut output = String::from("native-path:");
            for byte in encoded {
                use std::fmt::Write as _;
                let _ = write!(output, "{byte:02x}");
            }
            output
        },
        ToOwned::to_owned,
    )
}

fn normalize_internal_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_lowercase()
}

fn parent_internal_path(value: &str) -> Option<&str> {
    value.rsplit_once('/').map(|(parent, _)| parent)
}

fn safe_fts_query(query: &str) -> Option<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| term.chars().count() >= 2)
        .take(12)
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn chunk_text(input: &str, max_chars: usize) -> Vec<(usize, usize, &str)> {
    if input.is_empty() {
        return vec![(0, 0, "")];
    }
    let mut chunks = Vec::new();
    let mut start_byte = 0;
    let mut char_count = 0;
    for (byte_index, character) in input.char_indices() {
        char_count += 1;
        if char_count >= max_chars && (character.is_whitespace() || character == '.') {
            let end = byte_index + character.len_utf8();
            chunks.push((start_byte, end, &input[start_byte..end]));
            start_byte = end;
            char_count = 0;
        }
    }
    if start_byte < input.len() {
        chunks.push((start_byte, input.len(), &input[start_byte..]));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_migration_is_valid_and_has_no_fk_violations() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([7; 32]));
        assert!(database.is_ok());
        let database = database.unwrap_or_else(|error| panic!("database should open: {error}"));
        let violations = database
            .foreign_key_violation_count()
            .unwrap_or_else(|error| panic!("foreign key check should succeed: {error}"));
        assert_eq!(violations, 0);
        let version: i64 = database
            .lock()
            .and_then(|connection| {
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get(0))
                    .map_err(PersistenceError::Sql)
            })
            .unwrap_or_else(|error| panic!("schema version should load: {error}"));
        assert_eq!(version, 17);
    }

    #[test]
    fn cross_process_recovery_migration_is_registered_strict_and_retention_only() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([21; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        let connection = database
            .lock()
            .unwrap_or_else(|error| panic!("database should lock: {error}"));
        let migration_name: String = connection
            .query_row(
                "SELECT name FROM schema_migrations WHERE version = 15",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("recovery migration should register: {error}"));
        assert_eq!(migration_name, "0015_cross_process_recovery_hardening");
        for table in [
            "local_executor_sessions",
            "local_executor_requests",
            "local_execution_retention",
        ] {
            let strict: i64 = connection
                .query_row(
                    "SELECT strict FROM pragma_table_list
                     WHERE schema = 'main' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("{table} metadata should load: {error}"));
            assert_eq!(strict, 1, "{table} must remain STRICT");
        }
        let retention_columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_execution_retention')
                 WHERE name IN (
                    'finalized_at', 'journal_retention_reason',
                    'rollback_retention_reason', 'minimum_retain_until',
                    'active_recovery', 'rollback_eligible',
                    'cleanup_eligible_at', 'cleanup_eligibility_reason'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("retention columns should load: {error}"));
        let state_trigger_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND name = 'local_executor_request_state_one_way'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("request trigger should load: {error}"));
        let deletion_trigger_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND tbl_name IN (
                    'local_executor_sessions',
                    'local_executor_requests',
                    'local_execution_retention'
                   )
                   AND upper(sql) LIKE '%DELETE%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("trigger SQL should load: {error}"));
        assert_eq!(retention_columns, 8);
        assert_eq!(state_trigger_count, 1);
        assert_eq!(deletion_trigger_count, 0);
    }

    #[test]
    fn executor_request_state_is_one_way_and_response_replay_is_refused() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([22; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        {
            let connection = database
                .lock()
                .unwrap_or_else(|error| panic!("database should lock: {error}"));
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     INSERT INTO local_executor_requests(
                        request_id, session_id, execution_id, operation_id, direction,
                        request_sequence, request_nonce, request_digest,
                        intent_event_sequence, intent_event_digest, state, prepared_at
                     ) VALUES (
                        lower(hex(zeroblob(32))), lower(hex(randomblob(32))),
                        '00000000-0000-4000-8000-000000000001',
                        '00000000-0000-4000-8000-000000000002',
                        'forward', 1, randomblob(32), lower(hex(randomblob(32))),
                        2, lower(hex(randomblob(32))), 'intent_durable',
                        '2026-08-11T00:00:00.000Z'
                     );",
                )
                .unwrap_or_else(|error| panic!("request fixture should insert: {error}"));
        }
        let request_id = "0".repeat(64);
        let response_digest = "1".repeat(64);
        database
            .record_executor_response(
                &request_id,
                &response_digest,
                "success",
                Some(1),
                None,
                domain::ExecutorRequestState::AcknowledgedSuccess,
                "2026-08-11T00:00:01.000Z",
            )
            .unwrap_or_else(|error| panic!("first authenticated response should record: {error}"));
        assert!(
            database
                .record_executor_response(
                    &request_id,
                    &response_digest,
                    "success",
                    Some(1),
                    None,
                    domain::ExecutorRequestState::AcknowledgedSuccess,
                    "2026-08-11T00:00:02.000Z",
                )
                .is_err(),
            "the same response must never be accepted twice"
        );
        database
            .transition_executor_request_proof(
                &request_id,
                domain::ExecutorRequestState::ProvenApplied,
                "2026-08-11T00:00:03.000Z",
            )
            .unwrap_or_else(|error| panic!("success should accept applied proof: {error}"));
        assert!(
            database
                .transition_executor_request_proof(
                    &request_id,
                    domain::ExecutorRequestState::ProvenNotStarted,
                    "2026-08-11T00:00:04.000Z",
                )
                .is_err(),
            "proof state must not move backward or contradict applied proof"
        );
        let state: String = database
            .lock()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM local_executor_requests WHERE request_id = ?1",
                        [&request_id],
                        |row| row.get(0),
                    )
                    .map_err(PersistenceError::Sql)
            })
            .unwrap_or_else(|error| panic!("request state should load: {error}"));
        assert_eq!(state, "proven_applied");
    }

    #[test]
    fn execution_consent_migration_is_registered_and_strict() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([12; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        let connection = database
            .lock()
            .unwrap_or_else(|error| panic!("database should lock: {error}"));
        let migration_name: String = connection
            .query_row(
                "SELECT name FROM schema_migrations WHERE version = 11",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("consent migration should be registered: {error}"));
        assert_eq!(migration_name, "0011_execution_consent_attestation");
        let policy_migration_name: String = connection
            .query_row(
                "SELECT name FROM schema_migrations WHERE version = 14",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("policy migration should be registered: {error}"));
        assert_eq!(policy_migration_name, "0014_execution_safety_policy_v2");
        let strict: i64 = connection
            .query_row(
                "SELECT strict FROM pragma_table_list
                 WHERE schema = 'main' AND name = 'local_execution_consents'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("consent table metadata should load: {error}"));
        assert_eq!(strict, 1);
        let required_columns = [
            "state",
            "issued_at_unix_ms",
            "expires_at_unix_ms",
            "nonce",
            "safety_policy_version",
            "safety_policy_digest",
            "maximum_rehash_bytes",
            "allow_qualified_case_only_rename",
            "destination_root_canonical",
            "destination_volume_json",
            "attestation_mac",
        ];
        for column in required_columns {
            let exists: i64 = connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM pragma_table_info('local_execution_consents')
                        WHERE name = ?1
                     )",
                    [column],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("consent column should be inspectable: {error}"));
            assert_eq!(exists, 1, "missing consent column {column}");
        }
        let maximum_rehash_default: String = connection
            .query_row(
                "SELECT dflt_value
                 FROM pragma_table_info('local_execution_consents')
                 WHERE name = 'maximum_rehash_bytes'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("rehash default should be inspectable: {error}"));
        assert_eq!(
            maximum_rehash_default,
            domain::MAX_EXECUTION_VERIFICATION_BYTES.to_string()
        );
    }

    #[test]
    fn execution_consent_migration_preserves_and_invalidates_legacy_m8_rows() {
        let connection = Connection::open_in_memory()
            .unwrap_or_else(|error| panic!("SQLite connection should open: {error}"));
        for migration in [
            INITIAL_MIGRATION,
            SAFE_SCANNER_MIGRATION,
            SAFE_EXTRACTION_MIGRATION,
            LOCAL_SEARCH_REVIEW_MIGRATION,
            LOCAL_SEMANTIC_MIGRATION,
            LOCAL_RELATIONSHIPS_MIGRATION,
            LOCAL_ORGANIZATION_MIGRATION,
            SAFETY_GATED_EXECUTION_MIGRATION,
        ] {
            connection
                .execute_batch(migration)
                .unwrap_or_else(|error| panic!("M8 fixture migration should apply: {error}"));
        }
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO volumes(
                    id, workspace_id, platform, stable_identifier, display_name,
                    filesystem_type, case_sensitive, removable
                 ) VALUES (
                    'volume-legacy', 'workspace-legacy', 'macos', 'legacy-volume',
                    'Legacy volume', 'apfs', 0, 0
                 );
                 INSERT INTO roots(
                    id, workspace_id, volume_id, added_by_principal_id,
                    absolute_path, normalized_path, display_name
                 ) VALUES (
                    'root-legacy', 'workspace-legacy', 'volume-legacy', 'principal-legacy',
                    '/legacy/root', '/legacy/root', 'Legacy root'
                 );
                 INSERT INTO local_execution_sessions(
                    id, plan_id, proposal_id, proposal_revision_id, proposal_revision,
                    workspace_id, root_id, source_scan_id, status, recovery_state,
                    plan_digest, approved_operation_count, affected_file_count,
                    folder_count, move_count, rename_count, unchanged_count,
                    conflict_count, needs_review_count, preflight_ok_count, blocked_count,
                    confirmation_phrase_required, user_confirmed, approved_at
                 ) VALUES (
                    'execution-legacy', 'plan-legacy', 'proposal-legacy', 'revision-legacy', 1,
                    'workspace-legacy', 'root-legacy', 'scan-legacy', 'approved',
                    'recovery_not_required', lower(hex(zeroblob(32))), 1, 1,
                    0, 1, 0, 0, 0, 0, 1, 0, 0, 1, 'legacy-approved'
                 );",
            )
            .unwrap_or_else(|error| panic!("legacy M8 row should insert: {error}"));

        connection
            .execute_batch(EXECUTION_CONSENT_MIGRATION)
            .unwrap_or_else(|error| panic!("consent migration should apply: {error}"));
        let session_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM local_execution_sessions
                 WHERE id = 'execution-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("legacy execution should remain: {error}"));
        let consent: (String, String, String) = connection
            .query_row(
                "SELECT state, invalidation_reason, destination_root_display
                 FROM local_execution_consents
                 WHERE execution_id = 'execution-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| panic!("legacy consent should be backfilled: {error}"));
        assert_eq!(session_count, 1);
        assert_eq!(consent.0, "invalidated");
        assert_eq!(consent.1, "legacy_m8_confirmation_not_authenticated");
        assert_eq!(consent.2, "/legacy/root");
    }

    #[test]
    fn continuous_monitoring_migration_upgrades_version_eight_directly_to_nine() {
        let connection = Connection::open_in_memory()
            .unwrap_or_else(|error| panic!("SQLite connection should open: {error}"));
        connection
            .execute_batch(INITIAL_MIGRATION)
            .unwrap_or_else(|error| panic!("initial migration should apply: {error}"));
        connection
            .execute_batch(SAFE_SCANNER_MIGRATION)
            .unwrap_or_else(|error| panic!("scanner migration should apply: {error}"));
        connection
            .execute_batch(SAFE_EXTRACTION_MIGRATION)
            .unwrap_or_else(|error| panic!("extraction migration should apply: {error}"));
        connection
            .execute_batch(LOCAL_SEARCH_REVIEW_MIGRATION)
            .unwrap_or_else(|error| panic!("search migration should apply: {error}"));
        connection
            .execute_batch(LOCAL_SEMANTIC_MIGRATION)
            .unwrap_or_else(|error| panic!("semantic migration should apply: {error}"));
        connection
            .execute_batch(LOCAL_RELATIONSHIPS_MIGRATION)
            .unwrap_or_else(|error| panic!("relationship migration should apply: {error}"));
        connection
            .execute_batch(LOCAL_ORGANIZATION_MIGRATION)
            .unwrap_or_else(|error| panic!("organization migration should apply: {error}"));
        connection
            .execute_batch(SAFETY_GATED_EXECUTION_MIGRATION)
            .unwrap_or_else(|error| panic!("execution migration should apply: {error}"));
        let version_before: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version eight should load: {error}"));
        assert_eq!(version_before, 8);

        connection
            .execute_batch(CONTINUOUS_MONITORING_MIGRATION)
            .unwrap_or_else(|error| panic!("monitoring migration should apply: {error}"));
        let version_after: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version nine should load: {error}"));
        let monitoring_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_schema
                 WHERE type = 'table' AND name IN (
                    'workspace_monitoring_state', 'root_monitoring_settings',
                    'monitoring_exclusions', 'monitoring_jobs',
                    'monitoring_activity_batches'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("monitoring tables should load: {error}"));
        let violations = connection
            .prepare("PRAGMA foreign_key_check")
            .and_then(|mut statement| {
                let mut rows = statement.query([])?;
                let mut count = 0_u64;
                while rows.next()?.is_some() {
                    count = count.saturating_add(1);
                }
                Ok(count)
            })
            .unwrap_or_else(|error| panic!("foreign key check should run: {error}"));
        assert_eq!(version_after, 9);
        assert_eq!(monitoring_tables, 5);
        assert_eq!(violations, 0);
    }

    #[test]
    fn local_rules_migration_upgrades_version_nine_and_survives_reopen() {
        let key = DatabaseKey::from_bytes([11; 32]);
        let database_path = std::env::temp_dir().join(format!(
            "supremacy-local-rules-migration-{}.sqlite3",
            Uuid::now_v7()
        ));
        let connection = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("SQLite connection should open: {error}"));
        apply_cipher_key(&connection, &key)
            .unwrap_or_else(|error| panic!("SQLCipher key should apply: {error}"));
        for migration in [
            INITIAL_MIGRATION,
            SAFE_SCANNER_MIGRATION,
            SAFE_EXTRACTION_MIGRATION,
            LOCAL_SEARCH_REVIEW_MIGRATION,
            LOCAL_SEMANTIC_MIGRATION,
            LOCAL_RELATIONSHIPS_MIGRATION,
            LOCAL_ORGANIZATION_MIGRATION,
            SAFETY_GATED_EXECUTION_MIGRATION,
            CONTINUOUS_MONITORING_MIGRATION,
        ] {
            connection
                .execute_batch(migration)
                .unwrap_or_else(|error| panic!("prerequisite migration should apply: {error}"));
        }
        let version_before: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version nine should load: {error}"));
        assert_eq!(version_before, 9);
        drop(connection);

        let database = Database::open(&database_path, &key)
            .unwrap_or_else(|error| panic!("local-rules migration should apply: {error}"));
        let connection = database
            .lock()
            .unwrap_or_else(|error| panic!("database should lock: {error}"));
        let version_after: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("current version should load: {error}"));
        let rules_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name IN (
                    'local_user_rules', 'local_learning_observations',
                    'local_rule_suggestions', 'local_rule_file_matches'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("rules tables should load: {error}"));
        let preference_columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('local_organization_preferences')
                 WHERE name IN (
                    'personal_root_name', 'business_root_name',
                    'rename_template', 'review_threshold'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("preference columns should load: {error}"));
        assert_eq!(version_after, 17);
        assert_eq!(rules_tables, 4);
        assert_eq!(preference_columns, 4);
        drop(connection);
        drop(database);
        std::fs::remove_file(&database_path)
            .unwrap_or_else(|error| panic!("migration fixture should be removable: {error}"));
    }

    #[test]
    fn hybrid_search_migration_upgrades_version_eleven_to_twelve() {
        let key = DatabaseKey::from_bytes([12; 32]);
        let database_path = std::env::temp_dir().join(format!(
            "supremacy-hybrid-search-migration-{}.sqlite3",
            Uuid::now_v7()
        ));
        let connection = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("SQLite connection should open: {error}"));
        apply_cipher_key(&connection, &key)
            .unwrap_or_else(|error| panic!("SQLCipher key should apply: {error}"));
        for migration in [
            INITIAL_MIGRATION,
            SAFE_SCANNER_MIGRATION,
            SAFE_EXTRACTION_MIGRATION,
            LOCAL_SEARCH_REVIEW_MIGRATION,
            LOCAL_SEMANTIC_MIGRATION,
            LOCAL_RELATIONSHIPS_MIGRATION,
            LOCAL_ORGANIZATION_MIGRATION,
            SAFETY_GATED_EXECUTION_MIGRATION,
            CONTINUOUS_MONITORING_MIGRATION,
            LOCAL_RULES_LEARNING_MIGRATION,
            EXECUTION_CONSENT_MIGRATION,
        ] {
            connection
                .execute_batch(migration)
                .unwrap_or_else(|error| panic!("prerequisite migration should apply: {error}"));
        }
        let version_before: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version eleven should load: {error}"));
        assert_eq!(version_before, 11);
        drop(connection);

        let database = Database::open(&database_path, &key)
            .unwrap_or_else(|error| panic!("hybrid-search migration should apply: {error}"));
        let connection = database
            .lock()
            .unwrap_or_else(|error| panic!("database should lock: {error}"));
        let version_after: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version twelve should load: {error}"));
        let hybrid_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name IN (
                    'local_embedding_models', 'local_search_embeddings',
                    'local_search_embedding_state'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("hybrid tables should load: {error}"));
        assert_eq!(version_after, 17);
        assert_eq!(hybrid_tables, 3);
        drop(connection);
        drop(database);
        std::fs::remove_file(&database_path)
            .unwrap_or_else(|error| panic!("migration fixture should be removable: {error}"));
    }

    #[test]
    fn monitoring_hardening_migrates_version_twelve_without_losing_restore_state() {
        let key = DatabaseKey::from_bytes([13; 32]);
        let database_path = std::env::temp_dir().join(format!(
            "supremacy-monitoring-hardening-migration-{}.sqlite3",
            Uuid::now_v7()
        ));
        let connection = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("SQLite connection should open: {error}"));
        apply_cipher_key(&connection, &key)
            .unwrap_or_else(|error| panic!("SQLCipher key should apply: {error}"));
        for migration in [
            INITIAL_MIGRATION,
            SAFE_SCANNER_MIGRATION,
            SAFE_EXTRACTION_MIGRATION,
            LOCAL_SEARCH_REVIEW_MIGRATION,
            LOCAL_SEMANTIC_MIGRATION,
            LOCAL_RELATIONSHIPS_MIGRATION,
            LOCAL_ORGANIZATION_MIGRATION,
            SAFETY_GATED_EXECUTION_MIGRATION,
            CONTINUOUS_MONITORING_MIGRATION,
            LOCAL_RULES_LEARNING_MIGRATION,
            EXECUTION_CONSENT_MIGRATION,
            HYBRID_SEMANTIC_SEARCH_MIGRATION,
        ] {
            connection
                .execute_batch(migration)
                .unwrap_or_else(|error| panic!("prerequisite migration should apply: {error}"));
        }
        let workspace_id = WorkspaceId::new();
        connection
            .execute(
                "INSERT INTO principals(id, kind, display_name)
                 VALUES (?1, 'human', 'Local user')",
                [LOCAL_PRINCIPAL_ID],
            )
            .unwrap_or_else(|error| panic!("existing principal should persist: {error}"));
        connection
            .execute(
                "INSERT INTO workspaces(id, name, owner_principal_id)
                 VALUES (?1, 'Existing monitoring workspace', ?2)",
                params![workspace_id.to_string(), LOCAL_PRINCIPAL_ID],
            )
            .unwrap_or_else(|error| panic!("existing workspace should persist: {error}"));
        let legacy_root_id = RootId::new();
        let legacy_volume_id = Uuid::now_v7().to_string();
        connection
            .execute(
                "INSERT INTO volumes(
                    id, workspace_id, platform, stable_identifier, display_name,
                    filesystem_type, case_sensitive, removable
                 ) VALUES (?1, ?2, 'macos', 'legacy-volume', 'Legacy volume', 'apfs', 0, 0)",
                params![legacy_volume_id, workspace_id.to_string()],
            )
            .unwrap_or_else(|error| panic!("legacy volume should persist: {error}"));
        connection
            .execute(
                "INSERT INTO roots(
                    id, workspace_id, volume_id, added_by_principal_id,
                    absolute_path, normalized_path, display_name
                 ) VALUES (?1, ?2, ?3, ?4, '/legacy/lossy-�', 'legacy/lossy-�', 'Legacy root')",
                params![
                    legacy_root_id.to_string(),
                    workspace_id.to_string(),
                    legacy_volume_id,
                    LOCAL_PRINCIPAL_ID,
                ],
            )
            .unwrap_or_else(|error| panic!("legacy root should persist: {error}"));
        connection
            .execute(
                "INSERT INTO root_monitoring_settings(
                    root_id, workspace_id, enabled, status
                 ) VALUES (?1, ?2, 1, 'active')",
                params![legacy_root_id.to_string(), workspace_id.to_string()],
            )
            .unwrap_or_else(|error| panic!("legacy monitoring state should persist: {error}"));
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap_or_else(|error| panic!("legacy fixture should disable foreign keys: {error}"));
        connection
            .execute(
                "UPDATE application_restore_state
                 SET current_workspace_id = '01900000-0000-7000-8000-ffffffffffff'
                 WHERE singleton = 1",
                [],
            )
            .unwrap_or_else(|error| panic!("invalid legacy pointer should persist: {error}"));
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap_or_else(|error| panic!("legacy fixture should restore foreign keys: {error}"));
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap_or_else(|error| panic!("version twelve should load: {error}")),
            12
        );
        drop(connection);

        let database = Database::open(&database_path, &key)
            .unwrap_or_else(|error| panic!("hardening migration should apply: {error}"));
        assert_eq!(
            database
                .restore_current_workspace()
                .unwrap_or_else(|error| panic!("restore pointer should migrate: {error}"))
                .map(|workspace| workspace.id),
            Some(workspace_id)
        );
        let connection = database
            .lock()
            .unwrap_or_else(|error| panic!("database should lock: {error}"));
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap_or_else(|error| panic!("current version should load: {error}")),
            17
        );
        let legacy_root_state: (String, i64, String, Option<String>) = connection
            .query_row(
                "SELECT root.state, settings.enabled, settings.status, settings.last_error_code
                 FROM roots AS root
                 JOIN root_monitoring_settings AS settings ON settings.root_id = root.id
                 WHERE root.id = ?1",
                [legacy_root_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap_or_else(|error| panic!("legacy root state should migrate: {error}"));
        assert_eq!(
            legacy_root_state,
            (
                "offline".to_owned(),
                0,
                "offline".to_owned(),
                Some("legacy_lossy_path_requires_reregistration".to_owned()),
            )
        );
        let migration_name: String = connection
            .query_row(
                "SELECT name FROM schema_migrations WHERE version = 13",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("hardening migration should register: {error}"));
        assert_eq!(migration_name, "0013_monitoring_correctness_hardening");
        let foreign_key_violations: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| panic!("foreign keys should validate: {error}"));
        assert_eq!(foreign_key_violations, 0);
        drop(connection);
        drop(database);
        std::fs::remove_file(&database_path)
            .unwrap_or_else(|error| panic!("migration fixture should be removable: {error}"));
    }

    #[test]
    fn semantic_migration_upgrades_an_existing_version_four_catalog() {
        let key = DatabaseKey::from_bytes([8; 32]);
        let database_path = std::env::temp_dir().join(format!(
            "supremacy-semantic-migration-{}.sqlite3",
            Uuid::now_v7()
        ));
        let connection = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("SQLite connection should open: {error}"));
        apply_cipher_key(&connection, &key)
            .unwrap_or_else(|error| panic!("SQLCipher key should apply: {error}"));
        connection
            .execute_batch(INITIAL_MIGRATION)
            .unwrap_or_else(|error| panic!("initial migration should apply: {error}"));
        connection
            .execute_batch(SAFE_SCANNER_MIGRATION)
            .unwrap_or_else(|error| panic!("scanner migration should apply: {error}"));
        connection
            .execute_batch(SAFE_EXTRACTION_MIGRATION)
            .unwrap_or_else(|error| panic!("extraction migration should apply: {error}"));
        connection
            .execute_batch(LOCAL_SEARCH_REVIEW_MIGRATION)
            .unwrap_or_else(|error| panic!("search migration should apply: {error}"));
        let version_before: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version four should be readable: {error}"));
        assert_eq!(version_before, 4);
        drop(connection);

        let database = Database::open(&database_path, &key)
            .unwrap_or_else(|error| panic!("semantic migration should apply: {error}"));
        let connection = database
            .lock()
            .unwrap_or_else(|error| panic!("database should lock: {error}"));
        let version_after: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("version twelve should be readable: {error}"));
        let semantic_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name IN (
                    'semantic_analyses', 'semantic_fields', 'semantic_entities',
                    'semantic_evidence', 'semantic_user_corrections'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("semantic tables should be readable: {error}"));
        assert_eq!(version_after, 17);
        assert_eq!(semantic_tables, 5);
        drop(connection);
        drop(database);
        std::fs::remove_file(&database_path)
            .unwrap_or_else(|error| panic!("migration fixture should be removable: {error}"));
    }

    #[test]
    fn fts_queries_are_reduced_to_quoted_terms() {
        assert_eq!(
            safe_fts_query("facture client: ACME"),
            Some("\"facture\" AND \"client\" AND \"ACME\"".to_owned())
        );
        assert_eq!(safe_fts_query("!"), None);
    }

    #[test]
    fn chunking_preserves_unicode_boundaries() {
        let source = "échéance contrat client";
        let chunks = chunk_text(source, 8);
        assert_eq!(
            chunks.iter().map(|(_, _, text)| *text).collect::<String>(),
            source
        );
    }

    #[test]
    fn catalog_deletion_removes_the_stale_fts_record() {
        use domain::{
            DisplayLabel, FileFingerprint, FileId, FileKind, FileObservation, FileVersionId,
            NativeFileIdentity, PlatformKind, VolumeIdentity,
        };

        let database = Database::open_in_memory(&DatabaseKey::from_bytes([11; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        let workspace = database
            .create_workspace("FTS cascade")
            .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
        let root_id = RootId::new();
        let volume = VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: "test-volume".to_owned(),
            filesystem_type: Some("apfs".to_owned()),
            case_sensitive: false,
            removable: false,
            local: true,
        };
        database
            .register_root(
                workspace.id,
                root_id,
                Path::new("/fixture"),
                "fixture",
                &volume,
            )
            .unwrap_or_else(|error| panic!("root should register: {error}"));
        let scan_id = ScanId::new();
        database
            .begin_scan(workspace.id, root_id, scan_id)
            .unwrap_or_else(|error| panic!("scan should begin: {error}"));
        let file_id = FileId::new();
        let version_id = FileVersionId::new();
        let observation = FileObservation {
            file_id,
            version_id,
            workspace_id: workspace.id,
            root_id,
            scan_id,
            relative_path: NativePath {
                encoding: PathEncoding::UnixBytes,
                bytes: b"stale-needle.txt".to_vec(),
            },
            display_label: DisplayLabel::new("stale-needle.txt")
                .unwrap_or_else(|error| panic!("display label should be valid: {error}")),
            kind: FileKind::Regular,
            detected_mime: Some("text/plain".to_owned()),
            fingerprint: FileFingerprint {
                native_identity: NativeFileIdentity {
                    volume,
                    object_key: vec![7; 16],
                    parent_key: vec![8; 16],
                    leaf_name: NativePath {
                        encoding: PathEncoding::UnixBytes,
                        bytes: b"stale-needle.txt".to_vec(),
                    },
                    link_count: 1,
                    reparse_tag: None,
                },
                byte_size: 12,
                modified_at_ns: Some(1),
                created_at_ns: Some(1),
                attributes: 0,
                quick_digest: None,
                content_digest: None,
            },
            read_only: false,
            hidden: false,
            cloud_placeholder: false,
            encrypted: false,
        };
        database
            .complete_scan(&ScanCompletionInput {
                scan_id,
                workspace_id: workspace.id,
                root_id,
                status: "completed".to_owned(),
                files_discovered: 1,
                directories_discovered: 1,
                bytes_discovered: 12,
                files_hashed: 0,
                errors: 0,
                skipped_items: 0,
                truncated: false,
                files: vec![ScanFileInput {
                    observation,
                    extension: Some("txt".to_owned()),
                    accessed_at_ns: None,
                    readability_status: "unreadable".to_owned(),
                    scan_status: "indexed_with_errors".to_owned(),
                    hashing_status: "not_candidate".to_owned(),
                    error_code: Some("permission_denied".to_owned()),
                }],
                issues: Vec::new(),
                duplicate_groups: Vec::new(),
            })
            .unwrap_or_else(|error| panic!("scan should complete: {error}"));
        assert_eq!(
            database
                .local_search(
                    workspace.id,
                    SearchQuery {
                        text: "stale needle".to_owned(),
                        ..SearchQuery::default()
                    },
                )
                .unwrap_or_else(|error| panic!("search should run: {error}"))
                .total,
            1
        );
        let scanner_reviews = database
            .review_items(
                workspace.id,
                ReviewStatusFilter::NeedsReview,
                ReviewReasonFilter::Permissions,
                10,
                0,
            )
            .unwrap_or_else(|error| panic!("scanner review should load: {error}"));
        assert_eq!(scanner_reviews.total, 1);
        assert_eq!(scanner_reviews.items[0].reason, "permission_denied");
        assert_eq!(scanner_reviews.items[0].source_subsystem, "scanner");
        database
            .lock()
            .and_then(|connection| {
                connection
                    .execute("DELETE FROM files WHERE id = ?1", [file_id.to_string()])
                    .map(|_| ())
                    .map_err(PersistenceError::Sql)
            })
            .unwrap_or_else(|error| panic!("catalog record should delete: {error}"));
        assert_eq!(
            database
                .local_search(
                    workspace.id,
                    SearchQuery {
                        text: "stale needle".to_owned(),
                        ..SearchQuery::default()
                    },
                )
                .unwrap_or_else(|error| panic!("search should run after delete: {error}"))
                .total,
            0
        );
        database
            .local_search_integrity_check()
            .unwrap_or_else(|error| panic!("FTS integrity should hold: {error}"));
    }
}
