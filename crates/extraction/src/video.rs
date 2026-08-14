//! Bounded local video container metadata extraction.
//!
//! Parses container headers only (ISO BMFF / Matroska EBML). Never decodes
//! frames, executes codecs, or shells out to ffmpeg.

use crate::{
    engine::{ContentExtractor, ExtractionContext, ExtractionInput},
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionStatus,
        ExtractorType,
    },
};
use std::time::Instant;

const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_BOXES: usize = 4_096;
const MAX_EBML_ELEMENTS: usize = 4_096;

#[derive(Debug, Default)]
pub struct VideoMetadataExtractor;

impl ContentExtractor for VideoMetadataExtractor {
    fn can_handle(&self, kind: ContentKind) -> bool {
        kind == ContentKind::Video
    }

    fn extractor_type(&self, _kind: ContentKind) -> ExtractorType {
        ExtractorType::VideoMetadata
    }

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure> {
        if input.input_truncated {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                "video exceeds the configured input limit for metadata parsing",
            ));
        }
        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "video metadata extraction was cancelled",
            ));
        }
        let started = Instant::now();
        let media = input.detection.detected_content_type.as_str();
        let bytes = if input.bytes.len() > MAX_SCAN_BYTES {
            &input.bytes[..MAX_SCAN_BYTES]
        } else {
            input.bytes
        };
        let parsed = if media.contains("webm")
            || media.contains("matroska")
            || looks_like_ebml(bytes)
        {
            parse_matroska_metadata(bytes, context.is_cancelled)?
        } else if media.contains("mp4")
            || media.contains("quicktime")
            || media.contains("x-m4v")
            || looks_like_iso_bmff(bytes)
        {
            parse_iso_bmff_metadata(bytes, context.is_cancelled)?
        } else {
            return Ok(unsupported_video_payload(
                media,
                "video container is recognized but no safe metadata parser is available for this subtype",
            ));
        };

        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "video metadata extraction was cancelled",
            ));
        }

        let mut payload = ExtractionPayload::success(ExtractorType::VideoMetadata);
        if parsed.partial {
            payload.status = ExtractionStatus::Partial;
            payload.error_category = Some(ErrorCategory::ParserFailure);
            payload.error_message = Some(
                "video container metadata was only partially recovered from bounded headers"
                    .to_owned(),
            );
        }
        payload.metadata = serde_json::json!({
            "format": "video",
            "container": parsed.container,
            "durationSeconds": parsed.duration_seconds,
            "width": parsed.width,
            "height": parsed.height,
            "videoCodec": parsed.video_codec,
            "audioCodec": parsed.audio_codec,
            "creationTime": parsed.creation_time,
            "scannedBytes": bytes.len(),
            "elapsedMs": started.elapsed().as_millis(),
            "network": false,
            "framesDecoded": false
        });
        Ok(payload)
    }
}

#[derive(Debug, Default)]
struct VideoMetadata {
    container: &'static str,
    duration_seconds: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    creation_time: Option<String>,
    partial: bool,
}

fn unsupported_video_payload(media: &str, message: &str) -> ExtractionPayload {
    let mut payload = ExtractionPayload::success(ExtractorType::VideoMetadata);
    payload.status = ExtractionStatus::Unsupported;
    payload.error_category = Some(ErrorCategory::Unsupported);
    payload.error_message = Some(message.to_owned());
    payload.metadata = serde_json::json!({
        "format": "video",
        "detectedContentType": media,
        "network": false,
        "framesDecoded": false
    });
    payload
}

fn looks_like_iso_bmff(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let kind = &bytes[4..8];
    size >= 8
        && matches!(
            kind,
            b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot"
        )
}

fn looks_like_ebml(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
}

