use crate::{
    engine::{ContentExtractor, ExtractionContext, ExtractionInput},
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionStatus,
        ExtractorType,
    },
    ocr::{OcrErrorKind, OcrRequest},
};
use exif::{In, Reader, Tag};
use std::io::Cursor;

#[derive(Debug, Default)]
pub struct ImageExtractor;

impl ContentExtractor for ImageExtractor {
    fn can_handle(&self, kind: ContentKind) -> bool {
        kind == ContentKind::Image
    }

    fn extractor_type(&self, _kind: ContentKind) -> ExtractorType {
        ExtractorType::ImageMetadata
    }

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure> {
        if input.input_truncated {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                "image exceeds the configured input limit",
            ));
        }
        let dimensions = imagesize::blob_size(input.bytes).map_err(|_| {
            ExtractionFailure::failed(ErrorCategory::Corrupt, "image dimensions could not be read")
        })?;
        let width = u32::try_from(dimensions.width).map_err(|_| {
            ExtractionFailure::skipped(ErrorCategory::TooLarge, "image width is too large")
        })?;
        let height = u32::try_from(dimensions.height).map_err(|_| {
            ExtractionFailure::skipped(ErrorCategory::TooLarge, "image height is too large")
        })?;
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        if pixels > context.limits.max_image_pixels {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                format!(
                    "image has {pixels} pixels; limit is {}",
                    context.limits.max_image_pixels
                ),
            ));
        }
        let media_type = infer::get(input.bytes)
            .map(|kind| kind.mime_type())
            .filter(|value| value.starts_with("image/"))
            .unwrap_or(input.detection.detected_content_type.as_str());
        let exif = read_safe_exif(input.bytes);
        let ocr_supported = matches!(
            media_type,
            "image/png" | "image/jpeg" | "image/webp" | "image/tiff" | "image/bmp"
        );

        let mut payload = ExtractionPayload::success(ExtractorType::ImageMetadata);
        payload.image_width = Some(width);
        payload.image_height = Some(height);
        payload.requires_ocr = ocr_supported;
        payload.metadata = serde_json::json!({
            "format": media_type,
            "width": width,
            "height": height,
            "pixelCount": pixels,
            "exifTimestamp": exif.timestamp,
            "cameraMake": exif.camera_make,
            "cameraModel": exif.camera_model,
            "gpsPresent": exif.gps_present,
            "gpsCoordinatesStored": false,
            "network": false
        });
        if !ocr_supported {
            return Ok(payload);
        }
        let Some(ocr) = context.ocr_provider else {
            payload.status = ExtractionStatus::Partial;
            payload.error_category = Some(ErrorCategory::OcrUnavailable);
            payload.error_message =
                Some("image metadata was read but no trusted local OCR engine is installed".into());
            return Ok(payload);
        };
        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "image OCR was cancelled",
            ));
        }
        let gate = context.ocr_gate.lock().map_err(|_| {
            ExtractionFailure::failed(
                ErrorCategory::OcrFailed,
                "local OCR concurrency gate is unavailable",
            )
        })?;
        let result = ocr.recognize(
            &OcrRequest {
                image_bytes: input.bytes,
                media_type,
                languages: context.ocr_languages,
                max_output_characters: context.limits.max_ocr_output_characters,
            },
            context.is_cancelled,
        );
        drop(gate);

        match result {
            Ok(result) => {
                payload.extractor = ExtractorType::ImageOcr;
                payload.text = result.text;
                payload.ocr_used = true;
                payload.ocr_confidence = result.mean_confidence;
                payload.language_hint = result.language_hint;
                payload.metadata["ocrProvider"] =
                    serde_json::Value::String(ocr.provider_name().to_owned());
                payload.metadata["ocrEngineVersion"] =
                    serde_json::Value::String(result.engine_version);
                payload.metadata["ocrBlockCount"] = serde_json::json!(result.blocks.len());
                payload.metadata["ocrImageReference"] = serde_json::json!(0);
            }
            Err(error) if error.kind == OcrErrorKind::Cancelled => {
                return Err(ExtractionFailure::skipped(
                    ErrorCategory::Cancelled,
                    "image OCR was cancelled",
                ));
            }
            Err(error) => {
                payload.status = ExtractionStatus::Partial;
                payload.error_category = Some(if error.kind == OcrErrorKind::Unavailable {
                    ErrorCategory::OcrUnavailable
                } else {
                    ErrorCategory::OcrFailed
                });
                payload.error_message = Some(error.message);
            }
        }
        Ok(payload)
    }
}

#[derive(Debug, Default)]
struct SafeExif {
    timestamp: Option<String>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    gps_present: bool,
}

fn read_safe_exif(bytes: &[u8]) -> SafeExif {
    let mut cursor = Cursor::new(bytes);
    let Ok(exif) = Reader::new().read_from_container(&mut cursor) else {
        return SafeExif::default();
    };
    let timestamp = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .or_else(|| exif.get_field(Tag::DateTime, In::PRIMARY))
        .map(|field| bounded_display(field.display_value().with_unit(&exif).to_string()));
    let camera_make = exif
        .get_field(Tag::Make, In::PRIMARY)
        .map(|field| bounded_display(field.display_value().with_unit(&exif).to_string()));
    let camera_model = exif
        .get_field(Tag::Model, In::PRIMARY)
        .map(|field| bounded_display(field.display_value().with_unit(&exif).to_string()));
    let gps_present = exif.fields().any(|field| {
        matches!(
            field.tag,
            Tag::GPSLatitude
                | Tag::GPSLongitude
                | Tag::GPSLatitudeRef
                | Tag::GPSLongitudeRef
                | Tag::GPSAltitude
        )
    });
    SafeExif {
        timestamp,
        camera_make,
        camera_model,
        gps_present,
    }
}

fn bounded_display(value: String) -> String {
    value.trim().chars().take(256).collect()
}
