use crate::{
    archive::ZipMetadataExtractor,
    detection::detect_file_type,
    image::ImageExtractor,
    limits::ExtractionLimits,
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionPlan,
        ExtractionResult, ExtractionStatus, ExtractorType, OfficeKind, ReadMode,
    },
    ocr::{OcrProvider, PdfPageRenderer, PdftoppmRenderer, TesseractOcrProvider},
    office::OfficeExtractor,
    pdf::PdfExtractor,
    text::PlainTextExtractor,
    video::VideoMetadataExtractor,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_SCAN_BYTES_VIDEO: u64 = 4 * 1024 * 1024;

pub trait ContentExtractionEngine: Send + Sync {
    fn limits(&self) -> &ExtractionLimits;

    fn prepare(
        &self,
        extension: Option<&str>,
        declared_media_type: Option<&str>,
        prefix: &[u8],
        file_size: u64,
    ) -> ExtractionPlan;

    fn extract(
        &self,
        plan: &ExtractionPlan,
        bytes: &[u8],
        file_size: u64,
        is_cancelled: &dyn Fn() -> bool,
    ) -> ExtractionResult;
}

pub(crate) trait ContentExtractor: Send + Sync {
    fn can_handle(&self, kind: ContentKind) -> bool;

    fn extractor_type(&self, kind: ContentKind) -> ExtractorType;

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure>;
}

pub(crate) struct ExtractionInput<'a> {
    pub bytes: &'a [u8],
    pub input_truncated: bool,
    pub detection: &'a crate::model::FileTypeDetection,
}

pub(crate) struct ExtractionContext<'a> {
    pub limits: &'a ExtractionLimits,
    pub ocr_provider: Option<&'a dyn OcrProvider>,
    pub pdf_renderer: Option<&'a dyn PdfPageRenderer>,
    pub ocr_gate: &'a Mutex<()>,
    pub ocr_languages: &'a [String],
    pub is_cancelled: &'a dyn Fn() -> bool,
}

pub struct LocalExtractionEngine {
    limits: ExtractionLimits,
    extractors: Vec<Box<dyn ContentExtractor>>,
    ocr_provider: Option<Arc<dyn OcrProvider>>,
    pdf_renderer: Option<Arc<dyn PdfPageRenderer>>,
    ocr_gate: Mutex<()>,
    ocr_languages: Vec<String>,
}

impl std::fmt::Debug for LocalExtractionEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalExtractionEngine")
            .field("limits", &self.limits)
            .field("extractor_count", &self.extractors.len())
            .field(
                "ocr_provider",
                &self
                    .ocr_provider
                    .as_ref()
                    .map(|value| value.provider_name()),
            )
            .field(
                "pdf_renderer",
                &self
                    .pdf_renderer
                    .as_ref()
                    .map(|value| value.renderer_name()),
            )
            .field("network_enabled", &false)
            .finish()
    }
}

impl Default for LocalExtractionEngine {
    fn default() -> Self {
        Self::local_default()
    }
}

impl LocalExtractionEngine {
    #[must_use]
    pub fn local_default() -> Self {
        let limits = ExtractionLimits::default().normalized();
        let timeout = Duration::from_millis(limits.external_process_timeout_ms);
        let ocr_provider = TesseractOcrProvider::auto_detect(timeout)
            .map(|provider| Arc::new(provider) as Arc<dyn OcrProvider>);
        let pdf_renderer = PdftoppmRenderer::auto_detect(timeout)
            .map(|renderer| Arc::new(renderer) as Arc<dyn PdfPageRenderer>);
        Self::new(limits, ocr_provider, pdf_renderer)
    }

    #[must_use]
    pub fn new(
        limits: ExtractionLimits,
        ocr_provider: Option<Arc<dyn OcrProvider>>,
        pdf_renderer: Option<Arc<dyn PdfPageRenderer>>,
    ) -> Self {
        Self {
            limits: limits.normalized(),
            extractors: vec![
                Box::<PlainTextExtractor>::default(),
                Box::<PdfExtractor>::default(),
                Box::<OfficeExtractor>::default(),
                Box::<ImageExtractor>::default(),
                Box::<ZipMetadataExtractor>::default(),
                Box::<VideoMetadataExtractor>::default(),
            ],
            ocr_provider,
            pdf_renderer,
            ocr_gate: Mutex::new(()),
            ocr_languages: vec!["eng".to_owned()],
        }
    }

