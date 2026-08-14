use super::{
    Database, PersistenceError, ProposalRelationshipSourceRecord, ProposalSemanticSignalRecord,
    ProposalSourceFileRecord, ProposalWorkspaceSourceRecord, from_sql_u64, to_sql_u64,
    truncate_database_text,
};
use domain::{
    FileId, OrganizationPreferences, OrganizationProposal, OrganizationProposalDiff,
    OrganizationProposalOperation, OrganizationProposalOverride, OrganizationProposalStatus,
    OrganizationProposalSummary, OrganizationReason, OrganizationRevisionId,
    ProposalConfidenceLevel, ProposalConflictState, ProposalId, ProposalOperationKind,
    ProposalOverrideAction, ProposalOverrideId, ProposalSourceSnapshot, RootId, ScanId,
    VirtualNodeKind, VirtualProposalNode, WorkspaceId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashMap;
use uuid::Uuid;

impl Database {
    pub fn organization_preferences(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<OrganizationPreferences, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT
                    client_first, include_year_folders, maximum_depth,
                    minimum_group_size, keep_photos_inside_projects,
                    supplier_invoices_inside_projects, naming_language,
                    preserve_existing_folders, personal_root_name,
                    business_root_name, rename_template, review_threshold
                 FROM local_organization_preferences
                 WHERE workspace_id = ?1",
                [workspace_id.to_string()],
                |row| {
                    Ok(OrganizationPreferences {
                        client_first: row.get::<_, i64>(0)? != 0,
                        include_year_folders: row.get::<_, i64>(1)? != 0,
                        maximum_depth: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(6),
                        minimum_group_size: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(2),
                        keep_photos_inside_projects: row.get::<_, i64>(4)? != 0,
                        supplier_invoices_inside_projects: row.get::<_, i64>(5)? != 0,
                        naming_language: row.get(6)?,
                        preserve_existing_folders: row.get::<_, i64>(7)? != 0,
                        personal_root_name: row.get(8)?,
                        business_root_name: row.get(9)?,
                        rename_template: row.get(10)?,
                        review_threshold: row.get::<_, f64>(11)? as f32,
                    })
                },
            )
            .optional()
            .map(|value| value.unwrap_or_default())
            .map_err(PersistenceError::Sql)
    }

    pub fn store_organization_preferences(
        &self,
        workspace_id: WorkspaceId,
        preferences: &OrganizationPreferences,
    ) -> Result<OrganizationPreferences, PersistenceError> {
        if !(2..=8).contains(&preferences.maximum_depth)
            || !(1..=20).contains(&preferences.minimum_group_size)
            || !matches!(preferences.naming_language.as_str(), "en" | "fr")
            || !valid_preference_component(&preferences.personal_root_name)
            || !valid_preference_component(&preferences.business_root_name)
            || !valid_rename_template(&preferences.rename_template)
            || !(0.5..=0.99).contains(&preferences.review_threshold)
        {
            return Err(PersistenceError::InvalidProposal);
        }
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO local_organization_preferences(
                workspace_id, client_first, include_year_folders, maximum_depth,
                minimum_group_size, keep_photos_inside_projects,
                supplier_invoices_inside_projects, naming_language,
                preserve_existing_folders, personal_root_name,
                business_root_name, rename_template, review_threshold
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(workspace_id) DO UPDATE SET
                client_first = excluded.client_first,
                include_year_folders = excluded.include_year_folders,
                maximum_depth = excluded.maximum_depth,
                minimum_group_size = excluded.minimum_group_size,
                keep_photos_inside_projects = excluded.keep_photos_inside_projects,
                supplier_invoices_inside_projects =
                    excluded.supplier_invoices_inside_projects,
                naming_language = excluded.naming_language,
                preserve_existing_folders = excluded.preserve_existing_folders,
                personal_root_name = excluded.personal_root_name,
                business_root_name = excluded.business_root_name,
                rename_template = excluded.rename_template,
                review_threshold = excluded.review_threshold,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                workspace_id.to_string(),
                i64::from(preferences.client_first),
                i64::from(preferences.include_year_folders),
                i64::try_from(preferences.maximum_depth)
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                i64::try_from(preferences.minimum_group_size)
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                i64::from(preferences.keep_photos_inside_projects),
                i64::from(preferences.supplier_invoices_inside_projects),
                preferences.naming_language,
                i64::from(preferences.preserve_existing_folders),
                preferences.personal_root_name,
                preferences.business_root_name,
                preferences.rename_template,
                f64::from(preferences.review_threshold),
            ],
        )?;
        drop(connection);
        self.organization_preferences(workspace_id)
    }

    pub fn organization_source(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<ProposalWorkspaceSourceRecord, PersistenceError> {
        let root_id = self.unambiguous_organization_root(workspace_id)?;
        self.organization_source_for_root(workspace_id, root_id)
    }

    pub fn organization_source_for_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
    ) -> Result<ProposalWorkspaceSourceRecord, PersistenceError> {
        let connection = self.lock()?;
        let scan_id: String = connection
            .query_row(
                "SELECT id
                 FROM scans
                 WHERE workspace_id = ?1
                   AND root_id = ?2
                   AND status = 'completed'
                 ORDER BY completed_at DESC, created_at DESC
                 LIMIT 1",
                params![workspace_id.to_string(), root_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let scan_id = scan_id.parse::<ScanId>()?;

        let mut statement = connection.prepare(
            "SELECT
                f.id, fv.id, fl.relative_path, fl.basename, fv.byte_size,
                fv.modified_at,
                CASE WHEN cd.digest IS NULL THEN NULL ELSE lower(hex(cd.digest)) END,
                d.extraction_status,
                sa.status, COALESCE(sa.input_quality, 0.0)
             FROM file_locations fl
             JOIN files f ON f.id = fl.file_id
             JOIN file_versions fv ON fv.id = (
                SELECT current_version.id
                FROM file_versions AS current_version
                WHERE current_version.location_id = fl.id
                ORDER BY current_version.version_number DESC, current_version.id DESC
                LIMIT 1
             )
             LEFT JOIN content_digests cd
                ON cd.content_id = fv.content_id AND cd.algorithm = 'blake3'
             LEFT JOIN local_search_documents d
                ON d.file_version_id = fv.id
             LEFT JOIN semantic_analyses sa
                ON sa.file_id = f.id
               AND sa.file_version_id = fv.id
               AND sa.is_current = 1
             WHERE f.workspace_id = ?1
               AND fl.root_id = ?2
               AND fl.valid_to_scan_id IS NULL
               AND f.lifecycle_state = 'present'
             ORDER BY fl.normalized_relative_path, f.id",
        )?;
        let rows = statement.query_map(
            params![workspace_id.to_string(), root_id.to_string()],
            |row| {
                Ok(ProposalSourceFileRecord {
                    file_id: row.get(0)?,
                    file_version_id: row.get(1)?,
                    relative_path: row.get(2)?,
                    filename: row.get(3)?,
                    byte_size: from_sql_u64(row.get::<_, i64>(4)?)
                        .map_err(to_sql_conversion_error)?,
                    modified_at: row.get(5)?,
                    content_hash: row.get(6)?,
                    extraction_status: row.get(7)?,
                    semantic_status: row.get(8)?,
                    input_quality: row.get::<_, f64>(9)? as f32,
                    context: None,
                    document_type: None,
                    issue_date: None,
                    identifier: None,
                    amount: None,
                    currency: None,
                    relationships: Vec::new(),
                    review_reasons: Vec::new(),
                    duplicate_group_id: None,
                    duplicate_canonical: true,
                })
            },
        )?;
        let mut files = rows.collect::<Result<Vec<_>, _>>()?;
        let indexes = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.file_id.clone(), index))
            .collect::<HashMap<_, _>>();
        load_semantic_signals(&connection, workspace_id, &indexes, &mut files)?;
        load_relationships(&connection, workspace_id, &indexes, &mut files)?;
        load_review_reasons(&connection, workspace_id, &indexes, &mut files)?;
        load_duplicate_state(&connection, root_id, &mut files)?;
        let semantic_version = connection
            .query_row(
                "SELECT MAX(analyzer_version)
                 FROM semantic_analyses AS analysis
                 JOIN file_locations AS location
                   ON location.file_id = analysis.file_id
                  AND location.valid_to_scan_id IS NULL
                 WHERE analysis.workspace_id = ?1
                   AND location.root_id = ?2
                   AND analysis.is_current = 1
                   AND analysis.file_version_id = (
                        SELECT current_version.id
                        FROM file_versions AS current_version
                        WHERE current_version.location_id = location.id
                        ORDER BY current_version.version_number DESC, current_version.id DESC
                        LIMIT 1
                   )",
                params![workspace_id.to_string(), root_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let relationship_version = connection
            .query_row(
                "SELECT MAX(resolver_version)
                 FROM identity_relationships
                 WHERE workspace_id = ?1 AND active = 1",
                [workspace_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(ProposalWorkspaceSourceRecord {
            workspace_id,
            root_id,
            scan_id,
            semantic_version,
            relationship_version,
            files,
        })
    }

    pub fn organization_source_for_file(
        &self,
        workspace_id: WorkspaceId,
        file_id: FileId,
    ) -> Result<ProposalWorkspaceSourceRecord, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT location.root_id
             FROM file_locations AS location
             JOIN files AS file ON file.id = location.file_id
             WHERE location.file_id = ?1
               AND file.workspace_id = ?2
               AND location.valid_to_scan_id IS NULL
             ORDER BY location.root_id
             LIMIT 2",
        )?;
        let roots = statement
            .query_map(
                params![file_id.to_string(), workspace_id.to_string()],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        drop(connection);
        if roots.len() != 1 {
            return Err(PersistenceError::InvalidProposal);
        }
        self.organization_source_for_files(workspace_id, roots[0].parse::<RootId>()?, &[file_id])
    }

    pub fn organization_source_for_files(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        file_ids: &[FileId],
    ) -> Result<ProposalWorkspaceSourceRecord, PersistenceError> {
        if file_ids.is_empty() {
            let mut empty = self.organization_source_for_root(workspace_id, root_id)?;
            empty.files.clear();
            return Ok(empty);
        }
        let connection = self.lock()?;
        let scan_id: String = connection
            .query_row(
                "SELECT id
                 FROM scans
                 WHERE workspace_id = ?1
                   AND root_id = ?2
                   AND status = 'completed'
                 ORDER BY completed_at DESC, created_at DESC
                 LIMIT 1",
                params![workspace_id.to_string(), root_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let scan_id = scan_id.parse::<ScanId>()?;

        let placeholders = file_ids
            .iter()
            .enumerate()
            .map(|(index, _)| format!("?{}", index + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT
                f.id, fv.id, fl.relative_path, fl.basename, fv.byte_size,
                fv.modified_at,
                CASE WHEN cd.digest IS NULL THEN NULL ELSE lower(hex(cd.digest)) END,
                d.extraction_status,
                sa.status, COALESCE(sa.input_quality, 0.0)
             FROM file_locations fl
             JOIN files f ON f.id = fl.file_id
             JOIN file_versions fv ON fv.id = (
                SELECT current_version.id
                FROM file_versions AS current_version
                WHERE current_version.location_id = fl.id
                ORDER BY current_version.version_number DESC, current_version.id DESC
                LIMIT 1
             )
             LEFT JOIN content_digests cd
                ON cd.content_id = fv.content_id AND cd.algorithm = 'blake3'
             LEFT JOIN local_search_documents d
                ON d.file_version_id = fv.id
             LEFT JOIN semantic_analyses sa
                ON sa.file_id = f.id
               AND sa.file_version_id = fv.id
               AND sa.is_current = 1
             WHERE f.workspace_id = ?1
               AND fl.root_id = ?2
               AND fl.valid_to_scan_id IS NULL
               AND f.lifecycle_state = 'present'
               AND f.id IN ({placeholders})
             ORDER BY fl.normalized_relative_path, f.id"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut params_vec: Vec<rusqlite::types::Value> =
            vec![workspace_id.to_string().into(), root_id.to_string().into()];
        for file_id in file_ids {
            params_vec.push(file_id.to_string().into());
        }
        let rows = statement.query_map(rusqlite::params_from_iter(params_vec), |row| {
            Ok(ProposalSourceFileRecord {
                file_id: row.get(0)?,
                file_version_id: row.get(1)?,
                relative_path: row.get(2)?,
                filename: row.get(3)?,
                byte_size: from_sql_u64(row.get::<_, i64>(4)?).map_err(to_sql_conversion_error)?,
                modified_at: row.get(5)?,
                content_hash: row.get(6)?,
                extraction_status: row.get(7)?,
                semantic_status: row.get(8)?,
                input_quality: row.get::<_, f64>(9)? as f32,
                context: None,
                document_type: None,
                issue_date: None,
                identifier: None,
                amount: None,
                currency: None,
                relationships: Vec::new(),
                review_reasons: Vec::new(),
                duplicate_group_id: None,
                duplicate_canonical: true,
            })
        })?;
        let mut files = rows.collect::<Result<Vec<_>, _>>()?;
        let indexes = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.file_id.clone(), index))
            .collect::<HashMap<_, _>>();
        load_semantic_signals(&connection, workspace_id, &indexes, &mut files)?;
        load_relationships(&connection, workspace_id, &indexes, &mut files)?;
        load_review_reasons(&connection, workspace_id, &indexes, &mut files)?;
        load_duplicate_state(&connection, root_id, &mut files)?;
        let semantic_version = connection
            .query_row(
                "SELECT MAX(analyzer_version)
                 FROM semantic_analyses AS analysis
                 JOIN file_locations AS location
                   ON location.file_id = analysis.file_id
                  AND location.valid_to_scan_id IS NULL
                 WHERE analysis.workspace_id = ?1
                   AND location.root_id = ?2
                   AND analysis.is_current = 1",
                params![workspace_id.to_string(), root_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let relationship_version = connection
            .query_row(
                "SELECT MAX(resolver_version)
                 FROM identity_relationships
                 WHERE workspace_id = ?1 AND active = 1",
                [workspace_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(ProposalWorkspaceSourceRecord {
            workspace_id,
            root_id,
            scan_id,
            semantic_version,
            relationship_version,
            files,
        })
    }

    pub fn unambiguous_organization_root(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<RootId, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT root.id
             FROM roots AS root
             JOIN scans AS scan ON scan.root_id = root.id
             WHERE root.workspace_id = ?1
               AND root.state <> 'retired'
               AND scan.status = 'completed'
             ORDER BY root.id
             LIMIT 2",
        )?;
        let roots = statement
            .query_map([workspace_id.to_string()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if roots.len() != 1 {
            return Err(PersistenceError::InvalidProposal);
        }
        roots[0].parse::<RootId>().map_err(PersistenceError::from)
    }

    pub fn current_organization_proposal_id(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Option<ProposalId>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id
                 FROM local_organization_proposals
                 WHERE workspace_id = ?1
                   AND status IN (
                        'draft', 'ready_for_review', 'reviewed',
                        'approved_for_future_apply'
                   )
                 ORDER BY root_id
                 LIMIT 2",
        )?;
        let ids = statement
            .query_map([workspace_id.to_string()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        match ids.as_slice() {
            [] => Ok(None),
            [id] => id
                .parse::<ProposalId>()
                .map(Some)
                .map_err(PersistenceError::from),
            _ => Err(PersistenceError::InvalidProposal),
        }
    }

    pub fn current_organization_proposal_id_for_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
    ) -> Result<Option<ProposalId>, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT id
                 FROM local_organization_proposals
                 WHERE workspace_id = ?1
                   AND root_id = ?2
                   AND status IN (
                        'draft', 'ready_for_review', 'reviewed',
                        'approved_for_future_apply'
                   )
                 LIMIT 1",
                params![workspace_id.to_string(), root_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| value.parse::<ProposalId>().map_err(PersistenceError::from))
            .transpose()
    }

    pub fn current_organization_proposals(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<(RootId, ProposalId)>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT root_id, id
             FROM local_organization_proposals
             WHERE workspace_id = ?1
               AND status IN (
                    'draft', 'ready_for_review', 'reviewed',
                    'approved_for_future_apply'
               )
             ORDER BY root_id",
        )?;
        let rows = statement.query_map([workspace_id.to_string()], |row| {
            Ok((
                parse_uuid_column(row.get(0)?, 0)?,
                parse_uuid_column(row.get(1)?, 1)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn organization_proposal_revision_number(
        &self,
        proposal_id: ProposalId,
    ) -> Result<u32, PersistenceError> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT COALESCE(MAX(revision_number), 0)
                 FROM local_organization_proposal_revisions
                 WHERE proposal_id = ?1",
                [proposal_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or_default();
        u32::try_from(value).map_err(|_| PersistenceError::NumericOverflow)
    }

    pub fn organization_proposal_overrides(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Vec<OrganizationProposalOverride>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT
                id, file_id, action, destination_json, proposed_name,
                reason, created_at, updated_at
             FROM local_organization_user_overrides
             WHERE proposal_id = ?1 AND active = 1
             ORDER BY file_id",
        )?;
        let rows = statement.query_map([proposal_id.to_string()], |row| {
            let id = parse_uuid_column::<ProposalOverrideId>(row.get(0)?, 0)?;
            let file_id = parse_uuid_column::<FileId>(row.get(1)?, 1)?;
            let action = parse_override_action(&row.get::<_, String>(2)?)
                .map_err(|error| to_sql_conversion_boxed(2, error))?;
            let destination_json = row.get::<_, Option<String>>(3)?;
            let destination = destination_json
                .map(|value| {
                    serde_json::from_str::<Vec<String>>(&value)
                        .map_err(|error| to_sql_conversion_boxed(3, error))
                })
                .transpose()?;
            Ok(OrganizationProposalOverride {
                id,
                proposal_id,
                file_id,
                action,
                destination,
                proposed_name: row.get(4)?,
                reason: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::Sql)
    }

    pub fn persist_organization_proposal(
        &self,
        proposal: &OrganizationProposal,
        trigger_kind: &str,
    ) -> Result<(), PersistenceError> {
        self.persist_organization_proposal_with_meta(
            proposal,
            trigger_kind,
            "full",
            None,
            proposal.summary.files_analyzed,
        )
    }

    pub fn persist_organization_proposal_with_meta(
        &self,
        proposal: &OrganizationProposal,
        trigger_kind: &str,
        rebuild_mode: &str,
        rebuild_reason: Option<&str>,
        dirty_file_count: u64,
    ) -> Result<(), PersistenceError> {
        validate_proposal(proposal, trigger_kind)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        upsert_proposal_header(&transaction, proposal)?;
        insert_revision_with_meta(
            &transaction,
            proposal,
            trigger_kind,
            rebuild_mode,
            rebuild_reason,
            dirty_file_count,
        )?;
        for operation in &proposal.operations {
            insert_operation(&transaction, proposal, operation)?;
        }
        // Persist only root/folder virtual nodes. File nodes are reconstructed
        // on load from operations, avoiding O(N) file-node writes at 100k scale.
        for node in proposal
            .nodes
            .iter()
            .filter(|node| node.kind != VirtualNodeKind::File)
        {
            insert_virtual_node(&transaction, proposal, node)?;
        }
        replace_proposal_dependencies(&transaction, proposal)?;
        finalize_proposal_header(&transaction, proposal)?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn persist_organization_proposal_incremental(
        &self,
        proposal: &OrganizationProposal,
        trigger_kind: &str,
        previous_revision_id: OrganizationRevisionId,
        changed_file_ids: &std::collections::HashSet<FileId>,
        rebuild_mode: &str,
        rebuild_reason: Option<&str>,
        dirty_file_count: u64,
    ) -> Result<(), PersistenceError> {
        validate_proposal(proposal, trigger_kind)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        upsert_proposal_header(&transaction, proposal)?;
        insert_revision_with_meta(
            &transaction,
            proposal,
            trigger_kind,
            rebuild_mode,
            rebuild_reason,
            dirty_file_count,
        )?;

        let mut id_map = HashMap::<String, String>::new();
        let mut unchanged_statement = transaction.prepare(
            "SELECT
                id, file_id, file_version_id, operation_kind,
                source_relative_path, source_name, source_hash, source_byte_size,
                source_modified_at, machine_destination_json, machine_name,
                proposed_destination_json, proposed_name, confidence_score,
                confidence_level, conflict_state, needs_review, stale, user_override,
                disruption_score, proposed_path_length, proposed_depth,
                semantic_context, document_type, customer_name, supplier_name,
                project_name, duplicate_group_id, duplicate_canonical
             FROM local_organization_proposal_operations
             WHERE revision_id = ?1",
        )?;
        let unchanged_rows = unchanged_statement
            .query_map([previous_revision_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, f64>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, i64>(18)?,
                    row.get::<_, f64>(19)?,
                    row.get::<_, i64>(20)?,
                    row.get::<_, i64>(21)?,
                    row.get::<_, String>(22)?,
                    row.get::<_, String>(23)?,
                    row.get::<_, Option<String>>(24)?,
                    row.get::<_, Option<String>>(25)?,
                    row.get::<_, Option<String>>(26)?,
                    row.get::<_, Option<String>>(27)?,
                    row.get::<_, i64>(28)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(unchanged_statement);

        // Map in-memory unchanged operation IDs to copied DB IDs for dependency rows.
        let memory_unchanged = proposal
            .operations
            .iter()
            .filter(|operation| !changed_file_ids.contains(&operation.file_id))
            .map(|operation| (operation.file_id.to_string(), operation.id.to_string()))
            .collect::<HashMap<_, _>>();

        for row in unchanged_rows {
            let old_id = row.0;
            let file_id = row.1;
            let parsed_file = file_id
                .parse::<FileId>()
                .map_err(|_| PersistenceError::InvalidProposal)?;
            if changed_file_ids.contains(&parsed_file) {
                continue;
            }
            let new_id = memory_unchanged
                .get(&file_id)
                .cloned()
                .unwrap_or_else(|| Uuid::now_v7().to_string());
            id_map.insert(old_id.clone(), new_id.clone());
            transaction.execute(
                "INSERT INTO local_organization_proposal_operations(
                    id, proposal_id, revision_id, file_id, file_version_id,
                    operation_kind, source_relative_path, source_name, source_hash,
                    source_byte_size, source_modified_at, machine_destination_json,
                    machine_name, proposed_destination_json, proposed_name,
                    confidence_score, confidence_level, conflict_state, needs_review,
                    stale, user_override, disruption_score, proposed_path_length,
                    proposed_depth, semantic_context, document_type, customer_name,
                    supplier_name, project_name, duplicate_group_id, duplicate_canonical
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                    ?25, ?26, ?27, ?28, ?29, ?30, ?31
                 )",
                params![
                    new_id,
                    proposal.id.to_string(),
                    proposal.revision_id.to_string(),
                    file_id,
                    row.2,
                    row.3,
                    row.4,
                    row.5,
                    row.6,
                    row.7,
                    row.8,
                    row.9,
                    row.10,
                    row.11,
                    row.12,
                    row.13,
                    row.14,
                    row.15,
                    row.16,
                    row.17,
                    row.18,
                    row.19,
                    row.20,
                    row.21,
                    row.22,
                    row.23,
                    row.24,
                    row.25,
                    row.26,
                    row.27,
                    row.28,
                ],
            )?;
            let mut reason_statement = transaction.prepare(
                "SELECT reason_order, reason_code, explanation, evidence_references_json
                 FROM local_organization_proposal_reasons
                 WHERE operation_id = ?1
                 ORDER BY reason_order",
            )?;
            let reasons = reason_statement
                .query_map([&old_id], |reason_row| {
                    Ok((
                        reason_row.get::<_, i64>(0)?,
                        reason_row.get::<_, String>(1)?,
                        reason_row.get::<_, String>(2)?,
                        reason_row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(reason_statement);
            for (order, code, explanation, evidence) in reasons {
                transaction.execute(
                    "INSERT INTO local_organization_proposal_reasons(
                        id, operation_id, reason_order, reason_code,
                        explanation, evidence_references_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        Uuid::now_v7().to_string(),
                        new_id,
                        order,
                        code,
                        explanation,
                        evidence,
                    ],
                )?;
            }
        }

        for operation in &proposal.operations {
            if changed_file_ids.contains(&operation.file_id) {
                insert_operation(&transaction, proposal, operation)?;
            }
        }

        for node in proposal
            .nodes
            .iter()
            .filter(|node| node.kind != VirtualNodeKind::File)
        {
            insert_virtual_node(&transaction, proposal, node)?;
        }
        let _ = id_map;
        replace_proposal_dependencies(&transaction, proposal)?;
        finalize_proposal_header(&transaction, proposal)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn organization_proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<OrganizationProposal, PersistenceError> {
        let connection = self.lock()?;
        organization_proposal_from_connection(&connection, proposal_id, None)
    }

    /// UI-safe proposal load: full summary, folder/root nodes only, bounded operations.
    /// Execution/apply paths must continue using [`Self::organization_proposal`].
    pub fn organization_proposal_for_ui(
        &self,
        proposal_id: ProposalId,
        operation_limit: usize,
    ) -> Result<OrganizationProposal, PersistenceError> {
        let connection = self.lock()?;
        let limit = operation_limit.clamp(1, 2_000);
        organization_proposal_from_connection(
            &connection,
            proposal_id,
            Some(UiProposalBound { limit }),
        )
    }

    pub fn latest_organization_proposal(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<OrganizationProposal, PersistenceError> {
        let id = self
            .current_organization_proposal_id(workspace_id)?
            .ok_or(PersistenceError::NotFound)?;
        self.organization_proposal(id)
    }

    pub fn latest_organization_proposal_for_root(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
    ) -> Result<OrganizationProposal, PersistenceError> {
        let id = self
            .current_organization_proposal_id_for_root(workspace_id, root_id)?
            .ok_or(PersistenceError::NotFound)?;
        self.organization_proposal(id)
    }

    pub fn latest_organization_proposal_for_ui(
        &self,
        workspace_id: WorkspaceId,
        root_id: Option<RootId>,
        operation_limit: usize,
    ) -> Result<OrganizationProposal, PersistenceError> {
        let id = match root_id {
            Some(root_id) => {
                self.current_organization_proposal_id_for_root(workspace_id, root_id)?
            }
            None => self.current_organization_proposal_id(workspace_id)?,
        }
        .ok_or(PersistenceError::NotFound)?;
        self.organization_proposal_for_ui(id, operation_limit)
    }

    pub fn store_organization_override(
        &self,
        override_record: &OrganizationProposalOverride,
    ) -> Result<OrganizationProposalOverride, PersistenceError> {
        validate_override(override_record)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let is_current_file = transaction
            .query_row(
                "SELECT 1
                 FROM local_organization_proposals proposal
                 JOIN local_organization_proposal_operations operation
                    ON operation.revision_id = proposal.current_revision_id
                 WHERE proposal.id = ?1 AND operation.file_id = ?2",
                params![
                    override_record.proposal_id.to_string(),
                    override_record.file_id.to_string()
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !is_current_file {
            return Err(PersistenceError::NotFound);
        }
        transaction.execute(
            "UPDATE local_organization_user_overrides
             SET active = 0,
                 superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE proposal_id = ?1 AND file_id = ?2 AND active = 1",
            params![
                override_record.proposal_id.to_string(),
                override_record.file_id.to_string()
            ],
        )?;
        let destination_json = override_record
            .destination
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| PersistenceError::InvalidProposal)?;
        transaction.execute(
            "INSERT INTO local_organization_user_overrides(
                id, proposal_id, file_id, action, destination_json,
                proposed_name, reason, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                override_record.id.to_string(),
                override_record.proposal_id.to_string(),
                override_record.file_id.to_string(),
                override_record.action.database_name(),
                destination_json,
                override_record.proposed_name,
                override_record
                    .reason
                    .as_deref()
                    .map(|value| truncate_database_text(value, 512)),
                override_record.created_at,
                override_record.updated_at,
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        self.organization_proposal_overrides(override_record.proposal_id)?
            .into_iter()
            .find(|value| value.id == override_record.id)
            .ok_or(PersistenceError::NotFound)
    }

    pub fn set_organization_proposal_status(
        &self,
        proposal_id: ProposalId,
        status: OrganizationProposalStatus,
    ) -> Result<OrganizationProposal, PersistenceError> {
        if !matches!(
            status,
            OrganizationProposalStatus::Reviewed
                | OrganizationProposalStatus::ApprovedForFutureApply
                | OrganizationProposalStatus::Cancelled
        ) {
            return Err(PersistenceError::InvalidProposal);
        }
        let connection = self.lock()?;
        let current: String = connection
            .query_row(
                "SELECT status FROM local_organization_proposals WHERE id = ?1",
                [proposal_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let transition_valid = match status {
            OrganizationProposalStatus::Reviewed => {
                matches!(current.as_str(), "ready_for_review" | "reviewed")
            }
            OrganizationProposalStatus::ApprovedForFutureApply => {
                matches!(current.as_str(), "ready_for_review" | "reviewed")
            }
            OrganizationProposalStatus::Cancelled => !matches!(current.as_str(), "superseded"),
            _ => false,
        };
        if !transition_valid {
            return Err(PersistenceError::InvalidProposal);
        }
        connection.execute(
            "UPDATE local_organization_proposals
             SET status = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            params![proposal_id.to_string(), status.database_name()],
        )?;
        drop(connection);
        self.organization_proposal(proposal_id)
    }

    pub fn refresh_organization_proposal_drift(
        &self,
        proposal_id: ProposalId,
    ) -> Result<u64, PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_organization_proposal_operations AS operation
             SET stale = 1,
                 needs_review = 1,
                 conflict_state = 'stale_source'
             WHERE operation.revision_id = (
                SELECT current_revision_id
                FROM local_organization_proposals
                WHERE id = ?1
             )
             AND EXISTS (
                SELECT 1
                FROM file_versions current_version
                JOIN file_locations current_location
                    ON current_location.id = current_version.location_id
                LEFT JOIN content_digests current_digest
                    ON current_digest.content_id = current_version.content_id
                   AND current_digest.algorithm = 'blake3'
                WHERE current_version.file_id = operation.file_id
                  AND current_version.version_number = (
                    SELECT MAX(newer.version_number)
                    FROM file_versions newer
                    WHERE newer.file_id = operation.file_id
                  )
                  AND (
                    current_version.id <> operation.file_version_id
                    OR current_version.byte_size <> operation.source_byte_size
                    OR COALESCE(current_version.modified_at, '') <>
                       COALESCE(operation.source_modified_at, '')
                    OR COALESCE(lower(hex(current_digest.digest)), '') <>
                       COALESCE(operation.source_hash, '')
                    OR current_location.relative_path <> operation.source_relative_path
                  )
             )",
            [proposal_id.to_string()],
        )?;
        u64::try_from(changed).map_err(|_| PersistenceError::NumericOverflow)
    }
}

fn load_semantic_signals(
    connection: &Connection,
    workspace_id: WorkspaceId,
    indexes: &HashMap<String, usize>,
    files: &mut [ProposalSourceFileRecord],
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            sa.file_id, sa.file_version_id, sf.field_key,
            COALESCE(c.display_value, sf.display_value),
            CASE WHEN c.id IS NULL THEN sf.confidence ELSE 1.0 END,
            CASE WHEN c.id IS NULL THEN sf.field_status ELSE 'confirmed' END,
            c.correction_state
         FROM semantic_analyses sa
         JOIN semantic_fields sf ON sf.analysis_id = sa.id AND sf.is_primary = 1
         LEFT JOIN semantic_user_corrections c
            ON c.file_id = sa.file_id
           AND c.field_key = sf.field_key
           AND c.active = 1
         WHERE sa.workspace_id = ?1
           AND sa.is_current = 1
           AND sf.field_key IN (
                'document_type', 'context', 'issue_date', 'document_date',
                'invoice_number', 'quote_number', 'document_number',
                'total', 'amount', 'currency'
           )
         ORDER BY sa.file_id, CASE sf.field_key
            WHEN 'issue_date' THEN 0
            WHEN 'document_date' THEN 1
            WHEN 'invoice_number' THEN 0
            WHEN 'quote_number' THEN 1
            WHEN 'document_number' THEN 2
            WHEN 'total' THEN 0
            WHEN 'amount' THEN 1
            ELSE 0
         END",
    )?;
    let rows = statement.query_map([workspace_id.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)? as f32,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    for row in rows {
        let (file_id, file_version_id, field_key, value, confidence, status, correction) = row?;
        let Some(index) = indexes.get(&file_id).copied() else {
            continue;
        };
        if files[index].file_version_id != file_version_id {
            continue;
        }
        let Some(value) = value else {
            continue;
        };
        let signal = ProposalSemanticSignalRecord {
            value,
            confidence,
            status,
            user_confirmed: correction.is_some(),
        };
        let file = &mut files[index];
        match field_key.as_str() {
            "context" => file.context = Some(signal),
            "document_type" => file.document_type = Some(signal),
            "issue_date" | "document_date" if file.issue_date.is_none() => {
                file.issue_date = Some(signal);
            }
            "invoice_number" | "quote_number" | "document_number" if file.identifier.is_none() => {
                file.identifier = Some(signal);
            }
            "total" | "amount" if file.amount.is_none() => file.amount = Some(signal),
            "currency" => file.currency = Some(signal),
            _ => {}
        }
    }
    Ok(())
}

fn load_relationships(
    connection: &Connection,
    workspace_id: WorkspaceId,
    indexes: &HashMap<String, usize>,
    files: &mut [ProposalSourceFileRecord],
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            relationship.source_file_id, relationship.relationship_type,
            target.id, target.display_name, relationship.confidence,
            relationship.status, relationship.user_confirmation_state,
            (
                SELECT customer.display_name
                FROM identity_relationships project_customer
                JOIN resolved_identities customer
                    ON customer.id = project_customer.target_identity_id
                WHERE relationship.relationship_type = 'file_project'
                  AND project_customer.source_kind = 'identity'
                  AND project_customer.source_identity_id = target.id
                  AND project_customer.relationship_type = 'project_customer'
                  AND project_customer.active = 1
                  AND project_customer.status IN ('auto_linked', 'user_confirmed')
                ORDER BY
                    (project_customer.status = 'user_confirmed') DESC,
                    project_customer.confidence DESC,
                    customer.display_name
                LIMIT 1
            )
         FROM identity_relationships relationship
         JOIN resolved_identities target
            ON target.id = relationship.target_identity_id
         WHERE relationship.workspace_id = ?1
           AND relationship.source_kind = 'file'
           AND relationship.active = 1
           AND target.lifecycle_status = 'active'
         ORDER BY relationship.source_file_id, relationship.relationship_type,
                  relationship.confidence DESC, target.display_name",
    )?;
    let rows = statement.query_map([workspace_id.to_string()], |row| {
        let status = row.get::<_, String>(5)?;
        let confirmation = row.get::<_, Option<String>>(6)?;
        Ok((
            row.get::<_, String>(0)?,
            ProposalRelationshipSourceRecord {
                relationship_type: row.get(1)?,
                identity_id: row.get(2)?,
                display_name: row.get(3)?,
                confidence: row.get::<_, f64>(4)? as f32,
                user_confirmed: status == "user_confirmed"
                    || confirmation.as_deref() == Some("confirmed"),
                status,
                project_customer_name: row.get(7)?,
            },
        ))
    })?;
    for row in rows {
        let (file_id, relationship) = row?;
        if let Some(index) = indexes.get(&file_id) {
            files[*index].relationships.push(relationship);
        }
    }
    Ok(())
}

fn load_review_reasons(
    connection: &Connection,
    workspace_id: WorkspaceId,
    indexes: &HashMap<String, usize>,
    files: &mut [ProposalSourceFileRecord],
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT file_id, reason
         FROM file_review_items
         WHERE workspace_id = ?1 AND status = 'needs_review'
         ORDER BY file_id, reason",
    )?;
    let rows = statement.query_map([workspace_id.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (file_id, reason) = row?;
        if let Some(index) = indexes.get(&file_id) {
            files[*index].review_reasons.push(reason);
        }
    }
    Ok(())
}

fn load_duplicate_state(
    connection: &Connection,
    root_id: RootId,
    files: &mut [ProposalSourceFileRecord],
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT dgm.file_version_id, dg.id, fl.normalized_relative_path
         FROM duplicate_groups dg
         JOIN duplicate_group_members dgm ON dgm.duplicate_group_id = dg.id
         JOIN file_versions fv ON fv.id = dgm.file_version_id
         JOIN file_locations fl ON fl.id = fv.location_id
         WHERE dg.root_id = ?1
           AND fl.root_id = ?1
           AND fl.valid_to_scan_id IS NULL
         ORDER BY dg.id, fl.normalized_relative_path, dgm.file_version_id",
    )?;
    let rows = statement.query_map([root_id.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut groups = HashMap::<String, Vec<(String, String)>>::new();
    for row in rows {
        let (file_version_id, group_id, path) = row?;
        groups
            .entry(group_id)
            .or_default()
            .push((file_version_id, path));
    }
    let index_by_version = files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.file_version_id.clone(), index))
        .collect::<HashMap<_, _>>();
    for (group_id, members) in groups {
        let canonical = members.first().map(|(version, _)| version.clone());
        for (version, _) in members {
            if let Some(index) = index_by_version.get(&version) {
                files[*index].duplicate_group_id = Some(group_id.clone());
                files[*index].duplicate_canonical = canonical.as_deref() == Some(version.as_str());
            }
        }
    }
    Ok(())
}

fn insert_revision_with_meta(
    transaction: &Transaction<'_>,
    proposal: &OrganizationProposal,
    trigger_kind: &str,
    rebuild_mode: &str,
    rebuild_reason: Option<&str>,
    dirty_file_count: u64,
) -> Result<(), PersistenceError> {
    if !matches!(rebuild_mode, "full" | "incremental") {
        return Err(PersistenceError::InvalidProposal);
    }
    if rebuild_reason.is_some_and(|value| value.is_empty() || value.len() > 256) {
        return Err(PersistenceError::InvalidProposal);
    }
    let summary = &proposal.summary;
    let diff = &proposal.diff;
    transaction.execute(
        "INSERT INTO local_organization_proposal_revisions(
            id, proposal_id, revision_number, trigger_kind, status,
            source_semantic_version, source_relationship_version,
            files_analyzed, proposed_moves, proposed_renames, unchanged_count,
            needs_review_count, unresolved_count, conflict_count,
            high_confidence_count, medium_confidence_count, low_confidence_count,
            duplicate_no_action_count, average_depth, maximum_depth,
            destinations_changed, files_added, conflicts_resolved, moved_to_review,
            created_at, rebuild_mode, rebuild_reason, dirty_file_count
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
            ?26, ?27, ?28
         )",
        params![
            proposal.revision_id.to_string(),
            proposal.id.to_string(),
            i64::from(proposal.revision),
            trigger_kind,
            revision_status_name(proposal.status),
            proposal.source_semantic_version,
            proposal.source_relationship_version,
            to_sql_u64(summary.files_analyzed)?,
            to_sql_u64(summary.proposed_moves)?,
            to_sql_u64(summary.proposed_renames)?,
            to_sql_u64(summary.unchanged)?,
            to_sql_u64(summary.needs_review)?,
            to_sql_u64(summary.unresolved)?,
            to_sql_u64(summary.conflicts)?,
            to_sql_u64(summary.high_confidence)?,
            to_sql_u64(summary.medium_confidence)?,
            to_sql_u64(summary.low_confidence)?,
            to_sql_u64(summary.duplicate_no_action)?,
            f64::from(summary.average_depth),
            i64::from(summary.maximum_depth),
            to_sql_u64(diff.destinations_changed)?,
            to_sql_u64(diff.files_added)?,
            to_sql_u64(diff.conflicts_resolved)?,
            to_sql_u64(diff.moved_to_review)?,
            proposal.updated_at,
            rebuild_mode,
            rebuild_reason,
            to_sql_u64(dirty_file_count)?,
        ],
    )?;
    Ok(())
}

fn upsert_proposal_header(
    transaction: &Transaction<'_>,
    proposal: &OrganizationProposal,
) -> Result<(), PersistenceError> {
    let exists = transaction
        .query_row(
            "SELECT 1 FROM local_organization_proposals WHERE id = ?1",
            [proposal.id.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists && proposal.status != OrganizationProposalStatus::Cancelled {
        transaction.execute(
            "UPDATE local_organization_proposals
             SET status = 'superseded',
                 superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1
               AND root_id = ?2
               AND status IN (
                    'draft', 'ready_for_review', 'reviewed',
                    'approved_for_future_apply'
               )",
            params![
                proposal.workspace_id.to_string(),
                proposal.root_id.to_string()
            ],
        )?;
    }
    if !exists {
        transaction.execute(
            "INSERT INTO local_organization_proposals(
                id, workspace_id, root_id, source_scan_id, status,
                engine_version, policy_version, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                proposal.id.to_string(),
                proposal.workspace_id.to_string(),
                proposal.root_id.to_string(),
                proposal.source_scan_id.to_string(),
                proposal.status.database_name(),
                proposal.engine_version,
                proposal.policy_version,
                proposal.created_at,
                proposal.updated_at,
            ],
        )?;
    }
    Ok(())
}

fn finalize_proposal_header(
    transaction: &Transaction<'_>,
    proposal: &OrganizationProposal,
) -> Result<(), PersistenceError> {
    if proposal.status != OrganizationProposalStatus::Cancelled {
        transaction.execute(
            "UPDATE local_organization_proposals
             SET root_id = ?2,
                 source_scan_id = ?3,
                 current_revision_id = ?4,
                 status = ?5,
                 engine_version = ?6,
                 policy_version = ?7,
                 updated_at = ?8
             WHERE id = ?1",
            params![
                proposal.id.to_string(),
                proposal.root_id.to_string(),
                proposal.source_scan_id.to_string(),
                proposal.revision_id.to_string(),
                proposal.status.database_name(),
                proposal.engine_version,
                proposal.policy_version,
                proposal.updated_at,
            ],
        )?;
    }
    Ok(())
}

fn replace_proposal_dependencies(
    transaction: &Transaction<'_>,
    proposal: &OrganizationProposal,
) -> Result<(), PersistenceError> {
    // Table may be absent on pre-migration fixtures opened mid-test; ignore.
    let exists: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'local_organization_proposal_dependencies'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return Ok(());
    }
    transaction.execute(
        "DELETE FROM local_organization_proposal_dependencies WHERE revision_id = ?1",
        [proposal.revision_id.to_string()],
    )?;
    let mut statement = transaction.prepare(
        "INSERT INTO local_organization_proposal_dependencies(
            revision_id, file_id, dependency_kind, dependency_key
         ) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for operation in &proposal.operations {
        if let Some(customer) = &operation.customer_name {
            statement.execute(params![
                proposal.revision_id.to_string(),
                operation.file_id.to_string(),
                "identity_customer",
                customer.to_ascii_lowercase(),
            ])?;
        }
        if let Some(supplier) = &operation.supplier_name {
            statement.execute(params![
                proposal.revision_id.to_string(),
                operation.file_id.to_string(),
                "identity_supplier",
                supplier.to_ascii_lowercase(),
            ])?;
        }
        if let Some(project) = &operation.project_name {
            statement.execute(params![
                proposal.revision_id.to_string(),
                operation.file_id.to_string(),
                "identity_project",
                project.to_ascii_lowercase(),
            ])?;
        }
        let destination_key = operation
            .proposed_destination
            .iter()
            .map(|segment| segment.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\\");
        if !destination_key.is_empty() {
            statement.execute(params![
                proposal.revision_id.to_string(),
                operation.file_id.to_string(),
                "destination_prefix",
                truncate_database_text(&destination_key, 1024),
            ])?;
        }
        let collision = format!(
            "{}\\{}",
            operation
                .machine_destination
                .iter()
                .map(|segment| segment.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("\\"),
            operation.machine_name.to_ascii_lowercase()
        );
        statement.execute(params![
            proposal.revision_id.to_string(),
            operation.file_id.to_string(),
            "collision_key",
            truncate_database_text(&collision, 1024),
        ])?;
    }
    Ok(())
}

fn insert_operation(
    transaction: &Transaction<'_>,
    proposal: &OrganizationProposal,
    operation: &OrganizationProposalOperation,
) -> Result<(), PersistenceError> {
    let machine_destination = serde_json::to_string(&operation.machine_destination)
        .map_err(|_| PersistenceError::InvalidProposal)?;
    let proposed_destination = serde_json::to_string(&operation.proposed_destination)
        .map_err(|_| PersistenceError::InvalidProposal)?;
    transaction.execute(
        "INSERT INTO local_organization_proposal_operations(
            id, proposal_id, revision_id, file_id, file_version_id,
            operation_kind, source_relative_path, source_name, source_hash,
            source_byte_size, source_modified_at, machine_destination_json,
            machine_name, proposed_destination_json, proposed_name,
            confidence_score, confidence_level, conflict_state, needs_review,
            stale, user_override, disruption_score, proposed_path_length,
            proposed_depth, semantic_context, document_type, customer_name,
            supplier_name, project_name, duplicate_group_id, duplicate_canonical
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
            ?25, ?26, ?27, ?28, ?29, ?30, ?31
         )",
        params![
            operation.id.to_string(),
            proposal.id.to_string(),
            proposal.revision_id.to_string(),
            operation.file_id.to_string(),
            operation.file_version_id.to_string(),
            operation.operation_kind.database_name(),
            operation.source.relative_path,
            operation.source_name,
            operation.source.content_hash,
            to_sql_u64(operation.source.byte_size)?,
            operation.source.modified_at,
            machine_destination,
            operation.machine_name,
            proposed_destination,
            operation.proposed_name,
            f64::from(operation.confidence_score),
            operation.confidence_level.database_name(),
            operation.conflict_state.database_name(),
            i64::from(operation.needs_review),
            i64::from(operation.stale),
            i64::from(operation.user_override),
            f64::from(operation.disruption_score),
            i64::try_from(operation.proposed_path_length)
                .map_err(|_| PersistenceError::NumericOverflow)?,
            i64::try_from(operation.proposed_depth)
                .map_err(|_| PersistenceError::NumericOverflow)?,
            operation.semantic_context,
            operation.document_type,
            operation.customer_name,
            operation.supplier_name,
            operation.project_name,
            operation.duplicate_group_id,
            i64::from(operation.duplicate_canonical),
        ],
    )?;
    for (index, reason) in operation.reasons.iter().enumerate() {
        let evidence = serde_json::to_string(&reason.evidence_references)
            .map_err(|_| PersistenceError::InvalidProposal)?;
        transaction.execute(
            "INSERT INTO local_organization_proposal_reasons(
                id, operation_id, reason_order, reason_code,
                explanation, evidence_references_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::now_v7().to_string(),
                operation.id.to_string(),
                i64::try_from(index).map_err(|_| PersistenceError::NumericOverflow)?,
                reason.code,
                reason.explanation,
                evidence,
            ],
        )?;
    }
    Ok(())
}

fn insert_virtual_node(
    transaction: &Transaction<'_>,
    proposal: &OrganizationProposal,
    node: &VirtualProposalNode,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT INTO local_organization_virtual_nodes(
            id, revision_id, parent_id, node_kind, name, virtual_path,
            operation_id, child_count, needs_review_count, conflict_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            node.id.to_string(),
            proposal.revision_id.to_string(),
            node.parent_id.map(|value| value.to_string()),
            virtual_node_kind_name(node.kind),
            node.name,
            node.virtual_path,
            node.operation_id.map(|value| value.to_string()),
            to_sql_u64(node.child_count)?,
            to_sql_u64(node.needs_review_count)?,
            to_sql_u64(node.conflict_count)?,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct UiProposalBound {
    limit: usize,
}

fn organization_proposal_from_connection(
    connection: &Connection,
    proposal_id: ProposalId,
    ui_bound: Option<UiProposalBound>,
) -> Result<OrganizationProposal, PersistenceError> {
    #[allow(clippy::type_complexity)]
    let row: (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        Option<String>,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        f64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = connection
        .query_row(
            "SELECT
                proposal.workspace_id, proposal.root_id, proposal.source_scan_id,
                proposal.current_revision_id, proposal.status,
                proposal.engine_version, proposal.policy_version,
                proposal.created_at, revision.revision_number,
                revision.source_semantic_version, revision.source_relationship_version,
                revision.files_analyzed, revision.proposed_moves,
                revision.proposed_renames, revision.unchanged_count,
                revision.needs_review_count, revision.unresolved_count,
                revision.conflict_count, revision.high_confidence_count,
                revision.medium_confidence_count, revision.low_confidence_count,
                revision.average_depth, revision.maximum_depth,
                revision.duplicate_no_action_count, revision.destinations_changed,
                revision.files_added, revision.conflicts_resolved,
                revision.moved_to_review
             FROM local_organization_proposals proposal
             JOIN local_organization_proposal_revisions revision
                ON revision.id = proposal.current_revision_id
             WHERE proposal.id = ?1",
            [proposal_id.to_string()],
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
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                    row.get(17)?,
                    row.get(18)?,
                    row.get(19)?,
                    row.get(20)?,
                    row.get(21)?,
                    row.get(22)?,
                    row.get(23)?,
                    row.get(24)?,
                    row.get(25)?,
                    row.get(26)?,
                    row.get(27)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let revision_id = row.3.parse::<OrganizationRevisionId>()?;
    let operations = match ui_bound {
        Some(bound) => load_operations_bounded(connection, revision_id, bound.limit)?,
        None => load_operations(connection, revision_id)?,
    };
    let nodes = match ui_bound {
        Some(_) => load_folder_nodes(connection, revision_id)?,
        None => load_nodes(connection, revision_id)?,
    };
    let updated_at = connection.query_row(
        "SELECT updated_at FROM local_organization_proposals WHERE id = ?1",
        [proposal_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(OrganizationProposal {
        id: proposal_id,
        revision_id,
        workspace_id: row.0.parse()?,
        root_id: row.1.parse()?,
        source_scan_id: row.2.parse()?,
        revision: u32::try_from(row.8).map_err(|_| PersistenceError::NumericOverflow)?,
        status: parse_proposal_status(&row.4)?,
        engine_version: row.5,
        policy_version: row.6,
        source_semantic_version: row.9,
        source_relationship_version: row.10,
        created_at: row.7,
        updated_at,
        summary: OrganizationProposalSummary {
            files_analyzed: from_sql_u64(row.11)?,
            proposed_moves: from_sql_u64(row.12)?,
            proposed_renames: from_sql_u64(row.13)?,
            unchanged: from_sql_u64(row.14)?,
            needs_review: from_sql_u64(row.15)?,
            unresolved: from_sql_u64(row.16)?,
            conflicts: from_sql_u64(row.17)?,
            high_confidence: from_sql_u64(row.18)?,
            medium_confidence: from_sql_u64(row.19)?,
            low_confidence: from_sql_u64(row.20)?,
            duplicate_no_action: from_sql_u64(row.23)?,
            average_depth: row.21 as f32,
            maximum_depth: u32::try_from(row.22).map_err(|_| PersistenceError::NumericOverflow)?,
        },
        diff: OrganizationProposalDiff {
            destinations_changed: from_sql_u64(row.24)?,
            files_added: from_sql_u64(row.25)?,
            conflicts_resolved: from_sql_u64(row.26)?,
            moved_to_review: from_sql_u64(row.27)?,
        },
        nodes,
        operations,
    })
}

fn load_operations(
    connection: &Connection,
    revision_id: OrganizationRevisionId,
) -> Result<Vec<OrganizationProposalOperation>, PersistenceError> {
    let mut reason_statement = connection.prepare(
        "SELECT
            reason.operation_id, reason.reason_code, reason.explanation,
            reason.evidence_references_json
         FROM local_organization_proposal_reasons reason
         JOIN local_organization_proposal_operations operation
            ON operation.id = reason.operation_id
         WHERE operation.revision_id = ?1
         ORDER BY reason.operation_id, reason.reason_order",
    )?;
    let reason_rows = reason_statement.query_map([revision_id.to_string()], |row| {
        let evidence = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(3)?)
            .map_err(|error| to_sql_conversion_boxed(3, error))?;
        Ok((
            row.get::<_, String>(0)?,
            OrganizationReason {
                code: row.get(1)?,
                explanation: row.get(2)?,
                evidence_references: evidence,
            },
        ))
    })?;
    let mut reasons = HashMap::<String, Vec<OrganizationReason>>::new();
    for row in reason_rows {
        let (operation_id, reason) = row?;
        reasons.entry(operation_id).or_default().push(reason);
    }

    let mut statement = connection.prepare(
        "SELECT
            id, file_id, file_version_id, operation_kind,
            source_relative_path, source_name, source_hash, source_byte_size,
            source_modified_at, machine_destination_json, machine_name,
            proposed_destination_json, proposed_name, confidence_score,
            confidence_level, conflict_state, needs_review, stale, user_override,
            disruption_score, proposed_path_length, proposed_depth,
            semantic_context, document_type, customer_name, supplier_name,
            project_name, duplicate_group_id, duplicate_canonical
         FROM local_organization_proposal_operations
         WHERE revision_id = ?1
         ORDER BY proposed_destination_json, proposed_name, source_relative_path",
    )?;
    let rows = statement.query_map([revision_id.to_string()], |row| {
        let operation_id_text = row.get::<_, String>(0)?;
        let machine_destination = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(9)?)
            .map_err(|error| to_sql_conversion_boxed(9, error))?;
        let proposed_destination = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(11)?)
            .map_err(|error| to_sql_conversion_boxed(11, error))?;
        Ok((
            operation_id_text.clone(),
            OrganizationProposalOperation {
                id: parse_uuid_column(operation_id_text, 0)?,
                file_id: parse_uuid_column(row.get(1)?, 1)?,
                file_version_id: parse_uuid_column(row.get(2)?, 2)?,
                operation_kind: parse_operation_kind(&row.get::<_, String>(3)?)
                    .map_err(|error| to_sql_conversion_boxed(3, error))?,
                source: ProposalSourceSnapshot {
                    relative_path: row.get(4)?,
                    content_hash: row.get(6)?,
                    byte_size: from_sql_u64(row.get::<_, i64>(7)?)
                        .map_err(to_sql_conversion_error)?,
                    modified_at: row.get(8)?,
                },
                source_name: row.get(5)?,
                machine_destination,
                machine_name: row.get(10)?,
                proposed_destination,
                proposed_name: row.get(12)?,
                confidence_score: row.get::<_, f64>(13)? as f32,
                confidence_level: parse_confidence_level(&row.get::<_, String>(14)?)
                    .map_err(|error| to_sql_conversion_boxed(14, error))?,
                reasons: Vec::new(),
                conflict_state: parse_conflict_state(&row.get::<_, String>(15)?)
                    .map_err(|error| to_sql_conversion_boxed(15, error))?,
                needs_review: row.get::<_, i64>(16)? != 0,
                stale: row.get::<_, i64>(17)? != 0,
                user_override: row.get::<_, i64>(18)? != 0,
                disruption_score: row.get::<_, f64>(19)? as f32,
                proposed_path_length: usize::try_from(row.get::<_, i64>(20)?)
                    .map_err(|error| to_sql_conversion_boxed(20, error))?,
                proposed_depth: usize::try_from(row.get::<_, i64>(21)?)
                    .map_err(|error| to_sql_conversion_boxed(21, error))?,
                semantic_context: row.get(22)?,
                document_type: row.get(23)?,
                customer_name: row.get(24)?,
                supplier_name: row.get(25)?,
                project_name: row.get(26)?,
                duplicate_group_id: row.get(27)?,
                duplicate_canonical: row.get::<_, i64>(28)? != 0,
            },
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let (operation_id, mut operation) = row?;
        operation.reasons = reasons.remove(&operation_id).unwrap_or_default();
        output.push(operation);
    }
    Ok(output)
}

fn load_nodes(
    connection: &Connection,
    revision_id: OrganizationRevisionId,
) -> Result<Vec<VirtualProposalNode>, PersistenceError> {
    load_nodes_filtered(connection, revision_id, false)
}

fn load_folder_nodes(
    connection: &Connection,
    revision_id: OrganizationRevisionId,
) -> Result<Vec<VirtualProposalNode>, PersistenceError> {
    load_nodes_filtered(connection, revision_id, true)
}

fn load_nodes_filtered(
    connection: &Connection,
    revision_id: OrganizationRevisionId,
    folders_only: bool,
) -> Result<Vec<VirtualProposalNode>, PersistenceError> {
    let sql = if folders_only {
        "SELECT
            id, parent_id, node_kind, name, virtual_path, operation_id,
            child_count, needs_review_count, conflict_count
         FROM local_organization_virtual_nodes
         WHERE revision_id = ?1
           AND node_kind IN ('root', 'folder')
         ORDER BY length(virtual_path), virtual_path, node_kind"
    } else {
        "SELECT
            id, parent_id, node_kind, name, virtual_path, operation_id,
            child_count, needs_review_count, conflict_count
         FROM local_organization_virtual_nodes
         WHERE revision_id = ?1
         ORDER BY length(virtual_path), virtual_path, node_kind"
    };
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([revision_id.to_string()], |row| {
        Ok(VirtualProposalNode {
            id: parse_uuid_column(row.get(0)?, 0)?,
            parent_id: row
                .get::<_, Option<String>>(1)?
                .map(|value| parse_uuid_column(value, 1))
                .transpose()?,
            kind: parse_node_kind(&row.get::<_, String>(2)?)
                .map_err(|error| to_sql_conversion_boxed(2, error))?,
            name: row.get(3)?,
            virtual_path: row.get(4)?,
            operation_id: row
                .get::<_, Option<String>>(5)?
                .map(|value| parse_uuid_column(value, 5))
                .transpose()?,
            child_count: from_sql_u64(row.get::<_, i64>(6)?).map_err(to_sql_conversion_error)?,
            needs_review_count: from_sql_u64(row.get::<_, i64>(7)?)
                .map_err(to_sql_conversion_error)?,
            conflict_count: from_sql_u64(row.get::<_, i64>(8)?).map_err(to_sql_conversion_error)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(PersistenceError::Sql)
}

fn load_operations_bounded(
    connection: &Connection,
    revision_id: OrganizationRevisionId,
    limit: usize,
) -> Result<Vec<OrganizationProposalOperation>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            id, file_id, file_version_id, operation_kind,
            source_relative_path, source_name, source_hash, source_byte_size,
            source_modified_at, machine_destination_json, machine_name,
            proposed_destination_json, proposed_name, confidence_score,
            confidence_level, conflict_state, needs_review, stale, user_override,
            disruption_score, proposed_path_length, proposed_depth,
            semantic_context, document_type, customer_name, supplier_name,
            project_name, duplicate_group_id, duplicate_canonical
         FROM local_organization_proposal_operations
         WHERE revision_id = ?1
         ORDER BY proposed_destination_json, proposed_name, source_relative_path
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![revision_id.to_string(), limit as i64], |row| {
        let operation_id_text = row.get::<_, String>(0)?;
        let machine_destination = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(9)?)
            .map_err(|error| to_sql_conversion_boxed(9, error))?;
        let proposed_destination = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(11)?)
            .map_err(|error| to_sql_conversion_boxed(11, error))?;
        Ok((
            operation_id_text.clone(),
            OrganizationProposalOperation {
                id: parse_uuid_column(operation_id_text, 0)?,
                file_id: parse_uuid_column(row.get(1)?, 1)?,
                file_version_id: parse_uuid_column(row.get(2)?, 2)?,
                operation_kind: parse_operation_kind(&row.get::<_, String>(3)?)
                    .map_err(|error| to_sql_conversion_boxed(3, error))?,
                source: ProposalSourceSnapshot {
                    relative_path: row.get(4)?,
                    content_hash: row.get(6)?,
                    byte_size: from_sql_u64(row.get::<_, i64>(7)?)
                        .map_err(to_sql_conversion_error)?,
                    modified_at: row.get(8)?,
                },
                source_name: row.get(5)?,
                machine_destination,
                machine_name: row.get(10)?,
                proposed_destination,
                proposed_name: row.get(12)?,
                confidence_score: row.get::<_, f64>(13)? as f32,
                confidence_level: parse_confidence_level(&row.get::<_, String>(14)?)
                    .map_err(|error| to_sql_conversion_boxed(14, error))?,
                reasons: Vec::new(),
                conflict_state: parse_conflict_state(&row.get::<_, String>(15)?)
                    .map_err(|error| to_sql_conversion_boxed(15, error))?,
                needs_review: row.get::<_, i64>(16)? != 0,
                stale: row.get::<_, i64>(17)? != 0,
                user_override: row.get::<_, i64>(18)? != 0,
                disruption_score: row.get::<_, f64>(19)? as f32,
                proposed_path_length: usize::try_from(row.get::<_, i64>(20)?)
                    .map_err(|error| to_sql_conversion_boxed(20, error))?,
                proposed_depth: usize::try_from(row.get::<_, i64>(21)?)
                    .map_err(|error| to_sql_conversion_boxed(21, error))?,
                semantic_context: row.get(22)?,
                document_type: row.get(23)?,
                customer_name: row.get(24)?,
                supplier_name: row.get(25)?,
                project_name: row.get(26)?,
                duplicate_group_id: row.get(27)?,
                duplicate_canonical: row.get::<_, i64>(28)? != 0,
            },
        ))
    })?;
    let mut output = Vec::new();
    let mut operation_ids = Vec::new();
    for row in rows {
        let (operation_id, operation) = row?;
        operation_ids.push(operation_id);
        output.push(operation);
    }
    if operation_ids.is_empty() {
        return Ok(output);
    }
    let placeholders = operation_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let reason_sql = format!(
        "SELECT
            reason.operation_id, reason.reason_code, reason.explanation,
            reason.evidence_references_json
         FROM local_organization_proposal_reasons reason
         WHERE reason.operation_id IN ({placeholders})
         ORDER BY reason.operation_id, reason.reason_order"
    );
    let mut reason_statement = connection.prepare(&reason_sql)?;
    let params_dyn = operation_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect::<Vec<_>>();
    let reason_rows = reason_statement.query_map(params_dyn.as_slice(), |row| {
        let evidence = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(3)?)
            .map_err(|error| to_sql_conversion_boxed(3, error))?;
        Ok((
            row.get::<_, String>(0)?,
            OrganizationReason {
                code: row.get(1)?,
                explanation: row.get(2)?,
                evidence_references: evidence,
            },
        ))
    })?;
    let mut reasons = HashMap::<String, Vec<OrganizationReason>>::new();
    for row in reason_rows {
        let (operation_id, reason) = row?;
        reasons.entry(operation_id).or_default().push(reason);
    }
    for (index, operation_id) in operation_ids.iter().enumerate() {
        output[index].reasons = reasons.remove(operation_id).unwrap_or_default();
    }
    Ok(output)
}

fn valid_preference_component(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 80
        && !matches!(trimmed, "." | "..")
        && !trimmed
            .chars()
            .any(|character| character.is_control() || r#"\/:*?"<>|"#.contains(character))
}

fn valid_rename_template(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > 256
        || trimmed.contains("..")
        || trimmed
            .chars()
            .any(|character| character.is_control() || r#"\/:*?"<>|"#.contains(character))
    {
        return false;
    }
    let mut rest = trimmed;
    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            return false;
        };
        if !matches!(
            &after_open[..close],
            "date" | "party" | "document_type" | "identifier" | "project" | "original"
        ) {
            return false;
        }
        rest = &after_open[close + 1..];
    }
    !rest.contains('}')
}

fn validate_proposal(
    proposal: &OrganizationProposal,
    trigger_kind: &str,
) -> Result<(), PersistenceError> {
    if !matches!(
        trigger_kind,
        "initial"
            | "manual_recompute"
            | "semantic_changed"
            | "relationships_changed"
            | "user_override"
            | "algorithm_changed"
    ) || proposal.revision == 0
        || proposal.engine_version.is_empty()
        || proposal.engine_version.len() > 64
        || proposal.policy_version.is_empty()
        || proposal.policy_version.len() > 64
        || proposal.operations.len() as u64 != proposal.summary.files_analyzed
        || proposal.summary.maximum_depth > 8
    {
        return Err(PersistenceError::InvalidProposal);
    }
    let operation_ids = proposal
        .operations
        .iter()
        .map(|operation| operation.id)
        .collect::<std::collections::HashSet<_>>();
    if operation_ids.len() != proposal.operations.len()
        || proposal.nodes.iter().any(|node| {
            node.operation_id
                .is_some_and(|operation_id| !operation_ids.contains(&operation_id))
        })
    {
        return Err(PersistenceError::InvalidProposal);
    }
    Ok(())
}

fn validate_override(value: &OrganizationProposalOverride) -> Result<(), PersistenceError> {
    let destination_valid = value.destination.as_ref().is_none_or(|segments| {
        !segments.is_empty()
            && segments.len() <= 8
            && segments
                .iter()
                .all(|segment| !segment.is_empty() && segment.chars().count() <= 512)
    });
    let name_valid = value
        .proposed_name
        .as_ref()
        .is_none_or(|name| !name.is_empty() && name.chars().count() <= 512);
    let required_fields = match value.action {
        ProposalOverrideAction::Destination => value.destination.is_some(),
        ProposalOverrideAction::Rename => value.proposed_name.is_some(),
        ProposalOverrideAction::DestinationAndRename => {
            value.destination.is_some() && value.proposed_name.is_some()
        }
        ProposalOverrideAction::KeepInPlace
        | ProposalOverrideAction::ToReview
        | ProposalOverrideAction::Reject => true,
    };
    if !destination_valid || !name_valid || !required_fields {
        return Err(PersistenceError::InvalidProposal);
    }
    Ok(())
}

fn parse_operation_kind(value: &str) -> Result<ProposalOperationKind, PersistenceError> {
    match value {
        "move_proposal" => Ok(ProposalOperationKind::MoveProposal),
        "rename_proposal" => Ok(ProposalOperationKind::RenameProposal),
        "create_folder_proposal" => Ok(ProposalOperationKind::CreateFolderProposal),
        "keep_in_place" => Ok(ProposalOperationKind::KeepInPlace),
        "to_review" => Ok(ProposalOperationKind::ToReview),
        "no_action" => Ok(ProposalOperationKind::NoAction),
        _ => Err(PersistenceError::InvalidProposal),
    }
}

fn parse_confidence_level(value: &str) -> Result<ProposalConfidenceLevel, PersistenceError> {
    match value {
        "very_high" => Ok(ProposalConfidenceLevel::VeryHigh),
        "high" => Ok(ProposalConfidenceLevel::High),
        "medium" => Ok(ProposalConfidenceLevel::Medium),
        "low" => Ok(ProposalConfidenceLevel::Low),
        _ => Err(PersistenceError::InvalidProposal),
    }
}

fn parse_conflict_state(value: &str) -> Result<ProposalConflictState, PersistenceError> {
    match value {
        "none" => Ok(ProposalConflictState::None),
        "auto_resolved" => Ok(ProposalConflictState::AutoResolved),
        "destination_collision" => Ok(ProposalConflictState::DestinationCollision),
        "invalid_path" => Ok(ProposalConflictState::InvalidPath),
        "path_too_long" => Ok(ProposalConflictState::PathTooLong),
        "stale_source" => Ok(ProposalConflictState::StaleSource),
        _ => Err(PersistenceError::InvalidProposal),
    }
}

fn parse_proposal_status(value: &str) -> Result<OrganizationProposalStatus, PersistenceError> {
    match value {
        "draft" => Ok(OrganizationProposalStatus::Draft),
        "ready_for_review" => Ok(OrganizationProposalStatus::ReadyForReview),
        "reviewed" => Ok(OrganizationProposalStatus::Reviewed),
        "approved_for_future_apply" => Ok(OrganizationProposalStatus::ApprovedForFutureApply),
        "superseded" => Ok(OrganizationProposalStatus::Superseded),
        "cancelled" => Ok(OrganizationProposalStatus::Cancelled),
        _ => Err(PersistenceError::InvalidProposal),
    }
}

fn parse_override_action(value: &str) -> Result<ProposalOverrideAction, PersistenceError> {
    match value {
        "destination" => Ok(ProposalOverrideAction::Destination),
        "rename" => Ok(ProposalOverrideAction::Rename),
        "destination_and_rename" => Ok(ProposalOverrideAction::DestinationAndRename),
        "keep_in_place" => Ok(ProposalOverrideAction::KeepInPlace),
        "to_review" => Ok(ProposalOverrideAction::ToReview),
        "reject" => Ok(ProposalOverrideAction::Reject),
        _ => Err(PersistenceError::InvalidProposal),
    }
}

fn parse_node_kind(value: &str) -> Result<VirtualNodeKind, PersistenceError> {
    match value {
        "root" => Ok(VirtualNodeKind::Root),
        "folder" => Ok(VirtualNodeKind::Folder),
        "file" => Ok(VirtualNodeKind::File),
        _ => Err(PersistenceError::InvalidProposal),
    }
}

fn revision_status_name(status: OrganizationProposalStatus) -> &'static str {
    match status {
        OrganizationProposalStatus::Draft => "draft",
        OrganizationProposalStatus::Cancelled => "cancelled",
        OrganizationProposalStatus::ReadyForReview
        | OrganizationProposalStatus::Reviewed
        | OrganizationProposalStatus::ApprovedForFutureApply
        | OrganizationProposalStatus::Superseded => "ready_for_review",
    }
}

fn virtual_node_kind_name(kind: VirtualNodeKind) -> &'static str {
    match kind {
        VirtualNodeKind::Root => "root",
        VirtualNodeKind::Folder => "folder",
        VirtualNodeKind::File => "file",
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

fn to_sql_conversion_error(error: PersistenceError) -> rusqlite::Error {
    to_sql_conversion_boxed(0, error)
}

fn to_sql_conversion_boxed(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}
