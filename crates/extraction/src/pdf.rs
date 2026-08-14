use crate::{
    engine::{ContentExtractor, ExtractionContext, ExtractionInput},
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionStatus,
        ExtractorType,
    },
    ocr::{OcrErrorKind, OcrRequest},
};

#[derive(Debug, Default)]
pub struct PdfExtractor;

impl ContentExtractor for PdfExtractor {
    fn can_handle(&self, kind: ContentKind) -> bool {
        kind == ContentKind::Pdf
    }

    fn extractor_type(&self, _kind: ContentKind) -> ExtractorType {
        ExtractorType::PdfText
    }

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure> {
        if input.input_truncated {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                "PDF exceeds the configured input limit",
            ));
        }
        if !input.bytes.starts_with(b"%PDF-") {
            return Err(ExtractionFailure::failed(
                ErrorCategory::Corrupt,
                "PDF header is missing or malformed",
            ));
        }
        // Fail closed before deeper parsing when an Encrypt dictionary is advertised.
        if pdf_advertises_encryption(input.bytes) {
            return Err(ExtractionFailure::failed(
                ErrorCategory::EncryptedDocument,
                "encrypted PDF detected; content is not opened, passwords are never requested, and decryption is not attempted",
            ));
        }
        let document = lopdf::Document::load_mem(input.bytes).map_err(|error| {
            let message = error.to_string();
            if message.to_ascii_lowercase().contains("encrypt") {
                ExtractionFailure::failed(
                    ErrorCategory::EncryptedDocument,
                    "encrypted PDF detected; password entry and decryption are not supported",
                )
            } else {
                ExtractionFailure::failed(ErrorCategory::Corrupt, "PDF structure is malformed")
            }
        })?;
        if document.is_encrypted() {
            return Err(ExtractionFailure::failed(
                ErrorCategory::EncryptedDocument,
                "encrypted PDF detected; content is not opened, passwords are never requested, and decryption is not attempted",
            ));
        }
        let page_count = u32::try_from(document.get_pages().len()).unwrap_or(u32::MAX);
        if page_count > context.limits.max_pages {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooManyPages,
                format!(
                    "PDF has {page_count} pages; limit is {}",
                    context.limits.max_pages
                ),
            ));
        }
        let document_metadata = extract_document_metadata(&document);

        let extracted = pdf_extract::extract_text_from_mem(input.bytes);
        let native_text = extracted.as_deref().unwrap_or_default();
        if page_count == 0 || has_usable_text(native_text, page_count) {
            let mut payload = ExtractionPayload::success(ExtractorType::PdfText);
            payload.text = native_text.to_owned();
            payload.page_count = Some(page_count);
            payload.metadata = serde_json::json!({
                "format": "pdf",
                "pageCount": page_count,
                "nativeTextAvailable": !native_text.trim().is_empty(),
                "documentMetadata": document_metadata,
                "network": false
            });
            if extracted.is_err() {
                payload.status = ExtractionStatus::Partial;
                payload.error_category = Some(ErrorCategory::ParserFailure);
                payload.error_message =
                    Some("PDF metadata was read but text extraction was incomplete".to_owned());
            }
            return Ok(payload);
        }

        extract_scanned_pdf(
            input,
            context,
            page_count,
            extracted.err().map(|error| error.to_string()),
            document_metadata,
        )
    }
}

