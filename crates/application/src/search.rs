use crate::{ApplicationError, ScannerApplicationService};
use domain::{SemanticRuleField, WorkspaceId};
use persistence::{FileDetailRecord, SemanticEvidenceRecord, SemanticFieldRecord};
use search::{
    AnnIndexStatus, AnnSearchPolicy, EmbeddingAvailability, EmbeddingInput, LocalEmbeddingProvider,
    QueryClock, SearchPage, SearchQuery, SearchTimings, UnavailableEmbeddingProvider,
    dequantize_unit_vector, interpret_query, local_embedding_descriptor_is_safe,
};
use std::time::Instant;
use time::OffsetDateTime;

impl ScannerApplicationService {
    /// Extends FTS5 with local structured facts and optional local vectors/ANN.
    /// Provider or ANN failure is non-fatal: lexical and structured retrieval remain
    /// available and the renderer receives one bounded page, never the corpus.
    pub fn search_files(
        &self,
        workspace_id: WorkspaceId,
        query: SearchQuery,
    ) -> Result<SearchPage, ApplicationError> {
        self.database.workspace(workspace_id)?;
        let query = query.bounded();
        let now = OffsetDateTime::now_utc();
        let interpretation = interpret_query(
            &query.text,
            QueryClock::new(now.year(), u8::from(now.month()), now.day()),
            &query.disabled_intents,
        );
        let provider_descriptor = self.embedding_provider.descriptor();
        let provider_availability = self.embedding_provider.availability();
        let provider_is_safe =
            local_embedding_descriptor_is_safe(&provider_descriptor, provider_availability);
        let (descriptor, availability) = if provider_is_safe {
            (provider_descriptor, provider_availability)
        } else {
            (
                UnavailableEmbeddingProvider.descriptor(),
                EmbeddingAvailability::Unavailable,
            )
        };

        let mut prior_timings = SearchTimings::default();
        let embed_started = Instant::now();
        let query_vector = if query.semantic_search
            && provider_is_safe
            && availability != EmbeddingAvailability::Unavailable
            && !query.text.trim().is_empty()
        {
            self.embedding_provider
                .embed_batch(&[EmbeddingInput {
                    source_id: "query".to_owned(),
                    source_kind: "semantic_summary".to_owned(),
                    text: query.text.chars().take(512).collect(),
                    start_offset: None,
                    end_offset: None,
                }])
                .ok()
                .and_then(|mut output| output.pop())
                .map(|output| output.values)
        } else {
            None
        };
        prior_timings.query_embed_ms =
            u64::try_from(embed_started.elapsed().as_millis()).unwrap_or(u64::MAX);

        let mut ann_candidates = None;
        let mut use_exact_scan = true;
        let mut ann_status = None;
        if let Some(vector) = query_vector.as_deref()
            && let Some(ann) = self.ann_index_for(workspace_id)
        {
            let status = ann.status();
            ann_status = Some(status);
            match status {
                AnnIndexStatus::Ready | AnnIndexStatus::Degraded => {
                    let ann_started = Instant::now();
                    if let Ok(hits) = ann.search(vector, AnnSearchPolicy::default()) {
                        let mapped = hits
                            .into_iter()
                            .map(|hit| (hit.key, hit.similarity))
                            .collect::<Vec<_>>();
                        if let Ok(candidates) =
                            self.database
                                .map_ann_hits_to_files(workspace_id, &descriptor, &mapped)
                        {
                            prior_timings.ann_ms = u64::try_from(ann_started.elapsed().as_millis())
                                .unwrap_or(u64::MAX);
                            use_exact_scan = false;
                            ann_candidates = Some(candidates);
                        }
                    }
                }
                AnnIndexStatus::RebuildRequired | AnnIndexStatus::Failed => {
                    use_exact_scan = true;
                }
                _ => {}
            }
        }

        let mut page = self
            .database
            .hybrid_local_search_with_ann(
                workspace_id,
                query,
                interpretation,
                &descriptor,
                availability,
                if use_exact_scan {
                    query_vector.as_deref()
                } else {
                    None
                },
                ann_candidates.as_deref(),
                prior_timings,
            )
            .map_err(ApplicationError::Persistence)?;
        if let Some(status) = ann_status {
            page.embeddings.ann_index_status = Some(status.as_str().to_owned());
        }
        Ok(page)
    }

