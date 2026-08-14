use super::{
    Database, LocalEmbeddingIndexStats, PersistenceError, from_sql_u64, to_sql_integer, to_sql_u64,
};
use crate::ann_chunks::AnnFileCandidate;
use domain::WorkspaceId;
use rusqlite::{Connection, params, params_from_iter, types::Value};
use search::{
    ContextFilter, DocumentTypeFilter, EmbeddingAvailability, EmbeddingIndexEntry,
    EmbeddingProviderDescriptor, EmbeddingSearchStatus, HybridCandidate, HybridRankingPolicy,
    MatchSource, ModifiedFilter, QueryInterpretation, RankedAmountFact, RankedDateFact,
    RankedRelationshipFact, RankedSemanticFact, SearchFilters, SearchPage, SearchQuery,
    SearchResult, SearchSort, SearchTimings, SemanticStatusFilter, cosine_similarity_quantized,
    local_embedding_descriptor_is_safe, normalize_search_text, rank_hybrid_candidates,
    safe_fts_query,
};
use std::{cmp::Ordering, collections::HashMap, time::Instant};
use uuid::Uuid;

const MAX_STRUCTURED_CANDIDATES_PER_SIGNAL: usize = 2_500;
const MAX_FUSION_CANDIDATES: usize = 20_000;
const MAX_VECTOR_FILES: usize = 100_000;
const MAX_VECTOR_ENTRIES: usize = 400_000;
const MAX_VECTOR_CANDIDATES: usize = 2_000;
const SQLITE_ID_CHUNK: usize = 400;

#[derive(Debug, Clone, Default)]
struct CandidateSeed {
    lexical_score: Option<f64>,
    match_source: Option<MatchSource>,
    snippet: String,
    vector_similarity: Option<f32>,
}

#[derive(Debug, Clone)]
struct LoadedCandidate {
    hybrid: HybridCandidate,
    type_group: String,
}

#[derive(Debug, Clone, Copy)]
pub struct FileEmbeddingReplacement<'a> {
    pub workspace_id: WorkspaceId,
    pub file_id: &'a str,
    pub file_version_id: &'a str,
    pub semantic_analysis_id: &'a str,
    pub descriptor: &'a EmbeddingProviderDescriptor,
    pub availability: EmbeddingAvailability,
    pub entries: &'a [EmbeddingIndexEntry],
}

impl Database {
    pub fn register_embedding_provider(
        &self,
        descriptor: &EmbeddingProviderDescriptor,
        availability: EmbeddingAvailability,
    ) -> Result<(), PersistenceError> {
        validate_embedding_descriptor(descriptor, availability)?;
        let connection = self.lock()?;
        upsert_embedding_provider(&connection, descriptor, availability)
    }

    pub fn replace_file_embeddings(
        &self,
        replacement: FileEmbeddingReplacement<'_>,
    ) -> Result<(), PersistenceError> {
        let FileEmbeddingReplacement {
            workspace_id,
            file_id,
            file_version_id,
            semantic_analysis_id,
            descriptor,
            availability,
            entries,
        } = replacement;
        validate_embedding_descriptor(descriptor, availability)?;
        if availability == EmbeddingAvailability::Unavailable
            || descriptor.dimensions == 0
            || entries.is_empty()
            || entries.len() > 32
            || entries.iter().any(|entry| {
                entry.vector.len() != descriptor.dimensions
                    || !matches!(
                        entry.source_kind.as_str(),
                        "semantic_summary" | "text_chunk"
                    )
                    || entry.source_id.is_empty()
                    || entry.source_id.chars().count() > 64
                    || entry
                        .start_offset
                        .zip(entry.end_offset)
                        .is_some_and(|(start, end)| end < start)
            })
        {
            return Err(PersistenceError::InvalidSemanticOutput);
        }

        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        upsert_embedding_provider(&transaction, descriptor, availability)?;
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

        transaction.execute(
            "DELETE FROM local_search_embeddings
             WHERE file_id = ?1 AND provider_id = ?2",
            params![file_id, descriptor.provider_id],
        )?;
        transaction.execute(
            "DELETE FROM local_search_embedding_state
             WHERE file_id = ?1 AND provider_id = ?2",
            params![file_id, descriptor.provider_id],
        )?;
        for entry in entries {
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
        }
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
        Ok(())
    }

