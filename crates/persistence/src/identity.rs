use super::{
    Database, IdentityAuditEventRecord, IdentityCandidateAction, IdentityCandidateRecord,
    IdentityDetailRecord, IdentityIdentifierRecord, IdentityMatchEvidenceRecord,
    IdentityMutationRecord, IdentityOccurrenceRecord, IdentityOccurrenceSyncRecord,
    IdentityRelationshipRecord, IdentityResolverRunRecord, IdentityReviewGroupRecord,
    IdentityReviewPageRecord, IdentitySummaryRecord, PersistenceError,
    StoredIdentityCandidateRecord, from_sql_u64, truncate_database_text,
};
use domain::WorkspaceId;
use knowledge::{
    EvidencePolarity, IdentityEvidence, IdentityOccurrence, IdentityRole, IdentityType,
    MatchAssessment, RESOLVER_ID, RESOLVER_VERSION, ResolutionDecision, SignalKind,
    normalize_company_identifier, normalize_domain, normalize_email, normalize_name,
    normalize_phone,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::{BTreeSet, HashSet};
use uuid::Uuid;

const IDENTITY_FILE_BATCH_SIZE: usize = 32;
const MAX_BLOCKED_OCCURRENCES: usize = 64;
const MAX_REVIEW_CANDIDATES_PER_GROUP: usize = 20;
const SIGNAL_PROXIMITY_CHARS: i64 = 500;

#[derive(Debug, Clone)]
struct SemanticSource {
    semantic_entity_id: Option<String>,
    semantic_field_id: Option<String>,
    identity_type: IdentityType,
    role: Option<IdentityRole>,
    original_value: String,
    normalized_value: String,
    normalized_core: String,
    legal_suffix: Option<String>,
    confidence: f32,
    source_method: String,
    analyzer_version: String,
    start_offset: Option<i64>,
    signals: Vec<SourceSignal>,
}

#[derive(Debug, Clone)]
struct SourceSignal {
    kind: SignalKind,
    original_value: String,
    normalized_value: String,
    semantic_entity_id: Option<String>,
    semantic_field_id: Option<String>,
    confidence: f32,
    source_method: String,
    analyzer_version: String,
    start_offset: Option<i64>,
}

type IdentityOccurrenceSourceRow = (
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    f64,
    String,
    Option<String>,
);

impl Database {
    pub fn identity_workspace_for_file(
        &self,
        file_id: &str,
    ) -> Result<WorkspaceId, PersistenceError> {
        validate_uuid_text(file_id)?;
        let connection = self.lock()?;
        let workspace_id: String = connection
            .query_row(
                "SELECT workspace_id FROM files WHERE id = ?1",
                [file_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        workspace_id
            .parse()
            .map_err(PersistenceError::InvalidIdentifier)
    }

    pub fn invalidate_identity_resolution_for_file(
        &self,
        file_id: &str,
    ) -> Result<(), PersistenceError> {
        validate_uuid_text(file_id)?;
        let connection = self.lock()?;
        connection.execute(
            "UPDATE identity_resolution_state
             SET status = 'pending',
                 source_digest = NULL,
                 last_run_id = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_id = ?1",
            [file_id],
        )?;
        Ok(())
    }

    pub fn begin_identity_resolver_run(
        &self,
        workspace_id: WorkspaceId,
        trigger_kind: &str,
    ) -> Result<IdentityResolverRunRecord, PersistenceError> {
        if !matches!(
            trigger_kind,
            "semantic_analysis"
                | "semantic_correction"
                | "new_file"
                | "resolver_upgrade"
                | "manual"
        ) {
            return Err(PersistenceError::InvalidIdentityInput);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let run_id = Uuid::now_v7().to_string();
        let inserted = transaction.execute(
            "INSERT INTO identity_resolver_runs(
                id, workspace_id, trigger_kind, status, resolver_id, resolver_version
             )
             SELECT ?1, id, ?2, 'running', ?3, ?4
             FROM workspaces
             WHERE id = ?5",
            params![
                run_id,
                trigger_kind,
                RESOLVER_ID,
                RESOLVER_VERSION,
                workspace_id.to_string(),
            ],
        )?;
        if inserted != 1 {
            return Err(PersistenceError::NotFound);
        }
        supersede_outdated_machine_candidates(
            &transaction,
            &workspace_id.to_string(),
            RESOLVER_VERSION,
        )?;
        transaction.execute(
            "INSERT INTO identity_audit_events(
                id, workspace_id, event_type, decision_source, reason, resolver_version
             ) VALUES (?1, ?2, 'resolver_started', 'resolver', ?3, ?4)",
            params![
                Uuid::now_v7().to_string(),
                workspace_id.to_string(),
                trigger_kind,
                RESOLVER_VERSION,
            ],
        )?;
        transaction.commit()?;
        identity_run_by_id(&connection, &run_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_identity_resolver_run(
        &self,
        run_id: &str,
        status: &str,
        files_considered: u64,
        occurrences_processed: u64,
        blocking_memberships: u64,
        comparisons: u64,
        candidates_created: u64,
        auto_links_created: u64,
        error_message: Option<&str>,
    ) -> Result<IdentityResolverRunRecord, PersistenceError> {
        if !matches!(status, "completed" | "cancelled" | "failed") {
            return Err(PersistenceError::InvalidIdentityInput);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE identity_resolver_runs
             SET status = ?2,
                 files_considered = ?3,
                 occurrences_processed = ?4,
                 blocking_memberships = ?5,
                 comparisons = ?6,
                 candidates_created = ?7,
                 auto_links_created = ?8,
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error_message = ?9
             WHERE id = ?1 AND status = 'running'",
            params![
                run_id,
                status,
                sql_u64(files_considered)?,
                sql_u64(occurrences_processed)?,
                sql_u64(blocking_memberships)?,
                sql_u64(comparisons)?,
                sql_u64(candidates_created)?,
                sql_u64(auto_links_created)?,
                error_message.map(|value| truncate_database_text(value, 512)),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        let (workspace_id, trigger_kind): (String, String) = transaction.query_row(
            "SELECT workspace_id, trigger_kind FROM identity_resolver_runs WHERE id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        transaction.execute(
            "INSERT INTO identity_audit_events(
                id, workspace_id, event_type, decision_source, reason, resolver_version
             ) VALUES (?1, ?2, ?3, 'resolver', ?4, ?5)",
            params![
                Uuid::now_v7().to_string(),
                workspace_id,
                match status {
                    "completed" => "resolver_completed",
                    "cancelled" => "resolver_cancelled",
                    _ => "resolver_failed",
                },
                error_message.unwrap_or(&trigger_kind),
                RESOLVER_VERSION,
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        let connection = self.lock()?;
        identity_run_by_id(&connection, run_id)
    }

    pub fn identity_files_to_process(
        &self,
        workspace_id: WorkspaceId,
        run_id: &str,
        force: bool,
        limit: usize,
    ) -> Result<Vec<String>, PersistenceError> {
        validate_uuid_text(run_id)?;
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sa.file_id
             FROM semantic_analyses sa
             LEFT JOIN identity_resolution_state irs ON irs.file_id = sa.file_id
             WHERE sa.workspace_id = ?1
               AND sa.is_current = 1
               AND sa.status IN ('success', 'partial', 'unknown')
               AND COALESCE(irs.last_run_id, '') <> ?4
               AND (
                    ?2 = 1
                    OR irs.file_id IS NULL
                    OR irs.semantic_analysis_id IS NOT sa.id
                    OR irs.resolver_version <> ?3
                    OR irs.status IN ('pending', 'cancelled', 'failed')
               )
             ORDER BY sa.file_id
             LIMIT ?5",
        )?;
        let rows = statement.query_map(
            params![
                workspace_id.to_string(),
                i64::from(force),
                RESOLVER_VERSION,
                run_id,
                sql_usize(limit.clamp(1, IDENTITY_FILE_BATCH_SIZE))?,
            ],
            |row| row.get::<_, String>(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn synchronize_identity_occurrences(
        &self,
        file_id: &str,
        run_id: &str,
    ) -> Result<IdentityOccurrenceSyncRecord, PersistenceError> {
        validate_uuid_text(file_id)?;
        validate_uuid_text(run_id)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (
            workspace_id,
            file_version_id,
            semantic_analysis_id,
            analyzer_version,
            relative_path,
        ): (String, String, String, String, String) = transaction
            .query_row(
                "SELECT
                    sa.workspace_id, sa.file_version_id, sa.id, sa.analyzer_version,
                    fl.relative_path
                 FROM semantic_analyses sa
                 JOIN file_versions fv ON fv.id = sa.file_version_id
                 JOIN file_locations fl ON fl.id = fv.location_id
                 WHERE sa.file_id = ?1
                   AND sa.is_current = 1
                   AND sa.status IN ('success', 'partial', 'unknown')",
                [file_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let run_workspace: String = transaction
            .query_row(
                "SELECT workspace_id
                 FROM identity_resolver_runs
                 WHERE id = ?1 AND status = 'running'",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        if run_workspace != workspace_id {
            return Err(PersistenceError::IdentityConflict);
        }

        transaction.execute(
            "INSERT INTO identity_resolution_state(
                file_id, workspace_id, semantic_analysis_id, resolver_version,
                status, source_digest, last_run_id
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6)
             ON CONFLICT(file_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                semantic_analysis_id = excluded.semantic_analysis_id,
                resolver_version = excluded.resolver_version,
                status = 'pending',
                source_digest = excluded.source_digest,
                last_run_id = excluded.last_run_id,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                file_id,
                workspace_id,
                semantic_analysis_id,
                RESOLVER_VERSION,
                blake3::hash(semantic_analysis_id.as_bytes())
                    .as_bytes()
                    .as_slice(),
                run_id,
            ],
        )?;

        let mut sources =
            semantic_identity_sources(&transaction, &semantic_analysis_id, &analyzer_version)?;
        attach_semantic_signals(
            &transaction,
            &semantic_analysis_id,
            &relative_path,
            &mut sources,
        )?;
        let current_source_keys = sources
            .iter()
            .map(|source| source_key(file_id, source))
            .collect::<Result<HashSet<_>, _>>()?;

        let mut old_statement = transaction.prepare(
            "SELECT id, source_key
             FROM identity_occurrences
             WHERE file_id = ?1 AND active = 1",
        )?;
        let old_rows = old_statement.query_map([file_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let old_occurrences = old_rows.collect::<Result<Vec<_>, _>>()?;
        drop(old_statement);
        let mut deactivated_count = 0_u64;
        for (occurrence_id, old_key) in old_occurrences {
            if !current_source_keys.contains(&old_key) {
                deactivated_count = deactivated_count.saturating_add(
                    u64::try_from(transaction.execute(
                        "UPDATE identity_occurrences
                         SET active = 0,
                             superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                         WHERE id = ?1 AND active = 1",
                        [occurrence_id],
                    )?)
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                );
            }
        }

        let mut occurrence_ids = Vec::with_capacity(sources.len());
        let mut created_count = 0_u64;
        let mut updated_count = 0_u64;
        for source in &sources {
            let key = source_key(file_id, source)?;
            let existing: Option<(String, String, String)> = transaction
                .query_row(
                    "SELECT id, identity_id, normalized_value
                     FROM identity_occurrences
                     WHERE workspace_id = ?1
                       AND (
                            source_key = ?2
                            OR (?3 IS NOT NULL AND semantic_entity_id = ?3)
                            OR (?4 IS NOT NULL AND semantic_field_id = ?4)
                       )
                     ORDER BY CASE WHEN source_key = ?2 THEN 0 ELSE 1 END
                     LIMIT 1",
                    params![
                        workspace_id,
                        key,
                        source.semantic_entity_id,
                        source.semantic_field_id,
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let (occurrence_id, identity_id) =
                if let Some((occurrence_id, identity_id, previous_normalized_value)) = existing {
                    let mut identity_id = canonical_identity_id(&transaction, &identity_id)?;
                    if previous_normalized_value != source.normalized_value
                        && !identity_user_locked(&transaction, &identity_id)?
                    {
                        supersede_machine_candidates_for_identity(
                            &transaction,
                            &workspace_id,
                            &identity_id,
                        )?;
                        let other_active_occurrences: i64 = transaction.query_row(
                            "SELECT COUNT(*) FROM identity_occurrences
                         WHERE identity_id = ?1 AND id <> ?2 AND active = 1",
                            params![identity_id, occurrence_id],
                            |row| row.get(0),
                        )?;
                        if other_active_occurrences > 0 {
                            let previous_identity_id = identity_id.clone();
                            identity_id =
                                insert_unresolved_identity(&transaction, &workspace_id, source)?;
                            record_resolver_occurrence_detachment(
                                &transaction,
                                &workspace_id,
                                &previous_identity_id,
                                &identity_id,
                                &occurrence_id,
                            )?;
                        } else {
                            transaction.execute(
                                "UPDATE resolved_identities
                             SET display_name = ?2,
                                 normalized_display_name = ?3,
                                 resolution_status = 'unresolved',
                                 confidence = ?4,
                                 resolver_version = ?5,
                                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             WHERE id = ?1 AND lifecycle_status = 'active'",
                                params![
                                    identity_id,
                                    source.original_value,
                                    source.normalized_value,
                                    f64::from(source.confidence),
                                    RESOLVER_VERSION,
                                ],
                            )?;
                        }
                    }
                    transaction.execute(
                        "UPDATE identity_occurrences
                     SET identity_id = ?2,
                         source_key = ?3,
                         file_version_id = ?4,
                         semantic_analysis_id = ?5,
                         semantic_entity_id = ?6,
                         semantic_field_id = ?7,
                         occurrence_type = ?8,
                         original_value = ?9,
                         normalized_value = ?10,
                         normalized_core = ?11,
                         legal_suffix = ?12,
                         confidence = ?13,
                         source_method = ?14,
                         analyzer_version = ?15,
                         resolver_version = ?16,
                         active = 1,
                         superseded_at = NULL,
                         last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                        params![
                            occurrence_id,
                            identity_id,
                            key,
                            file_version_id,
                            semantic_analysis_id,
                            source.semantic_entity_id,
                            source.semantic_field_id,
                            source.identity_type.database_name(),
                            source.original_value,
                            source.normalized_value,
                            source.normalized_core,
                            source.legal_suffix,
                            f64::from(source.confidence),
                            source.source_method,
                            source.analyzer_version,
                            RESOLVER_VERSION,
                        ],
                    )?;
                    updated_count = updated_count.saturating_add(1);
                    (occurrence_id, identity_id)
                } else {
                    let identity_id =
                        insert_unresolved_identity(&transaction, &workspace_id, source)?;
                    let occurrence_id = Uuid::now_v7().to_string();
                    transaction.execute(
                        "INSERT INTO identity_occurrences(
                        id, workspace_id, identity_id, source_key, file_id,
                        file_version_id, semantic_analysis_id, semantic_entity_id,
                        semantic_field_id, occurrence_type, original_value,
                        normalized_value, normalized_core, legal_suffix, confidence,
                        source_method, analyzer_version, resolver_version
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
                     )",
                        params![
                            occurrence_id,
                            workspace_id,
                            identity_id,
                            key,
                            file_id,
                            file_version_id,
                            semantic_analysis_id,
                            source.semantic_entity_id,
                            source.semantic_field_id,
                            source.identity_type.database_name(),
                            source.original_value,
                            source.normalized_value,
                            source.normalized_core,
                            source.legal_suffix,
                            f64::from(source.confidence),
                            source.source_method,
                            source.analyzer_version,
                            RESOLVER_VERSION,
                        ],
                    )?;
                    created_count = created_count.saturating_add(1);
                    (occurrence_id, identity_id)
                };

            transaction.execute(
                "UPDATE identity_occurrences SET identity_id = ?2 WHERE id = ?1",
                params![occurrence_id, identity_id],
            )?;
            transaction.execute(
                "UPDATE identity_aliases
                 SET occurrence_id = NULL
                 WHERE occurrence_id = ?1 AND identity_id <> ?2",
                params![occurrence_id, identity_id],
            )?;
            transaction.execute(
                "UPDATE identity_roles
                 SET active = 0,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE occurrence_id = ?1 AND identity_id <> ?2 AND active = 1",
                params![occurrence_id, identity_id],
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO identity_aliases(
                    id, identity_id, occurrence_id, original_value,
                    normalized_value, legal_suffix, source
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'occurrence')",
                params![
                    Uuid::now_v7().to_string(),
                    identity_id,
                    occurrence_id,
                    source.original_value,
                    source.normalized_value,
                    source.legal_suffix,
                ],
            )?;
            if let Some(role) = source.role {
                transaction.execute(
                    "INSERT INTO identity_roles(
                        id, identity_id, role, occurrence_id, status, confidence
                     ) VALUES (?1, ?2, ?3, ?4, 'observed', ?5)
                     ON CONFLICT(identity_id, role, occurrence_id) DO UPDATE SET
                        confidence = excluded.confidence,
                        active = 1,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                    params![
                        Uuid::now_v7().to_string(),
                        identity_id,
                        role.database_name(),
                        occurrence_id,
                        f64::from(source.confidence),
                    ],
                )?;
            }
            transaction.execute(
                "DELETE FROM identity_occurrence_signals WHERE occurrence_id = ?1",
                [occurrence_id.as_str()],
            )?;
            insert_source_signal(
                &transaction,
                &occurrence_id,
                &SourceSignal {
                    kind: SignalKind::Name,
                    original_value: source.original_value.clone(),
                    normalized_value: source.normalized_value.clone(),
                    semantic_entity_id: source.semantic_entity_id.clone(),
                    semantic_field_id: source.semantic_field_id.clone(),
                    confidence: source.confidence,
                    source_method: source.source_method.clone(),
                    analyzer_version: source.analyzer_version.clone(),
                    start_offset: source.start_offset,
                },
            )?;
            for signal in &source.signals {
                insert_source_signal(&transaction, &occurrence_id, signal)?;
            }
            occurrence_ids.push(occurrence_id);
        }

        supersede_orphaned_machine_candidates(&transaction, &workspace_id)?;
        refresh_file_relationships(&transaction, file_id)?;
        transaction.commit()?;
        Ok(IdentityOccurrenceSyncRecord {
            file_id: file_id.to_owned(),
            semantic_analysis_id,
            occurrence_ids,
            created_count,
            updated_count,
            deactivated_count,
        })
    }

    pub fn mark_identity_file_resolution(
        &self,
        file_id: &str,
        run_id: &str,
        status: &str,
    ) -> Result<(), PersistenceError> {
        if !matches!(status, "completed" | "cancelled" | "failed") {
            return Err(PersistenceError::InvalidIdentityInput);
        }
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE identity_resolution_state
             SET status = ?3,
                 last_run_id = ?2,
                 resolver_version = ?4,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE file_id = ?1",
            params![file_id, run_id, status, RESOLVER_VERSION],
        )?;
        if changed != 1 {
            return Err(PersistenceError::NotFound);
        }
        Ok(())
    }

    pub fn identity_occurrence(
        &self,
        occurrence_id: &str,
    ) -> Result<IdentityOccurrence, PersistenceError> {
        let connection = self.lock()?;
        identity_occurrence_from_connection(&connection, occurrence_id)
    }

    pub fn blocked_identity_occurrences(
        &self,
        occurrence_id: &str,
        limit: usize,
    ) -> Result<Vec<IdentityOccurrence>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT other.id
             FROM identity_occurrences base
             JOIN identity_occurrences other
               ON other.workspace_id = base.workspace_id
              AND other.occurrence_type = base.occurrence_type
              AND other.active = 1
              AND other.id <> base.id
              AND other.file_id <> base.file_id
             WHERE base.id = ?1
               AND base.active = 1
               AND (
                    other.normalized_value = base.normalized_value
                    OR other.normalized_core = base.normalized_core
                    OR EXISTS (
                        SELECT 1
                        FROM identity_occurrence_signals left_signal
                        JOIN identity_occurrence_signals right_signal
                          ON right_signal.signal_kind = left_signal.signal_kind
                         AND right_signal.normalized_value = left_signal.normalized_value
                        WHERE left_signal.occurrence_id = base.id
                          AND right_signal.occurrence_id = other.id
                          AND left_signal.signal_kind IN (
                            'company_identifier', 'vat_identifier', 'email', 'domain',
                            'phone', 'account_identifier', 'project_reference',
                            'customer_identity'
                          )
                    )
               )
             ORDER BY
                CASE
                    WHEN other.normalized_value = base.normalized_value THEN 0
                    WHEN EXISTS (
                        SELECT 1
                        FROM identity_occurrence_signals left_signal
                        JOIN identity_occurrence_signals right_signal
                          ON right_signal.signal_kind = left_signal.signal_kind
                         AND right_signal.normalized_value = left_signal.normalized_value
                        WHERE left_signal.occurrence_id = base.id
                          AND right_signal.occurrence_id = other.id
                          AND left_signal.signal_kind IN (
                            'company_identifier', 'vat_identifier', 'email', 'phone',
                            'account_identifier', 'project_reference'
                          )
                    ) THEN 1
                    WHEN other.normalized_core = base.normalized_core THEN 2
                    ELSE 3
                END,
                other.id
             LIMIT ?2",
        )?;
        let ids = statement
            .query_map(
                params![
                    occurrence_id,
                    sql_usize(limit.clamp(1, MAX_BLOCKED_OCCURRENCES))?
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| identity_occurrence_from_connection(&connection, id))
            .collect()
    }

    pub fn store_identity_candidate(
        &self,
        left_occurrence_id: &str,
        right_occurrence_id: &str,
        assessment: &MatchAssessment,
        creation_source: &str,
    ) -> Result<StoredIdentityCandidateRecord, PersistenceError> {
        if !matches!(creation_source, "resolver" | "incremental") {
            return Err(PersistenceError::InvalidIdentityInput);
        }
        if assessment.decision == ResolutionDecision::Unknown {
            return Err(PersistenceError::InvalidIdentityInput);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (workspace_id, left_identity_id, left_type, left_name): (
            String,
            String,
            String,
            String,
        ) = transaction
            .query_row(
                "SELECT workspace_id, identity_id, occurrence_type, normalized_core
                 FROM identity_occurrences
                 WHERE id = ?1 AND active = 1",
                [left_occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let (right_workspace, right_identity_id, right_type, right_name): (
            String,
            String,
            String,
            String,
        ) = transaction
            .query_row(
                "SELECT workspace_id, identity_id, occurrence_type, normalized_core
                 FROM identity_occurrences
                 WHERE id = ?1 AND active = 1",
                [right_occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        if workspace_id != right_workspace || left_type != right_type {
            return Err(PersistenceError::IdentityConflict);
        }
        let left_identity_id = canonical_identity_id(&transaction, &left_identity_id)?;
        let right_identity_id = canonical_identity_id(&transaction, &right_identity_id)?;
        if left_identity_id == right_identity_id {
            transaction.commit()?;
            return Ok(StoredIdentityCandidateRecord {
                candidate_id: None,
                left_identity_id: left_identity_id.clone(),
                right_identity_id,
                status: "already_linked".to_owned(),
                created: false,
                rejected_by_user: false,
            });
        }
        let (left_identity_id, right_identity_id) =
            ordered_pair(&left_identity_id, &right_identity_id);
        let pair_key = format!("{left_identity_id}:{right_identity_id}");
        let rejected = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM identity_rejection_constraints
                WHERE workspace_id = ?1 AND pair_key = ?2 AND active = 1
             )",
            params![workspace_id, pair_key],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )?;
        if rejected {
            transaction.commit()?;
            return Ok(StoredIdentityCandidateRecord {
                candidate_id: None,
                left_identity_id,
                right_identity_id,
                status: "user_rejected".to_owned(),
                created: false,
                rejected_by_user: true,
            });
        }

        let review_group_key = format!(
            "{}:{}",
            left_type,
            if left_name <= right_name {
                left_name
            } else {
                right_name
            }
        );
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT id, status
                 FROM identity_candidates
                 WHERE workspace_id = ?1
                   AND pair_key = ?2
                   AND resolver_version = ?3",
                params![workspace_id, pair_key, RESOLVER_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((candidate_id, status)) = existing.as_ref()
            && matches!(status.as_str(), "user_confirmed" | "user_rejected")
        {
            transaction.commit()?;
            return Ok(StoredIdentityCandidateRecord {
                candidate_id: Some(candidate_id.clone()),
                left_identity_id,
                right_identity_id,
                status: status.clone(),
                created: false,
                rejected_by_user: status == "user_rejected",
            });
        }

        let status = match assessment.decision {
            ResolutionDecision::AutoLink => "auto_linked",
            ResolutionDecision::Review => "candidate",
            ResolutionDecision::KeepSeparate => "conflicting",
            ResolutionDecision::Unknown => return Err(PersistenceError::InvalidIdentityInput),
        };
        let created = existing.is_none();
        let candidate_id = existing
            .map(|(candidate_id, _)| candidate_id)
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        transaction.execute(
            "INSERT INTO identity_candidates(
                id, workspace_id, left_identity_id, right_identity_id, pair_key,
                review_group_key, score, policy_decision, status, creation_source,
                resolver_version, active
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1)
             ON CONFLICT(workspace_id, pair_key, resolver_version) DO UPDATE SET
                score = excluded.score,
                policy_decision = excluded.policy_decision,
                status = CASE
                    WHEN identity_candidates.status IN ('user_confirmed', 'user_rejected')
                    THEN identity_candidates.status
                    ELSE excluded.status
                END,
                review_group_key = excluded.review_group_key,
                active = 1,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                candidate_id,
                workspace_id,
                left_identity_id,
                right_identity_id,
                pair_key,
                review_group_key,
                f64::from(assessment.score),
                assessment.decision.database_name(),
                status,
                creation_source,
                RESOLVER_VERSION,
            ],
        )?;
        transaction.execute(
            "DELETE FROM identity_candidate_evidence WHERE candidate_id = ?1",
            [candidate_id.as_str()],
        )?;
        for evidence in &assessment.evidence {
            insert_candidate_evidence(&transaction, &candidate_id, evidence)?;
        }
        if created {
            let decision_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO identity_decisions(
                    id, workspace_id, decision_type, decision_source, candidate_id,
                    primary_identity_id, secondary_identity_id, reason, resolver_version
                 ) VALUES (
                    ?1, ?2, 'candidate_created', 'resolver', ?3, ?4, ?5, ?6, ?7
                 )",
                params![
                    decision_id,
                    workspace_id,
                    candidate_id,
                    left_identity_id,
                    right_identity_id,
                    assessment.decision.database_name(),
                    RESOLVER_VERSION,
                ],
            )?;
            insert_audit(
                &transaction,
                &workspace_id,
                "candidate_created",
                "resolver",
                Some(&left_identity_id),
                Some(&right_identity_id),
                Some(&candidate_id),
                None,
                Some(assessment.decision.database_name()),
            )?;
        }

        let mut final_status = status.to_owned();
        if assessment.decision == ResolutionDecision::AutoLink {
            let auto_allowed = !identity_user_locked(&transaction, &left_identity_id)?
                && !identity_user_locked(&transaction, &right_identity_id)?;
            if auto_allowed {
                merge_identities(
                    &transaction,
                    &workspace_id,
                    &left_identity_id,
                    &right_identity_id,
                    "resolver",
                    Some(&candidate_id),
                    "auto_linked",
                )?;
            } else {
                final_status = "candidate".to_owned();
                transaction.execute(
                    "UPDATE identity_candidates
                     SET status = 'candidate', policy_decision = 'review',
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                    [candidate_id.as_str()],
                )?;
            }
        }
        refresh_review_group(&transaction, &workspace_id, &review_group_key)?;
        transaction.commit()?;
        Ok(StoredIdentityCandidateRecord {
            candidate_id: Some(candidate_id),
            left_identity_id,
            right_identity_id,
            status: final_status,
            created,
            rejected_by_user: false,
        })
    }

    pub fn decide_identity_candidate(
        &self,
        candidate_id: &str,
        action: IdentityCandidateAction,
        reason: Option<&str>,
    ) -> Result<IdentityMutationRecord, PersistenceError> {
        validate_optional_reason(reason)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (workspace_id, left_identity_id, right_identity_id, status, review_group_key): (
            String,
            String,
            String,
            String,
            String,
        ) = transaction
            .query_row(
                "SELECT
                    workspace_id, left_identity_id, right_identity_id, status,
                    review_group_key
                 FROM identity_candidates
                 WHERE id = ?1 AND active = 1",
                [candidate_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        if matches!(
            status.as_str(),
            "auto_linked" | "user_confirmed" | "user_rejected"
        ) {
            return Err(PersistenceError::IdentityConflict);
        }
        let left_identity_id = canonical_identity_id(&transaction, &left_identity_id)?;
        let right_identity_id = canonical_identity_id(&transaction, &right_identity_id)?;
        let decision_id = match action {
            IdentityCandidateAction::Confirm => merge_identities(
                &transaction,
                &workspace_id,
                &left_identity_id,
                &right_identity_id,
                "user",
                Some(candidate_id),
                reason.unwrap_or("user confirmed the identity match"),
            )?,
            IdentityCandidateAction::Reject | IdentityCandidateAction::KeepSeparate => {
                reject_identity_pair(
                    &transaction,
                    &workspace_id,
                    &left_identity_id,
                    &right_identity_id,
                    Some(candidate_id),
                    reason.unwrap_or(match action {
                        IdentityCandidateAction::Reject => "user rejected the identity match",
                        IdentityCandidateAction::KeepSeparate => {
                            "user requested that identities remain separate"
                        }
                        IdentityCandidateAction::Confirm => unreachable!(),
                    }),
                    if action == IdentityCandidateAction::Reject {
                        "reject_match"
                    } else {
                        "keep_separate"
                    },
                )?
            }
        };
        let candidate_status = if action == IdentityCandidateAction::Confirm {
            "user_confirmed"
        } else {
            "user_rejected"
        };
        transaction.execute(
            "UPDATE identity_candidates
             SET status = ?2,
                 policy_decision = CASE WHEN ?2 = 'user_rejected'
                    THEN 'keep_separate' ELSE policy_decision END,
                 decided_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            params![candidate_id, candidate_status],
        )?;
        refresh_review_group(&transaction, &workspace_id, &review_group_key)?;
        let created_at: String = transaction.query_row(
            "SELECT created_at FROM identity_decisions WHERE id = ?1",
            [decision_id.as_str()],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(IdentityMutationRecord {
            decision_id,
            primary_identity_id: left_identity_id,
            secondary_identity_id: Some(right_identity_id),
            occurrence_id: None,
            action: match action {
                IdentityCandidateAction::Confirm => "confirm_match",
                IdentityCandidateAction::Reject => "reject_match",
                IdentityCandidateAction::KeepSeparate => "keep_separate",
            }
            .to_owned(),
            created_at,
        })
    }

    pub fn merge_identity_records(
        &self,
        primary_identity_id: &str,
        secondary_identity_id: &str,
        reason: Option<&str>,
    ) -> Result<IdentityMutationRecord, PersistenceError> {
        validate_optional_reason(reason)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let workspace_id: String = transaction
            .query_row(
                "SELECT workspace_id
                 FROM resolved_identities
                 WHERE id = ?1 AND lifecycle_status = 'active'",
                [primary_identity_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let decision_id = merge_identities(
            &transaction,
            &workspace_id,
            primary_identity_id,
            secondary_identity_id,
            "user",
            None,
            reason.unwrap_or("user merged semantic identities"),
        )?;
        let created_at: String = transaction.query_row(
            "SELECT created_at FROM identity_decisions WHERE id = ?1",
            [decision_id.as_str()],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(IdentityMutationRecord {
            decision_id,
            primary_identity_id: primary_identity_id.to_owned(),
            secondary_identity_id: Some(secondary_identity_id.to_owned()),
            occurrence_id: None,
            action: "user_merge".to_owned(),
            created_at,
        })
    }

    pub fn unlink_identity_occurrence(
        &self,
        identity_id: &str,
        occurrence_id: &str,
        reason: Option<&str>,
    ) -> Result<IdentityMutationRecord, PersistenceError> {
        validate_optional_reason(reason)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (workspace_id, file_id, identity_type, original_value, normalized_value, confidence): (
            String,
            String,
            String,
            String,
            String,
            f64,
        ) = transaction
            .query_row(
                "SELECT
                    workspace_id, file_id, occurrence_type, original_value,
                    normalized_value, confidence
                 FROM identity_occurrences
                 WHERE id = ?1 AND identity_id = ?2 AND active = 1",
                params![occurrence_id, identity_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let occurrence_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM identity_occurrences
             WHERE identity_id = ?1 AND active = 1",
            [identity_id],
            |row| row.get(0),
        )?;
        if occurrence_count < 2 {
            return Err(PersistenceError::IdentityConflict);
        }
        let split_identity_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO resolved_identities(
                id, workspace_id, identity_type, display_name,
                normalized_display_name, resolution_status, lifecycle_status,
                user_locked, confidence, creation_source, resolver_version
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, 'user_confirmed', 'active',
                1, ?6, 'split', ?7
             )",
            params![
                split_identity_id,
                workspace_id,
                identity_type,
                original_value,
                normalized_value,
                confidence,
                RESOLVER_VERSION,
            ],
        )?;
        transaction.execute(
            "UPDATE identity_occurrences SET identity_id = ?2 WHERE id = ?1",
            params![occurrence_id, split_identity_id],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO identity_aliases(
                id, identity_id, occurrence_id, original_value,
                normalized_value, legal_suffix, source
             )
             SELECT ?1, ?2, id, original_value, normalized_value, legal_suffix, 'split'
             FROM identity_occurrences WHERE id = ?3",
            params![Uuid::now_v7().to_string(), split_identity_id, occurrence_id,],
        )?;
        let roles = roles_for_occurrence(&transaction, occurrence_id)?;
        for (role, role_confidence) in roles {
            transaction.execute(
                "INSERT OR IGNORE INTO identity_roles(
                    id, identity_id, role, occurrence_id, status, confidence
                 ) VALUES (?1, ?2, ?3, ?4, 'user_confirmed', ?5)",
                params![
                    Uuid::now_v7().to_string(),
                    split_identity_id,
                    role,
                    occurrence_id,
                    role_confidence,
                ],
            )?;
        }
        transaction.execute(
            "UPDATE identity_aliases
             SET active = 0
             WHERE occurrence_id = ?1 AND identity_id <> ?2 AND active = 1",
            params![occurrence_id, split_identity_id],
        )?;
        transaction.execute(
            "UPDATE identity_roles
             SET active = 0,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE occurrence_id = ?1 AND identity_id <> ?2 AND active = 1",
            params![occurrence_id, split_identity_id],
        )?;
        transaction.execute(
            "UPDATE resolved_identities
             SET user_locked = 1,
                 resolution_status = 'user_confirmed',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            [identity_id],
        )?;
        let decision_id = Uuid::now_v7().to_string();
        let decision_reason = reason.unwrap_or("user unlinked one semantic occurrence");
        transaction.execute(
            "INSERT INTO identity_decisions(
                id, workspace_id, decision_type, decision_source,
                primary_identity_id, secondary_identity_id, occurrence_id,
                reason, resolver_version
             ) VALUES (
                ?1, ?2, 'unlink_occurrence', 'user', ?3, ?4, ?5, ?6, ?7
             )",
            params![
                decision_id,
                workspace_id,
                identity_id,
                split_identity_id,
                occurrence_id,
                decision_reason,
                RESOLVER_VERSION,
            ],
        )?;
        transaction.execute(
            "UPDATE identity_merge_history
             SET restored_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE occurrence_id = ?1
               AND to_identity_id = ?2
               AND restored_at IS NULL",
            params![occurrence_id, identity_id],
        )?;
        insert_rejection_constraint(
            &transaction,
            &workspace_id,
            identity_id,
            &split_identity_id,
            &decision_id,
            decision_reason,
        )?;
        insert_audit(
            &transaction,
            &workspace_id,
            "occurrence_unlinked",
            "user",
            Some(identity_id),
            Some(&split_identity_id),
            None,
            Some(occurrence_id),
            Some(decision_reason),
        )?;
        refresh_file_relationships(&transaction, &file_id)?;
        let created_at: String = transaction.query_row(
            "SELECT created_at FROM identity_decisions WHERE id = ?1",
            [decision_id.as_str()],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(IdentityMutationRecord {
            decision_id,
            primary_identity_id: identity_id.to_owned(),
            secondary_identity_id: Some(split_identity_id),
            occurrence_id: Some(occurrence_id.to_owned()),
            action: "unlink_occurrence".to_owned(),
            created_at,
        })
    }

    pub fn identity_review_groups(
        &self,
        workspace_id: WorkspaceId,
        status: &str,
        limit: usize,
        offset: usize,
    ) -> Result<IdentityReviewPageRecord, PersistenceError> {
        if !matches!(status, "needs_review" | "resolved" | "ignored" | "all") {
            return Err(PersistenceError::InvalidIdentityInput);
        }
        let connection = self.lock()?;
        let total: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM identity_review_groups
             WHERE workspace_id = ?1
               AND (?2 = 'all' OR status = ?2)",
            params![workspace_id.to_string(), status],
            |row| row.get(0),
        )?;
        let limit = limit.clamp(1, 50);
        let mut statement = connection.prepare(
            "SELECT id
             FROM identity_review_groups
             WHERE workspace_id = ?1
               AND (?2 = 'all' OR status = ?2)
             ORDER BY
                CASE status WHEN 'needs_review' THEN 0 ELSE 1 END,
                max_score DESC,
                updated_at DESC,
                id
             LIMIT ?3 OFFSET ?4",
        )?;
        let ids = statement
            .query_map(
                params![
                    workspace_id.to_string(),
                    status,
                    sql_usize(limit)?,
                    sql_usize(offset)?,
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let items = ids
            .iter()
            .map(|id| identity_review_group_by_id(&connection, id))
            .collect::<Result<Vec<_>, _>>()?;
        let total = from_sql_u64(total)?;
        Ok(IdentityReviewPageRecord {
            total,
            limit,
            offset,
            has_more: u64::try_from(offset)
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(items.len()).unwrap_or(u64::MAX))
                < total,
            items,
        })
    }

    pub fn identity_detail(
        &self,
        identity_id: &str,
    ) -> Result<IdentityDetailRecord, PersistenceError> {
        let connection = self.lock()?;
        let canonical_id = canonical_identity_id(&connection, identity_id)?;
        let identity_id = canonical_id.as_str();
        let identity = identity_summary(&connection, identity_id)?;
        let (resolver_version, updated_at): (String, String) = connection.query_row(
            "SELECT resolver_version, updated_at
             FROM resolved_identities WHERE id = ?1",
            [identity_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        const MAX_IDENTITY_OCCURRENCES: usize = 100;
        let occurrence_total: i64 = connection.query_row(
            "SELECT COUNT(*) FROM identity_occurrences WHERE identity_id = ?1",
            [identity_id],
            |row| row.get(0),
        )?;
        let mut occurrence_statement = connection.prepare(
            "SELECT
                io.id, io.file_id, fl.basename, fl.relative_path,
                io.original_value, io.normalized_value, io.confidence,
                (
                    SELECT role FROM identity_roles ir
                    WHERE ir.occurrence_id = io.id AND ir.active = 1
                    ORDER BY role LIMIT 1
                ),
                io.analyzer_version, io.active
             FROM identity_occurrences io
             JOIN file_versions fv ON fv.id = io.file_version_id
             JOIN file_locations fl ON fl.id = fv.location_id
             WHERE io.identity_id = ?1
             ORDER BY io.active DESC, io.last_observed_at DESC, io.id
             LIMIT ?2",
        )?;
        let occurrences = occurrence_statement
            .query_map(
                params![identity_id, (MAX_IDENTITY_OCCURRENCES as i64)],
                identity_occurrence_record_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let occurrences_truncated = (occurrence_total as usize) > occurrences.len();
        let mut identifier_statement = connection.prepare(
            "SELECT DISTINCT signal.signal_kind, signal.normalized_value
             FROM identity_occurrence_signals signal
             JOIN identity_occurrences occurrence
               ON occurrence.id = signal.occurrence_id
             WHERE occurrence.identity_id = ?1
               AND occurrence.active = 1
               AND signal.signal_kind IN (
                    'company_identifier', 'vat_identifier', 'email',
                    'domain', 'phone', 'account_identifier', 'project_reference'
               )
             ORDER BY signal.signal_kind, signal.normalized_value
             LIMIT 100",
        )?;
        let identifiers = identifier_statement
            .query_map([identity_id], |row| {
                Ok(IdentityIdentifierRecord {
                    kind: row.get(0)?,
                    value: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let relationships = relationships_for_identity(&connection, identity_id)?;
        let project_ids = project_identity_ids_for_identity(&connection, identity_id)?;
        let projects = project_ids
            .iter()
            .map(|id| identity_summary(&connection, id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut audit_statement = connection.prepare(
            "SELECT
                event_type, decision_source, related_identity_id, reason, created_at
             FROM identity_audit_events
             WHERE identity_id = ?1 OR related_identity_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT 100",
        )?;
        let audit_events = audit_statement
            .query_map([identity_id], |row| {
                Ok(IdentityAuditEventRecord {
                    event_type: row.get(0)?,
                    decision_source: row.get(1)?,
                    related_identity_id: row.get(2)?,
                    reason: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(IdentityDetailRecord {
            identity,
            occurrences,
            occurrence_total: from_sql_u64(occurrence_total)?,
            occurrences_truncated,
            identifiers,
            relationships,
            projects,
            audit_events,
            resolver_version,
            updated_at,
        })
    }
}

pub(super) fn relationships_for_file(
    connection: &Connection,
    file_id: &str,
) -> Result<Vec<IdentityRelationshipRecord>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            rel.id, rel.relationship_type, target.id, target.display_name,
            target.identity_type, rel.confidence, rel.status,
            rel.user_confirmation_state
         FROM identity_relationships rel
         JOIN resolved_identities target ON target.id = rel.target_identity_id
         WHERE rel.source_kind = 'file'
           AND rel.source_file_id = ?1
           AND rel.active = 1
           AND target.lifecycle_status = 'active'
         ORDER BY rel.relationship_type, target.display_name, rel.id",
    )?;
    let mut rows = statement.query([file_id])?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        let relationship_id: String = row.get(0)?;
        output.push(IdentityRelationshipRecord {
            relationship_id: relationship_id.clone(),
            relationship_type: row.get(1)?,
            identity_id: row.get(2)?,
            display_name: row.get(3)?,
            identity_type: row.get(4)?,
            confidence: row.get::<_, f64>(5)? as f32,
            status: row.get(6)?,
            user_confirmation_state: row.get(7)?,
            evidence: relationship_evidence(connection, &relationship_id)?,
        });
    }
    Ok(output)
}

fn identity_run_by_id(
    connection: &Connection,
    run_id: &str,
) -> Result<IdentityResolverRunRecord, PersistenceError> {
    connection
        .query_row(
            "SELECT
                id, workspace_id, trigger_kind, status, resolver_id, resolver_version,
                files_considered, occurrences_processed, blocking_memberships,
                comparisons, candidates_created, auto_links_created,
                started_at, completed_at
             FROM identity_resolver_runs
             WHERE id = ?1",
            [run_id],
            |row| {
                let workspace_id = row.get::<_, String>(1)?.parse().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(IdentityResolverRunRecord {
                    run_id: row.get(0)?,
                    workspace_id,
                    trigger_kind: row.get(2)?,
                    status: row.get(3)?,
                    resolver_id: row.get(4)?,
                    resolver_version: row.get(5)?,
                    files_considered: sql_row_u64(row, 6)?,
                    occurrences_processed: sql_row_u64(row, 7)?,
                    blocking_memberships: sql_row_u64(row, 8)?,
                    comparisons: sql_row_u64(row, 9)?,
                    candidates_created: sql_row_u64(row, 10)?,
                    auto_links_created: sql_row_u64(row, 11)?,
                    started_at: row.get(12)?,
                    completed_at: row.get(13)?,
                })
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn semantic_identity_sources(
    transaction: &Transaction<'_>,
    analysis_id: &str,
    analyzer_version: &str,
) -> Result<Vec<SemanticSource>, PersistenceError> {
    let mut sources = Vec::new();
    let mut field_statement = transaction.prepare(
        "SELECT
            sf.id, sf.field_key,
            COALESCE(c.display_value, sf.display_value),
            sf.confidence, sf.source_method, sf.analyzer_version,
            MIN(ev.start_offset),
            CASE WHEN c.id IS NULL THEN 0 ELSE 1 END
         FROM semantic_fields sf
         LEFT JOIN semantic_user_corrections c
           ON c.file_id = (
                SELECT file_id FROM semantic_analyses WHERE id = sf.analysis_id
           )
          AND c.field_key = sf.field_key
          AND c.active = 1
         LEFT JOIN semantic_evidence ev ON ev.field_id = sf.id
         WHERE sf.analysis_id = ?1
           AND sf.is_primary = 1
           AND sf.field_key IN (
                'supplier_candidate', 'customer_candidate', 'issuer',
                'project_reference_candidate'
           )
           AND COALESCE(c.display_value, sf.display_value) IS NOT NULL
         GROUP BY
            sf.id, sf.field_key, c.display_value, sf.display_value,
            sf.confidence, sf.source_method, sf.analyzer_version, c.id
         ORDER BY sf.field_key, sf.id",
    )?;
    let rows = field_statement.query_map([analysis_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, i64>(7)? != 0,
        ))
    })?;
    for row in rows {
        let (
            field_id,
            field_key,
            value,
            confidence,
            source_method,
            field_analyzer,
            start_offset,
            corrected,
        ) = row?;
        let (identity_type, role) = match field_key.as_str() {
            "supplier_candidate" => (IdentityType::Organization, Some(IdentityRole::Supplier)),
            "customer_candidate" => (IdentityType::Organization, Some(IdentityRole::Customer)),
            "issuer" => (IdentityType::Organization, None),
            "project_reference_candidate" => (IdentityType::Project, None),
            _ => continue,
        };
        if let Some(name) = normalize_name(&value) {
            sources.push(SemanticSource {
                semantic_entity_id: None,
                semantic_field_id: Some(field_id),
                identity_type,
                role,
                original_value: value,
                normalized_value: name.exact,
                normalized_core: name.core,
                legal_suffix: name.legal_suffix,
                confidence: if corrected { 1.0 } else { confidence as f32 },
                source_method: if corrected {
                    "user_correction".to_owned()
                } else {
                    source_method
                },
                analyzer_version: field_analyzer,
                start_offset,
                signals: Vec::new(),
            });
        }
    }
    drop(field_statement);

    let represented = sources
        .iter()
        .map(|source| {
            (
                source.identity_type,
                source.role,
                source.normalized_value.clone(),
            )
        })
        .collect::<HashSet<_>>();
    let represented_primary_kinds = sources
        .iter()
        .filter(|source| source.role.is_some() || source.identity_type == IdentityType::Project)
        .map(|source| (source.identity_type, source.role))
        .collect::<HashSet<_>>();
    let mut entity_statement = transaction.prepare(
        "SELECT
            se.id, se.entity_type, se.original_value, se.confidence,
            se.source_method, se.analyzer_version, MIN(ev.start_offset)
         FROM semantic_entities se
         LEFT JOIN semantic_evidence ev ON ev.entity_id = se.id
         WHERE se.analysis_id = ?1
           AND se.entity_type IN (
                'organization', 'person', 'customer_candidate',
                'supplier_candidate', 'project_candidate'
           )
         GROUP BY
            se.id, se.entity_type, se.original_value, se.confidence,
            se.source_method, se.analyzer_version
         ORDER BY se.entity_type, se.id",
    )?;
    let rows = entity_statement.query_map([analysis_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<i64>>(6)?,
        ))
    })?;
    for row in rows {
        let (
            entity_id,
            entity_type,
            value,
            confidence,
            source_method,
            entity_analyzer,
            start_offset,
        ) = row?;
        let (identity_type, role) = match entity_type.as_str() {
            "organization" => (IdentityType::Organization, None),
            "person" => (IdentityType::Person, None),
            "customer_candidate" => (IdentityType::Organization, Some(IdentityRole::Customer)),
            "supplier_candidate" => (IdentityType::Organization, Some(IdentityRole::Supplier)),
            "project_candidate" => (IdentityType::Project, None),
            _ => continue,
        };
        let Some(name) = normalize_name(&value) else {
            continue;
        };
        if represented.contains(&(identity_type, role, name.exact.clone()))
            || represented_primary_kinds.contains(&(identity_type, role))
        {
            continue;
        }
        sources.push(SemanticSource {
            semantic_entity_id: Some(entity_id),
            semantic_field_id: None,
            identity_type,
            role,
            original_value: value,
            normalized_value: name.exact,
            normalized_core: name.core,
            legal_suffix: name.legal_suffix,
            confidence: confidence as f32,
            source_method,
            analyzer_version: if entity_analyzer.is_empty() {
                analyzer_version.to_owned()
            } else {
                entity_analyzer
            },
            start_offset,
            signals: Vec::new(),
        });
    }
    sources.sort_by(|left, right| {
        left.identity_type
            .database_name()
            .cmp(right.identity_type.database_name())
            .then_with(|| left.normalized_value.cmp(&right.normalized_value))
            .then_with(|| {
                left.role
                    .map(IdentityRole::database_name)
                    .cmp(&right.role.map(IdentityRole::database_name))
            })
    });
    sources.dedup_by(|left, right| {
        left.identity_type == right.identity_type
            && left.role == right.role
            && left.normalized_value == right.normalized_value
    });
    Ok(sources)
}

fn attach_semantic_signals(
    transaction: &Transaction<'_>,
    analysis_id: &str,
    relative_path: &str,
    sources: &mut [SemanticSource],
) -> Result<(), PersistenceError> {
    let mut signals = semantic_auxiliary_signals(transaction, analysis_id)?;
    for source in sources.iter() {
        if source.identity_type == IdentityType::Project
            && source
                .original_value
                .chars()
                .any(|character| character.is_ascii_digit())
            && let Some(normalized_value) = normalize_company_identifier(&source.original_value)
        {
            signals.push(SourceSignal {
                kind: SignalKind::ProjectReference,
                original_value: source.original_value.clone(),
                normalized_value,
                semantic_entity_id: source.semantic_entity_id.clone(),
                semantic_field_id: source.semantic_field_id.clone(),
                confidence: source.confidence,
                source_method: source.source_method.clone(),
                analyzer_version: source.analyzer_version.clone(),
                start_offset: source.start_offset,
            });
        }
    }
    let customer_names = sources
        .iter()
        .filter(|source| source.role == Some(IdentityRole::Customer))
        .map(|source| source.normalized_value.clone())
        .collect::<BTreeSet<_>>();
    if customer_names.len() == 1 {
        let customer = customer_names.iter().next().cloned().unwrap_or_default();
        for source in sources
            .iter_mut()
            .filter(|source| source.identity_type == IdentityType::Project && !customer.is_empty())
        {
            source.signals.push(SourceSignal {
                kind: SignalKind::CustomerIdentity,
                original_value: customer.clone(),
                normalized_value: customer.clone(),
                semantic_entity_id: source.semantic_entity_id.clone(),
                semantic_field_id: source.semantic_field_id.clone(),
                confidence: source.confidence.min(0.9),
                source_method: "same_document_customer".to_owned(),
                analyzer_version: source.analyzer_version.clone(),
                start_offset: source.start_offset,
            });
        }
    }
    if let Some(parent) = relative_path.rsplit_once('/').map(|(parent, _)| parent)
        && let Some(normalized) = normalize_name(parent)
    {
        for source in sources.iter_mut() {
            source.signals.push(SourceSignal {
                kind: SignalKind::PathContext,
                original_value: truncate_database_text(parent, 512),
                normalized_value: normalized.exact.clone(),
                semantic_entity_id: source.semantic_entity_id.clone(),
                semantic_field_id: source.semantic_field_id.clone(),
                confidence: 0.2,
                source_method: "path_context".to_owned(),
                analyzer_version: source.analyzer_version.clone(),
                start_offset: None,
            });
        }
    }

    for signal in signals {
        let eligible = sources
            .iter()
            .enumerate()
            .filter(|(_, source)| signal_allowed(source.identity_type, signal.kind))
            .map(|(index, source)| {
                (
                    index,
                    source
                        .start_offset
                        .zip(signal.start_offset)
                        .map(|(left, right)| (left - right).abs()),
                )
            })
            .collect::<Vec<_>>();
        let target = if eligible.len() == 1 {
            eligible.first().map(|(index, _)| *index)
        } else {
            let mut positioned = eligible
                .iter()
                .filter_map(|(index, distance)| distance.map(|distance| (*index, distance)))
                .filter(|(_, distance)| *distance <= SIGNAL_PROXIMITY_CHARS)
                .collect::<Vec<_>>();
            positioned.sort_by_key(|(_, distance)| *distance);
            match positioned.as_slice() {
                [(index, _), ..]
                    if positioned.get(1).map(|item| item.1) != Some(positioned[0].1) =>
                {
                    Some(*index)
                }
                _ => None,
            }
        };
        if let Some(target) = target {
            sources[target].signals.push(signal);
        }
    }
    for source in sources.iter_mut() {
        source.signals.sort_by(|left, right| {
            left.kind
                .database_name()
                .cmp(right.kind.database_name())
                .then_with(|| left.normalized_value.cmp(&right.normalized_value))
        });
        source.signals.dedup_by(|left, right| {
            left.kind == right.kind && left.normalized_value == right.normalized_value
        });
    }
    Ok(())
}

fn semantic_auxiliary_signals(
    transaction: &Transaction<'_>,
    analysis_id: &str,
) -> Result<Vec<SourceSignal>, PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT
            se.id, se.entity_type, se.original_value, se.confidence,
            se.source_method, se.analyzer_version, MIN(ev.start_offset)
         FROM semantic_entities se
         LEFT JOIN semantic_evidence ev ON ev.entity_id = se.id
         WHERE se.analysis_id = ?1
           AND se.entity_type IN ('siret_or_company_id', 'email', 'phone', 'address')
         GROUP BY
            se.id, se.entity_type, se.original_value, se.confidence,
            se.source_method, se.analyzer_version
         ORDER BY se.entity_type, se.id",
    )?;
    let rows = statement.query_map([analysis_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<i64>>(6)?,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let (
            entity_id,
            entity_type,
            original_value,
            confidence,
            source_method,
            analyzer_version,
            start_offset,
        ) = row?;
        let (kind, normalized_value) = match entity_type.as_str() {
            "siret_or_company_id" => (
                SignalKind::CompanyIdentifier,
                normalize_company_identifier(&original_value),
            ),
            "email" => (SignalKind::Email, normalize_email(&original_value)),
            "phone" => (SignalKind::Phone, normalize_phone(&original_value)),
            "address" => (
                SignalKind::Address,
                normalize_name(&original_value).map(|name| name.exact),
            ),
            _ => continue,
        };
        let Some(normalized_value) = normalized_value else {
            continue;
        };
        output.push(SourceSignal {
            kind,
            original_value: original_value.clone(),
            normalized_value: normalized_value.clone(),
            semantic_entity_id: Some(entity_id.clone()),
            semantic_field_id: None,
            confidence: confidence as f32,
            source_method: source_method.clone(),
            analyzer_version: analyzer_version.clone(),
            start_offset,
        });
        if kind == SignalKind::Email
            && let Some((_, domain)) = normalized_value.split_once('@')
            && let Some(domain) = normalize_domain(domain)
        {
            output.push(SourceSignal {
                kind: SignalKind::Domain,
                original_value: domain.clone(),
                normalized_value: domain,
                semantic_entity_id: Some(entity_id),
                semantic_field_id: None,
                confidence: confidence as f32,
                source_method,
                analyzer_version,
                start_offset,
            });
        }
    }
    let mut project_statement = transaction.prepare(
        "SELECT
            sf.id, sf.display_value, sf.confidence, sf.source_method,
            sf.analyzer_version, MIN(ev.start_offset)
         FROM semantic_fields sf
         LEFT JOIN semantic_evidence ev ON ev.field_id = sf.id
         WHERE sf.analysis_id = ?1
           AND sf.field_key = 'project_reference_candidate'
           AND sf.display_value IS NOT NULL
         GROUP BY
            sf.id, sf.display_value, sf.confidence, sf.source_method,
            sf.analyzer_version
         ORDER BY sf.candidate_rank, sf.id",
    )?;
    let project_rows = project_statement
        .query_map([analysis_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (field_id, value, confidence, source_method, analyzer_version, start_offset) in project_rows
    {
        if !value.chars().any(|character| character.is_ascii_digit()) {
            continue;
        }
        let Some(normalized_value) = normalize_company_identifier(&value) else {
            continue;
        };
        output.push(SourceSignal {
            kind: SignalKind::ProjectReference,
            original_value: value,
            normalized_value,
            semantic_entity_id: None,
            semantic_field_id: Some(field_id),
            confidence: confidence as f32,
            source_method,
            analyzer_version,
            start_offset,
        });
    }
    Ok(output)
}

fn signal_allowed(identity_type: IdentityType, signal_kind: SignalKind) -> bool {
    match identity_type {
        IdentityType::Organization => matches!(
            signal_kind,
            SignalKind::CompanyIdentifier
                | SignalKind::VatIdentifier
                | SignalKind::Email
                | SignalKind::Domain
                | SignalKind::Phone
                | SignalKind::Address
                | SignalKind::AccountIdentifier
        ),
        IdentityType::Person => matches!(
            signal_kind,
            SignalKind::Email | SignalKind::Phone | SignalKind::Address
        ),
        IdentityType::Project => matches!(
            signal_kind,
            SignalKind::ProjectReference | SignalKind::Address
        ),
    }
}

fn source_key(file_id: &str, source: &SemanticSource) -> Result<String, PersistenceError> {
    let value = format!(
        "{file_id}:{}:{}:{}",
        source.identity_type.database_name(),
        source.role.map_or("none", IdentityRole::database_name),
        source.normalized_value
    );
    if value.chars().count() > 768 {
        return Err(PersistenceError::InvalidIdentityInput);
    }
    Ok(value)
}

fn insert_source_signal(
    transaction: &Transaction<'_>,
    occurrence_id: &str,
    signal: &SourceSignal,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT OR IGNORE INTO identity_occurrence_signals(
            id, occurrence_id, signal_kind, original_value, normalized_value,
            semantic_entity_id, semantic_field_id, confidence, source_method,
            analyzer_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            Uuid::now_v7().to_string(),
            occurrence_id,
            signal.kind.database_name(),
            truncate_database_text(&signal.original_value, 512),
            truncate_database_text(&signal.normalized_value, 512),
            signal.semantic_entity_id,
            signal.semantic_field_id,
            f64::from(signal.confidence.clamp(0.0, 1.0)),
            truncate_database_text(&signal.source_method, 128),
            truncate_database_text(&signal.analyzer_version, 64),
        ],
    )?;
    Ok(())
}

fn identity_occurrence_from_connection(
    connection: &Connection,
    occurrence_id: &str,
) -> Result<IdentityOccurrence, PersistenceError> {
    let (
        file_id,
        semantic_entity_id,
        semantic_field_id,
        occurrence_type,
        original_value,
        confidence,
        analyzer_version,
        role,
    ): IdentityOccurrenceSourceRow = connection
        .query_row(
            "SELECT
                io.file_id, io.semantic_entity_id, io.semantic_field_id,
                io.occurrence_type, io.original_value, io.confidence,
                io.analyzer_version,
                (
                    SELECT role FROM identity_roles ir
                    WHERE ir.occurrence_id = io.id AND ir.active = 1
                    ORDER BY role LIMIT 1
                )
             FROM identity_occurrences io
             WHERE io.id = ?1 AND io.active = 1",
            [occurrence_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let identity_type = parse_identity_type(&occurrence_type)?;
    let role = role.as_deref().map(parse_identity_role).transpose()?;
    let mut statement = connection.prepare(
        "SELECT signal_kind, original_value
         FROM identity_occurrence_signals
         WHERE occurrence_id = ?1 AND signal_kind <> 'name'
         ORDER BY signal_kind, normalized_value",
    )?;
    let signal_rows = statement
        .query_map([occurrence_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut signals = Vec::with_capacity(signal_rows.len());
    for (kind, value) in signal_rows {
        signals.push((parse_signal_kind(&kind)?, value));
    }
    IdentityOccurrence::new(
        occurrence_id,
        &file_id,
        semantic_entity_id,
        semantic_field_id,
        identity_type,
        role,
        &original_value,
        confidence as f32,
        &analyzer_version,
        signals,
    )
    .map_err(|_| PersistenceError::InvalidIdentityInput)
}

fn refresh_file_relationships(
    transaction: &Transaction<'_>,
    file_id: &str,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "UPDATE identity_relationships
         SET active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE source_kind = 'file'
           AND source_file_id = ?1
           AND user_confirmation_state IS NULL",
        [file_id],
    )?;
    transaction.execute(
        "UPDATE identity_relationships
         SET active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE source_kind = 'identity'
           AND relationship_type = 'project_customer'
           AND user_confirmation_state IS NULL
           AND source_identity_id IN (
                SELECT identity_id FROM identity_occurrences
                WHERE file_id = ?1 AND occurrence_type = 'project'
           )",
        [file_id],
    )?;
    let mut statement = transaction.prepare(
        "SELECT
            io.id, io.workspace_id, io.identity_id, io.occurrence_type,
            io.confidence,
            (SELECT role FROM identity_roles ir
             WHERE ir.occurrence_id = io.id AND ir.active = 1
             ORDER BY role LIMIT 1),
            ri.resolution_status
         FROM identity_occurrences io
         JOIN resolved_identities ri ON ri.id = io.identity_id
         WHERE io.file_id = ?1
           AND io.active = 1
           AND ri.lifecycle_status = 'active'
         ORDER BY io.id",
    )?;
    let rows = statement
        .query_map([file_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (
        occurrence_id,
        workspace_id,
        identity_id,
        occurrence_type,
        confidence,
        role,
        resolution_status,
    ) in rows
    {
        let relationship_type = if occurrence_type == "project" {
            Some("file_project")
        } else {
            match role.as_deref() {
                Some("customer") => Some("file_customer"),
                Some("supplier") => Some("file_supplier"),
                _ => None,
            }
        };
        let Some(relationship_type) = relationship_type else {
            continue;
        };
        let relationship_status = match resolution_status.as_str() {
            "user_confirmed" => "user_confirmed",
            "auto_linked" => "auto_linked",
            _ => "candidate",
        };
        let existing: Option<(String, Option<String>)> = transaction
            .query_row(
                "SELECT id, user_confirmation_state
                 FROM identity_relationships
                 WHERE workspace_id = ?1
                   AND source_file_id = ?2
                   AND target_identity_id = ?3
                   AND relationship_type = ?4
                 ORDER BY active DESC LIMIT 1",
                params![workspace_id, file_id, identity_id, relationship_type,],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let relationship_id = if let Some((relationship_id, user_state)) = existing {
            transaction.execute(
                "UPDATE identity_relationships
                 SET confidence = MAX(confidence, ?2),
                     status = CASE
                        WHEN ?3 = 'confirmed' THEN 'user_confirmed'
                        WHEN ?3 = 'rejected' THEN 'user_rejected'
                        ELSE ?4
                     END,
                     resolver_version = ?5,
                     active = 1,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1",
                params![
                    relationship_id,
                    confidence,
                    user_state,
                    relationship_status,
                    RESOLVER_VERSION,
                ],
            )?;
            relationship_id
        } else {
            let relationship_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO identity_relationships(
                    id, workspace_id, source_kind, source_file_id,
                    target_identity_id, relationship_type, confidence, status,
                    creation_source, resolver_version, active
                 ) VALUES (
                    ?1, ?2, 'file', ?3, ?4, ?5, ?6, ?7,
                    'semantic_occurrence', ?8, 1
                 )",
                params![
                    relationship_id,
                    workspace_id,
                    file_id,
                    identity_id,
                    relationship_type,
                    confidence,
                    relationship_status,
                    RESOLVER_VERSION,
                ],
            )?;
            relationship_id
        };
        transaction.execute(
            "INSERT OR IGNORE INTO identity_relationship_evidence(
                id, relationship_id, occurrence_id, evidence_type,
                explanation, exact_text
             )
             SELECT ?1, ?2, ?3, 'semantic_occurrence',
                    'relationship is backed by a preserved semantic occurrence',
                    original_value
             FROM identity_occurrences WHERE id = ?3",
            params![Uuid::now_v7().to_string(), relationship_id, occurrence_id,],
        )?;
    }
    let mut project_statement = transaction.prepare(
        "SELECT DISTINCT
            project.workspace_id, project.identity_id, customer.identity_id,
            project.id, MIN(project.confidence, customer.confidence),
            customer_signal.normalized_value
         FROM identity_occurrences project
         JOIN identity_occurrence_signals customer_signal
           ON customer_signal.occurrence_id = project.id
          AND customer_signal.signal_kind = 'customer_identity'
         JOIN identity_occurrences customer
           ON customer.file_id = project.file_id
          AND customer.workspace_id = project.workspace_id
          AND customer.occurrence_type = 'organization'
          AND customer.normalized_value = customer_signal.normalized_value
          AND customer.active = 1
         JOIN identity_roles customer_role
           ON customer_role.occurrence_id = customer.id
          AND customer_role.role = 'customer'
          AND customer_role.active = 1
         WHERE project.file_id = ?1
           AND project.occurrence_type = 'project'
           AND project.active = 1
           AND project.identity_id <> customer.identity_id
         ORDER BY project.identity_id, customer.identity_id",
    )?;
    let project_pairs = project_statement
        .query_map([file_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(project_statement);
    for (
        workspace_id,
        project_identity_id,
        customer_identity_id,
        project_occurrence_id,
        confidence,
        customer_name,
    ) in project_pairs
    {
        let existing: Option<String> = transaction
            .query_row(
                "SELECT id FROM identity_relationships
                 WHERE workspace_id = ?1
                   AND source_identity_id = ?2
                   AND target_identity_id = ?3
                   AND relationship_type = 'project_customer'
                 ORDER BY active DESC LIMIT 1",
                params![workspace_id, project_identity_id, customer_identity_id],
                |row| row.get(0),
            )
            .optional()?;
        let relationship_id = if let Some(relationship_id) = existing {
            transaction.execute(
                "UPDATE identity_relationships
                 SET confidence = MAX(confidence, ?2),
                     status = CASE
                        WHEN user_confirmation_state = 'confirmed' THEN 'user_confirmed'
                        WHEN user_confirmation_state = 'rejected' THEN 'user_rejected'
                        ELSE 'candidate'
                     END,
                     active = 1,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1",
                params![relationship_id, confidence],
            )?;
            relationship_id
        } else {
            let relationship_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO identity_relationships(
                    id, workspace_id, source_kind, source_identity_id,
                    target_identity_id, relationship_type, confidence, status,
                    creation_source, resolver_version
                 ) VALUES (
                    ?1, ?2, 'identity', ?3, ?4, 'project_customer', ?5,
                    'candidate', 'resolver', ?6
                 )",
                params![
                    relationship_id,
                    workspace_id,
                    project_identity_id,
                    customer_identity_id,
                    confidence,
                    RESOLVER_VERSION,
                ],
            )?;
            relationship_id
        };
        transaction.execute(
            "INSERT OR IGNORE INTO identity_relationship_evidence(
                id, relationship_id, occurrence_id, evidence_type,
                explanation, exact_text
             ) VALUES (
                ?1, ?2, ?3, 'same_document_customer',
                'project occurrence explicitly carries the same customer association',
                ?4
             )",
            params![
                Uuid::now_v7().to_string(),
                relationship_id,
                project_occurrence_id,
                customer_name,
            ],
        )?;
    }
    Ok(())
}

fn insert_candidate_evidence(
    transaction: &Transaction<'_>,
    candidate_id: &str,
    evidence: &IdentityEvidence,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT INTO identity_candidate_evidence(
            id, candidate_id, evidence_type, strength, polarity,
            left_value, right_value, weight, explanation
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            Uuid::now_v7().to_string(),
            candidate_id,
            truncate_database_text(&evidence.evidence_type, 64),
            evidence.strength.database_name(),
            match evidence.polarity {
                EvidencePolarity::Supports => "supports",
                EvidencePolarity::Conflicts => "conflicts",
            },
            truncate_database_text(&evidence.left_value, 512),
            truncate_database_text(&evidence.right_value, 512),
            f64::from(evidence.weight.clamp(0.0, 1.0)),
            truncate_database_text(&evidence.explanation, 512),
        ],
    )?;
    Ok(())
}

fn merge_identities(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    requested_primary_id: &str,
    requested_secondary_id: &str,
    decision_source: &str,
    candidate_id: Option<&str>,
    reason: &str,
) -> Result<String, PersistenceError> {
    if !matches!(decision_source, "resolver" | "user") {
        return Err(PersistenceError::InvalidIdentityInput);
    }
    let primary_id = canonical_identity_id(transaction, requested_primary_id)?;
    let secondary_id = canonical_identity_id(transaction, requested_secondary_id)?;
    if primary_id == secondary_id {
        return Err(PersistenceError::IdentityConflict);
    }
    let primary = identity_merge_state(transaction, &primary_id)?;
    let secondary = identity_merge_state(transaction, &secondary_id)?;
    if primary.0 != workspace_id
        || secondary.0 != workspace_id
        || primary.1 != secondary.1
        || primary.2 != "active"
        || secondary.2 != "active"
    {
        return Err(PersistenceError::IdentityConflict);
    }
    if decision_source == "resolver" && (primary.3 || secondary.3) {
        return Err(PersistenceError::IdentityConflict);
    }
    let (target_id, source_id) = if decision_source == "user" {
        (primary_id, secondary_id)
    } else {
        ordered_pair(&primary_id, &secondary_id)
    };
    let evidence_reason = if let Some(candidate_id) = candidate_id {
        candidate_evidence_summary(transaction, candidate_id)?.map_or_else(
            || reason.to_owned(),
            |summary| format!("{reason}; {summary}"),
        )
    } else {
        reason.to_owned()
    };
    let decision_id = Uuid::now_v7().to_string();
    let moved_occurrences = occurrence_ids_for_identity(transaction, &source_id)?;
    transaction.execute(
        "INSERT INTO identity_decisions(
            id, workspace_id, decision_type, decision_source, candidate_id,
            primary_identity_id, secondary_identity_id, reason, resolver_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            decision_id,
            workspace_id,
            if decision_source == "resolver" {
                "auto_linked"
            } else {
                "user_merge"
            },
            decision_source,
            candidate_id,
            target_id,
            source_id,
            truncate_database_text(&evidence_reason, 512),
            RESOLVER_VERSION,
        ],
    )?;
    if decision_source == "user" {
        let direct_pair_key = {
            let (left, right) = ordered_pair(&target_id, &source_id);
            format!("{left}:{right}")
        };
        transaction.execute(
            "UPDATE identity_decisions
             SET reversed_by_decision_id = ?3
             WHERE id IN (
                SELECT decision_id
                FROM identity_rejection_constraints
                WHERE workspace_id = ?1 AND pair_key = ?2 AND active = 1
             )",
            params![workspace_id, direct_pair_key, decision_id],
        )?;
        transaction.execute(
            "UPDATE identity_rejection_constraints
             SET active = 0,
                 revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND pair_key = ?2 AND active = 1",
            params![workspace_id, direct_pair_key],
        )?;
    }
    for occurrence_id in &moved_occurrences {
        transaction.execute(
            "INSERT INTO identity_merge_history(
                id, decision_id, occurrence_id, from_identity_id, to_identity_id
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::now_v7().to_string(),
                decision_id,
                occurrence_id,
                source_id,
                target_id,
            ],
        )?;
    }
    copy_aliases(transaction, &source_id, &target_id)?;
    copy_roles(transaction, &source_id, &target_id)?;
    copy_rejection_constraints(transaction, workspace_id, &source_id, &target_id)?;
    transaction.execute(
        "UPDATE identity_occurrences SET identity_id = ?2 WHERE identity_id = ?1",
        params![source_id, target_id],
    )?;
    transaction.execute(
        "UPDATE OR IGNORE identity_relationships
         SET target_identity_id = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE target_identity_id = ?1 AND active = 1",
        params![source_id, target_id],
    )?;
    transaction.execute(
        "UPDATE identity_relationships
         SET active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE target_identity_id = ?1 AND active = 1",
        [source_id.as_str()],
    )?;
    transaction.execute(
        "UPDATE OR IGNORE identity_relationships
         SET source_identity_id = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE source_identity_id = ?1 AND active = 1",
        params![source_id, target_id],
    )?;
    transaction.execute(
        "UPDATE identity_relationships
         SET active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE source_identity_id = ?1 AND active = 1",
        [source_id.as_str()],
    )?;
    transaction.execute(
        "UPDATE resolved_identities
         SET resolution_status = ?2,
             user_locked = CASE WHEN ?3 = 'user' THEN 1 ELSE user_locked END,
             confidence = MAX(confidence, (
                SELECT confidence FROM resolved_identities WHERE id = ?4
             )),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        params![
            target_id,
            if decision_source == "user" {
                "user_confirmed"
            } else {
                "auto_linked"
            },
            decision_source,
            source_id,
        ],
    )?;
    transaction.execute(
        "UPDATE resolved_identities
         SET lifecycle_status = 'merged',
             merged_into_identity_id = ?2,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND lifecycle_status = 'active'",
        params![source_id, target_id],
    )?;
    for file_id in file_ids_for_occurrences(transaction, &moved_occurrences)? {
        refresh_file_relationships(transaction, &file_id)?;
    }
    insert_audit(
        transaction,
        workspace_id,
        if decision_source == "resolver" {
            "auto_linked"
        } else {
            "user_merged"
        },
        decision_source,
        Some(&target_id),
        Some(&source_id),
        candidate_id,
        None,
        Some(&evidence_reason),
    )?;
    Ok(decision_id)
}

fn candidate_evidence_summary(
    transaction: &Transaction<'_>,
    candidate_id: &str,
) -> Result<Option<String>, PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT explanation
         FROM identity_candidate_evidence
         WHERE candidate_id = ?1
         ORDER BY
            CASE strength
                WHEN 'conflicting' THEN 0
                WHEN 'very_strong' THEN 1
                WHEN 'strong' THEN 2
                WHEN 'medium' THEN 3
                ELSE 4
            END,
            evidence_type,
            id
         LIMIT 8",
    )?;
    let explanations = statement
        .query_map([candidate_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if explanations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("evidence: {}", explanations.join("; "))))
    }
}

fn reject_identity_pair(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    left_identity_id: &str,
    right_identity_id: &str,
    candidate_id: Option<&str>,
    reason: &str,
    decision_type: &str,
) -> Result<String, PersistenceError> {
    let (left_identity_id, right_identity_id) = ordered_pair(left_identity_id, right_identity_id);
    let decision_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO identity_decisions(
            id, workspace_id, decision_type, decision_source, candidate_id,
            primary_identity_id, secondary_identity_id, reason, resolver_version
         ) VALUES (?1, ?2, ?3, 'user', ?4, ?5, ?6, ?7, ?8)",
        params![
            decision_id,
            workspace_id,
            decision_type,
            candidate_id,
            left_identity_id,
            right_identity_id,
            truncate_database_text(reason, 512),
            RESOLVER_VERSION,
        ],
    )?;
    insert_rejection_constraint(
        transaction,
        workspace_id,
        &left_identity_id,
        &right_identity_id,
        &decision_id,
        reason,
    )?;
    insert_audit(
        transaction,
        workspace_id,
        "user_rejected",
        "user",
        Some(&left_identity_id),
        Some(&right_identity_id),
        candidate_id,
        None,
        Some(reason),
    )?;
    Ok(decision_id)
}

fn insert_rejection_constraint(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    left_identity_id: &str,
    right_identity_id: &str,
    decision_id: &str,
    reason: &str,
) -> Result<(), PersistenceError> {
    let (left_identity_id, right_identity_id) = ordered_pair(left_identity_id, right_identity_id);
    let pair_key = format!("{left_identity_id}:{right_identity_id}");
    transaction.execute(
        "INSERT INTO identity_rejection_constraints(
            id, workspace_id, left_identity_id, right_identity_id,
            pair_key, decision_id, reason, active
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)
         ON CONFLICT(workspace_id, pair_key) WHERE active = 1 DO UPDATE SET
            decision_id = excluded.decision_id,
            reason = excluded.reason",
        params![
            Uuid::now_v7().to_string(),
            workspace_id,
            left_identity_id,
            right_identity_id,
            pair_key,
            decision_id,
            truncate_database_text(reason, 512),
        ],
    )?;
    Ok(())
}

fn refresh_review_group(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    group_key: &str,
) -> Result<(), PersistenceError> {
    let open: Option<(i64, f64, String, String, String, String)> = transaction
        .query_row(
            "SELECT
                COUNT(*), MAX(c.score), MIN(left_identity.display_name),
                MIN(right_identity.display_name), MIN(left_identity.identity_type),
                CASE WHEN SUM(CASE WHEN c.status = 'conflicting' THEN 1 ELSE 0 END) > 0
                     THEN 'conflicting' ELSE 'candidate' END
             FROM identity_candidates c
             JOIN resolved_identities left_identity ON left_identity.id = c.left_identity_id
             JOIN resolved_identities right_identity ON right_identity.id = c.right_identity_id
             WHERE c.workspace_id = ?1
               AND c.review_group_key = ?2
               AND c.active = 1
               AND c.status IN ('candidate', 'conflicting')
               AND left_identity.lifecycle_status = 'active'
               AND right_identity.lifecycle_status = 'active'
             HAVING COUNT(*) > 0",
            params![workspace_id, group_key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((candidate_count, max_score, left_name, right_name, identity_type, group_kind)) = open
    else {
        transaction.execute(
            "UPDATE identity_review_groups
             SET status = 'resolved',
                 resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1
               AND group_key = ?2
               AND status = 'needs_review'",
            params![workspace_id, group_key],
        )?;
        return Ok(());
    };
    let review_reason = if group_kind == "conflicting" {
        "conflicting_identity_evidence"
    } else {
        match identity_type.as_str() {
            "project" => "ambiguous_project_match",
            "person" => "ambiguous_person_match",
            _ => "possible_duplicate_identity",
        }
    };
    let title = format!("{left_name} ↔ {right_name}");
    let explanation = match review_reason {
        "conflicting_identity_evidence" => {
            "Candidate identities contain conflicting evidence and were not linked."
        }
        "ambiguous_project_match" => {
            "Project occurrences share bounded signals but require confirmation."
        }
        "ambiguous_person_match" => {
            "Person occurrences are intentionally unresolved without stronger evidence."
        }
        _ => "Cross-file identity occurrences may refer to the same application identity.",
    };
    let (occurrence_count, file_count): (i64, i64) = transaction.query_row(
        "SELECT COUNT(DISTINCT io.id), COUNT(DISTINCT io.file_id)
         FROM identity_occurrences io
         WHERE io.active = 1
           AND io.identity_id IN (
                SELECT left_identity_id FROM identity_candidates
                WHERE workspace_id = ?1 AND review_group_key = ?2
                  AND status IN ('candidate', 'conflicting') AND active = 1
                UNION
                SELECT right_identity_id FROM identity_candidates
                WHERE workspace_id = ?1 AND review_group_key = ?2
                  AND status IN ('candidate', 'conflicting') AND active = 1
           )",
        params![workspace_id, group_key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let existing_id: Option<String> = transaction
        .query_row(
            "SELECT id FROM identity_review_groups
             WHERE workspace_id = ?1 AND group_key = ?2",
            params![workspace_id, group_key],
            |row| row.get(0),
        )
        .optional()?;
    let review_group_id = existing_id.unwrap_or_else(|| Uuid::now_v7().to_string());
    transaction.execute(
        "INSERT INTO identity_review_groups(
            id, workspace_id, review_reason, group_key, title, explanation,
            max_score, candidate_count, occurrence_count, file_count,
            status, resolver_version
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            'needs_review', ?11
         )
         ON CONFLICT(workspace_id, group_key) DO UPDATE SET
            review_reason = excluded.review_reason,
            title = excluded.title,
            explanation = excluded.explanation,
            max_score = excluded.max_score,
            candidate_count = excluded.candidate_count,
            occurrence_count = excluded.occurrence_count,
            file_count = excluded.file_count,
            status = 'needs_review',
            resolver_version = excluded.resolver_version,
            resolved_at = NULL,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            review_group_id,
            workspace_id,
            review_reason,
            group_key,
            truncate_database_text(&title, 512),
            explanation,
            max_score,
            candidate_count,
            occurrence_count.max(2),
            file_count.max(1),
            RESOLVER_VERSION,
        ],
    )?;
    transaction.execute(
        "DELETE FROM identity_review_group_candidates WHERE review_group_id = ?1",
        [review_group_id.as_str()],
    )?;
    transaction.execute(
        "INSERT INTO identity_review_group_candidates(review_group_id, candidate_id)
         SELECT ?1, id
         FROM identity_candidates
         WHERE workspace_id = ?2
           AND review_group_key = ?3
           AND status IN ('candidate', 'conflicting')
           AND active = 1
         ORDER BY score DESC, id
         LIMIT ?4",
        params![
            review_group_id,
            workspace_id,
            group_key,
            sql_usize(MAX_REVIEW_CANDIDATES_PER_GROUP)?,
        ],
    )?;
    Ok(())
}

fn insert_unresolved_identity(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    source: &SemanticSource,
) -> Result<String, PersistenceError> {
    let identity_id = Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO resolved_identities(
            id, workspace_id, identity_type, display_name,
            normalized_display_name, resolution_status, lifecycle_status,
            user_locked, confidence, creation_source, resolver_version
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, 'unresolved', 'active',
            0, ?6, 'resolver', ?7
         )",
        params![
            identity_id,
            workspace_id,
            source.identity_type.database_name(),
            source.original_value,
            source.normalized_value,
            f64::from(source.confidence),
            RESOLVER_VERSION,
        ],
    )?;
    Ok(identity_id)
}

fn record_resolver_occurrence_detachment(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    previous_identity_id: &str,
    new_identity_id: &str,
    occurrence_id: &str,
) -> Result<(), PersistenceError> {
    let decision_id = Uuid::now_v7().to_string();
    let reason = "semantic value changed; prior automatic link was detached";
    transaction.execute(
        "INSERT INTO identity_decisions(
            id, workspace_id, decision_type, decision_source,
            primary_identity_id, secondary_identity_id, occurrence_id,
            reason, resolver_version
         ) VALUES (
            ?1, ?2, 'split_identity', 'resolver', ?3, ?4, ?5, ?6, ?7
         )",
        params![
            decision_id,
            workspace_id,
            previous_identity_id,
            new_identity_id,
            occurrence_id,
            reason,
            RESOLVER_VERSION,
        ],
    )?;
    transaction.execute(
        "UPDATE identity_merge_history
         SET restored_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE occurrence_id = ?1
           AND to_identity_id = ?2
           AND restored_at IS NULL",
        params![occurrence_id, previous_identity_id],
    )?;
    transaction.execute(
        "UPDATE resolved_identities
         SET display_name = (
                SELECT original_value
                FROM identity_occurrences
                WHERE identity_id = ?1 AND active = 1
                ORDER BY confidence DESC, length(original_value), original_value, id
                LIMIT 1
             ),
             normalized_display_name = (
                SELECT normalized_value
                FROM identity_occurrences
                WHERE identity_id = ?1 AND active = 1
                ORDER BY confidence DESC, length(original_value), original_value, id
                LIMIT 1
             ),
             confidence = COALESCE((
                SELECT MAX(confidence)
                FROM identity_occurrences
                WHERE identity_id = ?1 AND active = 1
             ), confidence),
             resolution_status = CASE
                WHEN (
                    SELECT COUNT(*)
                    FROM identity_occurrences
                    WHERE identity_id = ?1 AND active = 1
                ) > 1 THEN 'auto_linked'
                ELSE 'unresolved'
             END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND lifecycle_status = 'active' AND user_locked = 0",
        [previous_identity_id],
    )?;
    insert_audit(
        transaction,
        workspace_id,
        "identity_split",
        "resolver",
        Some(previous_identity_id),
        Some(new_identity_id),
        None,
        Some(occurrence_id),
        Some(reason),
    )
}

fn supersede_machine_candidates_for_identity(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    identity_id: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT DISTINCT review_group_key
         FROM identity_candidates
         WHERE workspace_id = ?1
           AND active = 1
           AND status IN ('candidate', 'conflicting')
           AND (left_identity_id = ?2 OR right_identity_id = ?2)
         ORDER BY review_group_key",
    )?;
    let group_keys = statement
        .query_map(params![workspace_id, identity_id], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    transaction.execute(
        "UPDATE identity_candidates
         SET status = 'superseded',
             active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE workspace_id = ?1
           AND active = 1
           AND status IN ('candidate', 'conflicting')
           AND (left_identity_id = ?2 OR right_identity_id = ?2)",
        params![workspace_id, identity_id],
    )?;
    for group_key in group_keys {
        refresh_review_group(transaction, workspace_id, &group_key)?;
    }
    Ok(())
}

fn supersede_outdated_machine_candidates(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    resolver_version: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT DISTINCT review_group_key
         FROM identity_candidates
         WHERE workspace_id = ?1
           AND resolver_version <> ?2
           AND active = 1
           AND status IN ('candidate', 'conflicting')
         ORDER BY review_group_key",
    )?;
    let group_keys = statement
        .query_map(params![workspace_id, resolver_version], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    transaction.execute(
        "UPDATE identity_candidates
         SET status = 'superseded',
             active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE workspace_id = ?1
           AND resolver_version <> ?2
           AND active = 1
           AND status IN ('candidate', 'conflicting')",
        params![workspace_id, resolver_version],
    )?;
    for group_key in group_keys {
        refresh_review_group(transaction, workspace_id, &group_key)?;
    }
    Ok(())
}

fn supersede_orphaned_machine_candidates(
    transaction: &Transaction<'_>,
    workspace_id: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT DISTINCT c.review_group_key
         FROM identity_candidates c
         WHERE c.workspace_id = ?1
           AND c.active = 1
           AND c.status IN ('candidate', 'conflicting')
           AND (
                NOT EXISTS (
                    SELECT 1 FROM identity_occurrences occurrence
                    WHERE occurrence.identity_id = c.left_identity_id
                      AND occurrence.active = 1
                )
                OR NOT EXISTS (
                    SELECT 1 FROM identity_occurrences occurrence
                    WHERE occurrence.identity_id = c.right_identity_id
                      AND occurrence.active = 1
                )
           )
         ORDER BY c.review_group_key",
    )?;
    let group_keys = statement
        .query_map([workspace_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if group_keys.is_empty() {
        return Ok(());
    }
    transaction.execute(
        "UPDATE identity_candidates
         SET status = 'superseded',
             active = 0,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE workspace_id = ?1
           AND active = 1
           AND status IN ('candidate', 'conflicting')
           AND (
                NOT EXISTS (
                    SELECT 1 FROM identity_occurrences occurrence
                    WHERE occurrence.identity_id = identity_candidates.left_identity_id
                      AND occurrence.active = 1
                )
                OR NOT EXISTS (
                    SELECT 1 FROM identity_occurrences occurrence
                    WHERE occurrence.identity_id = identity_candidates.right_identity_id
                      AND occurrence.active = 1
                )
           )",
        [workspace_id],
    )?;
    for group_key in group_keys {
        refresh_review_group(transaction, workspace_id, &group_key)?;
    }
    Ok(())
}

fn identity_review_group_by_id(
    connection: &Connection,
    review_group_id: &str,
) -> Result<IdentityReviewGroupRecord, PersistenceError> {
    let (
        review_reason,
        group_key,
        title,
        explanation,
        max_score,
        candidate_count,
        occurrence_count,
        file_count,
        status,
        resolver_version,
        created_at,
        updated_at,
    ): (
        String,
        String,
        String,
        String,
        f64,
        i64,
        i64,
        i64,
        String,
        String,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT
                review_reason, group_key, title, explanation, max_score,
                candidate_count, occurrence_count, file_count, status,
                resolver_version, created_at, updated_at
             FROM identity_review_groups WHERE id = ?1",
            [review_group_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let mut statement = connection.prepare(
        "SELECT candidate_id
         FROM identity_review_group_candidates
         WHERE review_group_id = ?1
         ORDER BY candidate_id",
    )?;
    let candidate_ids = statement
        .query_map([review_group_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let candidates = candidate_ids
        .iter()
        .map(|id| identity_candidate_by_id(connection, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(IdentityReviewGroupRecord {
        review_group_id: review_group_id.to_owned(),
        review_reason,
        group_key,
        title,
        explanation,
        max_score: max_score as f32,
        candidate_count: from_sql_u64(candidate_count)?,
        occurrence_count: from_sql_u64(occurrence_count)?,
        file_count: from_sql_u64(file_count)?,
        status,
        resolver_version,
        candidates,
        created_at,
        updated_at,
    })
}

fn identity_candidate_by_id(
    connection: &Connection,
    candidate_id: &str,
) -> Result<IdentityCandidateRecord, PersistenceError> {
    let (
        review_group_key,
        score,
        policy_decision,
        status,
        resolver_version,
        left_identity_id,
        right_identity_id,
        created_at,
        updated_at,
    ): (
        String,
        f64,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT
                review_group_key, score, policy_decision, status,
                resolver_version, left_identity_id, right_identity_id,
                created_at, updated_at
             FROM identity_candidates
             WHERE id = ?1",
            [candidate_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let mut evidence_statement = connection.prepare(
        "SELECT
            evidence_type, strength, polarity, left_value, right_value,
            weight, explanation
         FROM identity_candidate_evidence
         WHERE candidate_id = ?1
         ORDER BY
            CASE strength
                WHEN 'conflicting' THEN 0
                WHEN 'very_strong' THEN 1
                WHEN 'strong' THEN 2
                WHEN 'medium' THEN 3
                ELSE 4
            END,
            evidence_type, id",
    )?;
    let evidence = evidence_statement
        .query_map([candidate_id], |row| {
            Ok(IdentityMatchEvidenceRecord {
                evidence_type: row.get(0)?,
                strength: row.get(1)?,
                polarity: row.get(2)?,
                left_value: row.get(3)?,
                right_value: row.get(4)?,
                weight: row.get::<_, f64>(5)? as f32,
                explanation: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(IdentityCandidateRecord {
        candidate_id: candidate_id.to_owned(),
        review_group_key,
        score: score as f32,
        policy_decision,
        status,
        resolver_version,
        left: identity_summary(connection, &left_identity_id)?,
        right: identity_summary(connection, &right_identity_id)?,
        evidence,
        created_at,
        updated_at,
    })
}

fn identity_summary(
    connection: &Connection,
    identity_id: &str,
) -> Result<IdentitySummaryRecord, PersistenceError> {
    let (
        identity_type,
        display_name,
        normalized_display_name,
        resolution_status,
        lifecycle_status,
        confidence,
        user_locked,
        occurrence_count,
        file_count,
    ): (String, String, String, String, String, f64, i64, i64, i64) = connection
        .query_row(
            "SELECT
                identity_type, display_name, normalized_display_name,
                resolution_status, lifecycle_status, confidence, user_locked,
                (SELECT COUNT(*) FROM identity_occurrences io
                 WHERE io.identity_id = ri.id AND io.active = 1),
                (SELECT COUNT(DISTINCT file_id) FROM identity_occurrences io
                 WHERE io.identity_id = ri.id AND io.active = 1)
             FROM resolved_identities ri
             WHERE id = ?1",
            [identity_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let aliases = query_strings(
        connection,
        "SELECT DISTINCT original_value
         FROM identity_aliases
         WHERE identity_id = ?1 AND active = 1
         ORDER BY normalized_value, original_value
         LIMIT 50",
        identity_id,
    )?;
    let roles = query_strings(
        connection,
        "SELECT DISTINCT role
         FROM identity_roles
         WHERE identity_id = ?1 AND active = 1
         ORDER BY role",
        identity_id,
    )?;
    Ok(IdentitySummaryRecord {
        identity_id: identity_id.to_owned(),
        identity_type,
        display_name,
        normalized_display_name,
        resolution_status,
        lifecycle_status,
        confidence: confidence as f32,
        user_locked: user_locked != 0,
        occurrence_count: from_sql_u64(occurrence_count)?,
        file_count: from_sql_u64(file_count)?,
        aliases,
        roles,
    })
}

fn relationships_for_identity(
    connection: &Connection,
    identity_id: &str,
) -> Result<Vec<IdentityRelationshipRecord>, PersistenceError> {
    let mut output = Vec::new();
    let mut statement = connection.prepare(
        "SELECT
            rel.id, rel.relationship_type, target.id, target.display_name,
            target.identity_type, rel.confidence, rel.status,
            rel.user_confirmation_state
         FROM identity_relationships rel
         JOIN resolved_identities target ON target.id = rel.target_identity_id
         WHERE rel.source_kind = 'identity'
           AND rel.source_identity_id = ?1
           AND rel.active = 1
         ORDER BY rel.relationship_type, target.display_name",
    )?;
    let mut rows = statement.query([identity_id])?;
    while let Some(row) = rows.next()? {
        let relationship_id: String = row.get(0)?;
        output.push(IdentityRelationshipRecord {
            relationship_id: relationship_id.clone(),
            relationship_type: row.get(1)?,
            identity_id: row.get(2)?,
            display_name: row.get(3)?,
            identity_type: row.get(4)?,
            confidence: row.get::<_, f64>(5)? as f32,
            status: row.get(6)?,
            user_confirmation_state: row.get(7)?,
            evidence: relationship_evidence(connection, &relationship_id)?,
        });
    }
    Ok(output)
}

fn relationship_evidence(
    connection: &Connection,
    relationship_id: &str,
) -> Result<Vec<String>, PersistenceError> {
    query_strings(
        connection,
        "SELECT explanation
         FROM identity_relationship_evidence
         WHERE relationship_id = ?1
         ORDER BY id
         LIMIT 50",
        relationship_id,
    )
}

fn project_identity_ids_for_identity(
    connection: &Connection,
    identity_id: &str,
) -> Result<Vec<String>, PersistenceError> {
    query_strings(
        connection,
        "SELECT DISTINCT
            CASE
                WHEN source_identity.identity_type = 'project'
                THEN source_identity.id
                WHEN target_identity.identity_type = 'project'
                THEN target_identity.id
            END
         FROM identity_relationships rel
         LEFT JOIN resolved_identities source_identity
           ON source_identity.id = rel.source_identity_id
         JOIN resolved_identities target_identity
           ON target_identity.id = rel.target_identity_id
         WHERE rel.active = 1
           AND (rel.source_identity_id = ?1 OR rel.target_identity_id = ?1)
           AND (
                source_identity.identity_type = 'project'
                OR target_identity.identity_type = 'project'
           )
         ORDER BY 1",
        identity_id,
    )
}

fn query_strings(
    connection: &Connection,
    sql: &str,
    value: &str,
) -> Result<Vec<String>, PersistenceError> {
    let mut statement = connection.prepare(sql)?;
    statement
        .query_map([value], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(PersistenceError::Sql)
}

fn canonical_identity_id(
    connection: &Connection,
    identity_id: &str,
) -> Result<String, PersistenceError> {
    let mut current = identity_id.to_owned();
    for _ in 0..32 {
        let state: Option<(String, Option<String>)> = connection
            .query_row(
                "SELECT lifecycle_status, merged_into_identity_id
                 FROM resolved_identities WHERE id = ?1",
                [current.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((lifecycle_status, merged_into)) = state else {
            return Err(PersistenceError::NotFound);
        };
        if lifecycle_status != "merged" {
            return Ok(current);
        }
        current = merged_into.ok_or(PersistenceError::IdentityConflict)?;
    }
    Err(PersistenceError::IdentityConflict)
}

fn identity_merge_state(
    connection: &Connection,
    identity_id: &str,
) -> Result<(String, String, String, bool), PersistenceError> {
    connection
        .query_row(
            "SELECT workspace_id, identity_type, lifecycle_status, user_locked
             FROM resolved_identities WHERE id = ?1",
            [identity_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn identity_user_locked(
    connection: &Connection,
    identity_id: &str,
) -> Result<bool, PersistenceError> {
    connection
        .query_row(
            "SELECT user_locked FROM resolved_identities WHERE id = ?1",
            [identity_id],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)
}

fn copy_aliases(
    transaction: &Transaction<'_>,
    source_identity_id: &str,
    target_identity_id: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT occurrence_id, original_value, normalized_value, legal_suffix
         FROM identity_aliases
         WHERE identity_id = ?1 AND active = 1
         ORDER BY id",
    )?;
    let rows = statement
        .query_map([source_identity_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (occurrence_id, original_value, normalized_value, legal_suffix) in rows {
        transaction.execute(
            "INSERT OR IGNORE INTO identity_aliases(
                id, identity_id, occurrence_id, original_value,
                normalized_value, legal_suffix, source
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'merge')",
            params![
                Uuid::now_v7().to_string(),
                target_identity_id,
                occurrence_id,
                original_value,
                normalized_value,
                legal_suffix,
            ],
        )?;
    }
    Ok(())
}

fn copy_roles(
    transaction: &Transaction<'_>,
    source_identity_id: &str,
    target_identity_id: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT role, occurrence_id, status, confidence
         FROM identity_roles
         WHERE identity_id = ?1 AND active = 1
         ORDER BY id",
    )?;
    let rows = statement
        .query_map([source_identity_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (role, occurrence_id, status, confidence) in rows {
        transaction.execute(
            "INSERT OR IGNORE INTO identity_roles(
                id, identity_id, role, occurrence_id, status, confidence
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::now_v7().to_string(),
                target_identity_id,
                role,
                occurrence_id,
                status,
                confidence,
            ],
        )?;
    }
    Ok(())
}

fn copy_rejection_constraints(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    source_identity_id: &str,
    target_identity_id: &str,
) -> Result<(), PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT left_identity_id, right_identity_id, decision_id, reason
         FROM identity_rejection_constraints
         WHERE active = 1
           AND (left_identity_id = ?1 OR right_identity_id = ?1)
         ORDER BY id",
    )?;
    let rows = statement
        .query_map([source_identity_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (left, right, decision_id, reason) in rows {
        let other = if left == source_identity_id {
            right
        } else {
            left
        };
        if other != target_identity_id {
            insert_rejection_constraint(
                transaction,
                workspace_id,
                target_identity_id,
                &other,
                &decision_id,
                reason
                    .as_deref()
                    .unwrap_or("preserved rejection after merge"),
            )?;
        }
    }
    Ok(())
}

fn occurrence_ids_for_identity(
    connection: &Connection,
    identity_id: &str,
) -> Result<Vec<String>, PersistenceError> {
    query_strings(
        connection,
        "SELECT id FROM identity_occurrences
         WHERE identity_id = ?1 ORDER BY id",
        identity_id,
    )
}

fn file_ids_for_occurrences(
    connection: &Connection,
    occurrence_ids: &[String],
) -> Result<Vec<String>, PersistenceError> {
    let mut output = BTreeSet::new();
    for occurrence_id in occurrence_ids {
        let file_id: String = connection.query_row(
            "SELECT file_id FROM identity_occurrences WHERE id = ?1",
            [occurrence_id],
            |row| row.get(0),
        )?;
        output.insert(file_id);
    }
    Ok(output.into_iter().collect())
}

fn roles_for_occurrence(
    connection: &Connection,
    occurrence_id: &str,
) -> Result<Vec<(String, f64)>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT role, confidence FROM identity_roles
         WHERE occurrence_id = ?1 AND active = 1
         ORDER BY role",
    )?;
    statement
        .query_map([occurrence_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(PersistenceError::Sql)
}

#[allow(clippy::too_many_arguments)]
fn insert_audit(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    event_type: &str,
    decision_source: &str,
    identity_id: Option<&str>,
    related_identity_id: Option<&str>,
    candidate_id: Option<&str>,
    occurrence_id: Option<&str>,
    reason: Option<&str>,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT INTO identity_audit_events(
            id, workspace_id, event_type, decision_source, identity_id,
            related_identity_id, candidate_id, occurrence_id, reason,
            resolver_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            Uuid::now_v7().to_string(),
            workspace_id,
            event_type,
            decision_source,
            identity_id,
            related_identity_id,
            candidate_id,
            occurrence_id,
            reason.map(|value| truncate_database_text(value, 512)),
            RESOLVER_VERSION,
        ],
    )?;
    Ok(())
}

fn ordered_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_owned(), right.to_owned())
    } else {
        (right.to_owned(), left.to_owned())
    }
}

fn parse_identity_type(value: &str) -> Result<IdentityType, PersistenceError> {
    match value {
        "organization" => Ok(IdentityType::Organization),
        "person" => Ok(IdentityType::Person),
        "project" => Ok(IdentityType::Project),
        _ => Err(PersistenceError::InvalidIdentityInput),
    }
}

fn parse_identity_role(value: &str) -> Result<IdentityRole, PersistenceError> {
    match value {
        "customer" => Ok(IdentityRole::Customer),
        "supplier" => Ok(IdentityRole::Supplier),
        _ => Err(PersistenceError::InvalidIdentityInput),
    }
}

fn parse_signal_kind(value: &str) -> Result<SignalKind, PersistenceError> {
    match value {
        "name" => Ok(SignalKind::Name),
        "company_identifier" => Ok(SignalKind::CompanyIdentifier),
        "vat_identifier" => Ok(SignalKind::VatIdentifier),
        "email" => Ok(SignalKind::Email),
        "domain" => Ok(SignalKind::Domain),
        "phone" => Ok(SignalKind::Phone),
        "address" => Ok(SignalKind::Address),
        "account_identifier" => Ok(SignalKind::AccountIdentifier),
        "project_reference" => Ok(SignalKind::ProjectReference),
        "customer_identity" => Ok(SignalKind::CustomerIdentity),
        "date" => Ok(SignalKind::Date),
        "path_context" => Ok(SignalKind::PathContext),
        _ => Err(PersistenceError::InvalidIdentityInput),
    }
}

fn validate_uuid_text(value: &str) -> Result<(), PersistenceError> {
    value
        .parse::<Uuid>()
        .map(|_| ())
        .map_err(|_| PersistenceError::InvalidIdentityInput)
}

fn validate_optional_reason(reason: Option<&str>) -> Result<(), PersistenceError> {
    if reason.is_some_and(|value| value.chars().count() > 512 || value.contains('\0')) {
        return Err(PersistenceError::InvalidIdentityInput);
    }
    Ok(())
}

fn sql_usize(value: usize) -> Result<i64, PersistenceError> {
    i64::try_from(value).map_err(|_| PersistenceError::NumericOverflow)
}

fn sql_u64(value: u64) -> Result<i64, PersistenceError> {
    i64::try_from(value).map_err(|_| PersistenceError::NumericOverflow)
}

fn sql_row_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn identity_occurrence_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<IdentityOccurrenceRecord> {
    Ok(IdentityOccurrenceRecord {
        occurrence_id: row.get(0)?,
        file_id: row.get(1)?,
        filename: row.get(2)?,
        relative_path: row.get(3)?,
        original_value: row.get(4)?,
        normalized_value: row.get(5)?,
        confidence: row.get::<_, f64>(6)? as f32,
        role: row.get(7)?,
        analyzer_version: row.get(8)?,
        active: row.get::<_, i64>(9)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_pairs_are_stable() {
        assert_eq!(ordered_pair("b", "a"), ("a".to_owned(), "b".to_owned()));
    }

    #[test]
    fn parser_rejects_unknown_identity_enums() {
        assert!(parse_identity_type("customer").is_err());
        assert!(parse_signal_kind("shell_command").is_err());
    }
}
