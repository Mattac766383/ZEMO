use crate::{ApplicationError, ScannerApplicationService};
use domain::ScanId;
use extraction::{ErrorCategory, ExtractionPlan, ExtractionResult, ExtractionStatus, ReadMode};
use persistence::{
    ExtractionBatchRecord, ExtractionCandidate, ExtractionDetailRecord, ExtractionResultInput,
    PersistenceError,
};
use platform::PlatformError;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::sync_channel,
    },
    thread,
};

const CANDIDATE_PAGE_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentAnalysisPhase {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentAnalysisProgress {
    pub batch_id: String,
    pub scan_id: ScanId,
    pub phase: ContentAnalysisPhase,
    pub files_queued: u64,
    pub files_completed: u64,
    pub successful: u64,
    pub partial: u64,
    pub unsupported: u64,
    pub skipped: u64,
    pub failed: u64,
    pub ocr_processed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionRetryStatus {
    Succeeded,
    Partial,
    Failed,
    Unavailable,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionRetryOutcome {
    pub review_id: String,
    pub batch_id: Option<String>,
    pub file_id: Option<String>,
    pub status: ExtractionRetryStatus,
    pub extraction_status: Option<String>,
    pub message: String,
}

impl ScannerApplicationService {
    pub fn analyze_scan_content(
        &self,
        scan_id: ScanId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
        on_progress: &mut dyn FnMut(ContentAnalysisProgress),
    ) -> Result<ExtractionBatchRecord, ApplicationError> {
        let batch = self.database.begin_extraction_batch(scan_id)?;
        let mut progress = progress_from_batch(&batch, ContentAnalysisPhase::Running);
        on_progress(progress.clone());
        let mut offset = 0_usize;
        let mut fatal_error = None;

        while !is_cancelled() {
            let candidates =
                self.database
                    .extraction_candidates(scan_id, CANDIDATE_PAGE_SIZE, offset)?;
            if candidates.is_empty() {
                break;
            }
            offset = offset.saturating_add(candidates.len());
            if let Err(error) = self.process_candidate_page(
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
        let final_status = if cancelled {
            "cancelled"
        } else if fatal_error.is_some() {
            "failed"
        } else {
            "completed"
        };
        let final_batch = self.database.finish_extraction_batch(
            &batch.id,
            final_status,
            fatal_error
                .as_ref()
                .map(|_| "local extraction pipeline stopped unexpectedly"),
        )?;
        let final_phase = if cancelled {
            ContentAnalysisPhase::Cancelled
        } else if fatal_error.is_some() {
            ContentAnalysisPhase::Failed
        } else {
            ContentAnalysisPhase::Completed
        };
        on_progress(progress_from_batch(&final_batch, final_phase));
        if let Some(error) = fatal_error {
            return Err(error);
        }
        Ok(final_batch)
    }

    pub fn content_analysis_results(
        &self,
        batch_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExtractionDetailRecord>, ApplicationError> {
        self.database
            .extraction_results(batch_id, limit, offset)
            .map_err(ApplicationError::Persistence)
    }

    pub fn retry_review_extraction(
        &self,
        review_id: &str,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ExtractionRetryOutcome, ApplicationError> {
        crate::review::validate_review_id(review_id)?;
        let retry = match self.database.begin_review_retry(review_id) {
            Ok(retry) => retry,
            Err(PersistenceError::NotFound) => {
                return Ok(ExtractionRetryOutcome {
                    review_id: review_id.to_owned(),
                    batch_id: None,
                    file_id: None,
                    status: ExtractionRetryStatus::Unavailable,
                    extraction_status: None,
                    message: "Cette extraction ne peut pas être relancée dans son état actuel."
                        .to_owned(),
                });
            }
            Err(error) => return Err(ApplicationError::Persistence(error)),
        };
        if is_cancelled() {
            self.database
                .finish_extraction_batch(&retry.batch_id, "cancelled", None)?;
            return Ok(ExtractionRetryOutcome {
                review_id: retry.review_id,
                batch_id: Some(retry.batch_id),
                file_id: Some(retry.candidate.file_id),
                status: ExtractionRetryStatus::Cancelled,
                extraction_status: Some("skipped".to_owned()),
                message: "La nouvelle extraction a été annulée sans modifier le fichier."
                    .to_owned(),
            });
        }

        self.database
            .mark_extraction_running(&retry.batch_id, &retry.candidate.file_version_id)?;
        let result = match self.extract_candidate(&retry.candidate, is_cancelled) {
            Ok(result) => result,
            Err(error) => {
                let _ = self.database.finish_extraction_batch(
                    &retry.batch_id,
                    "failed",
                    Some("retry pipeline failed"),
                );
                return Err(error);
            }
        };
        self.database
            .store_extraction_result(&retry.batch_id, &retry.candidate, &result)?;
        let cancelled = is_cancelled() || result.error_category.as_deref() == Some("cancelled");
        self.database.finish_extraction_batch(
            &retry.batch_id,
            if cancelled { "cancelled" } else { "completed" },
            None,
        )?;

        let status = if cancelled {
            ExtractionRetryStatus::Cancelled
        } else if result.error_category.as_deref() == Some("ocr_unavailable") {
            ExtractionRetryStatus::Unavailable
        } else {
            match result.status.as_str() {
                "success" => ExtractionRetryStatus::Succeeded,
                "partial" => ExtractionRetryStatus::Partial,
                _ => ExtractionRetryStatus::Failed,
            }
        };
        let message = match status {
            ExtractionRetryStatus::Succeeded => "L’extraction locale a réussi.".to_owned(),
            ExtractionRetryStatus::Partial => "L’extraction locale reste partielle.".to_owned(),
            ExtractionRetryStatus::Unavailable => {
                "La dépendance locale requise reste indisponible.".to_owned()
            }
            ExtractionRetryStatus::Cancelled => {
                "La nouvelle extraction a été annulée sans modifier le fichier.".to_owned()
            }
            ExtractionRetryStatus::Failed => result
                .error_message
                .clone()
                .unwrap_or_else(|| "La nouvelle extraction locale a échoué.".to_owned()),
        };
        Ok(ExtractionRetryOutcome {
            review_id: retry.review_id,
            batch_id: Some(retry.batch_id),
            file_id: Some(retry.candidate.file_id),
            status,
            extraction_status: Some(result.status),
            message,
        })
    }

    fn process_candidate_page(
        &self,
        batch_id: &str,
        candidates: &[ExtractionCandidate],
        is_cancelled: &(dyn Fn() -> bool + Sync),
        progress: &mut ContentAnalysisProgress,
        on_progress: &mut dyn FnMut(ContentAnalysisProgress),
    ) -> Result<(), ApplicationError> {
        let worker_count = self
            .content_engine
            .limits()
            .max_workers
            .min(candidates.len())
            .max(1);
        let next_index = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let (sender, receiver) = sync_channel(worker_count);
        let mut fatal_error = None;

        thread::scope(|scope| {
            for _ in 0..worker_count {
                let sender = sender.clone();
                let next_index = &next_index;
                let stop = &stop;
                scope.spawn(move || {
                    loop {
                        if stop.load(Ordering::Relaxed) || is_cancelled() {
                            break;
                        }
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        let Some(candidate) = candidates.get(index) else {
                            break;
                        };
                        let result = self
                            .database
                            .mark_extraction_running(batch_id, &candidate.file_version_id)
                            .map_err(ApplicationError::Persistence)
                            .and_then(|()| self.extract_candidate(candidate, is_cancelled));
                        if result.is_err() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        if sender.send((index, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);

            for (index, result) in receiver {
                let Some(candidate) = candidates.get(index) else {
                    stop.store(true, Ordering::Relaxed);
                    fatal_error = Some(ApplicationError::ContentExtraction(
                        "worker returned an invalid candidate index".to_owned(),
                    ));
                    continue;
                };
                match result {
                    Ok(result) if fatal_error.is_none() => {
                        if let Err(error) = self
                            .database
                            .store_extraction_result(batch_id, candidate, &result)
                        {
                            stop.store(true, Ordering::Relaxed);
                            fatal_error = Some(ApplicationError::Persistence(error));
                            continue;
                        }
                        apply_result_to_progress(progress, &result);
                        on_progress(progress.clone());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        stop.store(true, Ordering::Relaxed);
                        if fatal_error.is_none() {
                            fatal_error = Some(error);
                        }
                    }
                }
            }
        });
        fatal_error.map_or(Ok(()), Err)
    }

    fn extract_candidate(
        &self,
        candidate: &ExtractionCandidate,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ExtractionResultInput, ApplicationError> {
        if !candidate.readable {
            return result_input(failed_result(
                candidate,
                ExtractionStatus::Failed,
                ErrorCategory::PermissionDenied,
                "scanner recorded this file as unreadable",
            ));
        }
        let root = Path::new(&candidate.root_path);
        let relative = Path::new(&candidate.relative_path);
        let detection_limit = self.content_engine.limits().detection_bytes;
        let prefix =
            match self
                .read_only_platform
                .read_prefix_scoped(root, relative, detection_limit)
            {
                Ok(prefix) => prefix,
                Err(error) => return result_input(platform_failure(candidate, &error)),
            };
        let plan = self.content_engine.prepare(
            candidate.extension.as_deref(),
            candidate.declared_media_type.as_deref(),
            &prefix,
            candidate.byte_size,
        );
        if !plan.requires_input() {
            return result_input(self.content_engine.extract(
                &plan,
                &[],
                candidate.byte_size,
                is_cancelled,
            ));
        }
        let bytes = match read_for_plan(self.read_only_platform.as_ref(), root, relative, &plan) {
            Ok(bytes) => bytes,
            Err(error) => return result_input(platform_failure(candidate, &error)),
        };
        if source_size_changed(candidate.byte_size, bytes.len(), plan.read_mode) {
            return result_input(failed_result(
                candidate,
                ExtractionStatus::Failed,
                ErrorCategory::SourceChanged,
                "file size changed after the inventory scan",
            ));
        }
        result_input(
            self.content_engine
                .extract(&plan, &bytes, candidate.byte_size, is_cancelled),
        )
    }
}

fn read_for_plan(
    platform: &dyn platform::ReadOnlyPlatform,
    root: &Path,
    relative: &Path,
    plan: &ExtractionPlan,
) -> Result<Vec<u8>, PlatformError> {
    match plan.read_mode {
        ReadMode::None => Ok(Vec::new()),
        ReadMode::Prefix(limit) => {
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            platform.read_prefix_scoped(root, relative, limit)
        }
        ReadMode::Whole(limit) => platform.read_bounded_scoped(root, relative, limit),
    }
}

fn source_size_changed(expected: u64, observed: usize, read_mode: ReadMode) -> bool {
    let observed = u64::try_from(observed).unwrap_or(u64::MAX);
    match read_mode {
        ReadMode::None => false,
        ReadMode::Whole(_) => observed != expected,
        ReadMode::Prefix(limit) if expected <= limit => observed != expected,
        ReadMode::Prefix(limit) => observed != limit,
    }
}

fn platform_failure(candidate: &ExtractionCandidate, error: &PlatformError) -> ExtractionResult {
    let (status, category, message) = match error {
        PlatformError::PermissionDenied => (
            ExtractionStatus::Failed,
            ErrorCategory::PermissionDenied,
            "permission was denied while reading the source",
        ),
        PlatformError::SourceMissing
        | PlatformError::OutsideRoot
        | PlatformError::ReparsePoint
        | PlatformError::Precondition(_) => (
            ExtractionStatus::Failed,
            ErrorCategory::SourceChanged,
            "source changed or left the registered scan scope",
        ),
        PlatformError::CloudPlaceholder => (
            ExtractionStatus::Skipped,
            ErrorCategory::Unreadable,
            "cloud placeholder was not hydrated",
        ),
        PlatformError::Io(io_error) if io_error.kind() == std::io::ErrorKind::PermissionDenied => (
            ExtractionStatus::Failed,
            ErrorCategory::PermissionDenied,
            "permission was denied while reading the source",
        ),
        PlatformError::Io(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => (
            ExtractionStatus::Failed,
            ErrorCategory::SourceChanged,
            "source disappeared after the inventory scan",
        ),
        _ => (
            ExtractionStatus::Failed,
            ErrorCategory::Unreadable,
            "source could not be read safely",
        ),
    };
    failed_result(candidate, status, category, message)
}

fn failed_result(
    candidate: &ExtractionCandidate,
    status: ExtractionStatus,
    category: ErrorCategory,
    message: &str,
) -> ExtractionResult {
    ExtractionResult {
        status,
        extractor: None,
        extractor_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        detected_content_type: candidate
            .declared_media_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_owned()),
        type_mismatch: false,
        text: String::new(),
        character_count: 0,
        page_count: None,
        sheet_count: None,
        slide_count: None,
        image_width: None,
        image_height: None,
        requires_ocr: false,
        ocr_used: false,
        ocr_confidence: None,
        language_hint: None,
        duration_ms: 0,
        truncated: false,
        metadata: serde_json::json!({"network": false}),
        error_category: Some(category),
        error_message: Some(message.to_owned()),
    }
}

fn result_input(result: ExtractionResult) -> Result<ExtractionResultInput, ApplicationError> {
    Ok(ExtractionResultInput {
        status: result.status.database_name().to_owned(),
        extractor_type: result
            .extractor
            .map(|extractor| extractor.database_name().to_owned()),
        extractor_version: result.extractor_version,
        detected_content_type: result.detected_content_type,
        type_mismatch: result.type_mismatch,
        extracted_text: result.text,
        character_count: result.character_count,
        page_count: result.page_count,
        sheet_count: result.sheet_count,
        slide_count: result.slide_count,
        image_width: result.image_width,
        image_height: result.image_height,
        requires_ocr: result.requires_ocr,
        ocr_used: result.ocr_used,
        ocr_confidence: result.ocr_confidence,
        language_hint: result.language_hint,
        extraction_duration_ms: result.duration_ms,
        truncated: result.truncated,
        structured_metadata_json: serde_json::to_string(&result.metadata)
            .map_err(|error| ApplicationError::ContentExtraction(error.to_string()))?,
        error_category: result
            .error_category
            .map(|category| category.database_name().to_owned()),
        error_message: result.error_message,
    })
}

fn progress_from_batch(
    batch: &ExtractionBatchRecord,
    phase: ContentAnalysisPhase,
) -> ContentAnalysisProgress {
    ContentAnalysisProgress {
        batch_id: batch.id.clone(),
        scan_id: batch.scan_id,
        phase,
        files_queued: batch.files_queued,
        files_completed: batch.files_completed,
        successful: batch.successful_count,
        partial: batch.partial_count,
        unsupported: batch.unsupported_count,
        skipped: batch.skipped_count,
        failed: batch.failed_count,
        ocr_processed: batch.ocr_processed_count,
    }
}

fn apply_result_to_progress(
    progress: &mut ContentAnalysisProgress,
    result: &ExtractionResultInput,
) {
    progress.files_completed = progress.files_completed.saturating_add(1);
    match result.status.as_str() {
        "success" => progress.successful = progress.successful.saturating_add(1),
        "partial" => progress.partial = progress.partial.saturating_add(1),
        "unsupported" => progress.unsupported = progress.unsupported.saturating_add(1),
        "skipped" => progress.skipped = progress.skipped.saturating_add(1),
        "failed" => progress.failed = progress.failed.saturating_add(1),
        _ => {}
    }
    if result.ocr_used {
        progress.ocr_processed = progress.ocr_processed.saturating_add(1);
    }
}