    pub fn local_embedding_count(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
    ) -> Result<u64, PersistenceError> {
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(DISTINCT embedding.file_id)
             FROM local_search_embeddings embedding
             JOIN semantic_analyses analysis
               ON analysis.id = embedding.semantic_analysis_id
              AND analysis.is_current = 1
             JOIN local_search_documents document
               ON document.file_id = embedding.file_id
              AND document.file_version_id = embedding.file_version_id
             JOIN files file
               ON file.id = embedding.file_id
              AND file.lifecycle_state = 'present'
             WHERE embedding.workspace_id = ?1
               AND embedding.provider_id = ?2
               AND embedding.embedding_version = ?3
               AND embedding.dimensions = ?4",
            params![
                workspace_id.to_string(),
                descriptor.provider_id,
                descriptor.version,
                to_sql_integer(descriptor.dimensions)?,
            ],
            |row| row.get(0),
        )?;
        from_sql_u64(count)
    }

    pub fn local_embedding_index_stats(
        &self,
        workspace_id: WorkspaceId,
        descriptor: &EmbeddingProviderDescriptor,
    ) -> Result<LocalEmbeddingIndexStats, PersistenceError> {
        let connection = self.lock()?;
        let row = connection.query_row(
            "SELECT
                COUNT(DISTINCT embedding.file_id), COUNT(*),
                COALESCE(SUM(length(embedding.vector)), 0)
             FROM local_search_embeddings embedding
             JOIN semantic_analyses analysis
               ON analysis.id = embedding.semantic_analysis_id
              AND analysis.is_current = 1
             JOIN local_search_documents document
               ON document.file_id = embedding.file_id
              AND document.file_version_id = embedding.file_version_id
             JOIN files file
               ON file.id = embedding.file_id
              AND file.lifecycle_state = 'present'
             WHERE embedding.workspace_id = ?1
               AND embedding.provider_id = ?2
               AND embedding.embedding_version = ?3
               AND embedding.dimensions = ?4",
            params![
                workspace_id.to_string(),
                descriptor.provider_id,
                descriptor.version,
                to_sql_integer(descriptor.dimensions)?,
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?;
        Ok(LocalEmbeddingIndexStats {
            file_count: from_sql_u64(row.0)?,
            vector_count: from_sql_u64(row.1)?,
            vector_bytes: from_sql_u64(row.2)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn hybrid_local_search(
        &self,
        workspace_id: WorkspaceId,
        query: SearchQuery,
        interpretation: QueryInterpretation,
        descriptor: &EmbeddingProviderDescriptor,
        availability: EmbeddingAvailability,
        query_vector: Option<&[f32]>,
    ) -> Result<SearchPage, PersistenceError> {
        self.hybrid_local_search_with_ann(
            workspace_id,
            query,
            interpretation,
            descriptor,
            availability,
            query_vector,
            None,
            SearchTimings::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn hybrid_local_search_with_ann(
        &self,
        workspace_id: WorkspaceId,
        query: SearchQuery,
        interpretation: QueryInterpretation,
        descriptor: &EmbeddingProviderDescriptor,
        availability: EmbeddingAvailability,
        query_vector: Option<&[f32]>,
        ann_candidates: Option<&[AnnFileCandidate]>,
        prior_timings: SearchTimings,
    ) -> Result<SearchPage, PersistenceError> {
        let started = Instant::now();
        let query = query.bounded();
        let connection = self.lock()?;
        let mut seeds = HashMap::<String, CandidateSeed>::new();

        let lexical_started = Instant::now();
        let lexical_query = safe_fts_query(&interpretation.lexical_text);
        if let Some(fts_query) = lexical_query.as_deref() {
            collect_lexical_candidates(&connection, workspace_id, fts_query, &mut seeds)?;
        }
        collect_structured_candidates(
            &connection,
            workspace_id,
            &query.filters,
            &interpretation,
            &mut seeds,
        )?;
        let has_interpreted_signal = has_interpreted_signal(&interpretation);
        let has_advanced_filter = has_advanced_filter(&query.filters);
        if query.text.trim().is_empty()
            && seeds.is_empty()
            && (has_advanced_filter || lexical_query.is_none())
        {
            collect_browse_candidates(&connection, workspace_id, &mut seeds)?;
        } else if !query.text.trim().is_empty()
            && lexical_query.is_none()
            && !has_interpreted_signal
        {
            return Ok(empty_hybrid_page(
                query,
                interpretation,
                descriptor,
                availability,
                started,
            ));
        }
        let lexical_and_structured_ms = elapsed_ms(lexical_started);

        let vector_started = Instant::now();
        let indexed_files = embedding_file_count(&connection, workspace_id, descriptor)?;
        if query.semantic_search && availability != EmbeddingAvailability::Unavailable {
            if let Some(candidates) = ann_candidates {
                for candidate in candidates {
                    let seed = seeds.entry(candidate.file_id.clone()).or_default();
                    seed.vector_similarity = Some(
                        seed.vector_similarity
                            .map_or(candidate.similarity, |current| {
                                current.max(candidate.similarity)
                            }),
                    );
                    if seed.snippet.is_empty() && !candidate.chunk_preview.is_empty() {
                        seed.snippet = candidate.chunk_preview.clone();
                    }
                }
            } else if let Some(vector) = query_vector
                && vector.len() == descriptor.dimensions
            {
                // Exact scan fallback for tests / ANN unavailable.
                collect_vector_candidates(
                    &connection,
                    workspace_id,
                    descriptor,
                    vector,
                    &mut seeds,
                )?;
            }
        }
        let vector_ms = elapsed_ms(vector_started);

        if seeds.len() > MAX_FUSION_CANDIDATES {
            truncate_seeds(&mut seeds);
        }

        let fusion_started = Instant::now();
        let modified_cutoff = modified_cutoff(&connection, query.filters.modified)?;
        let mut loaded = load_base_candidates(&connection, workspace_id, &seeds)?;
        load_semantic_facts(&connection, &mut loaded)?;
        load_identity_relationships(&connection, &mut loaded)?;
        load_explicit_rule_matches(&connection, &mut loaded)?;
        loaded.retain(|_, candidate| {
            candidate_matches_filters(candidate, &query.filters, modified_cutoff)
        });
        let candidates = loaded
            .into_values()
            .map(|candidate| candidate.hybrid)
            .collect::<Vec<_>>();
        let mut ranked =
            rank_hybrid_candidates(candidates, &interpretation, HybridRankingPolicy::default());
        apply_sort(&mut ranked, query.sort);

        let total = u64::try_from(ranked.len()).map_err(|_| PersistenceError::NumericOverflow)?;
        let start = query
            .page
            .checked_mul(query.page_size)
            .ok_or(PersistenceError::NumericOverflow)?;
        let results = ranked
            .into_iter()
            .skip(start)
            .take(query.page_size)
            .collect::<Vec<_>>();
        let returned_through = u64::try_from(start)
            .map_err(|_| PersistenceError::NumericOverflow)?
            .saturating_add(
                u64::try_from(results.len()).map_err(|_| PersistenceError::NumericOverflow)?,
            );
        let fusion_ms = elapsed_ms(fusion_started);
        Ok(SearchPage {
            query: query.text,
            page: query.page,
            page_size: query.page_size,
            total,
            has_more: returned_through < total,
            results,
            interpreted_query: interpretation.chips,
            embeddings: EmbeddingSearchStatus {
                availability,
                provider_id: descriptor.provider_id.clone(),
                version: descriptor.version.clone(),
                production_ready: descriptor.production_ready,
                indexed_files,
                ann_index_status: None,
            },
            timings: SearchTimings {
                total_ms: elapsed_ms(started),
                lexical_and_structured_ms,
                query_embed_ms: prior_timings.query_embed_ms,
                ann_ms: prior_timings.ann_ms,
                vector_ms,
                fusion_ms,
            },
        })
    }
}

fn validate_embedding_descriptor(
    descriptor: &EmbeddingProviderDescriptor,
    availability: EmbeddingAvailability,
) -> Result<(), PersistenceError> {
    if !local_embedding_descriptor_is_safe(descriptor, availability) {
        return Err(PersistenceError::InvalidSemanticOutput);
    }
    Ok(())
}

fn upsert_embedding_provider(
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
            local_only = 1,
            production_ready = excluded.production_ready,
            requires_download = 0,
            model_size_bytes = excluded.model_size_bytes,
            max_model_size_bytes = excluded.max_model_size_bytes,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
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

fn collect_lexical_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    fts_query: &str,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT
            d.file_id,
            bm25(local_search_fts, 8.0, 3.5, 1.0, 0.5),
            instr(highlight(local_search_fts, 0, char(31), char(30)), char(31)),
            instr(highlight(local_search_fts, 1, char(31), char(30)), char(31)),
            instr(highlight(local_search_fts, 3, char(31), char(30)), char(31)),
            snippet(local_search_fts, 2, '', '', ' … ', 28),
            d.filename, d.relative_path, d.metadata_text
         FROM local_search_fts
         CROSS JOIN local_search_documents d ON d.id = local_search_fts.rowid
         WHERE local_search_fts MATCH ?1
           AND d.workspace_id = ?2
         ORDER BY bm25(local_search_fts, 8.0, 3.5, 1.0, 0.5)
         LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![
            fts_query,
            workspace_id.to_string(),
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, i64>(4)? != 0,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        },
    )?;
    for row in rows {
        let (
            file_id,
            raw,
            filename_match,
            path_match,
            metadata_match,
            content,
            filename,
            path,
            metadata,
        ) = row?;
        let (source, snippet) = if filename_match {
            (MatchSource::Filename, filename)
        } else if path_match {
            (MatchSource::Path, path)
        } else if !content.is_empty() {
            (MatchSource::Content, content)
        } else if metadata_match {
            (MatchSource::Metadata, metadata)
        } else {
            (MatchSource::Content, String::new())
        };
        seeds.insert(
            file_id,
            CandidateSeed {
                // FTS5 BM25 is ordered ascending: a more negative value is a
                // stronger match. Preserve that direction while bounding the
                // score instead of taking an absolute-value reciprocal, which
                // inverted relevance and could drop the strongest matches.
                lexical_score: Some(1.0 / (1.0 + raw.exp())),
                match_source: Some(source),
                snippet: snippet.chars().take(500).collect(),
                vector_similarity: None,
            },
        );
    }
    Ok(())
}

fn collect_structured_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    filters: &SearchFilters,
    interpretation: &QueryInterpretation,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let document_type = interpretation
        .document_type
        .as_deref()
        .or_else(|| filters.document_type.database_name());
    if let Some(value) = document_type {
        collect_projection_candidates(
            connection,
            workspace_id,
            "semantic_document_type",
            value,
            seeds,
        )?;
    }
    let context = interpretation
        .context
        .as_deref()
        .or_else(|| filters.context.database_name());
    if let Some(value) = context {
        collect_projection_candidates(connection, workspace_id, "semantic_context", value, seeds)?;
    }

    for (kind, interpreted, explicit) in [
        (
            Some("file_supplier"),
            interpretation.supplier.as_deref(),
            filters.supplier.as_deref(),
        ),
        (
            Some("file_customer"),
            interpretation.customer.as_deref(),
            filters.customer.as_deref(),
        ),
        (
            Some("file_project"),
            interpretation.project.as_deref(),
            filters.project.as_deref(),
        ),
        (None, interpretation.party.as_deref(), None),
    ] {
        let Some(value) = interpreted.or(explicit) else {
            continue;
        };
        collect_relationship_candidates(connection, workspace_id, kind, value, seeds)?;
    }

    let amount_minimum = interpretation
        .amount
        .as_ref()
        .and_then(|amount| amount.minimum_minor)
        .or(filters.amount_minimum_minor);
    let amount_maximum = interpretation
        .amount
        .as_ref()
        .and_then(|amount| amount.maximum_minor)
        .or(filters.amount_maximum_minor);
    let currency = interpretation
        .amount
        .as_ref()
        .and_then(|amount| amount.currency.as_deref())
        .or(filters.currency.as_deref());
    if amount_minimum.is_some() || amount_maximum.is_some() {
        collect_amount_candidates(
            connection,
            workspace_id,
            amount_minimum,
            amount_maximum,
            currency,
            seeds,
        )?;
    }

    if let Some(date) = interpretation.date.as_ref() {
        collect_date_candidates(connection, workspace_id, &date.from, &date.to, seeds)?;
    } else if let Some(year) = filters.year {
        collect_date_candidates(
            connection,
            workspace_id,
            &format!("{year:04}-01-01"),
            &format!("{year:04}-12-31"),
            seeds,
        )?;
    }

    if let Some(status) = filters.semantic_status.database_name() {
        collect_projection_candidates(connection, workspace_id, "semantic_status", status, seeds)?;
    }
    Ok(())
}

fn collect_projection_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    column: &str,
    value: &str,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    if !matches!(
        column,
        "semantic_document_type" | "semantic_context" | "semantic_status"
    ) {
        return Err(PersistenceError::InvalidSemanticOutput);
    }
    let sql = format!(
        "SELECT file_id
         FROM local_search_documents
         WHERE workspace_id = ?1 AND {column} = ?2
         ORDER BY file_id
         LIMIT ?3"
    );
    collect_id_query(
        connection,
        &sql,
        params![
            workspace_id.to_string(),
            value,
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        seeds,
    )
}

fn collect_relationship_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    relationship_type: Option<&str>,
    value: &str,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let like = like_pattern(value);
    let mut statement = connection.prepare(
        "SELECT DISTINCT ir.source_file_id
         FROM identity_relationships ir
         JOIN resolved_identities identity ON identity.id = ir.target_identity_id
         WHERE ir.workspace_id = ?1
           AND ir.source_kind = 'file'
           AND ir.source_file_id IS NOT NULL
           AND ir.active = 1
           AND ir.status NOT IN ('user_rejected', 'conflicting')
           AND (?2 IS NULL OR ir.relationship_type = ?2)
           AND (
                lower(identity.normalized_display_name) LIKE ?3 ESCAPE '!'
                OR lower(identity.display_name) LIKE ?3 ESCAPE '!'
                OR EXISTS(
                    SELECT 1 FROM identity_aliases alias
                    WHERE alias.identity_id = identity.id
                      AND alias.active = 1
                      AND lower(alias.normalized_value) LIKE ?3 ESCAPE '!'
                )
           )
         ORDER BY ir.source_file_id
         LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            workspace_id.to_string(),
            relationship_type,
            like,
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        seeds.entry(row?).or_default();
    }

    let field_keys = match relationship_type {
        Some("file_supplier") => ["supplier_candidate", "issuer", ""],
        Some("file_customer") => ["customer_candidate", "", ""],
        Some("file_project") => ["project_reference_candidate", "", ""],
        _ => [
            "supplier_candidate",
            "customer_candidate",
            "project_reference_candidate",
        ],
    };
    let mut semantic = connection.prepare(
        "SELECT DISTINCT sa.file_id
         FROM semantic_analyses sa
         JOIN semantic_fields sf ON sf.analysis_id = sa.id AND sf.is_primary = 1
         LEFT JOIN semantic_user_corrections correction
           ON correction.file_id = sa.file_id
          AND correction.field_key = sf.field_key
          AND correction.active = 1
         WHERE sa.workspace_id = ?1
           AND sa.is_current = 1
           AND sf.field_key IN (?2, ?3, ?4)
           AND lower(COALESCE(correction.display_value, sf.display_value, ''))
               LIKE ?5 ESCAPE '!'
         ORDER BY sa.file_id
         LIMIT ?6",
    )?;
    let rows = semantic.query_map(
        params![
            workspace_id.to_string(),
            field_keys[0],
            field_keys[1],
            field_keys[2],
            like_pattern(value),
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        seeds.entry(row?).or_default();
    }
    Ok(())
}

fn collect_amount_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    minimum: Option<i64>,
    maximum: Option<i64>,
    currency: Option<&str>,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT sa.file_id
         FROM semantic_analyses sa
         JOIN semantic_fields sf ON sf.analysis_id = sa.id AND sf.is_primary = 1
         LEFT JOIN semantic_user_corrections correction
           ON correction.file_id = sa.file_id
          AND correction.field_key = sf.field_key
          AND correction.active = 1
         WHERE sa.workspace_id = ?1
           AND sa.is_current = 1
           AND sf.field_key IN ('subtotal', 'tax', 'total', 'amount')
           AND (
               ?2 IS NULL OR
               CAST(json_extract(
                   COALESCE(correction.normalized_value_json, sf.normalized_value_json),
                   '$.amount_minor'
               ) AS INTEGER) >= ?2
           )
           AND (
               ?3 IS NULL OR
               CAST(json_extract(
                   COALESCE(correction.normalized_value_json, sf.normalized_value_json),
                   '$.amount_minor'
               ) AS INTEGER) <= ?3
           )
           AND (
               ?4 IS NULL OR
               json_extract(
                   COALESCE(correction.normalized_value_json, sf.normalized_value_json),
                   '$.currency'
               ) = ?4
           )
         ORDER BY sa.file_id
         LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![
            workspace_id.to_string(),
            minimum,
            maximum,
            currency,
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        seeds.entry(row?).or_default();
    }

    // M5 keeps unlabeled currency matches as semantic entities rather than
    // inventing a "total" field. They are still valid bounded search facts.
    let mut entities = connection.prepare(
        "SELECT sa.file_id, entity.normalized_value
         FROM semantic_analyses sa
         JOIN semantic_entities entity ON entity.analysis_id = sa.id
         WHERE sa.workspace_id = ?1
           AND sa.is_current = 1
           AND entity.entity_type = 'amount'
         ORDER BY sa.file_id
         LIMIT ?2",
    )?;
    let rows = entities.query_map(
        params![
            workspace_id.to_string(),
            to_sql_integer(MAX_VECTOR_ENTRIES)?
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut entity_matches = 0_usize;
    for row in rows {
        let (file_id, normalized) = row?;
        let Some((amount_minor, actual_currency)) = money_from_entity(&normalized) else {
            continue;
        };
        if minimum.is_some_and(|value| amount_minor < value)
            || maximum.is_some_and(|value| amount_minor > value)
            || currency.is_some_and(|expected| {
                actual_currency
                    .as_deref()
                    .is_none_or(|actual| !actual.eq_ignore_ascii_case(expected))
            })
        {
            continue;
        }
        seeds.entry(file_id).or_default();
        entity_matches = entity_matches.saturating_add(1);
        if entity_matches >= MAX_STRUCTURED_CANDIDATES_PER_SIGNAL {
            break;
        }
    }
    Ok(())
}

fn collect_date_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    from: &str,
    to: &str,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT sa.file_id
         FROM semantic_analyses sa
         JOIN semantic_fields sf ON sf.analysis_id = sa.id AND sf.is_primary = 1
         LEFT JOIN semantic_user_corrections correction
           ON correction.file_id = sa.file_id
          AND correction.field_key = sf.field_key
          AND correction.active = 1
         WHERE sa.workspace_id = ?1
           AND sa.is_current = 1
           AND sf.field_key IN (
               'issue_date', 'due_date', 'expiration_date', 'document_date'
           )
           AND json_extract(
               COALESCE(correction.normalized_value_json, sf.normalized_value_json),
               '$.iso_date'
           ) BETWEEN ?2 AND ?3
         ORDER BY sa.file_id
         LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            workspace_id.to_string(),
            from,
            to,
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        seeds.entry(row?).or_default();
    }

    let mut entities = connection.prepare(
        "SELECT DISTINCT sa.file_id
         FROM semantic_analyses sa
         JOIN semantic_entities entity ON entity.analysis_id = sa.id
         WHERE sa.workspace_id = ?1
           AND sa.is_current = 1
           AND entity.entity_type = 'date'
           AND entity.normalized_value BETWEEN ?2 AND ?3
         ORDER BY sa.file_id
         LIMIT ?4",
    )?;
    let rows = entities.query_map(
        params![
            workspace_id.to_string(),
            from,
            to,
            to_sql_integer(MAX_STRUCTURED_CANDIDATES_PER_SIGNAL)?
        ],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        seeds.entry(row?).or_default();
    }
    Ok(())
}

fn collect_id_query<P>(
    connection: &Connection,
    sql: &str,
    params: P,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params, |row| row.get::<_, String>(0))?;
    for row in rows {
        seeds.entry(row?).or_default();
    }
    Ok(())
}

fn collect_browse_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT file_id
         FROM local_search_documents
         WHERE workspace_id = ?1
         ORDER BY filename COLLATE NOCASE, file_id
         LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![workspace_id.to_string(), to_sql_integer(MAX_VECTOR_FILES)?],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        seeds.entry(row?).or_default();
    }
    Ok(())
}

