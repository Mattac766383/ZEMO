use serde::{Deserialize, Serialize};

/// Central safety policy shared by every local extractor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionLimits {
    pub detection_bytes: usize,
    pub max_text_input_bytes: usize,
    pub max_pdf_input_bytes: u64,
    pub max_office_input_bytes: u64,
    pub max_image_input_bytes: u64,
    pub max_archive_input_bytes: u64,
    pub max_extracted_characters: usize,
    pub max_pages: u32,
    pub max_spreadsheet_cells: usize,
    pub max_archive_entries: usize,
    pub max_archive_metadata_bytes: usize,
    pub max_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
    pub max_image_pixels: u64,
    pub max_ocr_pages: u32,
    pub max_ocr_output_characters: usize,
    pub max_workers: usize,
    pub max_ocr_workers: usize,
    pub external_process_timeout_ms: u64,
}

impl Default for ExtractionLimits {
    fn default() -> Self {
        Self {
            detection_bytes: 64 * 1024,
            max_text_input_bytes: 4 * 1024 * 1024,
            max_pdf_input_bytes: 64 * 1024 * 1024,
            max_office_input_bytes: 64 * 1024 * 1024,
            max_image_input_bytes: 32 * 1024 * 1024,
            max_archive_input_bytes: 32 * 1024 * 1024,
            max_extracted_characters: 2_000_000,
            max_pages: 500,
            max_spreadsheet_cells: 200_000,
            max_archive_entries: 10_000,
            max_archive_metadata_bytes: 2 * 1024 * 1024,
            max_uncompressed_bytes: 128 * 1024 * 1024,
            max_compression_ratio: 1_000,
            max_image_pixels: 40_000_000,
            max_ocr_pages: 20,
            max_ocr_output_characters: 500_000,
            max_workers: 4,
            max_ocr_workers: 1,
            external_process_timeout_ms: 60_000,
        }
    }
}

impl ExtractionLimits {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.detection_bytes = self.detection_bytes.clamp(512, 1024 * 1024);
        self.max_text_input_bytes = self.max_text_input_bytes.max(self.detection_bytes);
        self.max_extracted_characters = self.max_extracted_characters.max(1);
        self.max_pages = self.max_pages.max(1);
        self.max_spreadsheet_cells = self.max_spreadsheet_cells.max(1);
        self.max_archive_entries = self.max_archive_entries.max(1);
        self.max_archive_metadata_bytes = self.max_archive_metadata_bytes.max(1);
        self.max_uncompressed_bytes = self.max_uncompressed_bytes.max(1);
        self.max_compression_ratio = self.max_compression_ratio.max(1);
        self.max_image_pixels = self.max_image_pixels.max(1);
        self.max_ocr_pages = self.max_ocr_pages.min(self.max_pages).max(1);
        self.max_ocr_output_characters = self.max_ocr_output_characters.max(1);
        self.max_workers = self.max_workers.clamp(1, 16);
        self.max_ocr_workers = self.max_ocr_workers.clamp(1, self.max_workers);
        self.external_process_timeout_ms = self.external_process_timeout_ms.max(1_000);
        self
    }
}
