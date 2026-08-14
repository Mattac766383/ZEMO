//! Deterministic synthetic large-scale fixtures for Milestone 13 qualification.
//!
//! Builds a realistic catalog/search/identity/review/vector dataset without
//! creating 100k physical binary files. Architecture/scale validation only.

use crate::{
    Database, DatabaseKey, DuplicateGroupInput, PersistenceError, RootRecord, ScanCompletionInput,
    ScanFileInput, WorkspaceRecord,
};
use domain::{
    DisplayLabel, FileFingerprint, FileId, FileKind, FileObservation, FileVersionId,
    NativeFileIdentity, NativePath, PathEncoding, PlatformKind, RootId, ScanId, VolumeIdentity,
    WorkspaceId,
};
use rusqlite::{OptionalExtension, params};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const BATCH_SIZE: usize = 2_000;
const PROVIDER_ID: &str = "deterministic-m13";
const PROVIDER_VERSION: &str = "m13-scale-v1";
const CHUNKING_POLICY: &str = "chunking-v1-m13-scale";

#[derive(Debug, Clone)]
pub struct LargeScaleFixtureConfig {
    pub file_count: usize,
    pub identity_count: usize,
    pub project_count: usize,
    pub review_item_target: usize,
    pub vector_file_count: usize,
    pub root_label: String,
    pub root_path: PathBuf,
}