fn embedding_file_count(
    connection: &Connection,
    workspace_id: WorkspaceId,
    descriptor: &EmbeddingProviderDescriptor,
) -> Result<u64, PersistenceError> {
    if descriptor.dimensions == 0 {
        return Ok(0);
    }
    let count: i64 = connection.query_row(
        "SELECT COUNT(DISTINCT embedding.file_id)
         FROM local_search_embeddings embedding
         JOIN semantic_analyses analysis
           ON analysis.id = embedding.semantic_analysis_id
          AND analysis.is_current = 1
         JOIN local_search_documents document
           ON document.file_id = embedding.file_id
          AND document.file_version_id = embedding.file_version_id
         JOIN files file
           ON file.id = embedding.file_id
          AND file.lifecycle_state = 'present'
         WHERE embedding.workspace_id = ?1
           AND embedding.provider_id = ?2
           AND embedding.embedding_version = ?3
           AND embedding.dimensions = ?4",
        params![
            workspace_id.to_string(),
            descriptor.provider_id,
            descriptor.version,
            to_sql_integer(descriptor.dimensions)?,
        ],
        |row| row.get(0),
    )?;
    from_sql_u64(count)
}

fn collect_vector_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    descriptor: &EmbeddingProviderDescriptor,
    query_vector: &[f32],
    seeds: &mut HashMap<String, CandidateSeed>,
) -> Result<(), PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT embedding.file_id, embedding.vector
         FROM local_search_embeddings embedding
         JOIN semantic_analyses analysis
           ON analysis.id = embedding.semantic_analysis_id
          AND analysis.is_current = 1
         JOIN local_search_documents document
           ON document.file_id = embedding.file_id
          AND document.file_version_id = embedding.file_version_id
         JOIN files file
           ON file.id = embedding.file_id
          AND file.lifecycle_state = 'present'
         WHERE embedding.workspace_id = ?1
           AND embedding.provider_id = ?2
           AND embedding.embedding_version = ?3
           AND embedding.dimensions = ?4
         ORDER BY embedding.file_id, embedding.source_id
         LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![
            workspace_id.to_string(),
            descriptor.provider_id,
            descriptor.version,
            to_sql_integer(descriptor.dimensions)?,
            to_sql_integer(MAX_VECTOR_ENTRIES)?,
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )?;
    let mut best = HashMap::<String, f32>::new();
    for row in rows {
        let (file_id, vector) = row?;
        let similarity = cosine_similarity_quantized(query_vector, &vector);
        best.entry(file_id)
            .and_modify(|current| *current = current.max(similarity))
            .or_insert(similarity);
    }
    let mut ranked = best.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    for (file_id, similarity) in ranked.into_iter().take(MAX_VECTOR_CANDIDATES) {
        if similarity < 0.10 {
            break;
        }
        let seed = seeds.entry(file_id).or_default();
        seed.vector_similarity = Some(
            seed.vector_similarity
                .map_or(similarity, |current| current.max(similarity)),
        );
    }
    Ok(())
}

