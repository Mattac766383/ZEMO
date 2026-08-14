use crate::{
    engine::{ContentExtractor, ExtractionContext, ExtractionInput},
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionStatus,
        ExtractorType,
    },
};

#[derive(Debug, Default)]
pub struct PlainTextExtractor;

impl ContentExtractor for PlainTextExtractor {
    fn can_handle(&self, kind: ContentKind) -> bool {
        kind == ContentKind::Text
    }

    fn extractor_type(&self, _kind: ContentKind) -> ExtractorType {
        ExtractorType::PlainText
    }

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        _context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure> {
        let bytes = input
            .bytes
            .strip_prefix(&[0xef, 0xbb, 0xbf])
            .unwrap_or(input.bytes);
        if bytes.contains(&0) {
            return Err(ExtractionFailure::failed(
                ErrorCategory::InvalidEncoding,
                "text contains binary NUL bytes",
            ));
        }
        let text = match std::str::from_utf8(bytes) {
            Ok(value) => value.to_owned(),
            Err(error) if input.input_truncated && error.error_len().is_none() => {
                let valid = &bytes[..error.valid_up_to()];
                std::str::from_utf8(valid).map(str::to_owned).map_err(|_| {
                    ExtractionFailure::failed(
                        ErrorCategory::InvalidEncoding,
                        "text is not valid UTF-8",
                    )
                })?
            }
            Err(_) => {
                return Err(ExtractionFailure::failed(
                    ErrorCategory::InvalidEncoding,
                    "text is not valid UTF-8",
                ));
            }
        };

        let mut payload = ExtractionPayload::success(ExtractorType::PlainText);
        payload.text = text;
        payload.metadata = serde_json::json!({
            "encoding": "utf-8",
            "inputBytesRead": input.bytes.len(),
            "network": false
        });
        if input.input_truncated {
            payload.status = ExtractionStatus::Partial;
            payload.truncated = true;
            payload.error_category = Some(ErrorCategory::TooLarge);
            payload.error_message =
                Some("text was truncated at the configured input limit".to_owned());
        }
        Ok(payload)
    }
}