impl Default for LargeScaleFixtureConfig {
    fn default() -> Self {
        Self {
            file_count: 100_000,
            identity_count: 2_500,
            project_count: 800,
            review_item_target: 12_000,
            vector_file_count: 100_000,
            root_label: "m13-scale".to_owned(),
            root_path: PathBuf::from("/tmp/supremacy-m13-synthetic-root"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LargeScaleFixtureStats {
    pub files: u64,
    pub identities: u64,
    pub projects: u64,
    pub review_items: u64,
    pub vector_rows: u64,
    pub catalog_ingest_ms: u128,
    pub enrichment_ms: u128,
    pub database_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct LargeScaleFixture {
    pub workspace: WorkspaceRecord,
    pub root: RootRecord,
    pub scan_id: ScanId,
    pub stats: LargeScaleFixtureStats,
}

#[derive(Debug, Clone, Copy)]
struct FileProfile {
    extension: &'static str,
    mime: &'static str,
    folder: &'static str,
    document_type: Option<&'static str>,
    context: Option<&'static str>,
    lexical_token: &'static str,
}

const PROFILES: &[FileProfile] = &[
    FileProfile {
        extension: "pdf",
        mime: "application/pdf",
        folder: "Documents/Taxes",
        document_type: Some("tax_document"),
        context: Some("personal"),
        lexical_token: "tax invoice fiscal",
    },
    FileProfile {
        extension: "pdf",
        mime: "application/pdf",
        folder: "Business/Clients",
        document_type: Some("contract"),
        context: Some("business"),
        lexical_token: "contract client agreement",
    },
    FileProfile {
        extension: "docx",
        mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        folder: "Documents/Insurance",
        document_type: Some("insurance_document"),
        context: Some("personal"),
        lexical_token: "insurance policy coverage",
    },
    FileProfile {
        extension: "xlsx",
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        folder: "Business/Quotes",
        document_type: Some("quote"),
        context: Some("business"),
        lexical_token: "quote supplier estimate",
    },
    FileProfile {
        extension: "pptx",
        mime: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        folder: "Business/Projects",
        document_type: Some("presentation"),
        context: Some("business"),
        lexical_token: "project presentation milestone",
    },
    FileProfile {
        extension: "txt",
        mime: "text/plain",
        folder: "Documents/Banking",
        document_type: Some("bank_statement"),
        context: Some("personal"),
        lexical_token: "bank statement transfer",
    },
    FileProfile {
        extension: "jpg",
        mime: "image/jpeg",
        folder: "Photos/Personal",
        document_type: Some("photo"),
        context: Some("personal"),
        lexical_token: "photo family vacation",
    },
    FileProfile {
        extension: "png",
        mime: "image/png",
        folder: "Photos/Business",
        document_type: Some("photo"),
        context: Some("business"),
        lexical_token: "site photo project",
    },
    FileProfile {
        extension: "heic",
        mime: "image/heic",
        folder: "Photos/Mixed",
        document_type: None,
        context: Some("unknown"),
        lexical_token: "camera roll image",
    },
    FileProfile {
        extension: "mp4",
        mime: "video/mp4",
        folder: "Videos",
        document_type: Some("video"),
        context: None,
        lexical_token: "video catalog only",
    },
    FileProfile {
        extension: "zip",
        mime: "application/zip",
        folder: "Archives",
        document_type: Some("archive"),
        context: None,
        lexical_token: "archive backup package",
    },
    FileProfile {
        extension: "pdf",
        mime: "application/pdf",
        folder: "Business/Suppliers",
        document_type: Some("invoice"),
        context: Some("business"),
        lexical_token: "supplier invoice vat",
    },
];

impl Database {
    /// Seed a deterministic synthetic large-scale workspace for M13 benchmarks.
    ///
    /// Uses the production `complete_scan` path in batches
    /// (SYNTHETIC CATALOG, REAL SQLITE). Does not create physical payloads.
    pub fn seed_large_scale_fixture(
        &self,
        config: &LargeScaleFixtureConfig,
    ) -> Result<LargeScaleFixture, PersistenceError> {
        let workspace = self.create_workspace("Milestone 13 large-scale fixture")?;
        let root_id = RootId::new();
        let volume = VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: format!("m13-volume-{}", workspace.id),
            filesystem_type: Some("apfs".to_owned()),
            case_sensitive: false,
            removable: false,
            local: true,
        };
        self.register_root(
            workspace.id,
            root_id,
            &config.root_path,
            &config.root_label,
            &volume,
        )?;
        let root = self
            .list_roots(workspace.id)?
            .into_iter()
            .find(|candidate| candidate.id == root_id)
            .ok_or(PersistenceError::NotFound)?;

        let ingest_started = std::time::Instant::now();
        let mut latest_scan_id = None;
        let mut persisted_total = 0_u64;

        for batch_start in (0..config.file_count).step_by(BATCH_SIZE) {
            let batch_end = (batch_start + BATCH_SIZE).min(config.file_count);
            let scan_id = ScanId::new();
            latest_scan_id = Some(scan_id);
            self.begin_scan(workspace.id, root_id, scan_id)?;

            let mut files = Vec::with_capacity(batch_end - batch_start);
            let mut batch_duplicates = Vec::new();
            let mut pending_primary: Option<(usize, FileVersionId, [u8; 32])> = None;

            for index in batch_start..batch_end {
                let profile = PROFILES[index % PROFILES.len()];
                let is_duplicate_primary = index % 20 == 0;
                let is_duplicate_copy = index % 20 == 1 && index > 0;

                let file_id = FileId::new();
                let version_id = FileVersionId::new();
                let (relative_path, filename, object_key, digest) = if is_duplicate_copy {
                    let primary_index = index - 1;
                    let primary_profile = PROFILES[primary_index % PROFILES.len()];
                    let filename = format!("dup_{primary_index:06}.{}", primary_profile.extension);
                    let relative_path = format!("Duplicates/{filename}");
                    let mut object_key = vec![0_u8; 16];
                    object_key[..8].copy_from_slice(&(index as u64).to_le_bytes());
                    object_key[8..16].copy_from_slice(&(primary_index as u64).to_le_bytes());
                    let mut digest = [0_u8; 32];
                    digest[..8].copy_from_slice(&(primary_index as u64).to_le_bytes());
                    digest[8] = 0xD0;
                    (relative_path, filename, object_key, Some(digest))
                } else {
                    let filename = format!("file_{index:06}.{}", profile.extension);
                    let relative_path = format!("{}/{}", profile.folder, filename);
                    let mut object_key = vec![0_u8; 16];
                    object_key[..8].copy_from_slice(&(index as u64).to_le_bytes());
                    object_key[8] = 0xA1;
                    let digest = if is_duplicate_primary || index % 7 == 0 {
                        let mut digest = [0_u8; 32];
                        digest[..8].copy_from_slice(&(index as u64).to_le_bytes());
                        digest[8] = 0xD0;
                        Some(digest)
                    } else {
                        None
                    };
                    (relative_path, filename, object_key, digest)
                };

                if is_duplicate_primary && let Some(digest) = digest {
                    pending_primary = Some((index, version_id, digest));
                }
                if is_duplicate_copy
                    && let Some((primary_index, primary_version, primary_digest)) = pending_primary
                    && primary_index + 1 == index
                {
                    batch_duplicates.push(DuplicateGroupInput {
                        digest: primary_digest.to_vec(),
                        byte_size: 4_096 + ((index as u64) % 50_000),
                        members: vec![primary_version, version_id],
                    });
                    pending_primary = None;
                }

                let leaf_bytes = filename.as_bytes().to_vec();
                let observation = FileObservation {
                    file_id,
                    version_id,
                    workspace_id: workspace.id,
                    root_id,
                    scan_id,
                    relative_path: NativePath {
                        encoding: PathEncoding::UnixBytes,
                        bytes: relative_path.as_bytes().to_vec(),
                    },
                    display_label: DisplayLabel::new(filename)
                        .map_err(|_| PersistenceError::InvalidNativePath)?,
                    kind: FileKind::Regular,
                    detected_mime: Some(profile.mime.to_owned()),
                    fingerprint: FileFingerprint {
                        native_identity: NativeFileIdentity {
                            volume: volume.clone(),
                            object_key,
                            parent_key: vec![0x50; 16],
                            leaf_name: NativePath {
                                encoding: PathEncoding::UnixBytes,
                                bytes: leaf_bytes,
                            },
                            link_count: 1,
                            reparse_tag: None,
                        },
                        byte_size: 1_024 + ((index as u64 * 97) % 250_000),
                        modified_at_ns: Some(1_700_000_000_000_000_000 + index as i128),
                        created_at_ns: Some(1_600_000_000_000_000_000 + index as i128),
                        attributes: 0,
                        quick_digest: None,
                        content_digest: digest,
                    },
                    read_only: false,
                    hidden: false,
                    cloud_placeholder: false,
                    encrypted: false,
                };
                files.push(ScanFileInput {
                    observation,
                    extension: Some(profile.extension.to_owned()),
                    accessed_at_ns: None,
                    readability_status: "readable".to_owned(),
                    scan_status: "indexed".to_owned(),
                    hashing_status: if digest.is_some() {
                        "hashed".to_owned()
                    } else {
                        "not_candidate".to_owned()
                    },
                    error_code: None,
                });
            }

            let bytes_discovered = files
                .iter()
                .map(|file| file.observation.fingerprint.byte_size)
                .fold(0_u64, u64::saturating_add);
            let hashed = files
                .iter()
                .filter(|file| file.observation.fingerprint.content_digest.is_some())
                .count();
            self.complete_scan(&ScanCompletionInput {
                scan_id,
                workspace_id: workspace.id,
                root_id,
                status: "completed".to_owned(),
                files_discovered: files.len() as u64,
                directories_discovered: 32,
                bytes_discovered,
                files_hashed: hashed as u64,
                errors: 0,
                skipped_items: 0,
                truncated: false,
                files,
                issues: Vec::new(),
                duplicate_groups: batch_duplicates,
            })?;
            persisted_total += (batch_end - batch_start) as u64;
        }

        let catalog_ingest_ms = ingest_started.elapsed().as_millis();
        let scan_id = latest_scan_id.ok_or(PersistenceError::NotFound)?;
        let enrichment_started = std::time::Instant::now();
        let enrichment = self.enrich_large_scale_fixture(workspace.id, scan_id, config)?;
        let enrichment_ms = enrichment_started.elapsed().as_millis();

        Ok(LargeScaleFixture {
            workspace,
            root,
            scan_id,
            stats: LargeScaleFixtureStats {
                files: persisted_total,
                identities: enrichment.identities,
                projects: enrichment.projects,
                review_items: enrichment.review_items,
                vector_rows: enrichment.vector_rows,
                catalog_ingest_ms,
                enrichment_ms,
                database_bytes: 0,
            },
        })
    }

    fn enrich_large_scale_fixture(
        &self,
        workspace_id: WorkspaceId,
        scan_id: ScanId,
        config: &LargeScaleFixtureConfig,
    ) -> Result<EnrichmentCounts, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;

        transaction.execute(
            "INSERT INTO local_embedding_models(
                provider_id, version, dimensions, availability, local_only,
                production_ready, requires_download, model_size_bytes, max_model_size_bytes
             ) VALUES (?1, ?2, 384, 'available_development', 1, 0, 0, 0, 1073741824)
             ON CONFLICT(provider_id, version) DO NOTHING",
            params![PROVIDER_ID, PROVIDER_VERSION],
        )?;
        transaction.execute(
            "INSERT INTO local_ann_index_state(
                workspace_id, provider_id, embedding_version, chunking_policy_version,
                index_format_version, dimensions, status, vector_count, next_ann_key
             ) VALUES (?1, ?2, ?3, ?4, 1, 384, 'ready', 0, 1)
             ON CONFLICT(workspace_id, provider_id, embedding_version) DO NOTHING",
            params![
                workspace_id.to_string(),
                PROVIDER_ID,
                PROVIDER_VERSION,
                CHUNKING_POLICY
            ],
        )?;

        let batch_id = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO semantic_analysis_batches(
                id, workspace_id, scan_id, status, files_queued, files_completed,
                high_confidence_count, needs_review_count, unknown_count,
                partial_count, failed_count, started_at, completed_at
             ) VALUES (
                ?1, ?2, ?3, 'completed', ?4, ?4, 0, 0, 0, 0, 0,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )",
            params![
                batch_id,
                workspace_id.to_string(),
                scan_id.to_string(),
                config.file_count as i64
            ],
        )?;

        let mut select = transaction.prepare(
            "SELECT d.file_id, d.file_version_id, d.filename, d.relative_path
             FROM local_search_documents d
             WHERE d.workspace_id = ?1
             ORDER BY d.filename",
        )?;
        let rows = select
            .query_map([workspace_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(select);

        let mut identity_ids = Vec::with_capacity(config.identity_count);
        let mut project_ids = Vec::with_capacity(config.project_count);
        for index in 0..config.identity_count {
            let id = Uuid::now_v7().to_string();
            let name = format!("Org {:04}", index % 1_000);
            transaction.execute(
                "INSERT INTO resolved_identities(
                    id, workspace_id, identity_type, display_name, normalized_display_name,
                    resolution_status, lifecycle_status, user_locked, confidence,
                    creation_source, resolver_version
                 ) VALUES (
                    ?1, ?2, 'organization', ?3, ?4, 'auto_linked', 'active', 0, 0.91,
                    'resolver', 'm13-scale-v1'
                 )",
                params![
                    id,
                    workspace_id.to_string(),
                    name,
                    name.to_ascii_lowercase()
                ],
            )?;
            identity_ids.push(id);
        }
        for index in 0..config.project_count {
            let id = Uuid::now_v7().to_string();
            let name = format!("Project {index:04}");
            transaction.execute(
                "INSERT INTO resolved_identities(
                    id, workspace_id, identity_type, display_name, normalized_display_name,
                    resolution_status, lifecycle_status, user_locked, confidence,
                    creation_source, resolver_version
                 ) VALUES (
                    ?1, ?2, 'project', ?3, ?4, 'auto_linked', 'active', 0, 0.88,
                    'resolver', 'm13-scale-v1'
                 )",
                params![
                    id,
                    workspace_id.to_string(),
                    name,
                    name.to_ascii_lowercase()
                ],
            )?;
            project_ids.push(id);
        }

        let mut review_items = 0_u64;
        let mut vector_rows = 0_u64;
        let mut next_ann_key = 1_i64;

        for (index, (file_id, file_version_id, filename, relative_path)) in rows.iter().enumerate()
        {
            let profile = PROFILES[index % PROFILES.len()];
            let extraction_status = match index % 11 {
                0 => "pending",
                1 => "failed",
                2 => "unsupported",
                3 => "partial",
                _ => "success",
            };
            let semantic_status = match index % 17 {
                0 => "unknown",
                1 => "partial",
                _ if profile.document_type.is_some() => "success",
                _ => "unknown",
            };
            let confidence = if profile.document_type.is_some() {
                Some(0.72_f64 + ((index % 25) as f64) * 0.01)
            } else {
                None
            };
            transaction.execute(
                "UPDATE local_search_documents
                 SET extraction_status = ?2,
                     semantic_document_type = ?3,
                     semantic_context = ?4,
                     semantic_status = ?5,
                     semantic_confidence = ?6,
                     metadata_text = trim(
                        COALESCE(extension, '') || ' ' ||
                        COALESCE(detected_type, '') || ' ' ||
                        COALESCE(?3, '') || ' ' ||
                        COALESCE(?4, '') || ' ' || ?7
                     ),
                     indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE file_id = ?1",
                params![
                    file_id,
                    extraction_status,
                    profile.document_type,
                    profile.context,
                    semantic_status,
                    confidence,
                    profile.lexical_token,
                ],
            )?;

            if review_items < config.review_item_target as u64
                && (index % 8 == 0
                    || semantic_status != "success"
                    || extraction_status != "success")
            {
                let (reason, source, severity) = match index % 6 {
                    0 => ("low_confidence_document_type", "semantic", "warning"),
                    1 => ("extraction_failed", "extraction", "error"),
                    2 => ("unsupported_format", "extraction", "information"),
                    3 => ("missing_critical_fields", "semantic", "warning"),
                    4 => ("partial_extraction", "extraction", "warning"),
                    _ => ("semantic_ambiguity", "semantic", "warning"),
                };
                let changed = transaction.execute(
                    "INSERT OR IGNORE INTO file_review_items(
                        id, workspace_id, file_id, file_version_id, reason, source_subsystem,
                        severity, status, explanation, retry_available, retry_count
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'needs_review', ?8, 0, 0
                     )",
                    params![
                        Uuid::now_v7().to_string(),
                        workspace_id.to_string(),
                        file_id,
                        file_version_id,
                        reason,
                        source,
                        severity,
                        format!("Synthetic review for {filename}"),
                    ],
                )?;
                if changed > 0 {
                    review_items = review_items.saturating_add(1);
                }
            }

            let analysis_id = Uuid::now_v7().to_string();
            let mut digest = [0_u8; 32];
            digest[..8].copy_from_slice(&(index as u64).to_le_bytes());
            transaction.execute(
                "INSERT INTO semantic_analyses(
                    id, batch_id, workspace_id, scan_id, file_id, file_version_id, status,
                    analyzer_id, analyzer_version, provider_id, provider_version,
                    schema_version, processing_location, input_digest,
                    input_character_count, analyzed_character_count, input_quality,
                    input_quality_status, input_quality_reasons_json, language,
                    duration_ms, is_current, started_at, completed_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    'm13-scale', 'm13-v1', 'deterministic', 'm13-v1',
                    1, 'local', ?8,
                    128, 128, 0.8, 'good', '[]', 'fr',
                    1, 1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    analysis_id,
                    batch_id,
                    workspace_id.to_string(),
                    scan_id.to_string(),
                    file_id,
                    file_version_id,
                    semantic_status,
                    digest.as_slice(),
                ],
            )?;

