//! Chunk ↔ ANN key mapping for M9.1 Step 2.

use super::{Database, PersistenceError, from_sql_u64, to_sql_integer, to_sql_u64};
use domain::WorkspaceId;
use rusqlite::{Connection, OptionalExtension, params};
use search::{
    AnnIndexStatus, CHUNKING_POLICY_VERSION, EmbeddingAvailability, EmbeddingIndexEntry,
    EmbeddingProviderDescriptor, SemanticChunk, local_embedding_descriptor_is_safe,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AnnFileCandidate {
    pub file_id: String,
    pub similarity: f32,
    pub chunk_preview: String,
    pub source_id: String,
}

#[derive(Debug, Clone)]
pub struct AnnUpsertRecord {
    pub ann_key: u64,
    pub vector: Vec<u8>,
    pub source_id: String,
}

#[derive(Debug, Clone)]
pub struct FileChunkReplaceResult {
    pub upserts: Vec<AnnUpsertRecord>,
    pub removed_keys: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct AnnRebuildVector {
    pub ann_key: u64,
    pub vector: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FileChunkReplacement<'a> {
    pub workspace_id: WorkspaceId,
    pub file_id: &'a str,
    pub file_version_id: &'a str,
    pub semantic_analysis_id: &'a str,
    pub descriptor: &'a EmbeddingProviderDescriptor,
    pub availability: EmbeddingAvailability,
    pub chunks: &'a [SemanticChunk],
    pub entries: &'a [EmbeddingIndexEntry],
}

impl Database {
    pub fn ensure_ann_index_state(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
        status: AnnIndexStatus,
    ) -> Result<(), PersistenceError> {
        validate_descriptor(descriptor)?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO local_ann_index_state(
                workspace_id, provider_id, embedding_version, chunking_policy_version,
                index_format_version, dimensions, status, vector_count, next_ann_key
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, 0, 1)
             ON CONFLICT(workspace_id, provider_id, embedding_version) DO UPDATE SET
                status = excluded.status,
                chunking_policy_version = excluded.chunking_policy_version,
                dimensions = excluded.dimensions,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                workspace_id.to_string(),
                descriptor.provider_id,
                descriptor.version,
                CHUNKING_POLICY_VERSION,
                to_sql_integer(descriptor.dimensions)?,
                status.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn ann_index_status(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
    ) -> Result<Option<String>, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT status FROM local_ann_index_state
                 WHERE workspace_id = ?1 AND provider_id = ?2 AND embedding_version = ?3",
                params![
                    workspace_id.to_string(),
                    descriptor.provider_id,
                    descriptor.version
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    pub fn replace_file_chunks_and_embeddings(
        &self,
        replacement: FileChunkReplacement<'_>,
    ) -> Result<FileChunkReplaceResult, PersistenceError> {
        let FileChunkReplacement {
            workspace_id,
            file_id,
            file_version_id,
            semantic_analysis_id,
            descriptor,
            availability,
            chunks,
            entries,
        } = replacement;
        validate_descriptor(descriptor)?;
        if availability == EmbeddingAvailability::Unavailable
            || descriptor.dimensions == 0
            || entries.is_empty()
            || chunks.len() != entries.len()
            || entries.len() > 32
            || entries.iter().any(|entry| {
                entry.vector.len() != descriptor.dimensions
                    || !matches!(
                        entry.source_kind.as_str(),
                        "semantic_summary" | "text_chunk"
                    )
                    || entry.source_id.is_empty()
                    || entry.source_id.chars().count() > 64
            })
        {
            return Err(PersistenceError::InvalidSemanticOutput);
        }

        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        upsert_provider(&transaction, descriptor, availability)?;

        let valid_source: i64 = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM local_search_documents d
                JOIN semantic_analyses sa
                  ON sa.id = ?4
                 AND sa.file_id = d.file_id
                 AND sa.file_version_id = d.file_version_id
                 AND sa.is_current = 1
                WHERE d.workspace_id = ?1
                  AND d.file_id = ?2
                  AND d.file_version_id = ?3
             )",
            params![
                workspace_id.to_string(),
                file_id,
                file_version_id,
                semantic_analysis_id
            ],
            |row| row.get(0),
        )?;
        if valid_source != 1 {
            return Err(PersistenceError::NotFound);
        }

        // Collect previous ANN keys for removal by the ANN sync caller.
        let mut old_keys = Vec::<u64>::new();
        {
            let mut stmt = transaction.prepare(
                "SELECT ann_key FROM local_semantic_chunks
                 WHERE file_id = ?1 AND provider_id = ?2
                   AND status IN ('active', 'partial')",
            )?;
            let rows = stmt.query_map(params![file_id, descriptor.provider_id], |row| {
                row.get::<_, i64>(0)
            })?;
            for row in rows {
                old_keys.push(from_sql_u64(row?)?);
            }
        }

        transaction.execute(
            "DELETE FROM local_search_embeddings
             WHERE file_id = ?1 AND provider_id = ?2",
            params![file_id, descriptor.provider_id],
        )?;
        // Physical delete avoids UNIQUE(source_id) conflicts with tombstones.
        // ANN stale prevention is handled by returned removed_keys.
        transaction.execute(
            "DELETE FROM local_semantic_chunks
             WHERE file_id = ?1 AND provider_id = ?2",
            params![file_id, descriptor.provider_id],
        )?;
        transaction.execute(
            "DELETE FROM local_search_embedding_state
             WHERE file_id = ?1 AND provider_id = ?2",
            params![file_id, descriptor.provider_id],
        )?;

        ensure_ann_state_row(&transaction, workspace_id, descriptor)?;
        let mut next_key: i64 = transaction.query_row(
            "SELECT next_ann_key FROM local_ann_index_state
             WHERE workspace_id = ?1 AND provider_id = ?2 AND embedding_version = ?3",
            params![
                workspace_id.to_string(),
                descriptor.provider_id,
                descriptor.version
            ],
            |row| row.get(0),
        )?;

        let mut upserts = Vec::with_capacity(entries.len());
        let truncated = chunks.iter().any(|chunk| chunk.truncated_file);
        for (chunk, entry) in chunks.iter().zip(entries.iter()) {
            if chunk.source_id != entry.source_id {
                return Err(PersistenceError::InvalidSemanticOutput);
            }
            let ann_key = from_sql_u64(next_key)?;
            next_key = next_key.saturating_add(1);
            let preview = chunk.text.chars().take(512).collect::<String>();
            let status = if truncated { "partial" } else { "active" };
            transaction.execute(
                "INSERT INTO local_semantic_chunks(
                    id, workspace_id, file_id, file_version_id, semantic_analysis_id,
                    provider_id, embedding_version, chunking_policy_version, dimensions,
                    ann_key, source_id, source_kind, sequence_index, start_offset, end_offset,
                    page_number, sheet_or_slide, text_preview, text_hash, status, truncated_file
                 ) VALUES (
                    ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21
                 )",
                params![
                    Uuid::now_v7().to_string(),
                    workspace_id.to_string(),
                    file_id,
                    file_version_id,
                    semantic_analysis_id,
                    descriptor.provider_id,
                    descriptor.version,
                    CHUNKING_POLICY_VERSION,
                    to_sql_integer(descriptor.dimensions)?,
                    to_sql_u64(ann_key)?,
                    entry.source_id,
                    entry.source_kind,
                    to_sql_integer(usize::try_from(chunk.sequence_index).unwrap_or(0))?,
                    chunk.start_offset.map(to_sql_integer).transpose()?,
                    chunk.end_offset.map(to_sql_integer).transpose()?,
                    chunk.page_number.map(i64::from),
                    chunk.sheet_or_slide,
                    preview,
                    chunk.text_hash.as_slice(),
                    status,
                    i64::from(chunk.truncated_file),
                ],
            )?;
            transaction.execute(
                "INSERT INTO local_search_embeddings(
                    id, workspace_id, file_id, file_version_id, semantic_analysis_id,
                    provider_id, embedding_version, dimensions, source_id, source_kind,
                    source_start_offset, source_end_offset, vector, input_digest
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                 )",
                params![
                    Uuid::now_v7().to_string(),
                    workspace_id.to_string(),
                    file_id,
                    file_version_id,
                    semantic_analysis_id,
                    descriptor.provider_id,
                    descriptor.version,
                    to_sql_integer(descriptor.dimensions)?,
                    entry.source_id,
                    entry.source_kind,
                    entry.start_offset.map(to_sql_integer).transpose()?,
                    entry.end_offset.map(to_sql_integer).transpose()?,
                    entry.vector,
                    entry.input_digest.as_slice(),
                ],
            )?;
            upserts.push(AnnUpsertRecord {
                ann_key,
                vector: entry.vector.clone(),
                source_id: entry.source_id.clone(),
            });
        }

        transaction.execute(
            "UPDATE local_ann_index_state
             SET next_ann_key = ?4,
                 status = 'ready',
                 vector_count = (
                    SELECT COUNT(*) FROM local_semantic_chunks
                    WHERE workspace_id = ?1
                      AND provider_id = ?2
                      AND embedding_version = ?3
                      AND status IN ('active', 'partial')
                 ),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND provider_id = ?2 AND embedding_version = ?3",
            params![
                workspace_id.to_string(),
                descriptor.provider_id,
                descriptor.version,
                next_key
            ],
        )?;
        // Truncation is recorded on chunk rows (`status=partial` / truncated_file).
        // Embedding-state CHECK only allows indexed|unavailable|failed|stale.
        transaction.execute(
            "INSERT INTO local_search_embedding_state(
                file_id, provider_id, embedding_version, file_version_id,
                semantic_analysis_id, status, source_count, error_code
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'indexed', ?6, NULL)",
            params![
                file_id,
                descriptor.provider_id,
                descriptor.version,
                file_version_id,
                semantic_analysis_id,
                to_sql_integer(entries.len())?,
            ],
        )?;
        transaction.commit()?;
        Ok(FileChunkReplaceResult {
            upserts,
            removed_keys: old_keys,
        })
    }

    /// Lookup previously embedded vectors by text hash for incremental reuse.
    pub fn embeddings_for_text_hashes(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
        hashes: &[[u8; 32]],
    ) -> Result<std::collections::HashMap<[u8; 32], Vec<u8>>, PersistenceError> {
        if hashes.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let connection = self.lock()?;
        let mut found = std::collections::HashMap::new();
        for hash in hashes {
            let row = connection
                .query_row(
                    "SELECT emb.vector
                     FROM local_semantic_chunks chunk
                     JOIN local_search_embeddings emb
                       ON emb.file_id = chunk.file_id
                      AND emb.provider_id = chunk.provider_id
                      AND emb.embedding_version = chunk.embedding_version
                      AND emb.source_id = chunk.source_id
                     WHERE chunk.workspace_id = ?1
                       AND chunk.provider_id = ?2
                       AND chunk.embedding_version = ?3
                       AND chunk.text_hash = ?4
                       AND chunk.status IN ('active', 'partial')
                     LIMIT 1",
                    params![
                        workspace_id.to_string(),
                        descriptor.provider_id,
                        descriptor.version,
                        hash.as_slice()
                    ],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?;
            if let Some(vector) = row {
                found.insert(*hash, vector);
            }
        }
        Ok(found)
    }

    pub fn list_active_chunk_vectors_for_rebuild(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
    ) -> Result<Vec<AnnRebuildVector>, PersistenceError> {
        let connection = self.lock()?;
        let mut stmt = connection.prepare(
            "SELECT chunk.ann_key, emb.vector
             FROM local_semantic_chunks chunk
             JOIN local_search_embeddings emb
               ON emb.file_id = chunk.file_id
              AND emb.provider_id = chunk.provider_id
              AND emb.embedding_version = chunk.embedding_version
              AND emb.source_id = chunk.source_id
             JOIN files file
               ON file.id = chunk.file_id
              AND file.lifecycle_state = 'present'
             WHERE chunk.workspace_id = ?1
               AND chunk.provider_id = ?2
               AND chunk.embedding_version = ?3
               AND chunk.status IN ('active', 'partial')
             ORDER BY chunk.ann_key",
        )?;
        let rows = stmt.query_map(
            params![
                workspace_id.to_string(),
                descriptor.provider_id,
                descriptor.version
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )?;
        let mut output = Vec::new();
        for row in rows {
            let (key, vector) = row?;
            output.push(AnnRebuildVector {
                ann_key: from_sql_u64(key)?,
                vector,
            });
        }
        Ok(output)
    }

    pub fn tombstone_file_chunks(
        &self,
        file_id: &str,
        provider_id: &str,
    ) -> Result<Vec<u64>, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let mut keys = Vec::new();
        {
            let mut stmt = transaction.prepare(
                "SELECT ann_key FROM local_semantic_chunks
                 WHERE file_id = ?1 AND provider_id = ?2
                   AND status IN ('active', 'partial')",
            )?;
            let rows = stmt.query_map(params![file_id, provider_id], |row| row.get::<_, i64>(0))?;
            for row in rows {
                keys.push(from_sql_u64(row?)?);
            }
        }
        transaction.execute(
            "UPDATE local_semantic_chunks
             SET status = 'tombstone'
             WHERE file_id = ?1 AND provider_id = ?2
               AND status IN ('active', 'partial')",
            params![file_id, provider_id],
        )?;
        transaction.execute(
            "DELETE FROM local_search_embeddings
             WHERE file_id = ?1 AND provider_id = ?2",
            params![file_id, provider_id],
        )?;
        transaction.commit()?;
        Ok(keys)
    }

    pub fn map_ann_hits_to_files(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
        hits: &[(u64, f32)],
    ) -> Result<Vec<AnnFileCandidate>, PersistenceError> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.lock()?;
        let mut best = std::collections::HashMap::<String, AnnFileCandidate>::new();
        for (key, similarity) in hits {
            let row = connection
                .query_row(
                    "SELECT chunk.file_id, chunk.text_preview, chunk.source_id
                     FROM local_semantic_chunks chunk
                     JOIN files file
                       ON file.id = chunk.file_id
                      AND file.lifecycle_state = 'present'
                     JOIN semantic_analyses analysis
                       ON analysis.id = chunk.semantic_analysis_id
                      AND analysis.is_current = 1
                     WHERE chunk.workspace_id = ?1
                       AND chunk.provider_id = ?2
                       AND chunk.embedding_version = ?3
                       AND chunk.ann_key = ?4
                       AND chunk.status IN ('active', 'partial')",
                    params![
                        workspace_id.to_string(),
                        descriptor.provider_id,
                        descriptor.version,
                        to_sql_u64(*key)?
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((file_id, preview, source_id)) = row else {
                continue;
            };
            best.entry(file_id.clone())
                .and_modify(|current| {
                    if *similarity > current.similarity {
                        current.similarity = *similarity;
                        current.chunk_preview = preview.clone();
                        current.source_id = source_id.clone();
                    }
                })
                .or_insert(AnnFileCandidate {
                    file_id,
                    similarity: *similarity,
                    chunk_preview: preview,
                    source_id,
                });
        }
        let mut output = best.into_values().collect::<Vec<_>>();
        output.sort_by(|left, right| {
            right
                .similarity
                .partial_cmp(&left.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_id.cmp(&right.file_id))
        });
        Ok(output)
    }

    pub fn list_active_ann_keys_for_file(
        &self,
        file_id: &str,
        provider_id: &str,
    ) -> Result<Vec<u64>, PersistenceError> {
        let connection = self.lock()?;
        let mut stmt = connection.prepare(
            "SELECT ann_key FROM local_semantic_chunks
             WHERE file_id = ?1 AND provider_id = ?2 AND status IN ('active', 'partial')",
        )?;
        let rows = stmt.query_map(params![file_id, provider_id], |row| row.get::<_, i64>(0))?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(from_sql_u64(row?)?);
        }
        Ok(keys)
    }
}

