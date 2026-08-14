use crate::{
    ContentExtractionEngine, ErrorCategory, ExtractionLimits, ExtractionStatus,
    LocalExtractionEngine, OcrBlock, OcrError, OcrProvider, OcrRequest, OcrResult, PdfPageRenderer,
};
use lopdf::{
    Document, Object, Stream,
    content::{Content, Operation},
    dictionary,
};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn analyze(extension: &str, bytes: &[u8]) -> crate::ExtractionResult {
    analyze_with(
        LocalExtractionEngine::new(ExtractionLimits::default(), None, None),
        extension,
        bytes,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        false,
    )
}

fn analyze_with(
    engine: LocalExtractionEngine,
    extension: &str,
    bytes: &[u8],
    file_size: u64,
    cancelled: bool,
) -> crate::ExtractionResult {
    let prefix_len = bytes.len().min(engine.limits().detection_bytes);
    let plan = engine.prepare(Some(extension), None, &bytes[..prefix_len], file_size);
    engine.extract(&plan, bytes, file_size, &|| cancelled)
}

fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    for (name, bytes) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .expect("synthetic ZIP entry should start");
        writer
            .write_all(bytes)
            .expect("synthetic ZIP bytes should write");
    }
    writer
        .finish()
        .expect("synthetic ZIP should finish")
        .into_inner()
}

fn make_pdf(text: Option<&str>) -> Vec<u8> {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Courier",
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let operations = text.map_or_else(Vec::new, |value| {
        vec![
            Operation::new("BT", vec![]),
            Operation::new(
                "Tf",
                vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
            ),
            Operation::new("Td", vec![Object::Integer(72), Object::Integer(720)]),
            Operation::new("Tj", vec![Object::string_literal(value)]),
            Operation::new("ET", vec![]),
        ]
    });
    let content = Content { operations }
        .encode()
        .expect("synthetic PDF content should encode");
    let content_id = document.add_object(Stream::new(dictionary! {}, content));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    document.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    let info_id = document.add_object(dictionary! {
        "Title" => Object::string_literal("Synthetic PDF"),
    });
    document.trailer.set("Root", catalog_id);
    document.trailer.set("Info", info_id);
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("synthetic PDF should save");
    bytes
}

fn tiny_png() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, b'I', b'D', b'A', b'T', 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, b'I',
        b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82,
    ]
}

fn tiny_jpeg() -> Vec<u8> {
    vec![
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x01, 0x00, 0x01, 0x03,
        0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00, 0xff, 0xd9,
    ]
}

#[derive(Debug)]
struct FixtureOcr;

impl OcrProvider for FixtureOcr {
    fn recognize(
        &self,
        _request: &OcrRequest<'_>,
        _is_cancelled: &dyn Fn() -> bool,
    ) -> Result<OcrResult, OcrError> {
        Ok(OcrResult {
            text: "LOCAL OCR TEXT".to_owned(),
            mean_confidence: Some(0.92),
            blocks: vec![OcrBlock {
                text: "LOCAL".to_owned(),
                normalized_box: [0.0, 0.0, 1.0, 1.0],
                confidence: Some(0.92),
            }],
            engine_version: "fixture-local".to_owned(),
            language_hint: Some("eng".to_owned()),
        })
    }

    fn provider_name(&self) -> &'static str {
        "fixture_local"
    }
}

#[derive(Debug)]
struct FixturePdfRenderer;

impl PdfPageRenderer for FixturePdfRenderer {
    fn render_page(
        &self,
        _pdf_bytes: &[u8],
        _page_number: u32,
        _is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, OcrError> {
        Ok(tiny_png())
    }

    fn renderer_name(&self) -> &'static str {
        "fixture_local_renderer"
    }
}

#[test]
fn extracts_normal_and_empty_utf8_text() {
    let normal = analyze("txt", b"Invoice 2026\nCustomer ACME");
    let empty = analyze("md", b"");
    assert_eq!(normal.status, ExtractionStatus::Success);
    assert!(normal.text.contains("Customer ACME"));
    assert_eq!(empty.status, ExtractionStatus::Success);
    assert!(empty.text.is_empty());
}