fn truncate_seeds(seeds: &mut HashMap<String, CandidateSeed>) {
    let mut ranked = seeds
        .drain()
        .map(|(file_id, seed)| {
            let priority = seed.lexical_score.unwrap_or(0.0)
                + f64::from(seed.vector_similarity.unwrap_or(0.0))
                + if seed.lexical_score.is_none() && seed.vector_similarity.is_none() {
                    0.5
                } else {
                    0.0
                };
            (file_id, seed, priority)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .2
            .partial_cmp(&left.2)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    seeds.extend(
        ranked
            .into_iter()
            .take(MAX_FUSION_CANDIDATES)
            .map(|(file_id, seed, _)| (file_id, seed)),
    );
}

fn load_base_candidates(
    connection: &Connection,
    workspace_id: WorkspaceId,
    seeds: &HashMap<String, CandidateSeed>,
) -> Result<HashMap<String, LoadedCandidate>, PersistenceError> {
    let ids = seeds.keys().cloned().collect::<Vec<_>>();
    let mut output = HashMap::new();
    for chunk in ids.chunks(SQLITE_ID_CHUNK) {
        let placeholders = (0..chunk.len())
            .map(|index| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT
                d.file_id, d.filename, d.relative_path, d.detected_type, d.extension,
                d.type_group, d.byte_size, d.modified_at_native, d.extraction_status,
                d.ocr_status,
                EXISTS(
                    SELECT 1 FROM duplicate_group_members duplicate
                    WHERE duplicate.file_version_id = d.file_version_id
                ),
                d.semantic_document_type, d.semantic_context, d.semantic_status,
                d.semantic_confidence
             FROM local_search_documents d
             WHERE d.workspace_id = ?1 AND d.file_id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(Value::Text(workspace_id.to_string()));
        values.extend(chunk.iter().cloned().map(Value::Text));
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, i64>(10)? != 0,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<f64>>(14)?,
            ))
        })?;
        for row in rows {
            let row = row?;
            let Some(seed) = seeds.get(&row.0) else {
                continue;
            };
            let document_type = row.11.map(|value| RankedSemanticFact {
                value,
                confidence: row.14.unwrap_or_default() as f32,
                user_confirmed: false,
            });
            let context = row.12.map(|value| RankedSemanticFact {
                value,
                confidence: row.14.unwrap_or_default() as f32,
                user_confirmed: false,
            });
            output.insert(
                row.0.clone(),
                LoadedCandidate {
                    type_group: row.5,
                    hybrid: HybridCandidate {
                        result: SearchResult {
                            file_id: row.0,
                            filename: row.1,
                            relative_path: row.2,
                            detected_type: row.3,
                            extension: row.4,
                            byte_size: from_sql_u64(row.6)?,
                            modified_at: row.7,
                            extraction_status: row.8,
                            ocr_status: row.9,
                            duplicate: row.10,
                            match_source: seed.match_source.unwrap_or(MatchSource::Structured),
                            relevance: 0.0,
                            snippet: seed.snippet.clone(),
                            why_matched: Vec::new(),
                        },
                        lexical_score: seed.lexical_score,
                        document_type,
                        context,
                        semantic_status: row.13,
                        semantic_confidence: row.14.map(|value| value as f32),
                        amounts: Vec::new(),
                        dates: Vec::new(),
                        relationships: Vec::new(),
                        vector_similarity: seed.vector_similarity,
                        explicit_rule_boost: 0.0,
                        explicit_rule_reasons: Vec::new(),
                    },
                },
            );
        }
    }
    Ok(output)
}