fn extract_scanned_pdf(
    input: &ExtractionInput<'_>,
    context: &ExtractionContext<'_>,
    page_count: u32,
    native_parser_error: Option<String>,
    document_metadata: serde_json::Value,
) -> Result<ExtractionPayload, ExtractionFailure> {
    let Some(renderer) = context.pdf_renderer else {
        return Ok(ocr_unavailable_payload(
            page_count,
            "PDF contains insufficient native text and no trusted local renderer is installed",
            native_parser_error,
            document_metadata,
        ));
    };
    let Some(ocr) = context.ocr_provider else {
        return Ok(ocr_unavailable_payload(
            page_count,
            "PDF contains insufficient native text and no trusted local OCR engine is installed",
            native_parser_error,
            document_metadata,
        ));
    };

    let pages_to_process = page_count.min(context.limits.max_ocr_pages);
    let mut text = String::new();
    let mut confidences = Vec::new();
    let mut processed_pages = Vec::new();
    let mut first_error = None;
    for page_number in 1..=pages_to_process {
        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "PDF OCR was cancelled",
            ));
        }
        let gate = context.ocr_gate.lock().map_err(|_| {
            ExtractionFailure::failed(
                ErrorCategory::OcrFailed,
                "local OCR concurrency gate is unavailable",
            )
        })?;
        let page_image = renderer.render_page(input.bytes, page_number, context.is_cancelled);
        let page_result = page_image.and_then(|image_bytes| {
            ocr.recognize(
                &OcrRequest {
                    image_bytes: &image_bytes,
                    media_type: "image/png",
                    languages: context.ocr_languages,
                    max_output_characters: context.limits.max_ocr_output_characters,
                },
                context.is_cancelled,
            )
        });
        drop(gate);

        match page_result {
            Ok(result) => {
                if !result.text.trim().is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&result.text);
                }
                if let Some(confidence) = result.mean_confidence {
                    confidences.push(confidence);
                }
                processed_pages.push(page_number);
            }
            Err(error) if error.kind == OcrErrorKind::Cancelled => {
                return Err(ExtractionFailure::skipped(
                    ErrorCategory::Cancelled,
                    "PDF OCR was cancelled",
                ));
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error.message);
                }
            }
        }
    }

    let mut payload = ExtractionPayload::success(ExtractorType::PdfOcr);
    payload.text = text;
    payload.page_count = Some(page_count);
    payload.requires_ocr = true;
    payload.ocr_used = !processed_pages.is_empty();
    payload.ocr_confidence = mean(&confidences);
    payload.language_hint = context.ocr_languages.first().cloned();
    payload.metadata = serde_json::json!({
        "format": "pdf",
        "pageCount": page_count,
        "ocrPages": processed_pages,
        "ocrProvider": ocr.provider_name(),
        "pdfRenderer": renderer.renderer_name(),
        "nativeParserWarning": native_parser_error,
        "documentMetadata": document_metadata,
        "network": false
    });

    if processed_pages.len() < usize::try_from(pages_to_process).unwrap_or(usize::MAX) {
        payload.status = ExtractionStatus::Partial;
        payload.error_category = Some(ErrorCategory::OcrFailed);
        payload.error_message = Some(
            first_error.unwrap_or_else(|| "one or more PDF pages could not be OCRed".to_owned()),
        );
    } else if pages_to_process < page_count {
        payload.status = ExtractionStatus::Partial;
        payload.truncated = true;
        payload.error_category = Some(ErrorCategory::TooManyPages);
        payload.error_message = Some(format!(
            "OCR stopped after the configured {}-page limit",
            context.limits.max_ocr_pages
        ));
    }
    Ok(payload)
}

fn ocr_unavailable_payload(
    page_count: u32,
    message: &str,
    native_parser_error: Option<String>,
    document_metadata: serde_json::Value,
) -> ExtractionPayload {
    let mut payload = ExtractionPayload::success(ExtractorType::PdfText);
    payload.status = ExtractionStatus::Partial;
    payload.page_count = Some(page_count);
    payload.requires_ocr = true;
    payload.error_category = Some(ErrorCategory::OcrUnavailable);
    payload.error_message = Some(message.to_owned());
    payload.metadata = serde_json::json!({
        "format": "pdf",
        "pageCount": page_count,
        "ocrCandidate": true,
        "nativeParserWarning": native_parser_error,
        "documentMetadata": document_metadata,
        "network": false
    });
    payload
}

fn has_usable_text(text: &str, page_count: u32) -> bool {
    let useful_characters = text
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_control())
        .count();
    let minimum = usize::try_from(page_count)
        .unwrap_or(usize::MAX)
        .saturating_mul(4)
        .max(16);
    useful_characters >= minimum
}

fn pdf_advertises_encryption(bytes: &[u8]) -> bool {
    // Conservative ASCII scan of the trailer region only. Never attempts to open
    // or decrypt the document when /Encrypt is present.
    let sample = if bytes.len() > 64 * 1024 {
        &bytes[bytes.len() - 64 * 1024..]
    } else {
        bytes
    };
    let Ok(text) = std::str::from_utf8(sample) else {
        return sample.windows(8).any(|window| window == b"/Encrypt");
    };
    text.contains("/Encrypt")
}

fn mean(values: &[f32]) -> Option<f32> {
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

fn extract_document_metadata(document: &lopdf::Document) -> serde_json::Value {
    let Some(dictionary) = document
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|value| value.as_reference().ok())
        .and_then(|object_id| document.get_dictionary(object_id).ok())
    else {
        return serde_json::json!({});
    };
    let mut metadata = serde_json::Map::new();
    for (pdf_key, json_key) in [
        (b"Title".as_slice(), "title"),
        (b"Author".as_slice(), "author"),
        (b"Subject".as_slice(), "subject"),
        (b"Creator".as_slice(), "creator"),
        (b"Producer".as_slice(), "producer"),
        (b"CreationDate".as_slice(), "creationDate"),
    ] {
        if let Some(value) = dictionary
            .get(pdf_key)
            .ok()
            .and_then(pdf_string)
            .filter(|value| !value.is_empty())
        {
            metadata.insert(json_key.to_owned(), serde_json::Value::String(value));
        }
    }
    serde_json::Value::Object(metadata)
}

fn pdf_string(object: &lopdf::Object) -> Option<String> {
    let bytes = match object {
        lopdf::Object::String(bytes, _) | lopdf::Object::Name(bytes) => bytes,
        _ => return None,
    };
    let value = if bytes.starts_with(&[0xfe, 0xff]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    Some(value.trim().chars().take(512).collect())
}
