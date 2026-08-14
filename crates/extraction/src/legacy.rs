//! Compatibility protocol for the existing parser-worker binary.

use crate::{
    engine::{ContentExtractionEngine, LocalExtractionEngine},
    limits::ExtractionLimits,
    model::{ErrorCategory, ExtractionStatus, ExtractorType},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionRequest {
    pub request_id: String,
    pub media_type: Option<String>,
    pub extension: Option<String>,
    pub bytes: Vec<u8>,
    pub max_output_chars: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    NativeText,
    Parser,
    Ocr,
    Hybrid,
    MetadataOnly,
}

impl ExtractionMethod {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::NativeText => "native_text",
            Self::Parser | Self::MetadataOnly => "parser",
            Self::Ocr => "ocr",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedPage {
    pub page_number: u32,
    pub text: String,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedDocument {
    pub request_id: String,
    pub title: Option<String>,
    pub text: String,
    pub method: ExtractionMethod,
    pub language: Option<String>,
    pub pages: Vec<ExtractedPage>,
    pub metadata: serde_json::Value,
    pub warnings: Vec<ExtractionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionWarning {
    OutputTruncated,
    OcrUnavailable,
    UnsupportedFormat,
    ArchiveBudgetExceeded,
    PotentialArchiveBomb,
    MalformedContent,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("input exceeds the configured extraction budget")]
    InputTooLarge,
    #[error("format is unsupported")]
    Unsupported,
    #[error("content is malformed: {0}")]
    Malformed(String),
    #[error("OCR provider is unavailable: {0}")]
    OcrUnavailable(String),
    #[error("sandboxed parser worker failed: {0}")]
    Worker(String),
}

pub trait ExtractionEngine: Send + Sync {
    fn extract(&self, request: &ExtractionRequest) -> Result<ExtractedDocument, ExtractionError>;
}

#[derive(Debug, Default)]
pub struct DeterministicExtractor;

impl ExtractionEngine for DeterministicExtractor {
    fn extract(&self, request: &ExtractionRequest) -> Result<ExtractedDocument, ExtractionError> {
        let limits = ExtractionLimits {
            max_extracted_characters: request.max_output_chars.max(1),
            ..ExtractionLimits::default()
        };
        let engine = LocalExtractionEngine::new(limits, None, None);
        let prefix_len = request.bytes.len().min(engine.limits().detection_bytes);
        let file_size = u64::try_from(request.bytes.len()).unwrap_or(u64::MAX);
        let plan = engine.prepare(
            request.extension.as_deref(),
            request.media_type.as_deref(),
            &request.bytes[..prefix_len],
            file_size,
        );
        let result = engine.extract(&plan, &request.bytes, file_size, &|| false);
        match result.status {
            ExtractionStatus::Unsupported => return Err(ExtractionError::Unsupported),
            ExtractionStatus::Skipped => return Err(ExtractionError::InputTooLarge),
            ExtractionStatus::Failed => {
                return Err(ExtractionError::Malformed(
                    result
                        .error_message
                        .unwrap_or_else(|| "local parser failed".to_owned()),
                ));
            }
            ExtractionStatus::Pending | ExtractionStatus::Running => {
                return Err(ExtractionError::Worker(
                    "local parser returned an invalid transient status".to_owned(),
                ));
            }
            ExtractionStatus::Success | ExtractionStatus::Partial => {}
        }

        let method = match result.extractor {
            Some(ExtractorType::PlainText) => ExtractionMethod::NativeText,
            Some(ExtractorType::PdfOcr | ExtractorType::ImageOcr) => ExtractionMethod::Ocr,
            Some(ExtractorType::ImageMetadata | ExtractorType::ZipMetadata) => {
                ExtractionMethod::MetadataOnly
            }
            _ => ExtractionMethod::Parser,
        };
        let mut warnings = Vec::new();
        if result.truncated {
            warnings.push(ExtractionWarning::OutputTruncated);
        }
        if result.error_category == Some(ErrorCategory::OcrUnavailable) {
            warnings.push(ExtractionWarning::OcrUnavailable);
        }
        if result.error_category == Some(ErrorCategory::PotentialArchiveBomb) {
            warnings.push(ExtractionWarning::PotentialArchiveBomb);
        }
        let title = result
            .text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(160).collect());
        let metadata = serde_json::json!({
            "details": result.metadata,
            "status": result.status,
            "detectedContentType": result.detected_content_type,
            "pageCount": result.page_count,
            "sheetCount": result.sheet_count,
            "slideCount": result.slide_count,
            "requiresOcr": result.requires_ocr,
            "ocrUsed": result.ocr_used,
            "network": false
        });
        Ok(ExtractedDocument {
            request_id: request.request_id.clone(),
            title,
            text: result.text,
            method,
            language: result.language_hint,
            pages: Vec::new(),
            metadata,
            warnings,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProcessExtractor {
    executable: PathBuf,
}

impl ProcessExtractor {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerRequest {
    protocol_version: u32,
    extraction: ExtractionRequest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WorkerResponse {
    Success {
        protocol_version: u32,
        document: ExtractedDocument,
    },
    Error {
        protocol_version: u32,
        request_id: Option<String>,
        code: String,
    },
}

impl ExtractionEngine for ProcessExtractor {
    fn extract(&self, request: &ExtractionRequest) -> Result<ExtractedDocument, ExtractionError> {
        let mut child = Command::new(&self.executable)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| ExtractionError::Worker(error.to_string()))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ExtractionError::Worker("worker stdin is unavailable".to_owned()))?;
        serde_json::to_writer(
            &mut stdin,
            &WorkerRequest {
                protocol_version: 1,
                extraction: request.clone(),
            },
        )
        .map_err(|error| ExtractionError::Worker(error.to_string()))?;
        stdin
            .write_all(b"\n")
            .map_err(|error| ExtractionError::Worker(error.to_string()))?;
        drop(stdin);

        let output = child
            .wait_with_output()
            .map_err(|error| ExtractionError::Worker(error.to_string()))?;
        if !output.status.success() || output.stdout.len() > 64 * 1024 * 1024 {
            return Err(ExtractionError::Worker(
                "worker failed or exceeded its output budget".to_owned(),
            ));
        }
        match serde_json::from_slice::<WorkerResponse>(&output.stdout)
            .map_err(|error| ExtractionError::Worker(error.to_string()))?
        {
            WorkerResponse::Success {
                protocol_version: 1,
                document,
            } => Ok(document),
            WorkerResponse::Success { .. } | WorkerResponse::Error { .. } => Err(
                ExtractionError::Worker("worker refused the extraction request".to_owned()),
            ),
        }
    }
}
