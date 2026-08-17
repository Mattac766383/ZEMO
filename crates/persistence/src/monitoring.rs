use crate::{
    CatalogSnapshotRecord, CoalescedWatchEventRecord, Database, LOCAL_PRINCIPAL_ID,
    MonitoredRootRecord, MonitoringActivityInput, MonitoringActivityRecord,
    MonitoringDashboardCountsRecord, MonitoringExclusionKind, MonitoringExclusionRecord,
    MonitoringJobRecord, MonitoringJobStage, MonitoringJobStatus, MonitoringMode,
    MonitoringRootStatus, MonitoringStabilitySample, PersistenceError, RootMonitoringConfiguration,
    RootMonitoringSettingsRecord, RootRecord, ScanKind, WatchBackend, WatchCheckpointRecord,
    WatchEventInput, WatchEventKind, WatchEventRecord, WatchEventScope, WatchRegistrationRecord,
    WatchRegistrationStatus, WorkspaceMonitoringStateRecord, WorkspaceRecord, from_sql_u64,
    to_sql_u64,
};
use domain::{RootId, ScanId, WorkspaceId};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

const MAX_PATH_CHARACTERS: usize = 4_096;
const MAX_NATIVE_PATH_BYTES: usize = 16_385;
const MAX_ERROR_CODE_CHARACTERS: usize = 256;
const MAX_ERROR_MESSAGE_CHARACTERS: usize = 2_048;
const MAX_JSON_BYTES: usize = 32_768;
const MAX_BATCH_LIMIT: usize = 500;
const MAX_SIZE_THRESHOLD_BYTES: u64 = 4 * 1_024 * 1_024 * 1_024 * 1_024;
const MAX_STARTUP_ENTRY_LIMIT: u32 = 1_000_000;
const MONITORING_JOB_LEASE_MS: i64 = 60_000;

const MONITORING_JOB_COLUMNS: &str = "
    id, workspace_id, root_id, watch_registration_id, event_kind,
    path_before_native, path_after_native, coalescing_path_native, status, attempt_count,
    maximum_attempts, sample_byte_size, sample_modified_at_ns,
    stable_sample_count, debounce_ready_at_unix_ms, retry_after_unix_ms,
    last_sampled_at_unix_ms, event_count, coalesced_event_count,
    reconciliation_scan_id, last_error_code, last_error_message,
    claimed_at, claim_token, lease_expires_at_unix_ms, processing_stage,
    event_scope, completed_at, created_at, updated_at
";