#[test]
fn large_text_is_read_as_a_bounded_partial_prefix() {
    let limits = ExtractionLimits {
        max_text_input_bytes: 8,
        ..ExtractionLimits::default()
    };
    let result = analyze_with(
        LocalExtractionEngine::new(limits, None, None),
        "txt",
        b"abcdefgh",
        64,
        false,
    );
    assert_eq!(result.status, ExtractionStatus::Partial);
    assert_eq!(result.text, "abcdefgh");
    assert!(result.truncated);
}

#[test]
fn invalid_text_encoding_fails_without_lossy_fabrication() {
    let result = analyze("txt", &[0xff, 0xfe, 0xfd]);
    assert_eq!(result.status, ExtractionStatus::Failed);
    assert_eq!(result.error_category, Some(ErrorCategory::InvalidEncoding));
    assert!(result.text.is_empty());
}

#[test]
fn extracts_text_pdf_and_detects_scanned_candidate() {
    let text_pdf = analyze("pdf", &make_pdf(Some("Invoice number 12345")));
    let empty_pdf = analyze("pdf", &make_pdf(None));
    assert_eq!(text_pdf.status, ExtractionStatus::Success);
    assert!(text_pdf.text.contains("Invoice number 12345"));
    assert_eq!(text_pdf.page_count, Some(1));
    assert_eq!(
        text_pdf.metadata["documentMetadata"]["title"],
        "Synthetic PDF"
    );
    assert_eq!(empty_pdf.status, ExtractionStatus::Partial);
    assert!(empty_pdf.requires_ocr);
    assert_eq!(
        empty_pdf.error_category,
        Some(ErrorCategory::OcrUnavailable)
    );
}

#[test]
fn scanned_pdf_uses_only_injected_local_renderer_and_ocr() {
    let pdf = make_pdf(None);
    let engine = LocalExtractionEngine::new(
        ExtractionLimits::default(),
        Some(Arc::new(FixtureOcr)),
        Some(Arc::new(FixturePdfRenderer)),
    );
    let result = analyze_with(
        engine,
        "pdf",
        &pdf,
        u64::try_from(pdf.len()).unwrap_or(u64::MAX),
        false,
    );
    assert_eq!(result.status, ExtractionStatus::Success);
    assert!(result.ocr_used);
    assert_eq!(result.text, "LOCAL OCR TEXT");
    assert_eq!(result.page_count, Some(1));
}

#[test]
fn malformed_pdf_fails_safely() {
    let result = analyze("pdf", b"%PDF-1.7\nnot a document");
    assert_eq!(result.status, ExtractionStatus::Failed);
    assert_eq!(result.error_category, Some(ErrorCategory::Corrupt));
}

#[test]
fn extracts_docx_and_handles_empty_and_malformed_documents() {
    let normal = make_zip(&[(
        "word/document.xml",
        br#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Hello DOCX</w:t></w:r></w:p></w:body></w:document>"#,
    )]);
    let empty = make_zip(&[(
        "word/document.xml",
        br#"<w:document xmlns:w="w"><w:body/></w:document>"#,
    )]);
    assert!(analyze("docx", &normal).text.contains("Hello DOCX"));
    assert_eq!(analyze("docx", &empty).status, ExtractionStatus::Success);
    assert_eq!(
        analyze("docx", b"PK\x03\x04broken").status,
        ExtractionStatus::Failed
    );
}

#[test]
fn extracts_xlsx_sheets_text_numbers_and_enforces_cell_limit() {
    let workbook =
        br#"<workbook><sheets><sheet name="January"/><sheet name="February"/></sheets></workbook>"#;
    let shared = br#"<sst><si><t>Revenue</t></si><si><t>Cost</t></si></sst>"#;
    let sheet_one =
        br#"<worksheet><sheetData><row><c t="s"><v>0</v></c><c><v>42.5</v></c></row></sheetData></worksheet>"#;
    let sheet_two =
        br#"<worksheet><sheetData><row><c t="s"><v>1</v></c><c><v>10</v></c></row></sheetData></worksheet>"#;
    let workbook_bytes = make_zip(&[
        ("xl/workbook.xml", workbook),
        ("xl/sharedStrings.xml", shared),
        ("xl/worksheets/sheet1.xml", sheet_one),
        ("xl/worksheets/sheet2.xml", sheet_two),
    ]);
    let result = analyze("xlsx", &workbook_bytes);
    assert_eq!(result.sheet_count, Some(2));
    assert!(result.text.contains("January"));
    assert!(result.text.contains("Revenue"));
    assert!(result.text.contains("42.5"));

    let limits = ExtractionLimits {
        max_spreadsheet_cells: 1,
        ..ExtractionLimits::default()
    };
    let limited = analyze_with(
        LocalExtractionEngine::new(limits, None, None),
        "xlsx",
        &workbook_bytes,
        u64::try_from(workbook_bytes.len()).unwrap_or(u64::MAX),
        false,
    );
    assert_eq!(limited.status, ExtractionStatus::Partial);
    assert_eq!(limited.error_category, Some(ErrorCategory::TooManyCells));
}