fn parse_iso_bmff_metadata(
    bytes: &[u8],
    is_cancelled: &dyn Fn() -> bool,
) -> Result<VideoMetadata, ExtractionFailure> {
    let mut metadata = VideoMetadata {
        container: "iso_bmff",
        ..VideoMetadata::default()
    };
    let mut offset = 0usize;
    let mut boxes = 0usize;
    while offset + 8 <= bytes.len() && boxes < MAX_BOXES {
        if is_cancelled() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "video metadata extraction was cancelled",
            ));
        }
        boxes += 1;
        let size_field = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let kind = &bytes[offset + 4..offset + 8];
        let (header_size, box_size) = if size_field == 1 {
            if offset + 16 > bytes.len() {
                metadata.partial = true;
                break;
            }
            let large = u64::from_be_bytes([
                bytes[offset + 8],
                bytes[offset + 9],
                bytes[offset + 10],
                bytes[offset + 11],
                bytes[offset + 12],
                bytes[offset + 13],
                bytes[offset + 14],
                bytes[offset + 15],
            ]);
            (16usize, large as usize)
        } else if size_field == 0 {
            (8usize, bytes.len().saturating_sub(offset))
        } else {
            (8usize, size_field as usize)
        };
        if box_size < header_size || offset + box_size > bytes.len() {
            metadata.partial = true;
            break;
        }
        let body = &bytes[offset + header_size..offset + box_size];
        match kind {
            b"moov" => parse_moov(body, &mut metadata, is_cancelled)?,
            b"ftyp" if body.len() >= 4 => {
                metadata.container = "mp4";
            }
            _ => {}
        }
        offset = offset.saturating_add(box_size);
    }
    if metadata.duration_seconds.is_none()
        && metadata.width.is_none()
        && metadata.height.is_none()
        && metadata.video_codec.is_none()
        && metadata.audio_codec.is_none()
    {
        metadata.partial = true;
    } else if metadata.duration_seconds.is_some() {
        // Duration alone is a useful metadata-only success for organization.
        metadata.partial = false;
    }
    Ok(metadata)
}

fn parse_moov(
    bytes: &[u8],
    metadata: &mut VideoMetadata,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), ExtractionFailure> {
    let mut offset = 0usize;
    let mut boxes = 0usize;
    while offset + 8 <= bytes.len() && boxes < MAX_BOXES {
        if is_cancelled() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "video metadata extraction was cancelled",
            ));
        }
        boxes += 1;
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if size < 8 || offset + size > bytes.len() {
            metadata.partial = true;
            break;
        }
        let body = &bytes[offset + 8..offset + size];
        match kind {
            b"mvhd" => parse_mvhd(body, metadata),
            b"trak" => parse_trak(body, metadata),
            _ => {}
        }
        offset += size;
    }
    Ok(())
}

fn parse_mvhd(body: &[u8], metadata: &mut VideoMetadata) {
    if body.is_empty() {
        return;
    }
    let version = body[0];
    if version == 0 && body.len() >= 20 {
        let timescale = u32::from_be_bytes([body[12], body[13], body[14], body[15]]);
        let duration = u32::from_be_bytes([body[16], body[17], body[18], body[19]]);
        if timescale > 0 {
            metadata.duration_seconds = Some(f64::from(duration) / f64::from(timescale));
        }
        if body.len() >= 12 {
            let creation = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
            metadata.creation_time = mp4_epoch_to_rfc3339(creation);
        }
    } else if version == 1 && body.len() >= 32 {
        let timescale = u32::from_be_bytes([body[20], body[21], body[22], body[23]]);
        let duration = u64::from_be_bytes([
            body[24], body[25], body[26], body[27], body[28], body[29], body[30], body[31],
        ]);
        if timescale > 0 {
            metadata.duration_seconds = Some(duration as f64 / f64::from(timescale));
        }
    }
}

fn parse_trak(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if size < 8 || offset + size > bytes.len() {
            break;
        }
        let body = &bytes[offset + 8..offset + size];
        if kind == b"mdia" {
            parse_mdia(body, metadata);
        }
        offset += size;
    }
}

fn parse_mdia(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if size < 8 || offset + size > bytes.len() {
            break;
        }
        let body = &bytes[offset + 8..offset + size];
        if kind == b"minf" {
            parse_minf(body, metadata);
        }
        offset += size;
    }
}

fn parse_minf(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if size < 8 || offset + size > bytes.len() {
            break;
        }
        let body = &bytes[offset + 8..offset + size];
        if kind == b"stbl" {
            parse_stbl(body, metadata);
        }
        offset += size;
    }
}