            if !identity_ids.is_empty() && index % 4 == 0 {
                let identity_id = &identity_ids[index % identity_ids.len()];
                let entity_id = Uuid::now_v7().to_string();
                let value = format!("Org {:04}", index % 1_000);
                transaction.execute(
                    "INSERT INTO semantic_entities(
                        id, analysis_id, candidate_key, entity_type, original_value,
                        normalized_value, confidence, field_status, source_method,
                        analyzer_version
                     ) VALUES (
                        ?1, ?2, ?3, 'organization', ?4, ?5, 0.9, 'inferred',
                        'deterministic_rule', 'm13-v1'
                     )",
                    params![
                        entity_id,
                        analysis_id,
                        format!("org-{index}"),
                        value,
                        value.to_ascii_lowercase()
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO identity_occurrences(
                        id, workspace_id, identity_id, source_key, file_id,
                        file_version_id, semantic_analysis_id, semantic_entity_id,
                        semantic_field_id, occurrence_type, original_value,
                        normalized_value, normalized_core, legal_suffix, confidence,
                        source_method, analyzer_version, resolver_version
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 'organization',
                        ?9, ?10, ?10, NULL, 0.9, 'deterministic_rule', 'm13-v1',
                        'm13-scale-v1'
                     )",
                    params![
                        Uuid::now_v7().to_string(),
                        workspace_id.to_string(),
                        identity_id,
                        format!("m13-occ-{index}"),
                        file_id,
                        file_version_id,
                        analysis_id,
                        entity_id,
                        value,
                        value.to_ascii_lowercase(),
                    ],
                )?;
                if index % 16 == 0 && !project_ids.is_empty() {
                    let project_id = &project_ids[index % project_ids.len()];
                    transaction.execute(
                        "INSERT OR IGNORE INTO identity_relationships(
                            id, workspace_id, source_kind, source_file_id,
                            source_identity_id, target_identity_id, relationship_type,
                            confidence, status, creation_source, resolver_version, active
                         ) VALUES (
                            ?1, ?2, 'file', ?3, NULL, ?4, 'file_project',
                            0.86, 'auto_linked', 'resolver', 'm13-scale-v1', 1
                         )",
                        params![
                            Uuid::now_v7().to_string(),
                            workspace_id.to_string(),
                            file_id,
                            project_id
                        ],
                    )?;
                }
            }

            if index < config.vector_file_count {
                let ann_key = next_ann_key;
                next_ann_key += 1;
                let mut vector = vec![0_u8; 384];
                let mut state = (index as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(1);
                for byte in &mut vector {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    *byte = ((state >> 33) as u8).wrapping_sub(128);
                }
                let mut text_hash = [0_u8; 32];
                text_hash[..8].copy_from_slice(&(index as u64).to_le_bytes());
                text_hash[8] = 0xC1;
                let source_id = format!("chunk-{index}");
                transaction.execute(
                    "INSERT INTO local_semantic_chunks(
                        id, workspace_id, file_id, file_version_id, semantic_analysis_id,
                        provider_id, embedding_version, chunking_policy_version, dimensions,
                        ann_key, source_id, source_kind, sequence_index, text_preview,
                        text_hash, status, truncated_file
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 384, ?9, ?10, 'text_chunk', 0,
                        ?11, ?12, 'active', 0
                     )",
                    params![
                        Uuid::now_v7().to_string(),
                        workspace_id.to_string(),
                        file_id,
                        file_version_id,
                        analysis_id,
                        PROVIDER_ID,
                        PROVIDER_VERSION,
                        CHUNKING_POLICY,
                        ann_key,
                        source_id,
                        format!("{filename} {relative_path}"),
                        text_hash.as_slice(),
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO local_search_embeddings(
                        id, workspace_id, file_id, file_version_id, semantic_analysis_id,
                        provider_id, embedding_version, dimensions, source_id, source_kind,
                        vector, input_digest
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, 384, ?8, 'text_chunk', ?9, ?10
                     )",
                    params![
                        Uuid::now_v7().to_string(),
                        workspace_id.to_string(),
                        file_id,
                        file_version_id,
                        analysis_id,
                        PROVIDER_ID,
                        PROVIDER_VERSION,
                        source_id,
                        vector,
                        text_hash.as_slice(),
                    ],
                )?;
                vector_rows = vector_rows.saturating_add(1);
            }
        }

        transaction.execute(
            "UPDATE local_ann_index_state
             SET vector_count = ?4,
                 next_ann_key = ?5,
                 status = 'ready',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND provider_id = ?2 AND embedding_version = ?3",
            params![
                workspace_id.to_string(),
                PROVIDER_ID,
                PROVIDER_VERSION,
                vector_rows as i64,
                next_ann_key
            ],
        )?;

        // Ensure FTS content view refresh for metadata_text updates.
        // External-content FTS5 requires delete+insert rebuild for updated rows.
        transaction.execute(
            "INSERT INTO local_search_fts(local_search_fts, rowid, filename, relative_path, extracted_text, metadata_text)
             SELECT 'delete', d.id, d.filename, d.relative_path, '', d.metadata_text
             FROM local_search_documents d
             WHERE d.workspace_id = ?1",
            [workspace_id.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO local_search_fts(rowid, filename, relative_path, extracted_text, metadata_text)
             SELECT d.id, d.filename, d.relative_path, '', d.metadata_text
             FROM local_search_documents d
             WHERE d.workspace_id = ?1",
            [workspace_id.to_string()],
        )?;

        let _ = transaction
            .query_row(
                "SELECT COUNT(*) FROM local_search_documents WHERE workspace_id = ?1",
                [workspace_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        transaction.commit()?;
        Ok(EnrichmentCounts {
            identities: config.identity_count as u64,
            projects: config.project_count as u64,
            review_items,
            vector_rows,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct EnrichmentCounts {
    identities: u64,
    projects: u64,
    review_items: u64,
    vector_rows: u64,
}

#[must_use]
pub fn database_file_size(path: &Path) -> u64 {
    let mut total = 0_u64;
    for suffix in ["", "-wal", "-shm"] {
        let candidate = if suffix.is_empty() {
            path.to_path_buf()
        } else {
            PathBuf::from(format!("{}{suffix}", path.display()))
        };
        if let Ok(meta) = std::fs::metadata(candidate) {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

pub fn open_scale_database(path: &Path, key: &DatabaseKey) -> Result<Database, PersistenceError> {
    Database::open(path, key)
}

pub const M13_PROVIDER_ID: &str = PROVIDER_ID;
pub const M13_PROVIDER_VERSION: &str = PROVIDER_VERSION;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_synthetic_fixture_seeds_expected_mix() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([13; 32]))
            .unwrap_or_else(|error| panic!("db open: {error}"));
        let fixture = database
            .seed_large_scale_fixture(&LargeScaleFixtureConfig {
                file_count: 240,
                identity_count: 20,
                project_count: 8,
                review_item_target: 40,
                vector_file_count: 120,
                ..LargeScaleFixtureConfig::default()
            })
            .unwrap_or_else(|error| panic!("seed: {error}"));
        assert_eq!(fixture.stats.files, 240);
        assert_eq!(fixture.stats.identities, 20);
        assert_eq!(fixture.stats.projects, 8);
        assert!(fixture.stats.review_items > 0);
        assert_eq!(fixture.stats.vector_rows, 120);
    }
}
