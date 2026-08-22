use crate::{
    Database, PersistenceError, ScanFileInput, ScanIssueInput, ScanRecord, persist_observation,
    synchronize_scanner_review, to_sql_integer, to_sql_u64, upsert_scan_search_document,
};
use domain::{ScanId, WorkspaceId};
use rusqlite::params;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerScanFinalization {
    pub scan_id: ScanId,
    pub status: String,
    pub files_discovered: u64,
    pub files_indexed: u64,
    pub directories_discovered: u64,
    pub bytes_discovered: u64,
    pub errors: u64,
    pub skipped_items: u64,
    pub issue_count: u64,
    pub truncated: bool,
}

impl Database {
    /// Persist one bounded consumer-scan batch without completing the scan.
    ///
    /// This keeps One-Click memory usage bounded even when a personal folder
    /// contains tens or hundreds of thousands of loose files. The scan remains
    /// `running` until `finalize_consumer_scan` is called.
    pub fn append_consumer_scan_batch(
        &self,
        workspace_id: WorkspaceId,
        scan_id: ScanId,
        files: &[ScanFileInput],
        issues: &[ScanIssueInput],
    ) -> Result<(), PersistenceError> {
        if files.is_empty() && issues.is_empty() {
            return Ok(());
        }

        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;

        for file in files {
            let persisted = persist_observation(&transaction, &file.observation, file.accessed_at_ns)?;
            transaction.execute(
                "INSERT INTO scan_file_statuses(
                    scan_id, file_version_id, extension, readability_status,
                    scan_status, hashing_status, error_code
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    scan_id.to_string(),
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
                workspace_id,
                &persisted,
                file.extension.as_deref(),
            )?;
            synchronize_scanner_review(
                &transaction,
                workspace_id,
                &persisted,
                &file.readability_status,
                file.error_code.as_deref(),
            )?;
        }

        for issue in issues {
            transaction.execute(
                "INSERT INTO scan_issues(
                    id, scan_id, relative_path, code, severity, message, details_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    Uuid::now_v7().to_string(),
                    scan_id.to_string(),
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

        transaction.commit()?;
        Ok(())
    }

    /// Complete a consumer scan whose files/issues were persisted incrementally.
    pub fn finalize_consumer_scan(
        &self,
        input: &ConsumerScanFinalization,
    ) -> Result<ScanRecord, PersistenceError> {
        let database_status = if input.status == "cancelled" {
            "cancelled"
        } else {
            "completed"
        };
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO scan_metrics(
                scan_id, files_indexed, directories_discovered, bytes_discovered,
                files_hashed, error_count, skipped_count, duplicate_group_count, truncated
             ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, 0, ?7)",
            params![
                input.scan_id.to_string(),
                to_sql_u64(input.files_indexed)?,
                to_sql_u64(input.directories_discovered)?,
                to_sql_u64(input.bytes_discovered)?,
                to_sql_u64(input.errors)?,
                to_sql_u64(input.skipped_items)?,
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
                to_sql_u64(input.files_indexed)?,
                to_sql_u64(input.issue_count)?,
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        self.scan(input.scan_id)
    }
}