fn parse_stbl(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        if size < 8 || offset + size > bytes.len() {
            break;
        }
        let body = &bytes[offset + 8..offset + size];
        if kind == b"stsd" {
            parse_stsd(body, metadata);
        }
        offset += size;
    }
}

fn parse_stsd(body: &[u8], metadata: &mut VideoMetadata) {
    if body.len() < 16 {
        return;
    }
    // version(1)+flags(3)+entry_count(4)+sample_entry
    let entry = &body[8..];
    if entry.len() < 8 {
        return;
    }
    let codec = std::str::from_utf8(&entry[4..8])
        .unwrap_or("unknown")
        .to_owned();
    // VisualSampleEntry layout includes width/height at +32/+34 from entry start
    // after reserved/data-reference fields (8 + 6 + 2 + 2*3*4 = 32).
    if entry.len() >= 36 {
        let width = u16::from_be_bytes([entry[32], entry[33]]);
        let height = u16::from_be_bytes([entry[34], entry[35]]);
        if width > 0 && height > 0 {
            metadata.width = Some(u32::from(width));
            metadata.height = Some(u32::from(height));
            metadata.video_codec = Some(codec);
            return;
        }
    }
    if metadata.audio_codec.is_none() {
        metadata.audio_codec = Some(codec);
    }
}

fn mp4_epoch_to_rfc3339(seconds_since_1904: u32) -> Option<String> {
    // MP4 epoch is 1904-01-01; Unix epoch offset includes 17 leap days.
    const OFFSET: u64 = 2_082_844_800;
    let unix = u64::from(seconds_since_1904).checked_sub(OFFSET)?;
    // Keep conversion dependency-free: emit an explicit Unix-second timestamp.
    Some(format!("unix:{unix}"))
}

fn parse_matroska_metadata(
    bytes: &[u8],
    is_cancelled: &dyn Fn() -> bool,
) -> Result<VideoMetadata, ExtractionFailure> {
    let mut metadata = VideoMetadata {
        container: "matroska",
        ..VideoMetadata::default()
    };
    let mut offset = 0usize;
    let mut elements = 0usize;
    while offset < bytes.len() && elements < MAX_EBML_ELEMENTS {
        if is_cancelled() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "video metadata extraction was cancelled",
            ));
        }
        elements += 1;
        let Some((id, id_len)) = read_ebml_id(&bytes[offset..]) else {
            metadata.partial = true;
            break;
        };
        let Some((size, size_len)) = read_ebml_size(&bytes[offset + id_len..]) else {
            metadata.partial = true;
            break;
        };
        let header = id_len + size_len;
        if offset + header > bytes.len() {
            metadata.partial = true;
            break;
        }
        let end = match size {
            Some(value) => offset + header + value,
            None => bytes.len(),
        };
        if end > bytes.len() {
            metadata.partial = true;
            break;
        }
        let body = &bytes[offset + header..end];
        match id {
            0x1853_8067 => {
                // Segment
                let nested = parse_matroska_metadata(body, is_cancelled)?;
                merge_video_metadata(&mut metadata, nested);
            }
            0x1549_A966 => parse_segment_info(body, &mut metadata),
            0x1654_AE6B => parse_tracks(body, &mut metadata),
            0x1F43_B675 => {
                // Cluster — stop scanning media payloads.
                break;
            }
            _ => {}
        }
        offset = end;
    }
    Ok(metadata)
}

fn merge_video_metadata(target: &mut VideoMetadata, source: VideoMetadata) {
    if target.duration_seconds.is_none() {
        target.duration_seconds = source.duration_seconds;
    }
    if target.width.is_none() {
        target.width = source.width;
    }
    if target.height.is_none() {
        target.height = source.height;
    }
    if target.video_codec.is_none() {
        target.video_codec = source.video_codec;
    }
    if target.audio_codec.is_none() {
        target.audio_codec = source.audio_codec;
    }
    target.partial |= source.partial;
}