fn load_semantic_facts(
    connection: &Connection,
    candidates: &mut HashMap<String, LoadedCandidate>,
) -> Result<(), PersistenceError> {
    let ids = candidates.keys().cloned().collect::<Vec<_>>();
    for chunk in ids.chunks(SQLITE_ID_CHUNK) {
        let placeholders = (0..chunk.len())
            .map(|index| format!("?{}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT
                sa.file_id, sf.field_key,
                COALESCE(correction.display_value, sf.display_value),
                COALESCE(correction.normalized_value_json, sf.normalized_value_json),
                sf.confidence, sf.field_status, correction.correction_state
             FROM semantic_analyses sa
             JOIN semantic_fields sf ON sf.analysis_id = sa.id AND sf.is_primary = 1
             LEFT JOIN semantic_user_corrections correction
               ON correction.file_id = sa.file_id
              AND correction.field_key = sf.field_key
              AND correction.active = 1
             WHERE sa.is_current = 1 AND sa.file_id IN ({placeholders})
             ORDER BY sa.file_id, sf.field_key"
        );
        let values = chunk.iter().cloned().map(Value::Text).collect::<Vec<_>>();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)? as f32,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (
                file_id,
                field_key,
                display,
                normalized_json,
                confidence,
                field_status,
                correction,
            ) = row?;
            let Some(candidate) = candidates.get_mut(&file_id) else {
                continue;
            };
            let user_confirmed = correction.is_some();
            match field_key.as_str() {
                "document_type" => {
                    if let Some(value) = display {
                        candidate.hybrid.document_type = Some(RankedSemanticFact {
                            value,
                            confidence,
                            user_confirmed,
                        });
                    }
                }
                "context" => {
                    if let Some(value) = display {
                        candidate.hybrid.context = Some(RankedSemanticFact {
                            value,
                            confidence,
                            user_confirmed,
                        });
                    }
                }
                "subtotal" | "tax" | "total" | "amount" => {
                    if let Some((amount_minor, currency)) = money_from_json(&normalized_json) {
                        candidate.hybrid.amounts.push(RankedAmountFact {
                            amount_minor,
                            currency,
                            user_confirmed,
                        });
                    }
                }
                "issue_date" | "due_date" | "expiration_date" | "document_date" => {
                    if let Some(iso_date) = date_from_json(&normalized_json) {
                        candidate.hybrid.dates.push(RankedDateFact {
                            iso_date,
                            user_confirmed,
                        });
                    }
                }
                "supplier_candidate"
                | "issuer"
                | "customer_candidate"
                | "project_reference_candidate" => {
                    if let Some(value) = display {
                        let relationship_type = match field_key.as_str() {
                            "supplier_candidate" | "issuer" => "file_supplier",
                            "customer_candidate" => "file_customer",
                            "project_reference_candidate" => "file_project",
                            _ => "semantic_party",
                        };
                        candidate.hybrid.relationships.push(RankedRelationshipFact {
                            relationship_type: relationship_type.to_owned(),
                            display_name: value,
                            confidence,
                            user_confirmed,
                        });
                    }
                }
                _ => {}
            }
            if field_status == "confirmed" && correction.is_some() {
                candidate.hybrid.semantic_confidence = Some(1.0);
            }
        }

        let entity_sql = format!(
            "SELECT sa.file_id, entity.entity_type, entity.normalized_value
             FROM semantic_analyses sa
             JOIN semantic_entities entity ON entity.analysis_id = sa.id
             WHERE sa.is_current = 1
               AND entity.entity_type IN ('amount', 'date')
               AND sa.file_id IN ({placeholders})
             ORDER BY sa.file_id, entity.entity_type, entity.normalized_value"
        );
        let values = chunk.iter().cloned().map(Value::Text).collect::<Vec<_>>();
        let mut entity_statement = connection.prepare(&entity_sql)?;
        let rows = entity_statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (file_id, entity_type, normalized) = row?;
            let Some(candidate) = candidates.get_mut(&file_id) else {
                continue;
            };
            match entity_type.as_str() {
                "amount" => {
                    if let Some((amount_minor, currency)) = money_from_entity(&normalized)
                        && !candidate.hybrid.amounts.iter().any(|fact| {
                            fact.amount_minor == amount_minor && fact.currency == currency
                        })
                    {
                        candidate.hybrid.amounts.push(RankedAmountFact {
                            amount_minor,
                            currency,
                            user_confirmed: false,
                        });
                    }
                }
                "date"
                    if is_iso_date(&normalized)
                        && !candidate
                            .hybrid
                            .dates
                            .iter()
                            .any(|fact| fact.iso_date == normalized) =>
                {
                    candidate.hybrid.dates.push(RankedDateFact {
                        iso_date: normalized,
                        user_confirmed: false,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn load_identity_relationships(
    connection: &Connection,
    candidates: &mut HashMap<String, LoadedCandidate>,
) -> Result<(), PersistenceError> {
    let ids = candidates.keys().cloned().collect::<Vec<_>>();
    for chunk in ids.chunks(SQLITE_ID_CHUNK) {
        let placeholders = (0..chunk.len())
            .map(|index| format!("?{}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT
                relationship.source_file_id, relationship.relationship_type,
                identity.display_name, relationship.confidence, relationship.status,
                relationship.user_confirmation_state, identity.resolution_status,
                identity.user_locked
             FROM identity_relationships relationship
             JOIN resolved_identities identity
               ON identity.id = relationship.target_identity_id
             WHERE relationship.source_kind = 'file'
               AND relationship.source_file_id IN ({placeholders})
               AND relationship.active = 1
               AND relationship.status NOT IN ('user_rejected', 'conflicting')
               AND identity.lifecycle_status = 'active'
             ORDER BY relationship.source_file_id, relationship.relationship_type"
        );
        let values = chunk.iter().cloned().map(Value::Text).collect::<Vec<_>>();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)? as f32,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)? != 0,
            ))
        })?;
        for row in rows {
            let (
                file_id,
                relationship_type,
                display_name,
                confidence,
                status,
                confirmation,
                resolution,
                locked,
            ) = row?;
            let Some(candidate) = candidates.get_mut(&file_id) else {
                continue;
            };
            candidate.hybrid.relationships.push(RankedRelationshipFact {
                relationship_type,
                display_name,
                confidence,
                user_confirmed: status == "user_confirmed"
                    || confirmation.as_deref() == Some("confirmed")
                    || resolution == "user_confirmed"
                    || locked,
            });
        }
    }
    Ok(())
}