#[test]
fn extracts_pptx_slide_text_and_rejects_malformed_xml() {
    let presentation = make_zip(&[
        (
            "ppt/slides/slide1.xml",
            br#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>First slide</a:t></p:sld>"#,
        ),
        (
            "ppt/slides/slide2.xml",
            br#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>Second slide</a:t></p:sld>"#,
        ),
    ]);
    let result = analyze("pptx", &presentation);
    assert_eq!(result.slide_count, Some(2));
    assert!(result.text.contains("First slide"));
    assert!(result.text.contains("Second slide"));

    let malformed = make_zip(&[("ppt/slides/slide1.xml", b"<broken")]);
    assert_eq!(analyze("pptx", &malformed).status, ExtractionStatus::Failed);
}

#[test]
fn extracts_png_and_jpeg_metadata_and_runs_injected_local_ocr() {
    let engine = LocalExtractionEngine::new(
        ExtractionLimits::default(),
        Some(Arc::new(FixtureOcr)),
        None,
    );
    let png = tiny_png();
    let result = analyze_with(
        engine,
        "png",
        &png,
        u64::try_from(png.len()).unwrap_or(u64::MAX),
        false,
    );
    assert_eq!(result.image_width, Some(1));
    assert_eq!(result.image_height, Some(1));
    assert!(result.ocr_used);
    assert_eq!(result.text, "LOCAL OCR TEXT");
    assert_eq!(result.ocr_confidence, Some(0.92));

    let jpeg = analyze("jpg", &tiny_jpeg());
    assert_eq!(jpeg.image_width, Some(1));
    assert_eq!(jpeg.image_height, Some(1));
    assert!(jpeg.requires_ocr);
}

#[test]
fn zip_metadata_blocks_traversal_and_entry_excess() {
    let safe = make_zip(&[("folder/report.txt", b"hello")]);
    let safe_result = analyze("zip", &safe);
    assert_eq!(safe_result.status, ExtractionStatus::Success);
    assert!(safe_result.text.contains("folder/report.txt"));

    let traversal = make_zip(&[("../escape.txt", b"never extracted")]);
    let traversal_result = analyze("zip", &traversal);
    assert_eq!(traversal_result.status, ExtractionStatus::Partial);
    assert_eq!(
        traversal_result.error_category,
        Some(ErrorCategory::ArchiveTraversal)
    );

    let many = make_zip(&[("one", b"1"), ("two", b"2")]);
    let limits = ExtractionLimits {
        max_archive_entries: 1,
        ..ExtractionLimits::default()
    };
    let excessive = analyze_with(
        LocalExtractionEngine::new(limits, None, None),
        "zip",
        &many,
        u64::try_from(many.len()).unwrap_or(u64::MAX),
        false,
    );
    assert_eq!(excessive.status, ExtractionStatus::Skipped);
    assert_eq!(
        excessive.error_category,
        Some(ErrorCategory::TooManyEntries)
    );
}

#[test]
fn malformed_zip_unsupported_binary_and_type_mismatch_fail_safely() {
    assert_eq!(
        analyze("zip", b"PK\x03\x04broken").error_category,
        Some(ErrorCategory::Corrupt)
    );
    assert_eq!(
        analyze("bin", &[0, 1, 2]).status,
        ExtractionStatus::Unsupported
    );
    let mismatch = analyze("pdf", b"MZ\x90\x00");
    assert_eq!(mismatch.status, ExtractionStatus::Failed);
    assert_eq!(mismatch.error_category, Some(ErrorCategory::TypeMismatch));
}

