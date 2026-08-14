use crate::{ApplicationError, ScannerApplicationService};
use domain::ScanId;
use knowledge::{
    ConfidencePolicy, SemanticAnalysis, SemanticFieldType, SemanticInput, SemanticStatus,
    normalize_user_correction,
};
use persistence::{
    FileChunkReplacement, SemanticAnalysisBatchRecord, SemanticAnalysisCandidate,
    SemanticCorrectionInput, SemanticCorrectionRecord,
};
use search::{
    ApproximateTokenCounter, ChunkingPolicy, EmbeddingAvailability, EmbeddingDocument,
    EmbeddingIndexEntry, EmbeddingOutput, chunk_embedding_document, dequantize_unit_vector,
    embedding_index_entries, local_embedding_descriptor_is_safe, quantize_unit_vector,
    semantic_chunks_to_embedding_inputs,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::sync_channel,
    },
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticAnalysisPhase {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticAnalysisProgress {
    pub batch_id: String,
    pub scan_id: ScanId,
    pub phase: SemanticAnalysisPhase,
    pub files_queued: u64,
    pub files_completed: u64,
    pub high_confidence: u64,
    pub needs_review: u64,
    pub unknown: u64,
    pub partial: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticCorrectionAction {
    Confirm,
    Correct,
}

const SEMANTIC_CANDIDATE_PAGE_SIZE: usize = 16;

impl ScannerApplicationService {
    pub fn analyze_scan_semantics(
        &self,
        scan_id: ScanId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(SemanticAnalysisProgress),
    ) -> Result<SemanticAnalysisBatchRecord, ApplicationError> {
        let batch = self.database.begin_semantic_batch(scan_id)?;
        let mut progress = semantic_progress_from_batch(&batch, SemanticAnalysisPhase::Running);
        on_progress(progress.clone());
        let limits = self.semantic_provider.limits();
        let mut offset = 0_usize;
        let mut fatal_error = None;

        while !is_cancelled() {
            let candidates = self.database.semantic_candidates(
                scan_id,
                SEMANTIC_CANDIDATE_PAGE_SIZE.min(limits.queue_capacity.max(1)),
                offset,
                limits.max_input_chars,
            )?;
            if candidates.is_empty() {
                break;
            }
            offset = offset.saturating_add(candidates.len());
            if let Err(error) = self.process_semantic_page(
                &batch.id,
                &candidates,
                is_cancelled,
                &mut progress,
                on_progress,
            ) {
                fatal_error = Some(error);
                break;
            }
        }

        let cancelled = is_cancelled();
        let status = if cancelled {
            "cancelled"
        } else if fatal_error.is_some() {
            "failed"
        } else {
            "completed"
        };
        let final_batch = self.database.finish_semantic_batch(
            &batch.id,
            status,
            fatal_error
                .as_ref()
                .map(|_| "local semantic pipeline stopped unexpectedly"),
        )?;
        if !cancelled && fatal_error.is_none() {
            self.resolve_after_semantic_batch(final_batch.workspace_id, is_cancelled)?;
            self.refresh_rule_matches_if_available(final_batch.workspace_id)?;
        }
        let phase = if cancelled {
            SemanticAnalysisPhase::Cancelled
        } else if fatal_error.is_some() {
            SemanticAnalysisPhase::Failed
        } else {
            SemanticAnalysisPhase::Completed
        };
        on_progress(semantic_progress_from_batch(&final_batch, phase));
        if let Some(error) = fatal_error {
            return Err(error);
        }
        Ok(final_batch)
    }

    pub fn store_semantic_correction(
        &self,
        file_id: &str,
        field_key: &str,
        action: SemanticCorrectionAction,
        value: Option<&str>,
    ) -> Result<SemanticCorrectionRecord, ApplicationError> {
        file_id
            .parse::<domain::FileId>()
            .map_err(|_| ApplicationError::NotFound)?;
        let field_type = semantic_field_type(field_key).ok_or(ApplicationError::NotFound)?;
        let input = match action {
            SemanticCorrectionAction::Confirm => SemanticCorrectionInput {
                field_key: field_type.database_name().to_owned(),
                correction_state: "user_confirmed".to_owned(),
                value_kind: "text".to_owned(),
                display_value: "confirmed".to_owned(),
                normalized_value_json: "null".to_owned(),
            },
            SemanticCorrectionAction::Correct => {
                let value = value.ok_or(ApplicationError::InvalidSemanticCorrection)?;
                let normalized = normalize_user_correction(field_type, value, Some("fr-FR"))
                    .map_err(|_| ApplicationError::InvalidSemanticCorrection)?;
                let normalized_value_json = serde_json::to_string(&normalized)
                    .map_err(|_| ApplicationError::InvalidSemanticCorrection)?;
                SemanticCorrectionInput {
                    field_key: field_type.database_name().to_owned(),
                    correction_state: "user_corrected".to_owned(),
                    value_kind: normalized.kind_name().to_owned(),
                    display_value: normalized.display_value(),
                    normalized_value_json,
                }
            }
        };
        let correction = self
            .database
            .store_semantic_correction(file_id, &input)
            .map_err(ApplicationError::Persistence)?;
        self.resolve_after_semantic_correction(file_id)?;
        self.observe_semantic_correction(&correction)?;
        Ok(correction)
    }

    fn process_semantic_page(
        &self,
        batch_id: &str,
        candidates: &[SemanticAnalysisCandidate],
        is_cancelled: &(dyn Fn() -> bool + Sync),
        progress: &mut SemanticAnalysisProgress,
        on_progress: &mut dyn FnMut(SemanticAnalysisProgress),
    ) -> Result<(), ApplicationError> {
        let limits = self.semantic_provider.limits();
        let worker_count = limits.max_workers.min(candidates.len()).max(1);
        let queue_capacity = limits
            .queue_capacity
            .max(worker_count)
            .min(candidates.len());
        let next_index = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let (sender, receiver) = sync_channel(queue_capacity);
        let descriptor = self.semantic_provider.descriptor();
        let mut fatal_error = None;

        thread::scope(|scope| {
            for _ in 0..worker_count {
                let sender = sender.clone();
                let next_index = &next_index;
                let stop = &stop;
                let provider = self.semantic_provider.clone();
                scope.spawn(move || {
                    loop {
                        if stop.load(Ordering::Relaxed) || is_cancelled() {
                            break;
                        }
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        let Some(candidate) = candidates.get(index) else {
                            break;
                        };
                        let input = semantic_input(candidate);
                        let input_digest =
                            *blake3::hash(input.extracted_text.as_bytes()).as_bytes();
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            provider.analyze(&input, is_cancelled)
                        }))
                        .map_err(|_| "local semantic provider panicked".to_owned())
                        .and_then(|result| result.map_err(|error| error.to_string()));
                        if sender.send((index, input_digest, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);

            for (index, input_digest, result) in receiver {
                let Some(candidate) = candidates.get(index) else {
                    stop.store(true, Ordering::Relaxed);
                    fatal_error = Some(ApplicationError::InvalidSemanticResult);
                    continue;
                };
                match result {
                    Ok(analysis) if fatal_error.is_none() => {
                        if let Err(error) = self.persist_semantic_result(
                            batch_id,
                            candidate,
                            &analysis,
                            &input_digest,
                        ) {
                            stop.store(true, Ordering::Relaxed);
                            fatal_error = Some(error);
                            continue;
                        }
                        apply_semantic_result_to_progress(progress, &analysis);
                        on_progress(progress.clone());
                    }
                    Ok(_) => {}
                    Err(message) if is_cancelled() || message.contains("cancelled") => {
                        stop.store(true, Ordering::Relaxed);
                    }
                    Err(message) if fatal_error.is_none() => {
                        if let Err(error) = self.database.store_semantic_failure(
                            batch_id,
                            candidate,
                            "local-document-understanding",
                            env!("CARGO_PKG_VERSION"),
                            &descriptor.provider_id,
                            &descriptor.provider_version,
                            &input_digest,
                            &message,
                        ) {
                            stop.store(true, Ordering::Relaxed);
                            fatal_error = Some(ApplicationError::Persistence(error));
                            continue;
                        }
                        progress.files_completed = progress.files_completed.saturating_add(1);
                        progress.failed = progress.failed.saturating_add(1);
                        on_progress(progress.clone());
                    }
                    Err(_) => {}
                }
            }
        });
        fatal_error.map_or(Ok(()), Err)
    }

    fn persist_semantic_result(
        &self,
        batch_id: &str,
        candidate: &SemanticAnalysisCandidate,
        analysis: &SemanticAnalysis,
        input_digest: &[u8; 32],
    ) -> Result<(), ApplicationError> {
        analysis.validate(self.semantic_provider.limits())?;
        let analysis_id =
            self.database
                .begin_semantic_analysis(batch_id, candidate, analysis, input_digest)?;
        self.database
            .store_semantic_analysis(&analysis_id, candidate, analysis)?;
        self.index_semantic_result(&analysis_id, candidate, analysis)?;
        Ok(())
    }

    fn index_semantic_result(
        &self,
        analysis_id: &str,
        candidate: &SemanticAnalysisCandidate,
        analysis: &SemanticAnalysis,
    ) -> Result<(), ApplicationError> {
        let descriptor = self.embedding_provider.descriptor();
        let availability = self.embedding_provider.availability();
        if !local_embedding_descriptor_is_safe(&descriptor, availability) {
            return Ok(());
        }
        self.database
            .register_embedding_provider(&descriptor, availability)?;
        if availability == EmbeddingAvailability::Unavailable {
            return Ok(());
        }
        let document = EmbeddingDocument {
            filename: candidate.filename.clone(),
            semantic_fields: analysis
                .fields
                .iter()
                .filter_map(|field| {
                    field.value.as_ref().map(|value| {
                        (
                            field.field_type.database_name().to_owned(),
                            value.display_value(),
                        )
                    })
                })
                .collect(),
            identities: analysis
                .entities
                .iter()
                .map(|entity| {
                    (
                        entity.entity_type.database_name().to_owned(),
                        entity.original_value.clone(),
                    )
                })
                .collect(),
            extracted_text: candidate.extracted_text.clone(),
        };
        let policy = ChunkingPolicy::default();
        let counter = ApproximateTokenCounter::from_policy(&policy);
        let chunks = chunk_embedding_document(&document, &policy, &counter);
        if chunks.is_empty() {
            return Ok(());
        }
        let inputs = semantic_chunks_to_embedding_inputs(&chunks);
        let hashes = chunks
            .iter()
            .map(|chunk| chunk.text_hash)
            .collect::<Vec<_>>();
        let reused = self
            .database
            .embeddings_for_text_hashes(candidate.workspace_id, &descriptor, &hashes)
            .unwrap_or_default();

        let mut outputs = Vec::with_capacity(inputs.len());
        let mut pending_inputs = Vec::new();
        let mut pending_indexes = Vec::new();
        for (index, (input, hash)) in inputs.iter().zip(hashes.iter()).enumerate() {
            if let Some(vector) = reused.get(hash)
                && vector.len() == descriptor.dimensions
            {
                let values = dequantize_unit_vector(vector);
                outputs.push(EmbeddingOutput {
                    source_id: input.source_id.clone(),
                    values,
                    input_digest: *hash,
                });
            } else {
                pending_indexes.push(index);
                pending_inputs.push(input.clone());
                outputs.push(EmbeddingOutput {
                    source_id: input.source_id.clone(),
                    values: Vec::new(),
                    input_digest: *hash,
                });
            }
        }
        if !pending_inputs.is_empty() {
            let Ok(fresh) = self.embedding_provider.embed_batch(&pending_inputs) else {
                return Ok(());
            };
            for (slot, output) in pending_indexes.into_iter().zip(fresh) {
                if let Some(target) = outputs.get_mut(slot) {
                    *target = output;
                }
            }
        }
        if outputs.iter().any(|output| output.values.is_empty()) {
            return Ok(());
        }
        let Ok(entries) = embedding_index_entries(&inputs, &outputs) else {
            return Ok(());
        };
        // Ensure reused vectors remain quantized for storage.
        let entries = entries
            .into_iter()
            .enumerate()
            .map(|(index, mut entry)| {
                if entry.vector.is_empty()
                    && let Some(output) = outputs.get(index)
                {
                    entry.vector = quantize_unit_vector(&output.values);
                }
                entry
            })
            .collect::<Vec<EmbeddingIndexEntry>>();
        let replaced = self
            .database
            .replace_file_chunks_and_embeddings(FileChunkReplacement {
                workspace_id: candidate.workspace_id,
                file_id: &candidate.file_id,
                file_version_id: &candidate.file_version_id,
                semantic_analysis_id: analysis_id,
                descriptor: &descriptor,
                availability,
                chunks: &chunks,
                entries: &entries,
            })?;
        self.sync_ann_vectors(
            candidate.workspace_id,
            &replaced.upserts,
            &replaced.removed_keys,
        );
        Ok(())
    }
}

fn semantic_input(candidate: &SemanticAnalysisCandidate) -> SemanticInput {
    SemanticInput {
        file_version_id: candidate.file_version_id.clone(),
        filename: candidate.filename.clone(),
        extension: candidate.extension.clone(),
        detected_content_type: candidate.detected_content_type.clone(),
        extraction_status: candidate.extraction_status.clone(),
        extracted_text: candidate.extracted_text.clone(),
        extractor_type: candidate.extractor_type.clone(),
        extractor_version: candidate.extractor_version.clone(),
        page_count: candidate.page_count,
        sheet_count: candidate.sheet_count,
        slide_count: candidate.slide_count,
        ocr_used: candidate.ocr_used,
        ocr_confidence: candidate.ocr_confidence,
        extraction_truncated: candidate.extraction_truncated,
        language_hint: candidate.language_hint.clone(),
        locale_hint: Some("fr-FR".to_owned()),
    }
}

fn semantic_progress_from_batch(
    batch: &SemanticAnalysisBatchRecord,
    phase: SemanticAnalysisPhase,
) -> SemanticAnalysisProgress {
    SemanticAnalysisProgress {
        batch_id: batch.id.clone(),
        scan_id: batch.scan_id,
        phase,
        files_queued: batch.files_queued,
        files_completed: batch.files_completed,
        high_confidence: batch.high_confidence_count,
        needs_review: batch.needs_review_count,
        unknown: batch.unknown_count,
        partial: batch.partial_count,
        failed: batch.failed_count,
    }
}

fn apply_semantic_result_to_progress(
    progress: &mut SemanticAnalysisProgress,
    analysis: &SemanticAnalysis,
) {
    progress.files_completed = progress.files_completed.saturating_add(1);
    if !analysis.review_reasons.is_empty() {
        progress.needs_review = progress.needs_review.saturating_add(1);
    }
    match analysis.status {
        SemanticStatus::Unknown => progress.unknown = progress.unknown.saturating_add(1),
        SemanticStatus::Partial => progress.partial = progress.partial.saturating_add(1),
        _ => {}
    }
    let high_document_type = analysis
        .primary_field(SemanticFieldType::DocumentType)
        .is_some_and(|field| {
            field.value.is_some() && field.confidence.value() >= ConfidencePolicy::default().high
        });
    if high_document_type && analysis.review_reasons.is_empty() {
        progress.high_confidence = progress.high_confidence.saturating_add(1);
    }
}

fn semantic_field_type(value: &str) -> Option<SemanticFieldType> {
    match value {
        "document_type" => Some(SemanticFieldType::DocumentType),
        "context" => Some(SemanticFieldType::Context),
        "supplier_candidate" => Some(SemanticFieldType::SupplierCandidate),
        "customer_candidate" => Some(SemanticFieldType::CustomerCandidate),
        "issuer" => Some(SemanticFieldType::Issuer),
        "invoice_number" => Some(SemanticFieldType::InvoiceNumber),
        "quote_number" => Some(SemanticFieldType::QuoteNumber),
        "document_number" => Some(SemanticFieldType::DocumentNumber),
        "issue_date" => Some(SemanticFieldType::IssueDate),
        "due_date" => Some(SemanticFieldType::DueDate),
        "expiration_date" => Some(SemanticFieldType::ExpirationDate),
        "document_date" => Some(SemanticFieldType::DocumentDate),
        "subtotal" => Some(SemanticFieldType::Subtotal),
        "tax" => Some(SemanticFieldType::Tax),
        "total" => Some(SemanticFieldType::Total),
        "amount" => Some(SemanticFieldType::Amount),
        "currency" => Some(SemanticFieldType::Currency),
        "purchase_order_reference" => Some(SemanticFieldType::PurchaseOrderReference),
        "project_reference_candidate" => Some(SemanticFieldType::ProjectReferenceCandidate),
        "contract_parties" => Some(SemanticFieldType::ContractParties),
        "contract_title" => Some(SemanticFieldType::ContractTitle),
        "contract_type" => Some(SemanticFieldType::ContractType),
        "company_identifier" => Some(SemanticFieldType::CompanyIdentifier),
        _ => None,
    }
}