fn load_explicit_rule_matches(
    connection: &Connection,
    candidates: &mut HashMap<String, LoadedCandidate>,
) -> Result<(), PersistenceError> {
    let ids = candidates.keys().cloned().collect::<Vec<_>>();
    for chunk in ids.chunks(SQLITE_ID_CHUNK) {
        let placeholders = (0..chunk.len())
            .map(|index| format!("?{}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT matches.file_id, matches.boost, matches.explanation
             FROM local_rule_file_matches matches
             JOIN local_user_rules rule
               ON rule.id = matches.rule_id
              AND rule.workspace_id = matches.workspace_id
              AND rule.enabled = 1
             WHERE matches.file_id IN ({placeholders})
             ORDER BY matches.file_id, matches.boost DESC, matches.rule_id"
        );
        let values = chunk.iter().cloned().map(Value::Text).collect::<Vec<_>>();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (file_id, boost, explanation) = row?;
            let Some(candidate) = candidates.get_mut(&file_id) else {
                continue;
            };
            candidate.hybrid.explicit_rule_boost =
                (candidate.hybrid.explicit_rule_boost + boost).clamp(0.0, 0.25);
            if candidate.hybrid.explicit_rule_reasons.len() < 3
                && !candidate
                    .hybrid
                    .explicit_rule_reasons
                    .contains(&explanation)
            {
                candidate.hybrid.explicit_rule_reasons.push(explanation);
            }
        }
    }
    Ok(())
}

