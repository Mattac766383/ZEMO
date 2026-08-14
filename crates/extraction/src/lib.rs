//! Bounded, deterministic, local-only content extraction.
//!
//! Extractors receive bounded bytes, never mutation capabilities or network
//! clients. Original corpus files remain owned by the read-only platform port.

mod archive;
mod detection;
mod engine;
mod image;
mod legacy;
mod limits;
mod model;
mod ocr;
mod office;
mod pdf;
mod text;
mod video;

pub use detection::detect_file_type;
pub use engine::{ContentExtractionEngine, LocalExtractionEngine};
pub use legacy::{
    DeterministicExtractor, ExtractedDocument, ExtractedPage, ExtractionEngine, ExtractionError,
    ExtractionMethod, ExtractionRequest, ExtractionWarning, ProcessExtractor,
};
pub use limits::ExtractionLimits;
pub use model::{
    ContentKind, ErrorCategory, ExtractionPlan, ExtractionResult, ExtractionStatus, ExtractorType,
    FileTypeDetection, OfficeKind, ReadMode,
};
pub use ocr::{
    OcrBlock, OcrError, OcrErrorKind, OcrProvider, OcrRequest, OcrResult, PdfPageRenderer,
    PdftoppmRenderer, TesseractOcrProvider, UnavailableOcrProvider,
};

#[cfg(test)]
mod tests;