    #[must_use]
    pub fn with_ocr_languages(mut self, languages: Vec<String>) -> Self {
        let sanitized = languages
            .into_iter()
            .map(|language| language.trim().to_ascii_lowercase())
            .filter(|language| {
                !language.is_empty()
                    && language.len() <= 16
                    && language
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .take(8)
            .collect::<Vec<_>>();
        if !sanitized.is_empty() {
            self.ocr_languages = sanitized;
        }
        self
    }

    #[must_use]
    pub const fn network_enabled(&self) -> bool {
        false
    }

    fn plan_for_detection(
        &self,
        detection: crate::model::FileTypeDetection,
        file_size: u64,
    ) -> ExtractionPlan {
        if detection.mismatch {
            return preflight_plan(
                detection,
                ExtractionStatus::Failed,
                ErrorCategory::TypeMismatch,
                "file extension disagrees with detected content; parser was not invoked",
            );
        }
        let read_mode = match detection.content_kind {
            ContentKind::Text => ReadMode::Prefix(
                u64::try_from(self.limits.max_text_input_bytes).unwrap_or(u64::MAX),
            ),
            ContentKind::Pdf => ReadMode::Whole(self.limits.max_pdf_input_bytes),
            ContentKind::Office(_) => ReadMode::Whole(self.limits.max_office_input_bytes),
            ContentKind::Image => ReadMode::Whole(self.limits.max_image_input_bytes),
            ContentKind::Zip => ReadMode::Whole(self.limits.max_archive_input_bytes),
            ContentKind::Video => ReadMode::Prefix(MAX_SCAN_BYTES_VIDEO),
            ContentKind::LegacyOffice => {
                return preflight_plan(
                    detection,
                    ExtractionStatus::Unsupported,
                    ErrorCategory::Unsupported,
                    "legacy Office formats are not opened or automated",
                );
            }
            ContentKind::Executable => {
                return preflight_plan(
                    detection,
                    ExtractionStatus::Unsupported,
                    ErrorCategory::Unsupported,
                    "executable content is never launched or parsed",
                );
            }
            ContentKind::Unknown => {
                return preflight_plan(
                    detection,
                    ExtractionStatus::Unsupported,
                    ErrorCategory::Unsupported,
                    "no safe local extractor supports this content type",
                );
            }
        };
        if let ReadMode::Whole(limit) = read_mode
            && file_size > limit
        {
            return preflight_plan(
                detection,
                ExtractionStatus::Skipped,
                ErrorCategory::TooLarge,
                format!("file exceeds the configured {limit}-byte extractor limit"),
            );
        }
        ExtractionPlan {
            detection,
            read_mode,
            preflight_status: None,
            preflight_error: None,
            preflight_message: None,
        }
    }

    fn result_from_failure(
        &self,
        plan: &ExtractionPlan,
        extractor: Option<ExtractorType>,
        failure: ExtractionFailure,
        duration_ms: u64,
    ) -> ExtractionResult {
        ExtractionResult {
            status: failure.status,
            extractor,
            extractor_version: extractor.map(|_| env!("CARGO_PKG_VERSION").to_owned()),
            detected_content_type: plan.detection.detected_content_type.clone(),
            type_mismatch: plan.detection.mismatch,
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
            duration_ms,
            truncated: false,
            metadata: serde_json::json!({"network": false}),
            error_category: Some(failure.category),
            error_message: Some(failure.message),
        }
    }
}

impl ContentExtractionEngine for LocalExtractionEngine {
    fn limits(&self) -> &ExtractionLimits {
        &self.limits
    }

    fn prepare(
        &self,
        extension: Option<&str>,
        _declared_media_type: Option<&str>,
        prefix: &[u8],
        file_size: u64,
    ) -> ExtractionPlan {
        self.plan_for_detection(detect_file_type(extension, prefix), file_size)
    }

    fn extract(
        &self,
        plan: &ExtractionPlan,
        bytes: &[u8],
        file_size: u64,
        is_cancelled: &dyn Fn() -> bool,
    ) -> ExtractionResult {
        if plan.preflight_status.is_some() || !plan.requires_input() {
            return ExtractionResult::preflight(plan);
        }
        if is_cancelled() {
            return self.result_from_failure(
                plan,
                None,
                ExtractionFailure::skipped(
                    ErrorCategory::Cancelled,
                    "content extraction was cancelled",
                ),
                0,
            );
        }
        let input_truncated = u64::try_from(bytes.len()).unwrap_or(u64::MAX) < file_size;
        match plan.read_mode {
            ReadMode::None => return ExtractionResult::preflight(plan),
            ReadMode::Whole(limit)
                if input_truncated || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit =>
            {
                return self.result_from_failure(
                    plan,
                    None,
                    ExtractionFailure::skipped(
                        ErrorCategory::TooLarge,
                        "complete parser input was not available within its safety limit",
                    ),
                    0,
                );
            }
            ReadMode::Prefix(limit) if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit => {
                return self.result_from_failure(
                    plan,
                    None,
                    ExtractionFailure::skipped(
                        ErrorCategory::TooLarge,
                        "text input exceeded its safety limit",
                    ),
                    0,
                );
            }
            ReadMode::Prefix(_) | ReadMode::Whole(_) => {}
        }

        let Some(extractor) = self
            .extractors
            .iter()
            .find(|extractor| extractor.can_handle(plan.detection.content_kind))
        else {
            return self.result_from_failure(
                plan,
                None,
                ExtractionFailure {
                    status: ExtractionStatus::Unsupported,
                    category: ErrorCategory::Unsupported,
                    message: "no safe extractor is registered for this content type".to_owned(),
                },
                0,
            );
        };
        let extractor_type = extractor.extractor_type(plan.detection.content_kind);
        let input = ExtractionInput {
            bytes,
            input_truncated,
            detection: &plan.detection,
        };
        let context = ExtractionContext {
            limits: &self.limits,
            ocr_provider: self.ocr_provider.as_deref(),
            pdf_renderer: self.pdf_renderer.as_deref(),
            ocr_gate: &self.ocr_gate,
            ocr_languages: &self.ocr_languages,
            is_cancelled,
        };
        let started = Instant::now();
        let extraction = catch_unwind(AssertUnwindSafe(|| extractor.extract(&input, &context)));
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut payload = match extraction {
            Ok(Ok(payload)) => payload,
            Ok(Err(failure)) => {
                return self.result_from_failure(plan, Some(extractor_type), failure, duration_ms);
            }
            Err(_) => {
                return self.result_from_failure(
                    plan,
                    Some(extractor_type),
                    ExtractionFailure::failed(
                        ErrorCategory::ParserFailure,
                        "local parser stopped while handling hostile or malformed input",
                    ),
                    duration_ms,
                );
            }
        };

        if payload.text.chars().count() > self.limits.max_extracted_characters {
            payload.text = payload
                .text
                .chars()
                .take(self.limits.max_extracted_characters)
                .collect();
            payload.truncated = true;
            if payload.status == ExtractionStatus::Success {
                payload.status = ExtractionStatus::Partial;
                payload.error_category = Some(ErrorCategory::TooLarge);
                payload.error_message =
                    Some("extracted text was truncated at the configured limit".to_owned());
            }
        }
        if !payload.metadata.is_object() {
            payload.metadata = serde_json::json!({"value": payload.metadata, "network": false});
        } else {
            payload.metadata["network"] = serde_json::Value::Bool(false);
        }
        let character_count = u64::try_from(payload.text.chars().count()).unwrap_or(u64::MAX);
        ExtractionResult {
            status: payload.status,
            extractor: Some(payload.extractor),
            extractor_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            detected_content_type: plan.detection.detected_content_type.clone(),
            type_mismatch: plan.detection.mismatch,
            text: payload.text,
            character_count,
            page_count: payload.page_count,
            sheet_count: payload.sheet_count,
            slide_count: payload.slide_count,
            image_width: payload.image_width,
            image_height: payload.image_height,
            requires_ocr: payload.requires_ocr,
            ocr_used: payload.ocr_used,
            ocr_confidence: payload.ocr_confidence,
            language_hint: payload.language_hint,
            duration_ms,
            truncated: payload.truncated,
            metadata: payload.metadata,
            error_category: payload.error_category,
            error_message: payload.error_message,
        }
    }
}

fn preflight_plan(
    detection: crate::model::FileTypeDetection,
    status: ExtractionStatus,
    category: ErrorCategory,
    message: impl Into<String>,
) -> ExtractionPlan {
    ExtractionPlan {
        detection,
        read_mode: ReadMode::None,
        preflight_status: Some(status),
        preflight_error: Some(category),
        preflight_message: Some(message.into()),
    }
}

#[must_use]
pub(crate) const fn office_extractor_type(kind: OfficeKind) -> ExtractorType {
    match kind {
        OfficeKind::Docx => ExtractorType::Docx,
        OfficeKind::Xlsx => ExtractorType::Xlsx,
        OfficeKind::Pptx => ExtractorType::Pptx,
    }
}