fn candidate_matches_filters(
    candidate: &LoadedCandidate,
    filters: &SearchFilters,
    modified_cutoff: Option<i128>,
) -> bool {
    if filters
        .file_type
        .database_name()
        .is_some_and(|expected| candidate.type_group != expected)
        || filters.extraction.database_name().is_some_and(|expected| {
            candidate.hybrid.result.extraction_status.as_deref() != Some(expected)
        })
        || filters
            .ocr
            .database_name()
            .is_some_and(|expected| candidate.hybrid.result.ocr_status.as_deref() != Some(expected))
        || filters
            .minimum_size
            .is_some_and(|minimum| candidate.hybrid.result.byte_size < minimum)
        || filters
            .maximum_size
            .is_some_and(|maximum| candidate.hybrid.result.byte_size > maximum)
        || modified_cutoff.is_some_and(|cutoff| {
            candidate
                .hybrid
                .result
                .modified_at
                .as_deref()
                .and_then(|value| value.parse::<i128>().ok())
                .is_none_or(|value| value < cutoff)
        })
        || filters
            .document_type
            .database_name()
            .is_some_and(|expected| {
                candidate
                    .hybrid
                    .document_type
                    .as_ref()
                    .is_none_or(|actual| actual.value != expected)
            })
        || filters.context.database_name().is_some_and(|expected| {
            candidate
                .hybrid
                .context
                .as_ref()
                .is_none_or(|actual| actual.value != expected)
        })
        || filters
            .semantic_status
            .database_name()
            .is_some_and(|expected| candidate.hybrid.semantic_status.as_deref() != Some(expected))
        || filters.minimum_confidence_percent.is_some_and(|minimum| {
            candidate
                .hybrid
                .semantic_confidence
                .is_none_or(|actual| actual * 100.0 < f32::from(minimum))
        })
        || !year_filter_matches(&candidate.hybrid.dates, filters.year)
        || !relationship_filter_matches(
            &candidate.hybrid.relationships,
            "file_supplier",
            filters.supplier.as_deref(),
        )
        || !relationship_filter_matches(
            &candidate.hybrid.relationships,
            "file_customer",
            filters.customer.as_deref(),
        )
        || !relationship_filter_matches(
            &candidate.hybrid.relationships,
            "file_project",
            filters.project.as_deref(),
        )
        || !amount_filter_matches(&candidate.hybrid.amounts, filters)
    {
        return false;
    }
    true
}

fn relationship_filter_matches(
    relationships: &[RankedRelationshipFact],
    kind: &str,
    expected: Option<&str>,
) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let expected = normalize_search_text(expected);
    let same_kind = relationships
        .iter()
        .filter(|relationship| relationship.relationship_type == kind)
        .collect::<Vec<_>>();
    let has_confirmed = same_kind
        .iter()
        .any(|relationship| relationship.user_confirmed);
    same_kind.into_iter().any(|relationship| {
        if has_confirmed && !relationship.user_confirmed {
            return false;
        }
        let actual = normalize_search_text(&relationship.display_name);
        actual == expected || actual.contains(&expected) || expected.contains(&actual)
    })
}

fn amount_filter_matches(amounts: &[RankedAmountFact], filters: &SearchFilters) -> bool {
    if filters.amount_minimum_minor.is_none()
        && filters.amount_maximum_minor.is_none()
        && filters.currency.is_none()
    {
        return true;
    }
    let has_confirmed = amounts.iter().any(|amount| amount.user_confirmed);
    amounts
        .iter()
        .filter(|amount| !has_confirmed || amount.user_confirmed)
        .any(|amount| {
            !filters
                .amount_minimum_minor
                .is_some_and(|minimum| amount.amount_minor < minimum)
                && !filters
                    .amount_maximum_minor
                    .is_some_and(|maximum| amount.amount_minor > maximum)
                && !filters.currency.as_ref().is_some_and(|currency| {
                    amount
                        .currency
                        .as_ref()
                        .is_none_or(|actual| actual != currency)
                })
        })
}

fn year_filter_matches(dates: &[RankedDateFact], year: Option<i32>) -> bool {
    let Some(year) = year else {
        return true;
    };
    let expected = format!("{year:04}");
    let has_confirmed = dates.iter().any(|date| date.user_confirmed);
    dates
        .iter()
        .filter(|date| !has_confirmed || date.user_confirmed)
        .any(|date| {
            date.iso_date
                .get(..4)
                .is_some_and(|actual| actual == expected)
        })
}

fn modified_cutoff(
    connection: &Connection,
    filter: ModifiedFilter,
) -> Result<Option<i128>, PersistenceError> {
    let modifier = match filter {
        ModifiedFilter::Any => return Ok(None),
        ModifiedFilter::Today => "start of day",
        ModifiedFilter::LastSevenDays => "-7 days",
        ModifiedFilter::LastThirtyDays => "-30 days",
        ModifiedFilter::ThisYear => "start of year",
    };
    let seconds: i64 = connection.query_row(
        "SELECT CAST(strftime('%s', 'now', ?1) AS INTEGER)",
        [modifier],
        |row| row.get(0),
    )?;
    Ok(Some(i128::from(seconds).saturating_mul(1_000_000_000)))
}