fn validate_descriptor(descriptor: &EmbeddingProviderDescriptor) -> Result<(), PersistenceError> {
    if descriptor.provider_id.is_empty()
        || descriptor.version.is_empty()
        || descriptor.dimensions == 0
        || descriptor.dimensions > 4096
        || !descriptor.local_only
    {
        return Err(PersistenceError::InvalidSemanticOutput);
    }
    let _ = local_embedding_descriptor_is_safe;
    Ok(())
}

fn upsert_provider(
    connection: &Connection,
    descriptor: &EmbeddingProviderDescriptor,
    availability: EmbeddingAvailability,
) -> Result<(), PersistenceError> {
    connection.execute(
        "INSERT INTO local_embedding_models(
            provider_id, version, dimensions, availability, local_only,
            production_ready, requires_download, model_size_bytes, max_model_size_bytes
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, 0, ?6, ?7)
         ON CONFLICT(provider_id, version) DO UPDATE SET
            dimensions = excluded.dimensions,
            availability = excluded.availability,
            production_ready = excluded.production_ready,
            model_size_bytes = excluded.model_size_bytes,
            max_model_size_bytes = excluded.max_model_size_bytes",
        params![
            descriptor.provider_id,
            descriptor.version,
            to_sql_integer(descriptor.dimensions)?,
            availability.database_name(),
            i64::from(descriptor.production_ready),
            to_sql_u64(descriptor.model_size_bytes)?,
            to_sql_u64(descriptor.max_model_size_bytes)?,
        ],
    )?;
    Ok(())
}

fn ensure_ann_state_row(
    connection: &Connection,
    workspace_id: WorkspaceId,
    descriptor: &EmbeddingProviderDescriptor,
) -> Result<(), PersistenceError> {
    connection.execute(
        "INSERT INTO local_ann_index_state(
            workspace_id, provider_id, embedding_version, chunking_policy_version,
            index_format_version, dimensions, status, vector_count, next_ann_key
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, 'ready', 0, 1)
         ON CONFLICT(workspace_id, provider_id, embedding_version) DO NOTHING",
        params![
            workspace_id.to_string(),
            descriptor.provider_id,
            descriptor.version,
            CHUNKING_POLICY_VERSION,
            to_sql_integer(descriptor.dimensions)?,
        ],
    )?;
    Ok(())
}
