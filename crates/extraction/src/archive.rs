use crate::{
    engine::{ContentExtractor, ExtractionContext, ExtractionInput},
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionStatus,
        ExtractorType,
    },
};
use std::io::Cursor;

#[derive(Debug, Default)]
pub struct ZipMetadataExtractor;

impl ContentExtractor for ZipMetadataExtractor {
    fn can_handle(&self, kind: ContentKind) -> bool {
        kind == ContentKind::Zip
    }

    fn extractor_type(&self, _kind: ContentKind) -> ExtractorType {
        ExtractorType::ZipMetadata
    }

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure> {
        if input.input_truncated {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                "ZIP archive exceeds the configured input limit",
            ));
        }
        let cursor = Cursor::new(input.bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|_| {
            ExtractionFailure::failed(ErrorCategory::Corrupt, "ZIP archive is malformed")
        })?;
        if archive.len() > context.limits.max_archive_entries {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooManyEntries,
                format!(
                    "ZIP archive has {} entries; limit is {}",
                    archive.len(),
                    context.limits.max_archive_entries
                ),
            ));
        }

        let mut manifest = String::new();
        let mut total_uncompressed = 0_u64;
        let mut observed_entries = 0_usize;
        let mut unsafe_entries = 0_usize;
        let mut suspicious_entries = 0_usize;
        let mut truncated = false;
        for index in 0..archive.len() {
            if (context.is_cancelled)() {
                return Err(ExtractionFailure::skipped(
                    ErrorCategory::Cancelled,
                    "ZIP metadata inspection was cancelled",
                ));
            }
            let entry = archive.by_index(index).map_err(|_| {
                ExtractionFailure::failed(
                    ErrorCategory::Corrupt,
                    "ZIP central directory is malformed",
                )
            })?;
            observed_entries = observed_entries.saturating_add(1);
            total_uncompressed = total_uncompressed.saturating_add(entry.size());
            let safe = safe_archive_path(entry.name());
            if !safe {
                unsafe_entries = unsafe_entries.saturating_add(1);
            }
            if suspicious_ratio(
                entry.size(),
                entry.compressed_size(),
                context.limits.max_compression_ratio,
            ) {
                suspicious_entries = suspicious_entries.saturating_add(1);
            }
            let line = format!(
                "{}\t{}\t{}\n",
                if safe { "SAFE" } else { "UNSAFE_PATH" },
                entry.name(),
                entry.size()
            );
            if manifest.len().saturating_add(line.len()) > context.limits.max_archive_metadata_bytes
            {
                truncated = true;
                break;
            }
            manifest.push_str(&line);
            if total_uncompressed > context.limits.max_uncompressed_bytes {
                truncated = true;
                break;
            }
        }

        let mut payload = ExtractionPayload::success(ExtractorType::ZipMetadata);
        payload.text = manifest;
        payload.truncated = truncated;
        payload.metadata = serde_json::json!({
            "format": "zip",
            "entryCount": archive.len(),
            "entriesObserved": observed_entries,
            "unsafePathCount": unsafe_entries,
            "suspiciousCompressionCount": suspicious_entries,
            "uncompressedBytesObserved": total_uncompressed,
            "contentExtracted": false,
            "nestedArchivesExtracted": false,
            "network": false
        });
        if unsafe_entries > 0 {
            payload.status = ExtractionStatus::Partial;
            payload.error_category = Some(ErrorCategory::ArchiveTraversal);
            payload.error_message =
                Some("ZIP contains unsafe absolute or parent-traversal entry paths".to_owned());
        } else if suspicious_entries > 0 {
            payload.status = ExtractionStatus::Partial;
            payload.error_category = Some(ErrorCategory::PotentialArchiveBomb);
            payload.error_message =
                Some("ZIP contains entries with suspicious compression ratios".to_owned());
        } else if truncated {
            payload.status = ExtractionStatus::Partial;
            payload.error_category = Some(ErrorCategory::TooLarge);
            payload.error_message =
                Some("ZIP metadata inspection stopped at a configured safety limit".to_owned());
        }
        Ok(payload)
    }
}

fn safe_archive_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['/', '\\'])
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.split('/').any(|component| component == "..")
        && !name.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

fn suspicious_ratio(size: u64, compressed_size: u64, max_ratio: u64) -> bool {
    (compressed_size == 0 && size > 0)
        || (compressed_size > 0 && size / compressed_size > max_ratio)
}

#[cfg(test)]
pub(crate) fn archive_path_is_safe(name: &str) -> bool {
    safe_archive_path(name)
}
