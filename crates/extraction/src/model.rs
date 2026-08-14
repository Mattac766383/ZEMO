use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionStatus {
    Pending,
    Running,
    Success,
    Partial,
    Unsupported,
    Skipped,
    Failed,
}

impl ExtractionStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "success",
            Self::Partial => "partial",
            Self::Unsupported => "unsupported",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Unreadable,
    EncryptedDocument,
    Unsupported,
    Corrupt,
    TooLarge,
    TooManyPages,
    TooManyCells,
    TooManyEntries,
    OcrFailed,
    OcrUnavailable,
    TypeMismatch,
    PermissionDenied,
    InvalidEncoding,
    ArchiveTraversal,
    PotentialArchiveBomb,
    SourceChanged,
    Cancelled,
    ParserFailure,
}

impl ErrorCategory {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::EncryptedDocument => "encrypted_document",
            Self::Unsupported => "unsupported",
            Self::Corrupt => "corrupt",
            Self::TooLarge => "too_large",
            Self::TooManyPages => "too_many_pages",
            Self::TooManyCells => "too_many_cells",
            Self::TooManyEntries => "too_many_entries",
            Self::OcrFailed => "ocr_failed",
            Self::OcrUnavailable => "ocr_unavailable",
            Self::TypeMismatch => "type_mismatch",
            Self::PermissionDenied => "permission_denied",
            Self::InvalidEncoding => "invalid_encoding",
            Self::ArchiveTraversal => "archive_traversal",
            Self::PotentialArchiveBomb => "potential_archive_bomb",
            Self::SourceChanged => "source_changed",
            Self::Cancelled => "cancelled",
            Self::ParserFailure => "parser_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractorType {
    PlainText,
    PdfText,
    PdfOcr,
    Docx,
    Xlsx,
    Pptx,
    ImageMetadata,
    ImageOcr,
    ZipMetadata,
    VideoMetadata,
}

impl ExtractorType {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::PlainText => "plain_text",
            Self::PdfText => "pdf_text",
            Self::PdfOcr => "pdf_ocr",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::ImageMetadata => "image_metadata",
            Self::ImageOcr => "image_ocr",
            Self::ZipMetadata => "zip_metadata",
            Self::VideoMetadata => "video_metadata",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfficeKind {
    Docx,
    Xlsx,
    Pptx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Pdf,
    Office(OfficeKind),
    Zip,
    Image,
    Video,
    LegacyOffice,
    Executable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTypeDetection {
    pub extension: Option<String>,
    pub content_kind: ContentKind,
    pub detected_content_type: String,
    pub magic_confirmed: bool,
    pub mismatch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "maxBytes", rename_all = "snake_case")]
pub enum ReadMode {
    None,
    Prefix(u64),
    Whole(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionPlan {
    pub detection: FileTypeDetection,
    pub read_mode: ReadMode,
    pub preflight_status: Option<ExtractionStatus>,
    pub preflight_error: Option<ErrorCategory>,
    pub preflight_message: Option<String>,
}

impl ExtractionPlan {
    #[must_use]
    pub fn requires_input(&self) -> bool {
        !matches!(self.read_mode, ReadMode::None)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionResult {
    pub status: ExtractionStatus,
    pub extractor: Option<ExtractorType>,
    pub extractor_version: Option<String>,
    pub detected_content_type: String,
    pub type_mismatch: bool,
    pub text: String,
    pub character_count: u64,
    pub page_count: Option<u32>,
    pub sheet_count: Option<u32>,
    pub slide_count: Option<u32>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub requires_ocr: bool,
    pub ocr_used: bool,
    pub ocr_confidence: Option<f32>,
    pub language_hint: Option<String>,
    pub duration_ms: u64,
    pub truncated: bool,
    pub metadata: serde_json::Value,
    pub error_category: Option<ErrorCategory>,
    pub error_message: Option<String>,
}

impl ExtractionResult {
    #[must_use]
    pub fn preflight(plan: &ExtractionPlan) -> Self {
        Self {
            status: plan
                .preflight_status
                .unwrap_or(ExtractionStatus::Unsupported),
            extractor: None,
            extractor_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
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
            duration_ms: 0,
            truncated: false,
            metadata: serde_json::json!({"network": false}),
            error_category: plan.preflight_error,
            error_message: plan.preflight_message.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractionPayload {
    pub status: ExtractionStatus,
    pub extractor: ExtractorType,
    pub text: String,
    pub page_count: Option<u32>,
    pub sheet_count: Option<u32>,
    pub slide_count: Option<u32>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub requires_ocr: bool,
    pub ocr_used: bool,
    pub ocr_confidence: Option<f32>,
    pub language_hint: Option<String>,
    pub truncated: bool,
    pub metadata: serde_json::Value,
    pub error_category: Option<ErrorCategory>,
    pub error_message: Option<String>,
}

impl ExtractionPayload {
    #[must_use]
    pub fn success(extractor: ExtractorType) -> Self {
        Self {
            status: ExtractionStatus::Success,
            extractor,
            text: String::new(),
            page_count: None,
            sheet_count: None,
            slide_count: None,
            image_width: None,
            image_height: None,
            requires_ocr: false,
            ocr_used: false,
            ocr_confidence: None,
            language_hint: None,
            truncated: false,
            metadata: serde_json::json!({"network": false}),
            error_category: None,
            error_message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractionFailure {
    pub status: ExtractionStatus,
    pub category: ErrorCategory,
    pub message: String,
}

impl ExtractionFailure {
    #[must_use]
    pub fn failed(category: ErrorCategory, message: impl Into<String>) -> Self {
        Self {
            status: ExtractionStatus::Failed,
            category,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn skipped(category: ErrorCategory, message: impl Into<String>) -> Self {
        Self {
            status: ExtractionStatus::Skipped,
            category,
            message: message.into(),
        }
    }
}