fn money_from_json(value: &str) -> Option<(i64, Option<String>)> {
    let value = serde_json::from_str::<serde_json::Value>(value).ok()?;
    let amount = value.get("amount_minor")?.as_i64()?;
    let scale = value
        .get("scale")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(2)
        .min(4) as u32;
    let normalized = if scale < 2 {
        amount.checked_mul(10_i64.checked_pow(2 - scale)?)?
    } else if scale > 2 {
        amount.checked_div(10_i64.checked_pow(scale - 2)?)?
    } else {
        amount
    };
    let currency = value
        .get("currency")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some((normalized, currency))
}

fn money_from_entity(value: &str) -> Option<(i64, Option<String>)> {
    let mut parts = value.split(':');
    let amount = parts.next()?.parse::<i64>().ok()?;
    let scale = parts.next()?.parse::<u32>().ok()?.min(4);
    let currency = parts
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if parts.next().is_some() {
        return None;
    }
    let normalized = if scale < 2 {
        amount.checked_mul(10_i64.checked_pow(2 - scale)?)?
    } else if scale > 2 {
        amount.checked_div(10_i64.checked_pow(scale - 2)?)?
    } else {
        amount
    };
    Some((normalized, currency))
}

fn is_iso_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn date_from_json(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("iso_date")?
        .as_str()
        .map(str::to_owned)
}

fn apply_sort(results: &mut [SearchResult], sort: SearchSort) {
    match sort {
        SearchSort::Relevance => {}
        SearchSort::Filename => results.sort_by(|left, right| {
            left.filename
                .to_lowercase()
                .cmp(&right.filename.to_lowercase())
                .then_with(|| left.file_id.cmp(&right.file_id))
        }),
        SearchSort::Size => results.sort_by(|left, right| {
            right
                .byte_size
                .cmp(&left.byte_size)
                .then_with(|| left.filename.cmp(&right.filename))
        }),
        SearchSort::Newest => results.sort_by(|left, right| {
            numeric_timestamp(right.modified_at.as_deref())
                .cmp(&numeric_timestamp(left.modified_at.as_deref()))
                .then_with(|| left.file_id.cmp(&right.file_id))
        }),
        SearchSort::Oldest => results.sort_by(|left, right| {
            numeric_timestamp(left.modified_at.as_deref())
                .cmp(&numeric_timestamp(right.modified_at.as_deref()))
                .then_with(|| left.file_id.cmp(&right.file_id))
        }),
    }
}

fn numeric_timestamp(value: Option<&str>) -> i128 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(i128::MAX)
}

fn has_interpreted_signal(interpretation: &QueryInterpretation) -> bool {
    !interpretation.lexical_text.is_empty()
        || interpretation.document_type.is_some()
        || interpretation.context.is_some()
        || interpretation.supplier.is_some()
        || interpretation.customer.is_some()
        || interpretation.project.is_some()
        || interpretation.party.is_some()
        || interpretation.amount.is_some()
        || interpretation.date.is_some()
}

fn has_advanced_filter(filters: &SearchFilters) -> bool {
    filters.document_type != DocumentTypeFilter::Any
        || filters.context != ContextFilter::Any
        || filters.customer.is_some()
        || filters.supplier.is_some()
        || filters.project.is_some()
        || filters.year.is_some()
        || filters.amount_minimum_minor.is_some()
        || filters.amount_maximum_minor.is_some()
        || filters.currency.is_some()
        || filters.semantic_status != SemanticStatusFilter::Any
        || filters.minimum_confidence_percent.is_some()
}

fn like_pattern(value: &str) -> String {
    let normalized = normalize_search_text(value);
    let escaped = normalized
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    format!("%{escaped}%")
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn empty_hybrid_page(
    query: SearchQuery,
    interpretation: QueryInterpretation,
    descriptor: &EmbeddingProviderDescriptor,
    availability: EmbeddingAvailability,
    started: Instant,
) -> SearchPage {
    SearchPage {
        query: query.text,
        page: query.page,
        page_size: query.page_size,
        total: 0,
        has_more: false,
        results: Vec::new(),
        interpreted_query: interpretation.chips,
        embeddings: EmbeddingSearchStatus {
            availability,
            provider_id: descriptor.provider_id.clone(),
            version: descriptor.version.clone(),
            production_ready: descriptor.production_ready,
            indexed_files: 0,
            ann_index_status: None,
        },
        timings: SearchTimings {
            total_ms: elapsed_ms(started),
            ..SearchTimings::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_relationship_controls_structured_filtering() {
        let relationships = vec![
            RankedRelationshipFact {
                relationship_type: "file_supplier".to_owned(),
                display_name: "Point P".to_owned(),
                confidence: 0.9,
                user_confirmed: false,
            },
            RankedRelationshipFact {
                relationship_type: "file_supplier".to_owned(),
                display_name: "Confirmed Other".to_owned(),
                confidence: 1.0,
                user_confirmed: true,
            },
        ];

        assert!(!relationship_filter_matches(
            &relationships,
            "file_supplier",
            Some("Point P")
        ));
        assert!(relationship_filter_matches(
            &relationships,
            "file_supplier",
            Some("Confirmed Other")
        ));
    }

    #[test]
    fn confirmed_amount_controls_structured_filtering() {
        let amounts = vec![
            RankedAmountFact {
                amount_minor: 140_000,
                currency: Some("EUR".to_owned()),
                user_confirmed: false,
            },
            RankedAmountFact {
                amount_minor: 160_000,
                currency: Some("EUR".to_owned()),
                user_confirmed: true,
            },
        ];
        let mut filters = SearchFilters {
            amount_minimum_minor: Some(139_000),
            amount_maximum_minor: Some(141_000),
            currency: Some("EUR".to_owned()),
            ..SearchFilters::default()
        };

        assert!(!amount_filter_matches(&amounts, &filters));
        filters.amount_minimum_minor = Some(159_000);
        filters.amount_maximum_minor = Some(161_000);
        assert!(amount_filter_matches(&amounts, &filters));
    }

    #[test]
    fn confirmed_date_controls_year_filtering() {
        let dates = vec![
            RankedDateFact {
                iso_date: "2025-06-01".to_owned(),
                user_confirmed: false,
            },
            RankedDateFact {
                iso_date: "2026-06-01".to_owned(),
                user_confirmed: true,
            },
        ];

        assert!(!year_filter_matches(&dates, Some(2025)));
        assert!(year_filter_matches(&dates, Some(2026)));
    }
}