fn parse_segment_info(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let Some((id, id_len)) = read_ebml_id(&bytes[offset..]) else {
            break;
        };
        let Some((size, size_len)) = read_ebml_size(&bytes[offset + id_len..]) else {
            break;
        };
        let header = id_len + size_len;
        let payload_size = size.unwrap_or(0);
        if offset + header + payload_size > bytes.len() {
            break;
        }
        let body = &bytes[offset + header..offset + header + payload_size];
        if id == 0x4489 && body.len() == 8 {
            // Duration in matroska is a float in timescale units; default timescale ns.
            let bits = u64::from_be_bytes([
                body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
            ]);
            let duration = f64::from_bits(bits);
            if duration.is_finite() && duration > 0.0 {
                metadata.duration_seconds = Some(duration / 1_000_000_000.0);
            }
        }
        offset += header + payload_size;
    }
}

fn parse_tracks(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let Some((id, id_len)) = read_ebml_id(&bytes[offset..]) else {
            break;
        };
        let Some((size, size_len)) = read_ebml_size(&bytes[offset + id_len..]) else {
            break;
        };
        let header = id_len + size_len;
        let payload_size = size.unwrap_or(0);
        if offset + header + payload_size > bytes.len() {
            break;
        }
        let body = &bytes[offset + header..offset + header + payload_size];
        if id == 0xAE {
            parse_track_entry(body, metadata);
        }
        offset += header + payload_size;
    }
}

fn parse_track_entry(bytes: &[u8], metadata: &mut VideoMetadata) {
    let mut offset = 0usize;
    let mut codec = None;
    let mut track_type = None;
    let mut width = None;
    let mut height = None;
    while offset < bytes.len() {
        let Some((id, id_len)) = read_ebml_id(&bytes[offset..]) else {
            break;
        };
        let Some((size, size_len)) = read_ebml_size(&bytes[offset + id_len..]) else {
            break;
        };
        let header = id_len + size_len;
        let payload_size = size.unwrap_or(0);
        if offset + header + payload_size > bytes.len() {
            break;
        }
        let body = &bytes[offset + header..offset + header + payload_size];
        match id {
            0x83 if !body.is_empty() => track_type = Some(body[0]),
            0x86 => codec = std::str::from_utf8(body).ok().map(str::to_owned),
            0xE0 => {
                let mut nested = 0usize;
                while nested < body.len() {
                    let Some((nid, nlen)) = read_ebml_id(&body[nested..]) else {
                        break;
                    };
                    let Some((nsize, nslen)) = read_ebml_size(&body[nested + nlen..]) else {
                        break;
                    };
                    let nheader = nlen + nslen;
                    let npayload = nsize.unwrap_or(0);
                    if nested + nheader + npayload > body.len() {
                        break;
                    }
                    let nbody = &body[nested + nheader..nested + nheader + npayload];
                    if nid == 0xB0 && npayload <= 4 {
                        width = Some(read_uint(nbody));
                    }
                    if nid == 0xBA && npayload <= 4 {
                        height = Some(read_uint(nbody));
                    }
                    nested += nheader + npayload;
                }
            }
            _ => {}
        }
        offset += header + payload_size;
    }
    match track_type {
        Some(1) => {
            metadata.video_codec = codec;
            metadata.width = width;
            metadata.height = height;
        }
        Some(2) if metadata.audio_codec.is_none() => {
            metadata.audio_codec = codec;
        }
        _ => {}
    }
}

fn read_uint(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0_u32, |acc, byte| (acc << 8) | u32::from(*byte))
}

fn read_ebml_id(bytes: &[u8]) -> Option<(u32, usize)> {
    let first = *bytes.first()?;
    let len = first.leading_zeros() as usize + 1;
    if !(1..=4).contains(&len) || bytes.len() < len {
        return None;
    }
    let mut value = 0_u32;
    for byte in &bytes[..len] {
        value = (value << 8) | u32::from(*byte);
    }
    Some((value, len))
}

fn read_ebml_size(bytes: &[u8]) -> Option<(Option<usize>, usize)> {
    let first = *bytes.first()?;
    let len = first.leading_zeros() as usize + 1;
    if !(1..=8).contains(&len) || bytes.len() < len {
        return None;
    }
    let mask = 0xFF_u8 >> len;
    let mut value = u64::from(first & mask);
    for byte in &bytes[1..len] {
        value = (value << 8) | u64::from(*byte);
    }
    // All-ones means unknown size.
    let unknown = (1_u64 << (7 * len)) - 1;
    if value == unknown {
        Some((None, len))
    } else {
        Some((Some(value as usize), len))
    }
}