    pub fn file_detail(&self, file_id: &str) -> Result<FileDetailRecord, ApplicationError> {
        let parsed = file_id
            .parse::<domain::FileId>()
            .map_err(|_| ApplicationError::NotFound)?;
        let mut detail = self
            .database
            .file_detail(&parsed.to_string())
            .map_err(ApplicationError::Persistence)?;
        let workspace_id = self.database.file_workspace_id(parsed)?;
        if let Ok(source) = self
            .database
            .organization_source_for_file(workspace_id, parsed)
            && let Some(source_file) = source
                .files
                .into_iter()
                .find(|source| source.file_id == file_id)
        {
            let rules = self.database.rules(workspace_id)?;
            let input = crate::proposal::source_input(source_file, &rules)?;
            if let Some(analysis) = detail.semantic_analysis.as_mut() {
                let analyzer_version = analysis.analyzer_version.clone();
                for (field, (value, matched)) in input.rule_evaluation.semantic_overrides {
                    if let Some(existing) = analysis
                        .fields
                        .iter_mut()
                        .find(|existing| existing.field_key == field.database_name())
                    {
                        if existing.machine_display_value.is_none() {
                            existing.machine_display_value = existing.display_value.clone();
                        }
                        existing.display_value = Some(value.clone());
                        existing.normalized_value = serde_json::Value::String(value);
                        existing.confidence = 1.0;
                        existing.status = "confirmed".to_owned();
                        existing.value_source = "user_rule".to_owned();
                        existing.user_state = Some("explicit_rule".to_owned());
                    } else {
                        analysis.fields.push(SemanticFieldRecord {
                            field_id: format!("rule:{}:{}", matched.id, field.database_name()),
                            field_key: field.database_name().to_owned(),
                            value_kind: Some(rule_value_kind(field).to_owned()),
                            display_value: Some(value.clone()),
                            machine_display_value: None,
                            normalized_value: serde_json::Value::String(value),
                            confidence: 1.0,
                            status: "confirmed".to_owned(),
                            source_method: "explicit_user_rule".to_owned(),
                            analyzer_version: analyzer_version.clone(),
                            value_source: "user_rule".to_owned(),
                            user_state: Some("explicit_rule".to_owned()),
                            evidence: vec![SemanticEvidenceRecord {
                                evidence_type: "user_rule".to_owned(),
                                exact_text: matched.explanation.clone(),
                                start_offset: None,
                                end_offset: None,
                                page_number: None,
                                sheet_name: None,
                                slide_number: None,
                                source_label: "Local user rule".to_owned(),
                                explanation: format!(
                                    "Applied your explicit rule: {}",
                                    matched.name
                                ),
                                extraction_method: "explicit_user_rule".to_owned(),
                                analyzer_version: analyzer_version.clone(),
                            }],
                            candidates: Vec::new(),
                        });
                    }
                }
                analysis
                    .fields
                    .sort_by(|left, right| left.field_key.cmp(&right.field_key));
            }
        }
        Ok(detail)
    }

    /// Sync ANN vectors for a file after chunk embeddings are persisted.
    pub(crate) fn sync_ann_vectors(
        &self,
        workspace_id: WorkspaceId,
        upserts: &[persistence::AnnUpsertRecord],
        removed_keys: &[u64],
    ) {
        let Some(ann) = self.ann_index_for(workspace_id) else {
            return;
        };
        if ann.status() == AnnIndexStatus::RebuildRequired {
            return;
        }
        for key in removed_keys {
            let _ = ann.remove_key(*key);
        }
        for record in upserts {
            let values = dequantize_unit_vector(&record.vector);
            let _ = ann.upsert_vector(record.ann_key, &values);
        }
        let _ = ann.persist_snapshot();
        let _ = self.database.ensure_ann_index_state(
            workspace_id,
            &self.embedding_provider.descriptor(),
            ann.status(),
        );
    }

    pub fn ann_index_status_label(&self, workspace_id: WorkspaceId) -> String {
        self.ann_index_for(workspace_id)
            .map(|ann| ann.status().as_str().to_owned())
            .unwrap_or_else(|| AnnIndexStatus::NotAvailable.as_str().to_owned())
    }

    /// Marks every in-memory ANN index as rebuild-required (e.g. after model removal).
    pub fn mark_all_ann_rebuild_required(&self, reason: &str) -> Result<(), ApplicationError> {
        let Ok(guard) = self.ann_indexes.lock() else {
            return Ok(());
        };
        for ann in guard.values() {
            let _ = ann.mark_rebuild_required(reason);
        }
        Ok(())
    }

    pub fn rebuild_semantic_ann_index(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<AnnIndexStatus, ApplicationError> {
        self.database.workspace(workspace_id)?;
        let descriptor = self.embedding_provider.descriptor();
        let availability = self.embedding_provider.availability();
        if !local_embedding_descriptor_is_safe(&descriptor, availability)
            || availability == EmbeddingAvailability::Unavailable
        {
            return Ok(AnnIndexStatus::NotAvailable);
        }
        let Some(ann) = self.ann_index_for(workspace_id) else {
            return Ok(AnnIndexStatus::NotAvailable);
        };
        ann.clear()
            .map_err(|_| ApplicationError::InvalidSemanticResult)?;
        ann.begin_build()
            .map_err(|_| ApplicationError::InvalidSemanticResult)?;
        let _ = self.database.ensure_ann_index_state(
            workspace_id,
            &descriptor,
            AnnIndexStatus::Building,
        );

        let rows = self
            .database
            .list_active_chunk_vectors_for_rebuild(workspace_id, &descriptor)
            .map_err(ApplicationError::Persistence)?;
        for row in rows {
            if is_cancelled() {
                // Leave prior cleared state as rebuild_required; do not mark Ready.
                let _ = ann.mark_rebuild_required("rebuild cancelled");
                let _ = self.database.ensure_ann_index_state(
                    workspace_id,
                    &descriptor,
                    AnnIndexStatus::RebuildRequired,
                );
                return Ok(AnnIndexStatus::RebuildRequired);
            }
            let values = dequantize_unit_vector(&row.vector);
            ann.upsert_vector(row.ann_key, &values)
                .map_err(|_| ApplicationError::InvalidSemanticResult)?;
        }
        ann.persist_snapshot()
            .map_err(|_| ApplicationError::InvalidSemanticResult)?;
        let status = ann.status();
        let _ = self
            .database
            .ensure_ann_index_state(workspace_id, &descriptor, status);
        Ok(status)
    }
}

const fn rule_value_kind(field: SemanticRuleField) -> &'static str {
    match field {
        SemanticRuleField::DocumentType => "document_type",
        SemanticRuleField::Context => "context",
        SemanticRuleField::Supplier | SemanticRuleField::Customer | SemanticRuleField::Project => {
            "text"
        }
    }
}