#[test]
fn cancellation_stops_before_parser_invocation() {
    let result = analyze_with(
        LocalExtractionEngine::new(ExtractionLimits::default(), None, None),
        "txt",
        b"private",
        7,
        true,
    );
    assert_eq!(result.status, ExtractionStatus::Skipped);
    assert_eq!(result.error_category, Some(ErrorCategory::Cancelled));
}

#[test]
fn runtime_has_no_network_or_cloud_ocr_fallback() {
    let engine = LocalExtractionEngine::new(ExtractionLimits::default(), None, None);
    assert!(!engine.network_enabled());
    let manifest = include_str!("../Cargo.toml");
    assert!(!manifest.contains("reqwest"));
    assert!(!manifest.contains("hyper"));
    assert!(!manifest.contains("openai"));
    assert!(!manifest.contains("anthropic"));
}

#[test]
fn archive_path_checks_reject_all_traversal_forms() {
    assert!(crate::archive::archive_path_is_safe("safe/report.txt"));
    assert!(!crate::archive::archive_path_is_safe("../escape"));
    assert!(!crate::archive::archive_path_is_safe("/absolute"));
    assert!(!crate::office::archive_path_is_safe("C:/absolute"));
    assert!(!crate::office::archive_path_is_safe(r"folder\escape"));
}

#[test]
fn video_mp4_metadata_is_extracted_without_decoding_frames() {
    // Hand-built ISO BMFF: ftyp(24) + moov/mvhd with timescale=1000, duration=5000.
    let bytes: Vec<u8> = {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x14]); // ftyp size 20
        out.extend_from_slice(b"ftyp");
        out.extend_from_slice(b"isom");
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(b"isom");
        // moov size = 8 + mvhd(108) = 116 = 0x74
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x74]);
        out.extend_from_slice(b"moov");
        // mvhd size 108 = 0x6C
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x6C]);
        out.extend_from_slice(b"mvhd");
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&5000u32.to_be_bytes());
        out.extend_from_slice(&mvhd);
        out
    };

    let result = analyze("mp4", &bytes);
    assert!(
        matches!(
            result.status,
            ExtractionStatus::Success | ExtractionStatus::Partial
        ),
        "status={:?} meta={:?} err={:?}",
        result.status,
        result.metadata,
        result.error_message
    );
    assert_eq!(result.extractor, Some(crate::ExtractorType::VideoMetadata));
    let duration = result
        .metadata
        .get("durationSeconds")
        .and_then(|value| value.as_f64())
        .unwrap_or_else(|| panic!("duration missing in {:?}", result.metadata));
    assert!((duration - 5.0).abs() < 0.01);
    assert_eq!(
        result
            .metadata
            .get("framesDecoded")
            .and_then(|value| value.as_bool()),
        Some(false)
    );
}

#[test]
fn encrypted_pdf_fails_closed_without_password_prompt() {
    // Trailer marks Encrypt; lopdf should surface encryption.
    let bytes = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<< /Size 1 /Root 1 0 R /Encrypt 1 0 R >>\nstartxref\n0\n%%EOF";
    let result = analyze("pdf", bytes);
    assert_eq!(result.status, ExtractionStatus::Failed);
    assert_eq!(
        result.error_category,
        Some(ErrorCategory::EncryptedDocument)
    );
    let message = result
        .error_message
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(message.contains("encrypt") || message.contains("password"));
    assert!(!message.contains("attempting password"));
}

#[test]
fn legacy_office_remains_explicitly_unsupported() {
    let ole = {
        let mut bytes = vec![0u8; 512];
        bytes[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        bytes
    };
    let result = analyze("doc", &ole);
    assert_eq!(result.status, ExtractionStatus::Unsupported);
    assert_eq!(result.error_category, Some(ErrorCategory::Unsupported));
}

#[test]
fn ocr_packaging_discovers_absolute_env_override_before_host_paths() {
    // Relative overrides must remain rejected by provider construction.
    assert!(
        crate::TesseractOcrProvider::new("tesseract", std::time::Duration::from_secs(1)).is_err()
    );
}