impl Database {
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, name, created_at
             FROM workspaces
             WHERE archived_at IS NULL
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(WorkspaceRecord {
                id: parse_uuid_column(row.get(0)?, 0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn list_roots(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<RootRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, display_name, absolute_path, absolute_path_native
             FROM roots
             WHERE workspace_id = ?1 AND state <> 'retired'
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([workspace_id.to_string()], |row| {
            Ok(RootRecord {
                id: parse_uuid_column(row.get(0)?, 0)?,
                workspace_id,
                display_label: row.get(1)?,
                absolute_path: row.get(2)?,
                absolute_path_native: decode_native_path(&row.get::<_, Vec<u8>>(3)?)
                    .map_err(to_sql_conversion_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn latest_scan_for_root(
        &self,
        root_id: RootId,
    ) -> Result<Option<crate::ScanRecord>, PersistenceError> {
        let connection = self.lock()?;
        let scan_id = connection
            .query_row(
                "SELECT id FROM scans WHERE root_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1",
                [root_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<ScanId>())
            .transpose()?;
        drop(connection);
        scan_id.map(|scan_id| self.scan(scan_id)).transpose()
    }

    pub fn restore_current_workspace(&self) -> Result<Option<WorkspaceRecord>, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO application_restore_state(singleton) VALUES (1)",
            [],
        )?;
        transaction.execute(
            "UPDATE application_restore_state
             SET current_workspace_id = CASE
                    WHEN (
                        SELECT COUNT(*) FROM workspaces WHERE archived_at IS NULL
                    ) = 1
                    THEN (
                        SELECT id FROM workspaces WHERE archived_at IS NULL
                    )
                    ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE singleton = 1
               AND (
                    current_workspace_id IS NULL
                    OR NOT EXISTS (
                        SELECT 1 FROM workspaces
                        WHERE id = application_restore_state.current_workspace_id
                          AND archived_at IS NULL
                    )
               )",
            [],
        )?;
        let workspace = transaction
            .query_row(
                "SELECT w.id, w.name, w.created_at
                 FROM application_restore_state AS state
                 JOIN workspaces AS w ON w.id = state.current_workspace_id
                 WHERE state.singleton = 1 AND w.archived_at IS NULL",
                [],
                |row| {
                    Ok(WorkspaceRecord {
                        id: parse_uuid_column(row.get(0)?, 0)?,
                        name: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        transaction.commit()?;
        Ok(workspace)
    }

    pub fn set_current_workspace(&self, workspace_id: WorkspaceId) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE application_restore_state
             SET current_workspace_id = ?1,
                 current_root_id = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM roots
                        WHERE id = current_root_id
                          AND workspace_id = ?1
                          AND state <> 'retired'
                    )
                    THEN current_root_id
                    ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE singleton = 1
               AND EXISTS (
                    SELECT 1 FROM workspaces
                    WHERE id = ?1 AND archived_at IS NULL
               )",
            [workspace_id.to_string()],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        Ok(())
    }

    pub fn set_current_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE application_restore_state
             SET current_workspace_id = ?1,
                 current_root_id = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE singleton = 1
               AND EXISTS (
                    SELECT 1 FROM roots
                    WHERE id = ?2
                      AND workspace_id = ?1
                      AND state <> 'retired'
               )",
            params![workspace_id.to_string(), root_id.to_string()],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        Ok(())
    }

    pub fn restore_current_root(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Option<RootRecord>, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE application_restore_state
             SET current_root_id = CASE
                    WHEN (
                        SELECT COUNT(*) FROM roots
                        WHERE workspace_id = ?1 AND state <> 'retired'
                    ) = 1
                    THEN (
                        SELECT id FROM roots
                        WHERE workspace_id = ?1 AND state <> 'retired'
                    )
                    ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE singleton = 1
               AND current_workspace_id = ?1
               AND (
                    current_root_id IS NULL
                    OR NOT EXISTS (
                        SELECT 1 FROM roots
                        WHERE id = current_root_id
                          AND workspace_id = ?1
                          AND state <> 'retired'
                    )
               )",
            [workspace_id.to_string()],
        )?;
        let root = transaction
            .query_row(
                "SELECT root.id, root.display_name, root.absolute_path,
                        root.absolute_path_native
                 FROM application_restore_state AS state
                 JOIN roots AS root ON root.id = state.current_root_id
                 WHERE state.singleton = 1
                   AND state.current_workspace_id = ?1
                   AND root.workspace_id = ?1
                   AND root.state <> 'retired'",
                [workspace_id.to_string()],
                |row| {
                    Ok(RootRecord {
                        id: parse_uuid_column(row.get(0)?, 0)?,
                        workspace_id,
                        display_label: row.get(1)?,
                        absolute_path: row.get(2)?,
                        absolute_path_native: decode_native_path(&row.get::<_, Vec<u8>>(3)?)
                            .map_err(to_sql_conversion_error)?,
                    })
                },
            )
            .optional()?;
        transaction.commit()?;
        Ok(root)
    }

    pub fn clear_current_workspace(&self) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        connection.execute(
            "UPDATE application_restore_state
             SET current_workspace_id = NULL,
                 current_root_id = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE singleton = 1",
            [],
        )?;
        Ok(())
    }

    pub fn ensure_workspace_monitoring_state(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        ensure_workspace_state(&transaction, workspace_id)?;
        let state = workspace_monitoring_state_from_connection(&transaction, workspace_id)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn get_workspace_monitoring_state(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
        let connection = self.lock()?;
        workspace_monitoring_state_from_connection(&connection, workspace_id)
    }

    pub fn set_global_monitoring_pause(
        &self,
        workspace_id: WorkspaceId,
        paused: bool,
    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        ensure_workspace_state(&transaction, workspace_id)?;
        transaction.execute(
            "UPDATE workspace_monitoring_state
             SET global_paused = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1",
            params![workspace_id.to_string(), i64::from(paused)],
        )?;
        let state = workspace_monitoring_state_from_connection(&transaction, workspace_id)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn configure_root_monitoring(
        &self,
        root_id: RootId,
        configuration: RootMonitoringConfiguration,
    ) -> Result<RootMonitoringSettingsRecord, PersistenceError> {
        validate_root_configuration(configuration)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = root_workspace_id(&transaction, root_id)?;
        ensure_workspace_state(&transaction, workspace_id)?;
        transaction.execute(
            "INSERT INTO root_monitoring_settings(
                root_id, workspace_id, enabled, status,
                size_threshold_bytes, startup_entry_limit
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(root_id) DO UPDATE SET
                enabled = excluded.enabled,
                status = excluded.status,
                size_threshold_bytes = excluded.size_threshold_bytes,
                startup_entry_limit = excluded.startup_entry_limit,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                i64::from(configuration.enabled),
                configuration.status.database_name(),
                to_sql_u64(configuration.size_threshold_bytes)?,
                i64::from(configuration.startup_entry_limit),
            ],
        )?;
        let settings = root_monitoring_settings_from_connection(&transaction, root_id)?;
        transaction.commit()?;
        Ok(settings)
    }

    pub fn set_root_monitoring_enabled(
        &self,
        root_id: RootId,
        enabled: bool,
    ) -> Result<RootMonitoringSettingsRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = root_workspace_id(&transaction, root_id)?;
        ensure_workspace_state(&transaction, workspace_id)?;
        transaction.execute(
            "INSERT INTO root_monitoring_settings(
                root_id, workspace_id, enabled, status
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(root_id) DO UPDATE SET
                enabled = excluded.enabled,
                status = excluded.status,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                i64::from(enabled),
                if enabled { "active" } else { "paused" },
            ],
        )?;
        let settings = root_monitoring_settings_from_connection(&transaction, root_id)?;
        transaction.commit()?;
        Ok(settings)
    }

    pub fn set_root_monitoring_status(
        &self,
        root_id: RootId,
        status: MonitoringRootStatus,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<RootMonitoringSettingsRecord, PersistenceError> {
        validate_error(error_code, error_message)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = root_workspace_id(&transaction, root_id)?;
        ensure_workspace_state(&transaction, workspace_id)?;
        transaction.execute(
            "INSERT INTO root_monitoring_settings(
                root_id, workspace_id, enabled, status, last_error_code, last_error_message
             ) VALUES (?1, ?2, 0, ?3, ?4, ?5)
             ON CONFLICT(root_id) DO UPDATE SET
                status = excluded.status,
                last_error_code = excluded.last_error_code,
                last_error_message = excluded.last_error_message,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                status.database_name(),
                error_code,
                error_message,
            ],
        )?;
        let settings = root_monitoring_settings_from_connection(&transaction, root_id)?;
        transaction.commit()?;
        Ok(settings)
    }

    pub fn update_root_monitoring_checkpoint(
        &self,
        root_id: RootId,
        sequence_number: u64,
    ) -> Result<RootMonitoringSettingsRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = root_workspace_id(&transaction, root_id)?;
        transaction.execute(
            "INSERT INTO root_monitoring_settings(
                root_id, workspace_id, last_checkpoint_sequence, last_checkpoint_at
             ) VALUES (
                ?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )
             ON CONFLICT(root_id) DO UPDATE SET
                last_checkpoint_sequence = excluded.last_checkpoint_sequence,
                last_checkpoint_at = excluded.last_checkpoint_at,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                to_sql_u64(sequence_number)?,
            ],
        )?;
        let settings = root_monitoring_settings_from_connection(&transaction, root_id)?;
        transaction.commit()?;
        Ok(settings)
    }

    pub fn mark_root_reconciled(
        &self,
        root_id: RootId,
        scan_id: ScanId,
    ) -> Result<RootMonitoringSettingsRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = root_workspace_id(&transaction, root_id)?;
        let valid_scan: i64 = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM scans
                WHERE id = ?1 AND root_id = ?2 AND kind = 'reconciliation'
             )",
            params![scan_id.to_string(), root_id.to_string()],
            |row| row.get(0),
        )?;
        if valid_scan == 0 {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        transaction.execute(
            "INSERT INTO root_monitoring_settings(
                root_id, workspace_id, last_reconciliation_scan_id,
                last_reconciled_at, status
             ) VALUES (
                ?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'paused'
             )
             ON CONFLICT(root_id) DO UPDATE SET
                last_reconciliation_scan_id = excluded.last_reconciliation_scan_id,
                last_reconciled_at = excluded.last_reconciled_at,
                status = CASE WHEN root_monitoring_settings.enabled = 1
                              THEN 'active' ELSE 'paused' END,
                last_error_code = NULL,
                last_error_message = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                scan_id.to_string(),
            ],
        )?;
        let settings = root_monitoring_settings_from_connection(&transaction, root_id)?;
        transaction.commit()?;
        Ok(settings)
    }

    pub fn list_monitored_roots(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<MonitoredRootRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT settings.root_id, settings.workspace_id,
                    roots.display_name, roots.absolute_path,
                    roots.absolute_path_native,
                    settings.enabled, settings.status,
                    settings.size_threshold_bytes, settings.startup_entry_limit,
                    (
                        SELECT COUNT(*)
                        FROM monitoring_jobs AS job
                        WHERE job.root_id = settings.root_id
                          AND job.status IN ('pending', 'waiting', 'processing')
                    ),
                    settings.last_reconciliation_scan_id,
                    settings.last_reconciled_at,
                    settings.last_checkpoint_sequence,
                    settings.last_checkpoint_at,
                    settings.last_error_code,
                    settings.last_error_message
             FROM root_monitoring_settings AS settings
             JOIN roots ON roots.id = settings.root_id
             WHERE settings.workspace_id = ?1 AND roots.state <> 'retired'
             ORDER BY roots.created_at, roots.id",
        )?;
        let rows = statement.query_map([workspace_id.to_string()], monitored_root_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn upsert_monitoring_exclusion(
        &self,
        workspace_id: WorkspaceId,
        root_id: Option<RootId>,
        kind: MonitoringExclusionKind,
        value: &str,
        enabled: bool,
    ) -> Result<MonitoringExclusionRecord, PersistenceError> {
        let normalized_value = normalize_exclusion(kind, value)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        ensure_workspace_exists(&transaction, workspace_id)?;
        if let Some(root_id) = root_id {
            ensure_root_workspace(&transaction, workspace_id, root_id)?;
        }
        let existing_id = transaction
            .query_row(
                "SELECT id
                 FROM monitoring_exclusions
                 WHERE workspace_id = ?1
                   AND root_id IS ?2
                   AND exclusion_kind = ?3
                   AND exclusion_value = ?4",
                params![
                    workspace_id.to_string(),
                    root_id.map(|value| value.to_string()),
                    kind.database_name(),
                    normalized_value,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let id = existing_id.unwrap_or_else(|| Uuid::now_v7().to_string());
        transaction.execute(
            "INSERT INTO monitoring_exclusions(
                id, workspace_id, root_id, exclusion_kind, exclusion_value, enabled
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                enabled = excluded.enabled,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                id,
                workspace_id.to_string(),
                root_id.map(|value| value.to_string()),
                kind.database_name(),
                normalized_value,
                i64::from(enabled),
            ],
        )?;
        let record = monitoring_exclusion_from_connection(&transaction, &id)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn list_monitoring_exclusions(
        &self,
        workspace_id: WorkspaceId,
        root_id: Option<RootId>,
    ) -> Result<Vec<MonitoringExclusionRecord>, PersistenceError> {
        let connection = self.lock()?;
        let (query, root_parameter) = if let Some(root_id) = root_id {
            (
                "SELECT id, workspace_id, root_id, exclusion_kind, exclusion_value,
                        enabled, created_at, updated_at
                 FROM monitoring_exclusions
                 WHERE workspace_id = ?1 AND (root_id IS NULL OR root_id = ?2)
                 ORDER BY root_id IS NOT NULL, exclusion_kind, exclusion_value",
                Some(root_id.to_string()),
            )
        } else {
            (
                "SELECT id, workspace_id, root_id, exclusion_kind, exclusion_value,
                        enabled, created_at, updated_at
                 FROM monitoring_exclusions
                 WHERE workspace_id = ?1 AND root_id IS NULL AND ?2 IS NULL
                 ORDER BY exclusion_kind, exclusion_value",
                None,
            )
        };
        let mut statement = connection.prepare(query)?;
        let rows = statement.query_map(
            params![workspace_id.to_string(), root_parameter],
            monitoring_exclusion_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn remove_monitoring_exclusion(
        &self,
        exclusion_id: &str,
    ) -> Result<bool, PersistenceError> {
        validate_local_identifier(exclusion_id, 128)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = transaction
            .query_row(
                "SELECT workspace_id FROM monitoring_exclusions WHERE id = ?1",
                [exclusion_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(workspace_id) = workspace_id else {
            transaction.commit()?;
            return Ok(false);
        };
        ensure_workspace_state(&transaction, workspace_id.parse::<WorkspaceId>()?)?;
        let changed = transaction.execute(
            "DELETE FROM monitoring_exclusions WHERE id = ?1",
            [exclusion_id],
        )?;
        transaction.execute(
            "UPDATE workspace_monitoring_state
             SET startup_reconciliation_pending = 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1",
            [workspace_id],
        )?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn ensure_watch_registration(
        &self,
        root_id: RootId,
        backend: WatchBackend,
        recursive: bool,
    ) -> Result<WatchRegistrationRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id = root_workspace_id(&transaction, root_id)?;
        ensure_workspace_state(&transaction, workspace_id)?;
        let existing_id = transaction
            .query_row(
                "SELECT id
                 FROM watch_registrations
                 WHERE root_id = ?1
                   AND status IN ('starting', 'active', 'paused', 'overflowed')
                 LIMIT 1",
                [root_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let id = existing_id.unwrap_or_else(|| Uuid::now_v7().to_string());
        transaction.execute(
            "INSERT INTO watch_registrations(
                id, root_id, requested_by_principal_id, backend, recursive,
                status, configuration_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'starting', '{}')
             ON CONFLICT(id) DO UPDATE SET
                backend = excluded.backend,
                recursive = excluded.recursive",
            params![
                id,
                root_id.to_string(),
                LOCAL_PRINCIPAL_ID,
                backend.database_name(),
                i64::from(recursive),
            ],
        )?;
        let record = watch_registration_from_connection(&transaction, &id)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn update_watch_registration(
        &self,
        registration_id: &str,
        status: WatchRegistrationStatus,
        backend_cursor: Option<&str>,
    ) -> Result<WatchRegistrationRecord, PersistenceError> {
        validate_local_identifier(registration_id, 128)?;
        validate_optional_text(backend_cursor, 4_096)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE watch_registrations
             SET status = ?2,
                 backend_cursor = ?3,
                 started_at = CASE
                    WHEN ?2 IN ('active', 'stopped') AND started_at IS NULL
                    THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE started_at
                 END,
                 stopped_at = CASE
                    WHEN ?2 = 'stopped'
                    THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE NULL
                 END
             WHERE id = ?1",
            params![registration_id, status.database_name(), backend_cursor],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        let record = watch_registration_from_connection(&transaction, registration_id)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn list_watch_registrations(
        &self,
        root_id: RootId,
    ) -> Result<Vec<WatchRegistrationRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, root_id, backend, recursive, status, backend_cursor,
                    configuration_json, started_at, stopped_at, created_at
             FROM watch_registrations
             WHERE root_id = ?1
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = statement.query_map([root_id.to_string()], watch_registration_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn record_watch_checkpoint(
        &self,
        registration_id: &str,
        backend_cursor: &str,
        state_json: &str,
    ) -> Result<WatchCheckpointRecord, PersistenceError> {
        validate_local_identifier(registration_id, 128)?;
        validate_required_text(backend_cursor, 4_096)?;
        validate_json(state_json)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (root_id, workspace_id) = registration_root_workspace(&transaction, registration_id)?;
        let sequence_number: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence_number), -1) + 1
             FROM watch_checkpoints
             WHERE watch_registration_id = ?1",
            [registration_id],
            |row| row.get(0),
        )?;
        let id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO watch_checkpoints(
                id, watch_registration_id, sequence_number, backend_cursor, state_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                registration_id,
                sequence_number,
                backend_cursor,
                state_json,
            ],
        )?;
        transaction.execute(
            "UPDATE watch_registrations SET backend_cursor = ?2 WHERE id = ?1",
            params![registration_id, backend_cursor],
        )?;
        transaction.execute(
            "INSERT INTO root_monitoring_settings(
                root_id, workspace_id, last_checkpoint_sequence, last_checkpoint_at
             ) VALUES (
                ?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )
             ON CONFLICT(root_id) DO UPDATE SET
                last_checkpoint_sequence = excluded.last_checkpoint_sequence,
                last_checkpoint_at = excluded.last_checkpoint_at,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                root_id.to_string(),
                workspace_id.to_string(),
                sequence_number,
            ],
        )?;
        let record = watch_checkpoint_from_connection(&transaction, &id)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn latest_watch_checkpoint(
        &self,
        registration_id: &str,
    ) -> Result<Option<WatchCheckpointRecord>, PersistenceError> {
        validate_local_identifier(registration_id, 128)?;
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id, watch_registration_id, sequence_number,
                        backend_cursor, state_json, checkpointed_at
                 FROM watch_checkpoints
                 WHERE watch_registration_id = ?1
                 ORDER BY sequence_number DESC
                 LIMIT 1",
                [registration_id],
                watch_checkpoint_from_row,
            )
            .optional()
            .map_err(PersistenceError::Sql)
    }

    pub fn append_watch_event_and_coalesce(
        &self,
        input: &WatchEventInput,
    ) -> Result<CoalescedWatchEventRecord, PersistenceError> {
        validate_watch_event_input(input)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let record = append_watch_event_in_transaction(&transaction, input)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn append_watch_events_and_coalesce(
        &self,
        inputs: &[WatchEventInput],
    ) -> Result<Vec<CoalescedWatchEventRecord>, PersistenceError> {
        if inputs.len() > MAX_BATCH_LIMIT {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        for input in inputs {
            validate_watch_event_input(input)?;
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let records = inputs
            .iter()
            .map(|input| append_watch_event_in_transaction(&transaction, input))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit()?;
        Ok(records)
    }

    pub fn list_watch_events(
        &self,
        registration_id: &str,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<WatchEventRecord>, PersistenceError> {
        validate_local_identifier(registration_id, 128)?;
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, watch_registration_id, resulting_scan_id, sequence_number,
                    event_kind, path_before_native, path_after_native, native_identity_key,
                    payload_json, observed_at, event_scope
             FROM watch_events
             WHERE watch_registration_id = ?1
               AND sequence_number > ?2
             ORDER BY sequence_number
             LIMIT ?3",
        )?;
        let after_sequence = after_sequence.map(to_sql_u64).transpose()?.unwrap_or(-1);
        let rows = statement.query_map(
            params![registration_id, after_sequence, bounded_limit(limit)?,],
            watch_event_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn list_due_monitoring_jobs(
        &self,
        now_unix_ms: i64,
        limit: usize,
    ) -> Result<Vec<MonitoringJobRecord>, PersistenceError> {
        let connection = self.lock()?;
        due_monitoring_jobs(&connection, now_unix_ms, limit)
    }

    pub fn list_due_monitoring_jobs_for_workspace(
        &self,
        workspace_id: WorkspaceId,
        now_unix_ms: i64,
        limit: usize,
    ) -> Result<Vec<MonitoringJobRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT job.id
             FROM monitoring_jobs AS job
             JOIN root_monitoring_settings AS settings
               ON settings.root_id = job.root_id
             WHERE job.workspace_id = ?1
               AND settings.workspace_id = ?1
               AND settings.enabled = 1
               AND (
                    job.status IN ('pending', 'waiting')
                    OR (
                        job.status = 'processing'
                        AND job.lease_expires_at_unix_ms IS NOT NULL
                        AND job.lease_expires_at_unix_ms <= ?2
                    )
               )
               AND job.debounce_ready_at_unix_ms <= ?2
               AND (job.retry_after_unix_ms IS NULL OR job.retry_after_unix_ms <= ?2)
             ORDER BY job.debounce_ready_at_unix_ms, job.created_at, job.id
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![workspace_id.to_string(), now_unix_ms, bounded_limit(limit)?,],
            |row| row.get::<_, String>(0),
        )?;
        let ids = rows.collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| monitoring_job_from_connection(&connection, id))
            .collect()
    }

    pub fn claim_due_monitoring_jobs(
        &self,
        now_unix_ms: i64,
        limit: usize,
    ) -> Result<Vec<MonitoringJobRecord>, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let ids = due_monitoring_job_ids(&transaction, now_unix_ms, limit)?;
        let jobs = claim_monitoring_job_ids(&transaction, &ids, now_unix_ms)?;
        transaction.commit()?;
        Ok(jobs)
    }

    pub fn claim_monitoring_jobs(
        &self,
        job_ids: &[String],
        now_unix_ms: i64,
    ) -> Result<Vec<MonitoringJobRecord>, PersistenceError> {
        if job_ids.len() > MAX_BATCH_LIMIT {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        for job_id in job_ids {
            validate_local_identifier(job_id, 128)?;
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let jobs = claim_monitoring_job_ids(&transaction, job_ids, now_unix_ms)?;
        transaction.commit()?;
        Ok(jobs)
    }

    pub fn update_monitoring_job_stability_sample(
        &self,
        job_id: &str,
        claim_token: &str,
        sample: &MonitoringStabilitySample,
    ) -> Result<MonitoringJobRecord, PersistenceError> {
        validate_local_identifier(job_id, 128)?;
        validate_local_identifier(claim_token, 128)?;
        if sample.stable_sample_count > 100 {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        validate_optional_text(sample.modified_at_ns.as_deref(), 64)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE monitoring_jobs
             SET sample_byte_size = ?3,
                 sample_modified_at_ns = ?4,
                 stable_sample_count = ?5,
                 last_sampled_at_unix_ms = ?6,
                 debounce_ready_at_unix_ms = ?7,
                 status = 'waiting',
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'queued',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND status = 'processing' AND claim_token = ?2",
            params![
                job_id,
                claim_token,
                to_sql_u64(sample.byte_size)?,
                sample.modified_at_ns,
                i64::from(sample.stable_sample_count),
                sample.sampled_at_unix_ms,
                sample.next_check_at_unix_ms,
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        let job = monitoring_job_from_connection(&transaction, job_id)?;
        transaction.commit()?;
        Ok(job)
    }

    pub fn reschedule_monitoring_job(
        &self,
        job_id: &str,
        claim_token: &str,
        retry_after_unix_ms: i64,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<MonitoringJobRecord, PersistenceError> {
        validate_local_identifier(job_id, 128)?;
        validate_local_identifier(claim_token, 128)?;
        validate_error(error_code, error_message)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE monitoring_jobs
             SET attempt_count = MIN(attempt_count + 1, maximum_attempts),
                 status = CASE
                    WHEN attempt_count + 1 >= maximum_attempts THEN 'failed'
                    ELSE 'waiting'
                 END,
                 retry_after_unix_ms = ?3,
                 last_error_code = ?4,
                 last_error_message = ?5,
                 claimed_at = NULL,
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'queued',
                 completed_at = CASE
                    WHEN attempt_count + 1 >= maximum_attempts
                    THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND status = 'processing' AND claim_token = ?2",
            params![
                job_id,
                claim_token,
                retry_after_unix_ms,
                error_code,
                error_message
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        let job = monitoring_job_from_connection(&transaction, job_id)?;
        transaction.commit()?;
        Ok(job)
    }

    pub fn mark_monitoring_job_completed(
        &self,
        job_id: &str,
        claim_token: &str,
    ) -> Result<bool, PersistenceError> {
        self.finish_monitoring_job(
            job_id,
            claim_token,
            MonitoringJobStatus::Completed,
            None,
            None,
        )
    }

    pub fn mark_monitoring_job_to_review(
        &self,
        job_id: &str,
        claim_token: &str,
        reason_code: &str,
    ) -> Result<bool, PersistenceError> {
        self.finish_monitoring_job(
            job_id,
            claim_token,
            MonitoringJobStatus::ToReview,
            Some(reason_code),
            None,
        )
    }

    pub fn mark_monitoring_job_failed(
        &self,
        job_id: &str,
        claim_token: &str,
        error_code: &str,
        error_message: Option<&str>,
    ) -> Result<bool, PersistenceError> {
        self.finish_monitoring_job(
            job_id,
            claim_token,
            MonitoringJobStatus::Failed,
            Some(error_code),
            error_message,
        )
    }

    pub fn requeue_monitoring_job_after_cancellation(
        &self,
        job_id: &str,
        claim_token: &str,
        retry_after_unix_ms: i64,
    ) -> Result<bool, PersistenceError> {
        validate_local_identifier(job_id, 128)?;
        validate_local_identifier(claim_token, 128)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE monitoring_jobs
             SET status = 'waiting',
                 retry_after_unix_ms = ?3,
                 sample_byte_size = NULL,
                 sample_modified_at_ns = NULL,
                 stable_sample_count = 0,
                 last_sampled_at_unix_ms = NULL,
                 last_error_code = 'cancelled_retryable',
                 last_error_message = NULL,
                 claimed_at = NULL,
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'queued',
                 completed_at = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1
               AND status = 'processing'
               AND claim_token = ?2",
            params![job_id, claim_token, retry_after_unix_ms],
        )?;
        Ok(changed == 1)
    }

    pub fn update_monitoring_job_stage(
        &self,
        job_id: &str,
        claim_token: &str,
        stage: MonitoringJobStage,
        now_unix_ms: i64,
    ) -> Result<bool, PersistenceError> {
        validate_local_identifier(job_id, 128)?;
        validate_local_identifier(claim_token, 128)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE monitoring_jobs
             SET processing_stage = ?3,
                 lease_expires_at_unix_ms = ?4,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1
               AND claim_token = ?2
               AND status = 'processing'",
            params![
                job_id,
                claim_token,
                stage.database_name(),
                now_unix_ms.saturating_add(MONITORING_JOB_LEASE_MS),
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn recover_processing_jobs_for_workspace(
        &self,
        workspace_id: WorkspaceId,
        retry_after_unix_ms: i64,
        error_code: &str,
    ) -> Result<u64, PersistenceError> {
        validate_required_text(error_code, MAX_ERROR_CODE_CHARACTERS)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE monitoring_jobs
             SET attempt_count = MIN(attempt_count + 1, maximum_attempts),
                 status = CASE
                    WHEN attempt_count + 1 >= maximum_attempts THEN 'failed'
                    ELSE 'waiting'
                 END,
                 retry_after_unix_ms = ?2,
                 last_error_code = ?3,
                 last_error_message = CASE
                    WHEN attempt_count + 1 >= maximum_attempts
                    THEN 'Monitoring failed repeatedly before reaching a durable terminal state.'
                    ELSE NULL
                 END,
                 claimed_at = NULL,
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'queued',
                 completed_at = CASE
                    WHEN attempt_count + 1 >= maximum_attempts
                    THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND status = 'processing'",
            params![workspace_id.to_string(), retry_after_unix_ms, error_code],
        )?;
        u64::try_from(changed).map_err(|_| PersistenceError::NumericOverflow)
    }

    pub fn mark_monitoring_job_excluded(
        &self,
        job_id: &str,
        claim_token: &str,
    ) -> Result<bool, PersistenceError> {
        self.finish_monitoring_job(
            job_id,
            claim_token,
            MonitoringJobStatus::Excluded,
            None,
            None,
        )
    }

    fn finish_monitoring_job(
        &self,
        job_id: &str,
        claim_token: &str,
        status: MonitoringJobStatus,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<bool, PersistenceError> {
        validate_local_identifier(job_id, 128)?;
        validate_local_identifier(claim_token, 128)?;
        validate_error(error_code, error_message)?;
        if !matches!(
            status,
            MonitoringJobStatus::Completed
                | MonitoringJobStatus::ToReview
                | MonitoringJobStatus::Failed
                | MonitoringJobStatus::Excluded
        ) {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE monitoring_jobs
             SET status = ?3,
                 last_error_code = ?4,
                 last_error_message = ?5,
                 claimed_at = NULL,
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'finalizing',
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1
               AND status = 'processing'
               AND claim_token = ?2",
            params![
                job_id,
                claim_token,
                status.database_name(),
                error_code,
                error_message
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn link_monitoring_job_to_reconciliation_scan(
        &self,
        job_id: &str,
        claim_token: &str,
        scan_id: ScanId,
    ) -> Result<bool, PersistenceError> {
        validate_local_identifier(job_id, 128)?;
        validate_local_identifier(claim_token, 128)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE monitoring_jobs
             SET reconciliation_scan_id = ?3,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1
               AND status = 'processing'
               AND claim_token = ?2
               AND EXISTS (
                    SELECT 1 FROM scans
                    WHERE scans.id = ?3
                      AND scans.root_id = monitoring_jobs.root_id
                      AND scans.kind = 'reconciliation'
               )",
            params![job_id, claim_token, scan_id.to_string()],
        )?;
        if changed == 1 {
            transaction.execute(
                "UPDATE watch_events
                 SET resulting_scan_id = ?3
                 WHERE id IN (
                    SELECT watch_event_id
                    FROM monitoring_job_events
                    WHERE monitoring_job_id = ?1
                 )",
                params![job_id, claim_token, scan_id.to_string()],
            )?;
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn normalize_interrupted_monitoring_jobs(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<u64, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        ensure_workspace_state(&transaction, workspace_id)?;
        // A processing job can coexist with a pending/waiting job for the same
        // coalescing key (the unique index excludes `processing`). Requeueing
        // blindly would violate UNIQUE and abort app startup.
        transaction.execute(
            "UPDATE monitoring_jobs AS interrupted
             SET attempt_count = MIN(attempt_count + 1, maximum_attempts),
                 status = 'failed',
                 retry_after_unix_ms = NULL,
                 claimed_at = NULL,
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'queued',
                 last_error_code = 'interrupted_duplicate',
                 last_error_message = 'Interrupted work collided with an already queued job.',
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1
               AND status = 'processing'
               AND EXISTS (
                    SELECT 1
                    FROM monitoring_jobs AS active
                    WHERE active.root_id = interrupted.root_id
                      AND active.id <> interrupted.id
                      AND active.status IN ('pending', 'waiting')
                      AND (
                            (interrupted.coalescing_path_native IS NOT NULL
                             AND active.coalescing_path_native = interrupted.coalescing_path_native)
                         OR (interrupted.coalescing_path_native IS NULL
                             AND active.coalescing_path_native IS NULL)
                      )
               )",
            [workspace_id.to_string()],
        )?;
        let changed = transaction.execute(
            "UPDATE monitoring_jobs
             SET attempt_count = MIN(attempt_count + 1, maximum_attempts),
                 status = CASE
                    WHEN attempt_count + 1 >= maximum_attempts THEN 'failed'
                    ELSE 'pending'
                 END,
                 retry_after_unix_ms = NULL,
                 claimed_at = NULL,
                 claim_token = NULL,
                 lease_expires_at_unix_ms = NULL,
                 processing_stage = 'queued',
                 last_error_code = 'interrupted',
                 last_error_message = CASE
                    WHEN attempt_count + 1 >= maximum_attempts
                    THEN 'Monitoring stopped before a durable terminal state.'
                    ELSE NULL
                 END,
                 completed_at = CASE
                    WHEN attempt_count + 1 >= maximum_attempts
                    THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                    ELSE NULL
                 END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND status = 'processing'",
            [workspace_id.to_string()],
        )?;
        transaction.execute(
            "UPDATE workspace_monitoring_state
             SET startup_reconciliation_pending = 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1",
            [workspace_id.to_string()],
        )?;
        transaction.commit()?;
        u64::try_from(changed).map_err(|_| PersistenceError::NumericOverflow)
    }

    pub fn mark_startup_reconciliation_pending(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
        self.set_startup_reconciliation_state(workspace_id, true)
    }

    pub fn mark_startup_reconciliation_completed(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
        self.set_startup_reconciliation_state(workspace_id, false)
    }

    fn set_startup_reconciliation_state(
        &self,
        workspace_id: WorkspaceId,
        pending: bool,
    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        ensure_workspace_state(&transaction, workspace_id)?;
        transaction.execute(
            "UPDATE workspace_monitoring_state
             SET startup_reconciliation_pending = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1",
            params![workspace_id.to_string(), i64::from(pending)],
        )?;
        if !pending {
            transaction.execute(
                "UPDATE root_monitoring_settings
                 SET status = CASE WHEN enabled = 1 THEN 'active' ELSE 'paused' END,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE workspace_id = ?1 AND status = 'reconciling'",
                [workspace_id.to_string()],
            )?;
        }
        let state = workspace_monitoring_state_from_connection(&transaction, workspace_id)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn begin_scan_with_kind(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        scan_id: ScanId,
        kind: ScanKind,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "INSERT INTO scans(
                id, workspace_id, root_id, requested_by_principal_id, kind, status, started_at
             )
             SELECT ?1, ?2, ?3, ?4, ?5, 'running',
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             FROM roots
             WHERE id = ?3 AND workspace_id = ?2 AND state <> 'retired'",
            params![
                scan_id.to_string(),
                workspace_id.to_string(),
                root_id.to_string(),
                LOCAL_PRINCIPAL_ID,
                kind.database_name(),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        Ok(())
    }

    pub fn catalog_snapshot_for_root(
        &self,
        root_id: RootId,
        limit: usize,
    ) -> Result<Vec<CatalogSnapshotRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT locations.file_id, locations.relative_path_native,
                    versions.byte_size, versions.modified_at
             FROM file_locations AS locations
             JOIN file_versions AS versions ON versions.id = (
                SELECT candidate.id
                FROM file_versions AS candidate
                WHERE candidate.location_id = locations.id
                ORDER BY candidate.version_number DESC, candidate.id DESC
                LIMIT 1
             )
             WHERE locations.root_id = ?1
               AND locations.valid_to_scan_id IS NULL
             ORDER BY locations.normalized_relative_path
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![
                root_id.to_string(),
                i64::try_from(limit.clamp(1, MAX_STARTUP_ENTRY_LIMIT as usize))
                    .map_err(|_| PersistenceError::NumericOverflow)?
            ],
            |row| {
                Ok(CatalogSnapshotRecord {
                    file_id: row.get(0)?,
                    current_relative_path: decode_native_path(&row.get::<_, Vec<u8>>(1)?)
                        .map_err(|error| native_path_sql_error(1, error))?,
                    byte_size: from_sql_u64(row.get(2)?).map_err(to_sql_conversion_error)?,
                    modified_at_ns: row.get(3)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn mark_current_path_missing(
        &self,
        root_id: RootId,
        relative_path: &Path,
        reconciliation_scan_id: ScanId,
    ) -> Result<bool, PersistenceError> {
        let relative_path = normalize_relative_native_path(relative_path)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let case_sensitive: i64 = transaction.query_row(
            "SELECT volume.case_sensitive
             FROM roots AS root
             JOIN volumes AS volume ON volume.id = root.volume_id
             WHERE root.id = ?1",
            [root_id.to_string()],
            |row| row.get(0),
        )?;
        let normalized_path = monitoring_path_key(&relative_path, case_sensitive != 0);
        let normalized_path_native = encode_native_path(&normalized_path)?;
        let valid_scan: i64 = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM scans
                WHERE id = ?1 AND root_id = ?2 AND kind = 'reconciliation'
             )",
            params![reconciliation_scan_id.to_string(), root_id.to_string()],
            |row| row.get(0),
        )?;
        if valid_scan == 0 {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        let location = transaction
            .query_row(
                "SELECT id, file_id
                 FROM file_locations
                 WHERE root_id = ?1
                   AND normalized_relative_path_native = ?2
                   AND valid_to_scan_id IS NULL",
                params![root_id.to_string(), normalized_path_native],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((location_id, file_id)) = location else {
            transaction.commit()?;
            return Ok(false);
        };
        let changed = transaction.execute(
            "UPDATE file_locations
             SET valid_to_scan_id = ?2,
                 last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND valid_to_scan_id IS NULL",
            params![location_id, reconciliation_scan_id.to_string()],
        )?;
        if changed == 1 {
            transaction.execute(
                "UPDATE files
                 SET lifecycle_state = 'missing',
                     last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1
                   AND NOT EXISTS (
                        SELECT 1 FROM file_locations
                        WHERE file_id = ?1 AND valid_to_scan_id IS NULL
                   )",
                [&file_id],
            )?;
            transaction.execute(
                "UPDATE native_identities
                 SET valid_to_scan_id = ?2,
                     last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE file_id = ?1
                   AND valid_to_scan_id IS NULL
                   AND NOT EXISTS (
                        SELECT 1 FROM file_locations
                        WHERE file_id = ?1 AND valid_to_scan_id IS NULL
                   )",
                params![file_id, reconciliation_scan_id.to_string()],
            )?;
            let file_is_missing: i64 = transaction.query_row(
                "SELECT lifecycle_state = 'missing' FROM files WHERE id = ?1",
                [&file_id],
                |row| row.get(0),
            )?;
            if file_is_missing != 0 {
                transaction.execute(
                    "DELETE FROM local_search_documents WHERE file_id = ?1",
                    [&file_id],
                )?;
                transaction.execute(
                    "DELETE FROM local_search_embeddings WHERE file_id = ?1",
                    [&file_id],
                )?;
                transaction.execute(
                    "DELETE FROM local_search_embedding_state WHERE file_id = ?1",
                    [&file_id],
                )?;
                transaction.execute(
                    "DELETE FROM local_semantic_chunks WHERE file_id = ?1",
                    [&file_id],
                )?;
                transaction.execute(
                    "DELETE FROM local_rule_file_matches WHERE file_id = ?1",
                    [&file_id],
                )?;
                transaction.execute(
                    "UPDATE semantic_analyses SET is_current = 0
                     WHERE file_id = ?1 AND is_current = 1",
                    [&file_id],
                )?;
                transaction.execute(
                    "UPDATE identity_relationships
                     SET active = 0,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE source_kind = 'file'
                       AND source_file_id = ?1
                       AND active = 1",
                    [&file_id],
                )?;
            }
            transaction.execute(
                "DELETE FROM duplicate_group_members
                 WHERE duplicate_group_id IN (
                    SELECT id FROM duplicate_groups WHERE root_id = ?2
                 )
                   AND file_version_id IN (
                    SELECT id FROM file_versions WHERE file_id = ?1
                 )",
                params![file_id, root_id.to_string()],
            )?;
            transaction.execute(
                "DELETE FROM duplicate_groups
                 WHERE root_id = ?1
                   AND (
                        SELECT COUNT(*)
                        FROM duplicate_group_members
                        WHERE duplicate_group_id = duplicate_groups.id
                   ) < 2",
                [root_id.to_string()],
            )?;
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn record_monitoring_activity(
        &self,
        input: &MonitoringActivityInput,
    ) -> Result<MonitoringActivityRecord, PersistenceError> {
        validate_monitoring_activity(input)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        ensure_workspace_exists(&transaction, input.workspace_id)?;
        if let Some(root_id) = input.root_id {
            ensure_root_workspace(&transaction, input.workspace_id, root_id)?;
        }
        if let Some(scan_id) = input.reconciliation_scan_id {
            let valid_scan: i64 = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM scans
                    WHERE id = ?1
                      AND workspace_id = ?2
                      AND kind = 'reconciliation'
                      AND (?3 IS NULL OR root_id = ?3)
                 )",
                params![
                    scan_id.to_string(),
                    input.workspace_id.to_string(),
                    input.root_id.map(|value| value.to_string()),
                ],
                |row| row.get(0),
            )?;
            if valid_scan == 0 {
                return Err(PersistenceError::InvalidMonitoringInput);
            }
        }
        transaction.execute(
            "INSERT INTO monitoring_activity_batches(
                batch_id, workspace_id, root_id, files_analyzed,
                ready_to_organize, needs_review, failed, summary,
                reconciliation_scan_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                input.batch_id,
                input.workspace_id.to_string(),
                input.root_id.map(|value| value.to_string()),
                to_sql_u64(input.files_analyzed)?,
                to_sql_u64(input.ready_to_organize)?,
                to_sql_u64(input.needs_review)?,
                to_sql_u64(input.failed)?,
                input.summary,
                input.reconciliation_scan_id.map(|value| value.to_string()),
            ],
        )?;
        let activity = monitoring_activity_from_connection(&transaction, &input.batch_id)?;
        transaction.commit()?;
        Ok(activity)
    }

    pub fn list_monitoring_activity(
        &self,
        workspace_id: WorkspaceId,
        limit: usize,
    ) -> Result<Vec<MonitoringActivityRecord>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT batch_id, workspace_id, root_id, files_analyzed,
                    ready_to_organize, needs_review, failed, summary,
                    reconciliation_scan_id, created_at
             FROM monitoring_activity_batches
             WHERE workspace_id = ?1
             ORDER BY created_at DESC, batch_id DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![workspace_id.to_string(), bounded_limit(limit)?],
            monitoring_activity_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn monitoring_dashboard_counts(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MonitoringDashboardCountsRecord, PersistenceError> {
        let connection = self.lock()?;
        let (files_analyzed, ready_to_organize, needs_review): (i64, i64, i64) = connection
            .query_row(
                "SELECT COALESCE(SUM(files_analyzed), 0),
                        COALESCE(SUM(ready_to_organize), 0),
                        COALESCE(SUM(needs_review), 0)
                 FROM monitoring_activity_batches
                 WHERE workspace_id = ?1",
                [workspace_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let pending_proposals: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM local_organization_proposals
             WHERE workspace_id = ?1
               AND status IN (
                    'draft', 'ready_for_review', 'reviewed',
                    'approved_for_future_apply'
               )",
            [workspace_id.to_string()],
            |row| row.get(0),
        )?;
        let pending_jobs: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM monitoring_jobs
             WHERE workspace_id = ?1
               AND status IN ('pending', 'waiting', 'processing')",
            [workspace_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(MonitoringDashboardCountsRecord {
            files_analyzed: from_sql_u64(files_analyzed)?,
            ready_to_organize: from_sql_u64(ready_to_organize)?,
            needs_review: from_sql_u64(needs_review)?,
            pending_proposals: from_sql_u64(pending_proposals)?,
            pending_jobs: from_sql_u64(pending_jobs)?,
        })
    }
}

fn append_watch_event_in_transaction(
    transaction: &Transaction<'_>,
    input: &WatchEventInput,
) -> Result<CoalescedWatchEventRecord, PersistenceError> {
    let (root_id, workspace_id, case_sensitive) =
        registration_root_workspace_case(transaction, &input.registration_id)?;
    let mut path_before = input
        .path_before
        .as_deref()
        .map(normalize_relative_native_path)
        .transpose()?;
    let mut path_after = input
        .path_after
        .as_deref()
        .map(normalize_relative_native_path)
        .transpose()?;
    if input.kind == WatchEventKind::RescanRequired && path_before.is_none() && path_after.is_none()
    {
        path_after = Some(PathBuf::from("."));
    }
    if input.kind != WatchEventKind::Overflow && path_before.is_none() && path_after.is_none() {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    let coalescing_path = if input.kind == WatchEventKind::Overflow {
        None
    } else {
        path_after
            .as_deref()
            .or(path_before.as_deref())
            .map(|path| monitoring_path_key(path, case_sensitive))
    };
    if input.kind == WatchEventKind::Overflow {
        path_before = None;
        path_after = None;
    }
    let path_before_native = path_before.as_deref().map(encode_native_path).transpose()?;
    let path_after_native = path_after.as_deref().map(encode_native_path).transpose()?;
    let coalescing_path_native = coalescing_path
        .as_deref()
        .map(encode_native_path)
        .transpose()?;
    let path_before_display = path_before.as_deref().map(native_path_display);
    let path_after_display = path_after.as_deref().map(native_path_display);
    let coalescing_path_display = coalescing_path.as_deref().map(native_path_display);
    let sequence_number: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence_number), -1) + 1
         FROM watch_events
         WHERE watch_registration_id = ?1",
        [&input.registration_id],
        |row| row.get(0),
    )?;
    let event_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO watch_events(
            id, watch_registration_id, sequence_number, event_kind,
            path_before, path_after, path_before_native, path_after_native,
            native_identity_key, payload_json, event_scope
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event_id,
            input.registration_id,
            sequence_number,
            input.kind.database_name(),
            path_before_display,
            path_after_display,
            path_before_native,
            path_after_native,
            input.native_identity_key,
            input.payload_json,
            input.scope.database_name(),
        ],
    )?;
    let existing_job_id = transaction
        .query_row(
            "SELECT id
             FROM monitoring_jobs
             WHERE root_id = ?1
               AND coalescing_path_native IS ?2
               AND status IN ('pending', 'waiting', 'processing')
             ORDER BY CASE status
                WHEN 'pending' THEN 0
                WHEN 'waiting' THEN 1
                ELSE 2
             END
             LIMIT 1",
            params![root_id.to_string(), coalescing_path_native],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let job_id = if let Some(job_id) = existing_job_id {
        transaction.execute(
            "UPDATE monitoring_jobs
             SET event_kind = ?2,
                 event_scope = CASE
                    WHEN event_scope = 'directory' OR ?9 = 'directory' THEN 'directory'
                    WHEN event_scope = 'unknown' OR ?9 = 'unknown' THEN 'unknown'
                    ELSE 'file'
                 END,
                 path_before = COALESCE(path_before, ?3),
                 path_after = COALESCE(?4, path_after),
                 path_before_native = COALESCE(path_before_native, ?5),
                 path_after_native = COALESCE(?6, path_after_native),
                 debounce_ready_at_unix_ms = MAX(debounce_ready_at_unix_ms, ?7),
                 retry_after_unix_ms = NULL,
                 maximum_attempts = MAX(maximum_attempts, ?8),
                 sample_byte_size = NULL,
                 sample_modified_at_ns = NULL,
                 stable_sample_count = 0,
                 last_sampled_at_unix_ms = NULL,
                 event_count = event_count + 1,
                 coalesced_event_count = coalesced_event_count + 1,
                 status = 'pending',
                 processing_stage = 'queued',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            params![
                job_id,
                input.kind.database_name(),
                path_before_display,
                path_after_display,
                path_before_native,
                path_after_native,
                input.debounce_ready_at_unix_ms,
                i64::from(input.maximum_attempts),
                input.scope.database_name(),
            ],
        )?;
        job_id
    } else {
        let job_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO monitoring_jobs(
                id, workspace_id, root_id, watch_registration_id,
                event_kind, path_before, path_after, coalescing_path,
                path_before_native, path_after_native, coalescing_path_native,
                event_scope, status, maximum_attempts, debounce_ready_at_unix_ms
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                ?12, 'pending', ?13, ?14
             )",
            params![
                job_id,
                workspace_id.to_string(),
                root_id.to_string(),
                input.registration_id,
                input.kind.database_name(),
                path_before_display,
                path_after_display,
                coalescing_path_display,
                path_before_native,
                path_after_native,
                coalescing_path_native,
                input.scope.database_name(),
                i64::from(input.maximum_attempts),
                input.debounce_ready_at_unix_ms,
            ],
        )?;
        job_id
    };
    transaction.execute(
        "INSERT INTO monitoring_job_events(watch_event_id, monitoring_job_id)
         VALUES (?1, ?2)",
        params![event_id, job_id],
    )?;
    let event = watch_event_from_connection(transaction, &event_id)?;
    let job = monitoring_job_from_connection(transaction, &job_id)?;
    Ok(CoalescedWatchEventRecord { event, job })
}

fn ensure_workspace_exists(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
) -> Result<(), PersistenceError> {
    let exists: i64 = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM workspaces WHERE id = ?1 AND archived_at IS NULL
         )",
        [workspace_id.to_string()],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(PersistenceError::NotFound);
    }
    Ok(())
}

fn ensure_workspace_state(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
) -> Result<(), PersistenceError> {
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO workspace_monitoring_state(workspace_id)
         SELECT id FROM workspaces WHERE id = ?1 AND archived_at IS NULL",
        [workspace_id.to_string()],
    )?;
    if changed == 0 {
        let exists: i64 = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM workspace_monitoring_state WHERE workspace_id = ?1
             )",
            [workspace_id.to_string()],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(PersistenceError::NotFound);
        }
    }
    Ok(())
}

fn workspace_monitoring_state_from_connection(
    connection: &Connection,
    workspace_id: WorkspaceId,
) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT operational_mode, global_paused,
                    startup_reconciliation_pending, updated_at
             FROM workspace_monitoring_state
             WHERE workspace_id = ?1",
            [workspace_id.to_string()],
            |row| {
                Ok(WorkspaceMonitoringStateRecord {
                    workspace_id,
                    mode: parse_monitoring_mode(&row.get::<_, String>(0)?)
                        .map_err(to_sql_conversion_error)?,
                    paused: row.get::<_, i64>(1)? != 0,
                    startup_reconciliation_pending: row.get::<_, i64>(2)? != 0,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn root_workspace_id(
    connection: &Connection,
    root_id: RootId,
) -> Result<WorkspaceId, PersistenceError> {
    connection
        .query_row(
            "SELECT workspace_id FROM roots WHERE id = ?1 AND state <> 'retired'",
            [root_id.to_string()],
            |row| parse_uuid_column(row.get(0)?, 0),
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn ensure_root_workspace(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
    root_id: RootId,
) -> Result<(), PersistenceError> {
    let exists: i64 = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM roots
            WHERE id = ?1 AND workspace_id = ?2 AND state <> 'retired'
         )",
        params![root_id.to_string(), workspace_id.to_string()],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(PersistenceError::NotFound);
    }
    Ok(())
}

fn validate_root_configuration(
    configuration: RootMonitoringConfiguration,
) -> Result<(), PersistenceError> {
    if configuration.size_threshold_bytes == 0
        || configuration.size_threshold_bytes > MAX_SIZE_THRESHOLD_BYTES
        || configuration.startup_entry_limit == 0
        || configuration.startup_entry_limit > MAX_STARTUP_ENTRY_LIMIT
    {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn root_monitoring_settings_from_connection(
    connection: &Connection,
    root_id: RootId,
) -> Result<RootMonitoringSettingsRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT root_id, workspace_id, enabled, status,
                    size_threshold_bytes, startup_entry_limit,
                    last_reconciliation_scan_id, last_reconciled_at,
                    last_checkpoint_sequence, last_checkpoint_at,
                    last_error_code, last_error_message, updated_at
             FROM root_monitoring_settings
             WHERE root_id = ?1",
            [root_id.to_string()],
            root_monitoring_settings_from_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn root_monitoring_settings_from_row(
    row: &Row<'_>,
) -> Result<RootMonitoringSettingsRecord, rusqlite::Error> {
    Ok(RootMonitoringSettingsRecord {
        root_id: parse_uuid_column(row.get(0)?, 0)?,
        workspace_id: parse_uuid_column(row.get(1)?, 1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        status: parse_monitoring_root_status(&row.get::<_, String>(3)?)
            .map_err(to_sql_conversion_error)?,
        size_threshold_bytes: from_sql_u64(row.get(4)?).map_err(to_sql_conversion_error)?,
        startup_entry_limit: u32_from_sql(row.get(5)?, 5)?,
        last_reconciliation_scan_id: parse_optional_uuid_column(row.get(6)?, 6)?,
        last_reconciled_at: row.get(7)?,
        last_checkpoint_sequence: optional_u64_from_sql(row.get(8)?, 8)?,
        last_checkpoint_at: row.get(9)?,
        last_error_code: row.get(10)?,
        last_error_message: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn monitored_root_from_row(row: &Row<'_>) -> Result<MonitoredRootRecord, rusqlite::Error> {
    let error_code: Option<String> = row.get(14)?;
    let error_message: Option<String> = row.get(15)?;
    let last_error = match (error_code, error_message) {
        (Some(code), Some(message)) => Some(format!("{code}: {message}")),
        (Some(code), None) => Some(code),
        (None, Some(message)) => Some(message),
        (None, None) => None,
    };
    Ok(MonitoredRootRecord {
        root_id: parse_uuid_column(row.get(0)?, 0)?,
        workspace_id: parse_uuid_column(row.get(1)?, 1)?,
        display_label: row.get(2)?,
        selected_path: row.get(3)?,
        selected_path_native: decode_native_path(&row.get::<_, Vec<u8>>(4)?)
            .map_err(to_sql_conversion_error)?,
        enabled: row.get::<_, i64>(5)? != 0,
        status: parse_monitoring_root_status(&row.get::<_, String>(6)?)
            .map_err(to_sql_conversion_error)?,
        size_threshold_bytes: from_sql_u64(row.get(7)?).map_err(to_sql_conversion_error)?,
        startup_entry_limit: u32_from_sql(row.get(8)?, 8)?,
        pending_jobs: from_sql_u64(row.get(9)?).map_err(to_sql_conversion_error)?,
        last_reconciliation_scan_id: parse_optional_uuid_column(row.get(10)?, 10)?,
        last_reconciled_at: row.get(11)?,
        last_checkpoint_sequence: optional_u64_from_sql(row.get(12)?, 12)?,
        last_checkpoint_at: row.get(13)?,
        last_error,
    })
}

fn normalize_exclusion(
    kind: MonitoringExclusionKind,
    value: &str,
) -> Result<String, PersistenceError> {
    match kind {
        MonitoringExclusionKind::PathPrefix => {
            normalize_relative_input_path(value).map(|value| value.to_lowercase())
        }
        MonitoringExclusionKind::Extension => {
            let value = value.trim().trim_start_matches('.').to_lowercase();
            if value.is_empty() || value.chars().count() > 64 || value.contains(['/', '\\', '\0']) {
                return Err(PersistenceError::InvalidMonitoringInput);
            }
            Ok(value)
        }
    }
}

fn monitoring_exclusion_from_connection(
    connection: &Connection,
    id: &str,
) -> Result<MonitoringExclusionRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT id, workspace_id, root_id, exclusion_kind, exclusion_value,
                    enabled, created_at, updated_at
             FROM monitoring_exclusions
             WHERE id = ?1",
            [id],
            monitoring_exclusion_from_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn monitoring_exclusion_from_row(
    row: &Row<'_>,
) -> Result<MonitoringExclusionRecord, rusqlite::Error> {
    Ok(MonitoringExclusionRecord {
        id: row.get(0)?,
        workspace_id: parse_uuid_column(row.get(1)?, 1)?,
        root_id: parse_optional_uuid_column(row.get(2)?, 2)?,
        kind: parse_exclusion_kind(&row.get::<_, String>(3)?).map_err(to_sql_conversion_error)?,
        value: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn watch_registration_from_connection(
    connection: &Connection,
    registration_id: &str,
) -> Result<WatchRegistrationRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT id, root_id, backend, recursive, status, backend_cursor,
                    configuration_json, started_at, stopped_at, created_at
             FROM watch_registrations
             WHERE id = ?1",
            [registration_id],
            watch_registration_from_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn watch_registration_from_row(row: &Row<'_>) -> Result<WatchRegistrationRecord, rusqlite::Error> {
    Ok(WatchRegistrationRecord {
        id: row.get(0)?,
        root_id: parse_uuid_column(row.get(1)?, 1)?,
        backend: parse_watch_backend(&row.get::<_, String>(2)?).map_err(to_sql_conversion_error)?,
        recursive: row.get::<_, i64>(3)? != 0,
        status: parse_watch_registration_status(&row.get::<_, String>(4)?)
            .map_err(to_sql_conversion_error)?,
        backend_cursor: row.get(5)?,
        configuration_json: row.get(6)?,
        started_at: row.get(7)?,
        stopped_at: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn registration_root_workspace(
    connection: &Connection,
    registration_id: &str,
) -> Result<(RootId, WorkspaceId), PersistenceError> {
    connection
        .query_row(
            "SELECT roots.id, roots.workspace_id
             FROM watch_registrations
             JOIN roots ON roots.id = watch_registrations.root_id
             WHERE watch_registrations.id = ?1",
            [registration_id],
            |row| {
                Ok((
                    parse_uuid_column(row.get(0)?, 0)?,
                    parse_uuid_column(row.get(1)?, 1)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn registration_root_workspace_case(
    connection: &Connection,
    registration_id: &str,
) -> Result<(RootId, WorkspaceId, bool), PersistenceError> {
    connection
        .query_row(
            "SELECT roots.id, roots.workspace_id, volumes.case_sensitive
             FROM watch_registrations
             JOIN roots ON roots.id = watch_registrations.root_id
             JOIN volumes ON volumes.id = roots.volume_id
             WHERE watch_registrations.id = ?1
               AND watch_registrations.status NOT IN ('failed', 'stopped')",
            [registration_id],
            |row| {
                Ok((
                    parse_uuid_column(row.get(0)?, 0)?,
                    parse_uuid_column(row.get(1)?, 1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn watch_checkpoint_from_connection(
    connection: &Connection,
    checkpoint_id: &str,
) -> Result<WatchCheckpointRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT id, watch_registration_id, sequence_number,
                    backend_cursor, state_json, checkpointed_at
             FROM watch_checkpoints
             WHERE id = ?1",
            [checkpoint_id],
            watch_checkpoint_from_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn watch_checkpoint_from_row(row: &Row<'_>) -> Result<WatchCheckpointRecord, rusqlite::Error> {
    Ok(WatchCheckpointRecord {
        id: row.get(0)?,
        registration_id: row.get(1)?,
        sequence_number: from_sql_u64(row.get(2)?).map_err(to_sql_conversion_error)?,
        backend_cursor: row.get(3)?,
        state_json: row.get(4)?,
        checkpointed_at: row.get(5)?,
    })
}

fn validate_watch_event_input(input: &WatchEventInput) -> Result<(), PersistenceError> {
    validate_local_identifier(&input.registration_id, 128)?;
    validate_json(&input.payload_json)?;
    if input.maximum_attempts == 0 || input.maximum_attempts > 20 {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    if input
        .native_identity_key
        .as_ref()
        .is_some_and(|key| key.len() > 4_096)
    {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn watch_event_from_connection(
    connection: &Connection,
    event_id: &str,
) -> Result<WatchEventRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT id, watch_registration_id, resulting_scan_id, sequence_number,
                    event_kind, path_before_native, path_after_native, native_identity_key,
                    payload_json, observed_at, event_scope
             FROM watch_events
             WHERE id = ?1",
            [event_id],
            watch_event_from_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn watch_event_from_row(row: &Row<'_>) -> Result<WatchEventRecord, rusqlite::Error> {
    Ok(WatchEventRecord {
        id: row.get(0)?,
        registration_id: row.get(1)?,
        resulting_scan_id: parse_optional_uuid_column(row.get(2)?, 2)?,
        sequence_number: from_sql_u64(row.get(3)?).map_err(to_sql_conversion_error)?,
        kind: parse_watch_event_kind(&row.get::<_, String>(4)?).map_err(to_sql_conversion_error)?,
        scope: parse_watch_event_scope(&row.get::<_, String>(10)?)
            .map_err(to_sql_conversion_error)?,
        path_before: decode_optional_native_path(row.get(5)?, 5)?,
        path_after: decode_optional_native_path(row.get(6)?, 6)?,
        native_identity_key: row.get(7)?,
        payload_json: row.get(8)?,
        observed_at: row.get(9)?,
    })
}

fn due_monitoring_jobs(
    connection: &Connection,
    now_unix_ms: i64,
    limit: usize,
) -> Result<Vec<MonitoringJobRecord>, PersistenceError> {
    let ids = due_monitoring_job_ids(connection, now_unix_ms, limit)?;
    ids.iter()
        .map(|id| monitoring_job_from_connection(connection, id))
        .collect()
}

fn due_monitoring_job_ids(
    connection: &Connection,
    now_unix_ms: i64,
    limit: usize,
) -> Result<Vec<String>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT id
         FROM monitoring_jobs
         WHERE (
                status IN ('pending', 'waiting')
                OR (
                    status = 'processing'
                    AND lease_expires_at_unix_ms IS NOT NULL
                    AND lease_expires_at_unix_ms <= ?1
                )
         )
           AND debounce_ready_at_unix_ms <= ?1
           AND (retry_after_unix_ms IS NULL OR retry_after_unix_ms <= ?1)
         ORDER BY debounce_ready_at_unix_ms, created_at, id
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![now_unix_ms, bounded_limit(limit)?], |row| {
        row.get::<_, String>(0)
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(PersistenceError::Sql)
}

fn claim_monitoring_job_ids(
    transaction: &Transaction<'_>,
    ids: &[String],
    now_unix_ms: i64,
) -> Result<Vec<MonitoringJobRecord>, PersistenceError> {
    let mut claimed = Vec::with_capacity(ids.len());
    for id in ids {
        let claim_token = Uuid::now_v7().to_string();
        let changed = transaction.execute(
            "UPDATE monitoring_jobs
             SET status = 'processing',
                 claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 claim_token = ?2,
                 lease_expires_at_unix_ms = ?3,
                 processing_stage = 'stability',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1
               AND (
                    status IN ('pending', 'waiting')
                    OR (
                        status = 'processing'
                        AND lease_expires_at_unix_ms IS NOT NULL
                        AND lease_expires_at_unix_ms <= ?4
                    )
               )",
            params![
                id,
                claim_token,
                now_unix_ms.saturating_add(MONITORING_JOB_LEASE_MS),
                now_unix_ms,
            ],
        )?;
        if changed == 1 {
            claimed.push(monitoring_job_from_connection(transaction, id)?);
        }
    }
    Ok(claimed)
}

fn monitoring_job_from_connection(
    connection: &Connection,
    job_id: &str,
) -> Result<MonitoringJobRecord, PersistenceError> {
    let query = format!(
        "SELECT {MONITORING_JOB_COLUMNS}
         FROM monitoring_jobs
         WHERE id = ?1"
    );
    connection
        .query_row(&query, [job_id], monitoring_job_from_row)
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn monitoring_job_from_row(row: &Row<'_>) -> Result<MonitoringJobRecord, rusqlite::Error> {
    Ok(MonitoringJobRecord {
        id: row.get(0)?,
        workspace_id: parse_uuid_column(row.get(1)?, 1)?,
        root_id: parse_uuid_column(row.get(2)?, 2)?,
        watch_registration_id: row.get(3)?,
        event_kind: parse_watch_event_kind(&row.get::<_, String>(4)?)
            .map_err(to_sql_conversion_error)?,
        path_before: decode_optional_native_path(row.get(5)?, 5)?,
        path_after: decode_optional_native_path(row.get(6)?, 6)?,
        coalescing_path: decode_optional_native_path(row.get(7)?, 7)?,
        status: parse_monitoring_job_status(&row.get::<_, String>(8)?)
            .map_err(to_sql_conversion_error)?,
        attempt_count: u32_from_sql(row.get(9)?, 9)?,
        maximum_attempts: u32_from_sql(row.get(10)?, 10)?,
        sample_byte_size: optional_u64_from_sql(row.get(11)?, 11)?,
        sample_modified_at_ns: row.get(12)?,
        stable_sample_count: u32_from_sql(row.get(13)?, 13)?,
        debounce_ready_at_unix_ms: row.get(14)?,
        retry_after_unix_ms: row.get(15)?,
        last_sampled_at_unix_ms: row.get(16)?,
        event_count: from_sql_u64(row.get(17)?).map_err(to_sql_conversion_error)?,
        coalesced_event_count: from_sql_u64(row.get(18)?).map_err(to_sql_conversion_error)?,
        reconciliation_scan_id: parse_optional_uuid_column(row.get(19)?, 19)?,
        last_error_code: row.get(20)?,
        last_error_message: row.get(21)?,
        claimed_at: row.get(22)?,
        claim_token: row.get(23)?,
        lease_expires_at_unix_ms: row.get(24)?,
        processing_stage: parse_monitoring_job_stage(&row.get::<_, String>(25)?)
            .map_err(to_sql_conversion_error)?,
        event_scope: parse_watch_event_scope(&row.get::<_, String>(26)?)
            .map_err(to_sql_conversion_error)?,
        completed_at: row.get(27)?,
        created_at: row.get(28)?,
        updated_at: row.get(29)?,
    })
}

fn validate_monitoring_activity(input: &MonitoringActivityInput) -> Result<(), PersistenceError> {
    validate_local_identifier(&input.batch_id, 128)?;
    validate_required_text(&input.summary, MAX_ERROR_MESSAGE_CHARACTERS)?;
    if input.ready_to_organize > input.files_analyzed
        || input.needs_review > input.files_analyzed
        || input.failed > input.files_analyzed
    {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn monitoring_activity_from_connection(
    connection: &Connection,
    batch_id: &str,
) -> Result<MonitoringActivityRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT batch_id, workspace_id, root_id, files_analyzed,
                    ready_to_organize, needs_review, failed, summary,
                    reconciliation_scan_id, created_at
             FROM monitoring_activity_batches
             WHERE batch_id = ?1",
            [batch_id],
            monitoring_activity_from_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn monitoring_activity_from_row(
    row: &Row<'_>,
) -> Result<MonitoringActivityRecord, rusqlite::Error> {
    Ok(MonitoringActivityRecord {
        id: row.get(0)?,
        workspace_id: parse_uuid_column(row.get(1)?, 1)?,
        root_id: parse_optional_uuid_column(row.get(2)?, 2)?,
        files_analyzed: from_sql_u64(row.get(3)?).map_err(to_sql_conversion_error)?,
        ready_to_organize: from_sql_u64(row.get(4)?).map_err(to_sql_conversion_error)?,
        needs_review: from_sql_u64(row.get(5)?).map_err(to_sql_conversion_error)?,
        failed: from_sql_u64(row.get(6)?).map_err(to_sql_conversion_error)?,
        summary: row.get(7)?,
        reconciliation_scan_id: parse_optional_uuid_column(row.get(8)?, 8)?,
        created_at: row.get(9)?,
    })
}

fn normalize_relative_native_path(value: &Path) -> Result<PathBuf, PersistenceError> {
    let mut normalized = PathBuf::new();
    for component in value.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir if value == Path::new(".") => return Ok(PathBuf::from(".")),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PersistenceError::InvalidMonitoringInput);
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    let encoded = encode_native_path(&normalized)?;
    if encoded.len() > MAX_NATIVE_PATH_BYTES {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(normalized)
}

pub(crate) fn encode_native_path(path: &Path) -> Result<Vec<u8>, PersistenceError> {
    let mut output = Vec::new();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let bytes = path.as_os_str().as_bytes();
        if bytes.contains(&0) {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        output.reserve(bytes.len().saturating_add(1));
        output.push(1);
        output.extend_from_slice(bytes);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if units.contains(&0) {
            return Err(PersistenceError::InvalidMonitoringInput);
        }
        output.reserve(units.len().saturating_mul(2).saturating_add(1));
        output.push(2);
        output.extend(units.into_iter().flat_map(u16::to_le_bytes));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let text = path
            .to_str()
            .ok_or(PersistenceError::InvalidMonitoringInput)?;
        output.reserve(text.len().saturating_add(1));
        output.push(0);
        output.extend_from_slice(text.as_bytes());
    }
    if output.len() > MAX_NATIVE_PATH_BYTES {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(output)
}

fn decode_optional_native_path(
    value: Option<Vec<u8>>,
    column: usize,
) -> Result<Option<PathBuf>, rusqlite::Error> {
    value
        .map(|value| {
            decode_native_path(&value).map_err(|error| native_path_sql_error(column, error))
        })
        .transpose()
}

pub(crate) fn decode_native_path(value: &[u8]) -> Result<PathBuf, PersistenceError> {
    let Some((&encoding, bytes)) = value.split_first() else {
        return Err(PersistenceError::InvalidNativePath);
    };
    let path = match encoding {
        0 => PathBuf::from(
            std::str::from_utf8(bytes).map_err(|_| PersistenceError::InvalidNativePath)?,
        ),
        1 => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt as _;
                PathBuf::from(OsString::from_vec(bytes.to_vec()))
            }
            #[cfg(not(unix))]
            {
                PathBuf::from(
                    std::str::from_utf8(bytes).map_err(|_| PersistenceError::InvalidNativePath)?,
                )
            }
        }
        2 => {
            let chunks = bytes.chunks_exact(2);
            if !chunks.remainder().is_empty() {
                return Err(PersistenceError::InvalidNativePath);
            }
            let units = chunks
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt as _;
                PathBuf::from(OsString::from_wide(&units))
            }
            #[cfg(not(windows))]
            {
                PathBuf::from(
                    String::from_utf16(&units).map_err(|_| PersistenceError::InvalidNativePath)?,
                )
            }
        }
        _ => return Err(PersistenceError::InvalidNativePath),
    };
    Ok(path)
}

fn native_path_display(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .chars()
        .take(MAX_PATH_CHARACTERS)
        .collect()
}

fn normalize_relative_input_path(value: &str) -> Result<String, PersistenceError> {
    let normalized = value.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.chars().count() > MAX_PATH_CHARACTERS
        || normalized.contains('\0')
        || normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
        || normalized
            .split('/')
            .any(|segment| segment == ".." || segment.is_empty())
    {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(normalized.trim_start_matches("./").to_owned())
}

pub(crate) fn monitoring_path_key(path: &Path, case_sensitive: bool) -> PathBuf {
    if case_sensitive {
        path.to_path_buf()
    } else if let Some(path) = path.to_str() {
        PathBuf::from(path.to_lowercase())
    } else {
        path.to_path_buf()
    }
}

fn native_path_sql_error(column: usize, error: PersistenceError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Blob, Box::new(error))
}

fn validate_error(
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), PersistenceError> {
    if let Some(error_code) = error_code {
        validate_required_text(error_code, MAX_ERROR_CODE_CHARACTERS)?;
    }
    validate_optional_text(error_message, MAX_ERROR_MESSAGE_CHARACTERS)
}

fn validate_local_identifier(value: &str, maximum: usize) -> Result<(), PersistenceError> {
    validate_required_text(value, maximum)?;
    if value.contains('\0') {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn validate_required_text(value: &str, maximum: usize) -> Result<(), PersistenceError> {
    let length = value.chars().count();
    if length == 0 || length > maximum {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, maximum: usize) -> Result<(), PersistenceError> {
    if value.is_some_and(|value| value.chars().count() > maximum) {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn validate_json(value: &str) -> Result<(), PersistenceError> {
    if value.len() > MAX_JSON_BYTES || serde_json::from_str::<serde_json::Value>(value).is_err() {
        return Err(PersistenceError::InvalidMonitoringInput);
    }
    Ok(())
}

fn bounded_limit(limit: usize) -> Result<i64, PersistenceError> {
    i64::try_from(limit.clamp(1, MAX_BATCH_LIMIT)).map_err(|_| PersistenceError::NumericOverflow)
}

fn parse_monitoring_mode(value: &str) -> Result<MonitoringMode, PersistenceError> {
    match value {
        "PRUDENT" => Ok(MonitoringMode::Prudent),
        "AUTOMATIC" => Ok(MonitoringMode::Automatic),
        "RULES" => Ok(MonitoringMode::Rules),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_monitoring_root_status(value: &str) -> Result<MonitoringRootStatus, PersistenceError> {
    match value {
        "starting" => Ok(MonitoringRootStatus::Starting),
        "active" => Ok(MonitoringRootStatus::Active),
        "paused" => Ok(MonitoringRootStatus::Paused),
        "reconciling" => Ok(MonitoringRootStatus::Reconciling),
        "overflowed" => Ok(MonitoringRootStatus::Overflowed),
        "offline" => Ok(MonitoringRootStatus::Offline),
        "failed" => Ok(MonitoringRootStatus::Failed),
        "stopped" => Ok(MonitoringRootStatus::Stopped),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_exclusion_kind(value: &str) -> Result<MonitoringExclusionKind, PersistenceError> {
    match value {
        "path_prefix" => Ok(MonitoringExclusionKind::PathPrefix),
        "extension" => Ok(MonitoringExclusionKind::Extension),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_watch_backend(value: &str) -> Result<WatchBackend, PersistenceError> {
    match value {
        "fsevents" => Ok(WatchBackend::Fsevents),
        "read_directory_changes" => Ok(WatchBackend::ReadDirectoryChanges),
        "inotify" => Ok(WatchBackend::Inotify),
        "polling" => Ok(WatchBackend::Polling),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_watch_registration_status(
    value: &str,
) -> Result<WatchRegistrationStatus, PersistenceError> {
    match value {
        "starting" => Ok(WatchRegistrationStatus::Starting),
        "active" => Ok(WatchRegistrationStatus::Active),
        "paused" => Ok(WatchRegistrationStatus::Paused),
        "overflowed" => Ok(WatchRegistrationStatus::Overflowed),
        "failed" => Ok(WatchRegistrationStatus::Failed),
        "stopped" => Ok(WatchRegistrationStatus::Stopped),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_watch_event_kind(value: &str) -> Result<WatchEventKind, PersistenceError> {
    match value {
        "created" => Ok(WatchEventKind::Created),
        "modified" => Ok(WatchEventKind::Modified),
        "moved" => Ok(WatchEventKind::Moved),
        "removed" => Ok(WatchEventKind::Removed),
        "metadata" => Ok(WatchEventKind::Metadata),
        "overflow" => Ok(WatchEventKind::Overflow),
        "rescan_required" => Ok(WatchEventKind::RescanRequired),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_monitoring_job_status(value: &str) -> Result<MonitoringJobStatus, PersistenceError> {
    match value {
        "pending" => Ok(MonitoringJobStatus::Pending),
        "waiting" => Ok(MonitoringJobStatus::Waiting),
        "processing" => Ok(MonitoringJobStatus::Processing),
        "completed" => Ok(MonitoringJobStatus::Completed),
        "to_review" => Ok(MonitoringJobStatus::ToReview),
        "failed" => Ok(MonitoringJobStatus::Failed),
        "cancelled" => Ok(MonitoringJobStatus::Cancelled),
        "excluded" => Ok(MonitoringJobStatus::Excluded),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_monitoring_job_stage(value: &str) -> Result<MonitoringJobStage, PersistenceError> {
    match value {
        "queued" => Ok(MonitoringJobStage::Queued),
        "stability" => Ok(MonitoringJobStage::Stability),
        "catalog" => Ok(MonitoringJobStage::Catalog),
        "content" => Ok(MonitoringJobStage::Content),
        "semantic" => Ok(MonitoringJobStage::Semantic),
        "relationships" => Ok(MonitoringJobStage::Relationships),
        "proposal" => Ok(MonitoringJobStage::Proposal),
        "search" => Ok(MonitoringJobStage::Search),
        "finalizing" => Ok(MonitoringJobStage::Finalizing),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_watch_event_scope(value: &str) -> Result<WatchEventScope, PersistenceError> {
    match value {
        "file" => Ok(WatchEventScope::File),
        "directory" => Ok(WatchEventScope::Directory),
        "unknown" => Ok(WatchEventScope::Unknown),
        _ => Err(PersistenceError::InvalidMonitoringInput),
    }
}

fn parse_uuid_column<T>(value: String, column: usize) -> Result<T, rusqlite::Error>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value
        .parse()
        .map_err(|error| to_sql_conversion_boxed(column, error))
}

fn parse_optional_uuid_column<T>(
    value: Option<String>,
    column: usize,
) -> Result<Option<T>, rusqlite::Error>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value
        .map(|value| parse_uuid_column(value, column))
        .transpose()
}

fn optional_u64_from_sql(
    value: Option<i64>,
    column: usize,
) -> Result<Option<u64>, rusqlite::Error> {
    value
        .map(|value| from_sql_u64(value).map_err(|error| to_sql_conversion_boxed(column, error)))
        .transpose()
}

fn u32_from_sql(value: i64, column: usize) -> Result<u32, rusqlite::Error> {
    u32::try_from(value).map_err(|error| to_sql_conversion_boxed(column, error))
}

fn to_sql_conversion_error(error: PersistenceError) -> rusqlite::Error {
    to_sql_conversion_boxed(0, error)
}

fn to_sql_conversion_boxed(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseKey, MonitoringActivityInput};
    use domain::{
        DisplayLabel, FileFingerprint, FileId, FileKind, FileObservation, FileVersionId,
        NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity,
    };
    use std::path::{Path, PathBuf};

    fn native_path_from_relative(path: &Path) -> NativePath {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            NativePath {
                encoding: PathEncoding::WindowsUtf16Le,
                bytes: path
                    .as_os_str()
                    .encode_wide()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            }
        }
        #[cfg(not(windows))]
        {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt as _;
                NativePath {
                    encoding: PathEncoding::UnixBytes,
                    bytes: path.as_os_str().as_bytes().to_vec(),
                }
            }
            #[cfg(not(unix))]
            {
                NativePath {
                    encoding: PathEncoding::UnixBytes,
                    bytes: path.to_string_lossy().as_bytes().to_vec(),
                }
            }
        }
    }

    struct MonitoringFixture {
        database: Database,
        workspace_id: WorkspaceId,
        root_id: RootId,
        volume: VolumeIdentity,
    }

    fn monitoring_fixture(seed: u8) -> MonitoringFixture {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([seed; 32]))
            .unwrap_or_else(|error| panic!("monitoring database should open: {error}"));
        let workspace = database
            .create_workspace("Monitoring")
            .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
        let root_id = RootId::new();
        let volume = VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: format!("monitoring-volume-{seed}"),
            filesystem_type: Some("apfs".to_owned()),
            case_sensitive: false,
            removable: false,
            local: true,
        };
        database
            .register_root(
                workspace.id,
                root_id,
                Path::new("/encrypted/selected-root"),
                "Selected root",
                &volume,
            )
            .unwrap_or_else(|error| panic!("root should register: {error}"));
        MonitoringFixture {
            database,
            workspace_id: workspace.id,
            root_id,
            volume,
        }
    }

    fn append_event(
        fixture: &MonitoringFixture,
        registration_id: &str,
        kind: WatchEventKind,
        ready_at: i64,
    ) -> CoalescedWatchEventRecord {
        fixture
            .database
            .append_watch_event_and_coalesce(&WatchEventInput {
                registration_id: registration_id.to_owned(),
                kind,
                scope: WatchEventScope::File,
                path_before: None,
                path_after: Some(PathBuf::from("Inbox/Invoice.pdf")),
                native_identity_key: None,
                payload_json: "{}".to_owned(),
                debounce_ready_at_unix_ms: ready_at,
                maximum_attempts: 5,
            })
            .unwrap_or_else(|error| panic!("watch event should append: {error}"))
    }

    fn enabled_registration(fixture: &MonitoringFixture) -> WatchRegistrationRecord {
        fixture
            .database
            .configure_root_monitoring(
                fixture.root_id,
                RootMonitoringConfiguration {
                    enabled: true,
                    status: MonitoringRootStatus::Active,
                    ..RootMonitoringConfiguration::default()
                },
            )
            .unwrap_or_else(|error| panic!("root should configure: {error}"));
        fixture
            .database
            .ensure_watch_registration(fixture.root_id, WatchBackend::Fsevents, true)
            .unwrap_or_else(|error| panic!("watch should register: {error}"))
    }

    #[test]
    fn workspace_restoration_settings_and_exclusions_are_durable_records() {
        let fixture = monitoring_fixture(31);
        assert_eq!(
            fixture
                .database
                .restore_current_workspace()
                .unwrap_or_else(|error| panic!("restoration backfill should load: {error}"))
                .map(|workspace| workspace.id),
            Some(fixture.workspace_id)
        );
        fixture
            .database
            .set_current_workspace(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("workspace should become current: {error}"));
        let restored = fixture
            .database
            .restore_current_workspace()
            .unwrap_or_else(|error| panic!("workspace should restore: {error}"))
            .unwrap_or_else(|| panic!("current workspace should exist"));
        assert_eq!(restored.id, fixture.workspace_id);
        assert_eq!(
            fixture
                .database
                .list_workspaces()
                .unwrap_or_else(|error| panic!("workspaces should list: {error}"))
                .len(),
            1
        );
        assert_eq!(
            fixture
                .database
                .list_roots(fixture.workspace_id)
                .unwrap_or_else(|error| panic!("roots should list: {error}"))
                .len(),
            1
        );

        let state = fixture
            .database
            .ensure_workspace_monitoring_state(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("monitoring state should exist: {error}"));
        assert_eq!(state.mode, MonitoringMode::Prudent);
        assert!(state.startup_reconciliation_pending);
        assert!(!state.paused);
        let configured = fixture
            .database
            .configure_root_monitoring(
                fixture.root_id,
                RootMonitoringConfiguration {
                    enabled: true,
                    status: MonitoringRootStatus::Active,
                    ..RootMonitoringConfiguration::default()
                },
            )
            .unwrap_or_else(|error| panic!("root should configure: {error}"));
        assert!(configured.enabled);

        let exclusion = fixture
            .database
            .upsert_monitoring_exclusion(
                fixture.workspace_id,
                Some(fixture.root_id),
                MonitoringExclusionKind::Extension,
                ".TMP",
                true,
            )
            .unwrap_or_else(|error| panic!("exclusion should persist: {error}"));
        assert_eq!(exclusion.value, "tmp");
        let exclusions = fixture
            .database
            .list_monitoring_exclusions(fixture.workspace_id, Some(fixture.root_id))
            .unwrap_or_else(|error| panic!("exclusions should list: {error}"));
        assert_eq!(exclusions, vec![exclusion.clone()]);
        let workspace_exclusion = fixture
            .database
            .upsert_monitoring_exclusion(
                fixture.workspace_id,
                None,
                MonitoringExclusionKind::PathPrefix,
                "Generated",
                true,
            )
            .unwrap_or_else(|error| panic!("workspace exclusion should persist: {error}"));
        assert_eq!(
            fixture
                .database
                .list_monitoring_exclusions(fixture.workspace_id, None)
                .unwrap_or_else(|error| panic!("workspace exclusions should list: {error}")),
            vec![workspace_exclusion]
        );
        assert!(
            fixture
                .database
                .remove_monitoring_exclusion(&exclusion.id)
                .unwrap_or_else(|error| panic!("exclusion should remove: {error}"))
        );
    }

    #[test]
    fn durable_watch_events_coalesce_and_link_to_one_reconciliation_scan() {
        let fixture = monitoring_fixture(32);
        let registration = fixture
            .database
            .ensure_watch_registration(fixture.root_id, WatchBackend::Fsevents, true)
            .unwrap_or_else(|error| panic!("watch should register: {error}"));
        let first = append_event(&fixture, &registration.id, WatchEventKind::Created, 100);
        let second = append_event(&fixture, &registration.id, WatchEventKind::Modified, 200);
        assert_eq!(first.event.sequence_number, 0);
        assert_eq!(second.event.sequence_number, 1);
        assert_eq!(first.job.id, second.job.id);
        assert_eq!(second.job.event_count, 2);
        assert_eq!(second.job.coalesced_event_count, 1);
        assert_eq!(second.job.debounce_ready_at_unix_ms, 200);
        assert!(
            fixture
                .database
                .list_due_monitoring_jobs(199, 10)
                .unwrap_or_else(|error| panic!("jobs should list: {error}"))
                .is_empty()
        );
        let claimed = fixture
            .database
            .claim_due_monitoring_jobs(200, 10)
            .unwrap_or_else(|error| panic!("job should claim: {error}"));
        assert_eq!(claimed.len(), 1);
        let claim_token = claimed[0]
            .claim_token
            .as_deref()
            .unwrap_or_else(|| panic!("claim token should exist"));

        let scan_id = ScanId::new();
        fixture
            .database
            .begin_scan_with_kind(
                fixture.workspace_id,
                fixture.root_id,
                scan_id,
                ScanKind::Reconciliation,
            )
            .unwrap_or_else(|error| panic!("reconciliation scan should begin: {error}"));
        assert!(
            fixture
                .database
                .link_monitoring_job_to_reconciliation_scan(&second.job.id, claim_token, scan_id,)
                .unwrap_or_else(|error| panic!("job should link to scan: {error}"))
        );
        let events = fixture
            .database
            .list_watch_events(&registration.id, None, 10)
            .unwrap_or_else(|error| panic!("events should list: {error}"));
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|event| event.resulting_scan_id == Some(scan_id))
        );
    }

    #[test]
    fn due_monitoring_jobs_are_scoped_to_enabled_roots_in_one_workspace() {
        let fixture = monitoring_fixture(39);
        fixture
            .database
            .configure_root_monitoring(
                fixture.root_id,
                RootMonitoringConfiguration {
                    enabled: true,
                    status: MonitoringRootStatus::Active,
                    ..RootMonitoringConfiguration::default()
                },
            )
            .unwrap_or_else(|error| panic!("first root should configure: {error}"));
        let first_registration = fixture
            .database
            .ensure_watch_registration(fixture.root_id, WatchBackend::Fsevents, true)
            .unwrap_or_else(|error| panic!("first watch should register: {error}"));
        let first = append_event(
            &fixture,
            &first_registration.id,
            WatchEventKind::Created,
            10,
        );

        let other_workspace = fixture
            .database
            .create_workspace("Other monitoring workspace")
            .unwrap_or_else(|error| panic!("other workspace should exist: {error}"));
        let other_root_id = RootId::new();
        let mut other_volume = fixture.volume.clone();
        other_volume.stable_identifier = "other-monitoring-volume".to_owned();
        fixture
            .database
            .register_root(
                other_workspace.id,
                other_root_id,
                Path::new("/encrypted/other-selected-root"),
                "Other selected root",
                &other_volume,
            )
            .unwrap_or_else(|error| panic!("other root should register: {error}"));
        fixture
            .database
            .configure_root_monitoring(
                other_root_id,
                RootMonitoringConfiguration {
                    enabled: true,
                    status: MonitoringRootStatus::Active,
                    ..RootMonitoringConfiguration::default()
                },
            )
            .unwrap_or_else(|error| panic!("other root should configure: {error}"));
        let other_registration = fixture
            .database
            .ensure_watch_registration(other_root_id, WatchBackend::Fsevents, true)
            .unwrap_or_else(|error| panic!("other watch should register: {error}"));
        let other = fixture
            .database
            .append_watch_event_and_coalesce(&WatchEventInput {
                registration_id: other_registration.id,
                kind: WatchEventKind::Created,
                scope: WatchEventScope::File,
                path_before: None,
                path_after: Some(PathBuf::from("Other.txt")),
                native_identity_key: None,
                payload_json: "{}".to_owned(),
                debounce_ready_at_unix_ms: 10,
                maximum_attempts: 5,
            })
            .unwrap_or_else(|error| panic!("other event should persist: {error}"));

        let first_due = fixture
            .database
            .list_due_monitoring_jobs_for_workspace(fixture.workspace_id, 10, 1)
            .unwrap_or_else(|error| panic!("first workspace jobs should list: {error}"));
        assert_eq!(first_due.len(), 1);
        assert_eq!(first_due[0].id, first.job.id);
        let other_due = fixture
            .database
            .list_due_monitoring_jobs_for_workspace(other_workspace.id, 10, 1)
            .unwrap_or_else(|error| panic!("other workspace jobs should list: {error}"));
        assert_eq!(other_due.len(), 1);
        assert_eq!(other_due[0].id, other.job.id);

        fixture
            .database
            .set_root_monitoring_enabled(fixture.root_id, false)
            .unwrap_or_else(|error| panic!("first root should disable: {error}"));
        assert!(
            fixture
                .database
                .list_due_monitoring_jobs_for_workspace(fixture.workspace_id, 10, 10)
                .unwrap_or_else(|error| panic!("disabled-root jobs should list: {error}"))
                .is_empty()
        );
    }

    #[test]
    fn startup_normalization_requeues_interrupted_monitoring_work() {
        let fixture = monitoring_fixture(33);
        let registration = fixture
            .database
            .ensure_watch_registration(fixture.root_id, WatchBackend::Polling, true)
            .unwrap_or_else(|error| panic!("watch should register: {error}"));
        let receipt = append_event(&fixture, &registration.id, WatchEventKind::Created, 10);
        let claimed = fixture
            .database
            .claim_due_monitoring_jobs(10, 10)
            .unwrap_or_else(|error| panic!("job should claim: {error}"));
        assert_eq!(claimed[0].status, MonitoringJobStatus::Processing);
        assert_eq!(
            fixture
                .database
                .normalize_interrupted_monitoring_jobs(fixture.workspace_id)
                .unwrap_or_else(|error| panic!("interrupted work should normalize: {error}")),
            1
        );
        let due = fixture
            .database
            .list_due_monitoring_jobs(10, 10)
            .unwrap_or_else(|error| panic!("normalized work should list: {error}"));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, receipt.job.id);
        assert_eq!(due[0].status, MonitoringJobStatus::Pending);
        assert_eq!(due[0].attempt_count, 1);
        assert_eq!(due[0].last_error_code.as_deref(), Some("interrupted"));
        assert!(
            fixture
                .database
                .get_workspace_monitoring_state(fixture.workspace_id)
                .unwrap_or_else(|error| panic!("monitoring state should load: {error}"))
                .startup_reconciliation_pending
        );
    }

    #[test]
    fn startup_normalization_survives_pending_and_processing_path_collision() {
        let fixture = monitoring_fixture(34);
        let registration = fixture
            .database
            .ensure_watch_registration(fixture.root_id, WatchBackend::Polling, true)
            .unwrap_or_else(|error| panic!("watch should register: {error}"));
        let first = append_event(&fixture, &registration.id, WatchEventKind::Created, 10);
        fixture
            .database
            .claim_due_monitoring_jobs(10, 10)
            .unwrap_or_else(|error| panic!("job should claim: {error}"));
        let second = append_event(&fixture, &registration.id, WatchEventKind::Modified, 11);
        assert_eq!(
            second.job.id, first.job.id,
            "in-flight work for the same path should coalesce instead of inserting a second active job"
        );
        fixture
            .database
            .normalize_interrupted_monitoring_jobs(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("collision must not fail restore: {error}"));
    }

    #[test]
    fn reconciliation_can_mark_a_current_catalog_path_missing() {
        let fixture = monitoring_fixture(34);
        let initial_scan_id = ScanId::new();
        let file_id = FileId::new();
        let relative = PathBuf::from("Inbox").join("removed.txt");
        let observation = FileObservation {
            file_id,
            version_id: FileVersionId::new(),
            workspace_id: fixture.workspace_id,
            root_id: fixture.root_id,
            scan_id: initial_scan_id,
            relative_path: native_path_from_relative(&relative),
            display_label: DisplayLabel::new("removed.txt")
                .unwrap_or_else(|error| panic!("label should be valid: {error}")),
            kind: FileKind::Regular,
            detected_mime: Some("text/plain".to_owned()),
            fingerprint: FileFingerprint {
                native_identity: NativeFileIdentity {
                    volume: fixture.volume.clone(),
                    object_key: vec![1; 16],
                    parent_key: vec![2; 16],
                    leaf_name: native_path_from_relative(Path::new("removed.txt")),
                    link_count: 1,
                    reparse_tag: None,
                },
                byte_size: 42,
                modified_at_ns: Some(123),
                created_at_ns: Some(100),
                attributes: 0,
                quick_digest: None,
                content_digest: None,
            },
            read_only: false,
            hidden: false,
            cloud_placeholder: false,
            encrypted: false,
        };
        fixture
            .database
            .persist_scan_detailed(
                fixture.workspace_id,
                fixture.root_id,
                initial_scan_id,
                &[observation],
                0,
            )
            .unwrap_or_else(|error| panic!("initial catalog should persist: {error}"));
        assert_eq!(
            fixture
                .database
                .catalog_snapshot_for_root(fixture.root_id, 10)
                .unwrap_or_else(|error| panic!("snapshot should load: {error}"))
                .len(),
            1
        );

        let reconciliation_scan_id = ScanId::new();
        fixture
            .database
            .begin_scan_with_kind(
                fixture.workspace_id,
                fixture.root_id,
                reconciliation_scan_id,
                ScanKind::Reconciliation,
            )
            .unwrap_or_else(|error| panic!("reconciliation should begin: {error}"));
        assert!(
            fixture
                .database
                .mark_current_path_missing(fixture.root_id, &relative, reconciliation_scan_id,)
                .unwrap_or_else(|error| panic!("path should become missing: {error}"))
        );
        assert!(
            fixture
                .database
                .catalog_snapshot_for_root(fixture.root_id, 10)
                .unwrap_or_else(|error| panic!("snapshot should reload: {error}"))
                .is_empty()
        );
        let lifecycle: String = fixture
            .database
            .lock()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT lifecycle_state FROM files WHERE id = ?1",
                        [file_id.to_string()],
                        |row| row.get(0),
                    )
                    .map_err(PersistenceError::Sql)
            })
            .unwrap_or_else(|error| panic!("lifecycle should load: {error}"));
        assert_eq!(lifecycle, "missing");
    }

    #[test]
    fn restoration_never_guesses_between_multiple_workspaces_or_roots() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([40; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        assert!(
            database
                .restore_current_workspace()
                .unwrap_or_else(|error| panic!("fresh restore should load: {error}"))
                .is_none()
        );

        let first = database
            .create_workspace("First")
            .unwrap_or_else(|error| panic!("first workspace should exist: {error}"));
        database
            .clear_current_workspace()
            .unwrap_or_else(|error| panic!("pointer should clear: {error}"));
        assert_eq!(
            database
                .restore_current_workspace()
                .unwrap_or_else(|error| panic!("single workspace should backfill: {error}"))
                .map(|workspace| workspace.id),
            Some(first.id)
        );

        let _second = database
            .create_workspace("Second")
            .unwrap_or_else(|error| panic!("second workspace should exist: {error}"));
        database
            .clear_current_workspace()
            .unwrap_or_else(|error| panic!("pointer should clear: {error}"));
        assert!(
            database
                .restore_current_workspace()
                .unwrap_or_else(|error| panic!("ambiguous restore should load: {error}"))
                .is_none()
        );

        database
            .set_current_workspace(first.id)
            .unwrap_or_else(|error| panic!("workspace pointer should persist: {error}"));
        let volume = VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: "restore-volume".to_owned(),
            filesystem_type: Some("apfs".to_owned()),
            case_sensitive: false,
            removable: false,
            local: true,
        };
        let first_root = RootId::new();
        database
            .register_root(
                first.id,
                first_root,
                Path::new("/restore/first"),
                "First root",
                &volume,
            )
            .unwrap_or_else(|error| panic!("first root should register: {error}"));
        assert_eq!(
            database
                .restore_current_root(first.id)
                .unwrap_or_else(|error| panic!("single root should backfill: {error}"))
                .map(|root| root.id),
            Some(first_root)
        );
        let second_root = RootId::new();
        database
            .register_root(
                first.id,
                second_root,
                Path::new("/restore/second"),
                "Second root",
                &volume,
            )
            .unwrap_or_else(|error| panic!("second root should register: {error}"));
        database
            .lock()
            .and_then(|connection| {
                connection
                    .execute(
                        "UPDATE application_restore_state SET current_root_id = NULL
                         WHERE singleton = 1",
                        [],
                    )
                    .map(|_| ())
                    .map_err(PersistenceError::Sql)
            })
            .unwrap_or_else(|error| panic!("root pointer should clear: {error}"));
        assert!(
            database
                .restore_current_root(first.id)
                .unwrap_or_else(|error| panic!("ambiguous root restore should load: {error}"))
                .is_none()
        );
        database
            .set_current_root(first.id, second_root)
            .unwrap_or_else(|error| panic!("explicit root should persist: {error}"));
        assert_eq!(
            database
                .restore_current_root(first.id)
                .unwrap_or_else(|error| panic!("explicit root should restore: {error}"))
                .map(|root| root.id),
            Some(second_root)
        );
    }

    #[test]
    fn leases_cancellation_and_new_events_keep_jobs_recoverable() {
        let fixture = monitoring_fixture(41);
        let registration = enabled_registration(&fixture);
        let created = append_event(&fixture, &registration.id, WatchEventKind::Created, 10);
        let mut claimed = fixture
            .database
            .claim_monitoring_jobs(std::slice::from_ref(&created.job.id), 10)
            .unwrap_or_else(|error| panic!("job should claim: {error}"));
        let first_claim = claimed
            .pop()
            .unwrap_or_else(|| panic!("claimed job should exist"));
        assert_eq!(first_claim.status, MonitoringJobStatus::Processing);
        assert_eq!(first_claim.processing_stage, MonitoringJobStage::Stability);
        assert!(first_claim.claim_token.is_some());
        let first_claim_token = first_claim
            .claim_token
            .as_deref()
            .unwrap_or_else(|| panic!("claim token should exist"));
        assert!(
            fixture
                .database
                .list_due_monitoring_jobs_for_workspace(fixture.workspace_id, 60_009, 10)
                .unwrap_or_else(|error| panic!("leased jobs should query: {error}"))
                .is_empty()
        );
        assert_eq!(
            fixture
                .database
                .list_due_monitoring_jobs_for_workspace(fixture.workspace_id, 60_010, 10)
                .unwrap_or_else(|error| panic!("expired jobs should query: {error}"))
                .len(),
            1
        );
        let reclaimed = fixture
            .database
            .claim_monitoring_jobs(std::slice::from_ref(&created.job.id), 60_010)
            .unwrap_or_else(|error| panic!("expired lease should reclaim: {error}"))
            .remove(0);
        let reclaimed_token = reclaimed
            .claim_token
            .as_deref()
            .unwrap_or_else(|| panic!("replacement claim token should exist"));
        assert_ne!(first_claim_token, reclaimed_token);
        assert!(
            !fixture
                .database
                .mark_monitoring_job_completed(&created.job.id, first_claim_token)
                .unwrap_or_else(|error| panic!("stale completion should be rejected: {error}"))
        );
        assert!(
            fixture
                .database
                .requeue_monitoring_job_after_cancellation(&created.job.id, reclaimed_token, 20,)
                .unwrap_or_else(|error| panic!("cancelled work should requeue: {error}"))
        );
        let waiting = fixture
            .database
            .list_due_monitoring_jobs_for_workspace(fixture.workspace_id, 20, 10)
            .unwrap_or_else(|error| panic!("requeued job should load: {error}"))
            .remove(0);
        assert_eq!(waiting.status, MonitoringJobStatus::Waiting);
        assert_eq!(
            waiting.last_error_code.as_deref(),
            Some("cancelled_retryable")
        );

        let sampled_claim = fixture
            .database
            .claim_monitoring_jobs(std::slice::from_ref(&created.job.id), 20)
            .unwrap_or_else(|error| panic!("requeued job should claim: {error}"))
            .remove(0);
        fixture
            .database
            .update_monitoring_job_stability_sample(
                &created.job.id,
                sampled_claim
                    .claim_token
                    .as_deref()
                    .unwrap_or_else(|| panic!("sample claim token should exist")),
                &MonitoringStabilitySample {
                    byte_size: 100,
                    modified_at_ns: Some("100".to_owned()),
                    stable_sample_count: 1,
                    sampled_at_unix_ms: 21,
                    next_check_at_unix_ms: 30,
                },
            )
            .unwrap_or_else(|error| panic!("sample should persist: {error}"));
        let changed = append_event(&fixture, &registration.id, WatchEventKind::Modified, 40);
        assert_eq!(changed.job.id, created.job.id);
        assert_eq!(changed.job.stable_sample_count, 0);
        assert!(changed.job.sample_byte_size.is_none());
        assert!(changed.job.sample_modified_at_ns.is_none());
    }

    #[test]
    fn processing_errors_requeue_then_fail_only_at_the_retry_bound() {
        let fixture = monitoring_fixture(44);
        let registration = enabled_registration(&fixture);
        let job = fixture
            .database
            .append_watch_event_and_coalesce(&WatchEventInput {
                registration_id: registration.id,
                kind: WatchEventKind::Created,
                scope: WatchEventScope::File,
                path_before: None,
                path_after: Some(PathBuf::from("db-failure.txt")),
                native_identity_key: None,
                payload_json: "{}".to_owned(),
                debounce_ready_at_unix_ms: 0,
                maximum_attempts: 2,
            })
            .unwrap_or_else(|error| panic!("failure job should persist: {error}"))
            .job;
        fixture
            .database
            .claim_monitoring_jobs(std::slice::from_ref(&job.id), 0)
            .unwrap_or_else(|error| panic!("failure job should claim: {error}"));
        assert_eq!(
            fixture
                .database
                .recover_processing_jobs_for_workspace(
                    fixture.workspace_id,
                    10,
                    "database_failure",
                )
                .unwrap_or_else(|error| panic!("processing failure should recover: {error}")),
            1
        );
        let waiting = fixture
            .database
            .list_due_monitoring_jobs_for_workspace(fixture.workspace_id, 10, 10)
            .unwrap_or_else(|error| panic!("retry should load: {error}"))
            .into_iter()
            .find(|candidate| candidate.id == job.id)
            .unwrap_or_else(|| panic!("retryable job should remain durable"));
        assert_eq!(waiting.status, MonitoringJobStatus::Waiting);
        assert_eq!(waiting.attempt_count, 1);

        fixture
            .database
            .claim_monitoring_jobs(std::slice::from_ref(&job.id), 10)
            .unwrap_or_else(|error| panic!("retry should claim: {error}"));
        fixture
            .database
            .recover_processing_jobs_for_workspace(fixture.workspace_id, 20, "database_failure")
            .unwrap_or_else(|error| panic!("retry bound should resolve: {error}"));
        let terminal = fixture
            .database
            .lock()
            .and_then(|connection| monitoring_job_from_connection(&connection, &job.id))
            .unwrap_or_else(|error| panic!("terminal job should load: {error}"));
        assert_eq!(terminal.status, MonitoringJobStatus::Failed);
        assert_eq!(terminal.attempt_count, 2);
        assert!(terminal.claim_token.is_none());
        assert!(terminal.lease_expires_at_unix_ms.is_none());
    }

    #[test]
    fn exclusion_removal_durably_requests_reconciliation() {
        let fixture = monitoring_fixture(42);
        fixture
            .database
            .mark_startup_reconciliation_completed(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("reconciliation should complete: {error}"));
        let exclusion = fixture
            .database
            .upsert_monitoring_exclusion(
                fixture.workspace_id,
                Some(fixture.root_id),
                MonitoringExclusionKind::PathPrefix,
                "Previously excluded",
                true,
            )
            .unwrap_or_else(|error| panic!("exclusion should persist: {error}"));
        assert!(
            fixture
                .database
                .remove_monitoring_exclusion(&exclusion.id)
                .unwrap_or_else(|error| panic!("exclusion should remove atomically: {error}"))
        );
        assert!(
            fixture
                .database
                .get_workspace_monitoring_state(fixture.workspace_id)
                .unwrap_or_else(|error| panic!("state should load: {error}"))
                .startup_reconciliation_pending
        );
    }

    #[cfg(unix)]
    #[test]
    fn watch_events_preserve_non_utf_native_paths_losslessly() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let fixture = monitoring_fixture(43);
        let registration = enabled_registration(&fixture);
        let native_path =
            PathBuf::from(OsString::from_vec(b"Inbox/non-utf-\xff-name.txt".to_vec()));
        fixture
            .database
            .append_watch_event_and_coalesce(&WatchEventInput {
                registration_id: registration.id.clone(),
                kind: WatchEventKind::Created,
                scope: WatchEventScope::File,
                path_before: None,
                path_after: Some(native_path.clone()),
                native_identity_key: None,
                payload_json: "{}".to_owned(),
                debounce_ready_at_unix_ms: 10,
                maximum_attempts: 5,
            })
            .unwrap_or_else(|error| panic!("native path should persist: {error}"));
        let event = fixture
            .database
            .list_watch_events(&registration.id, None, 10)
            .unwrap_or_else(|error| panic!("native event should reload: {error}"))
            .remove(0);
        assert_eq!(event.path_after.as_deref(), Some(native_path.as_path()));

        let first_root_path =
            PathBuf::from(OsString::from_vec(b"/encrypted/non-utf-\xff".to_vec()));
        let second_root_path =
            PathBuf::from(OsString::from_vec(b"/encrypted/non-utf-\xfe".to_vec()));
        let first_root_id = RootId::new();
        let second_root_id = RootId::new();
        fixture
            .database
            .register_root(
                fixture.workspace_id,
                first_root_id,
                &first_root_path,
                "First native root",
                &fixture.volume,
            )
            .unwrap_or_else(|error| panic!("first native root should persist: {error}"));
        fixture
            .database
            .register_root(
                fixture.workspace_id,
                second_root_id,
                &second_root_path,
                "Second native root",
                &fixture.volume,
            )
            .unwrap_or_else(|error| panic!("second native root should persist: {error}"));
        let roots = fixture
            .database
            .list_roots(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("native roots should reload: {error}"));
        assert!(roots.iter().any(|root| {
            root.id == first_root_id && root.absolute_path_native == first_root_path
        }));
        assert!(roots.iter().any(|root| {
            root.id == second_root_id && root.absolute_path_native == second_root_path
        }));
    }

    #[test]
    fn activity_is_one_row_per_batch_and_dashboard_counts_are_aggregated() {
        let fixture = monitoring_fixture(35);
        for (batch_id, analyzed, ready, review, failed) in
            [("batch-a", 3, 2, 1, 0), ("batch-b", 4, 1, 2, 1)]
        {
            fixture
                .database
                .record_monitoring_activity(&MonitoringActivityInput {
                    batch_id: batch_id.to_owned(),
                    workspace_id: fixture.workspace_id,
                    root_id: Some(fixture.root_id),
                    files_analyzed: analyzed,
                    ready_to_organize: ready,
                    needs_review: review,
                    failed,
                    summary: format!("{analyzed} files analyzed"),
                    reconciliation_scan_id: None,
                })
                .unwrap_or_else(|error| panic!("activity should persist: {error}"));
        }
        let activity = fixture
            .database
            .list_monitoring_activity(fixture.workspace_id, 10)
            .unwrap_or_else(|error| panic!("activity should list: {error}"));
        assert_eq!(activity.len(), 2);
        let counts = fixture
            .database
            .monitoring_dashboard_counts(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("dashboard counts should load: {error}"));
        assert_eq!(counts.files_analyzed, 7);
        assert_eq!(counts.ready_to_organize, 3);
        assert_eq!(counts.needs_review, 3);
        assert_eq!(counts.pending_proposals, 0);
        assert_eq!(counts.pending_jobs, 0);

        let duplicate = fixture
            .database
            .record_monitoring_activity(&MonitoringActivityInput {
                batch_id: "batch-a".to_owned(),
                workspace_id: fixture.workspace_id,
                root_id: Some(fixture.root_id),
                files_analyzed: 1,
                ready_to_organize: 0,
                needs_review: 0,
                failed: 0,
                summary: "duplicate".to_owned(),
                reconciliation_scan_id: None,
            });
        assert!(duplicate.is_err());
    }
}
