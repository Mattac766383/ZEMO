use crate::model::{ContentKind, FileTypeDetection, OfficeKind};

#[derive(Debug, Clone, Copy)]
struct ExpectedType {
    kind: ContentKind,
    media_type: &'static str,
}

#[must_use]
pub fn detect_file_type(extension: Option<&str>, prefix: &[u8]) -> FileTypeDetection {
    let extension = normalize_extension(extension);
    let expected = extension.as_deref().and_then(expected_type_for_extension);
    let magic = magic_type(prefix);

    let (content_kind, detected_content_type, magic_confirmed, mismatch) = match (expected, magic) {
        (Some(expected), Some((ContentKind::Zip, _)))
            if matches!(expected.kind, ContentKind::Office(_)) =>
        {
            (expected.kind, expected.media_type.to_owned(), true, false)
        }
        (Some(expected), Some((actual_kind, actual_media))) => {
            let compatible = kinds_are_compatible(expected.kind, actual_kind)
                && media_types_are_compatible(expected.media_type, actual_media);
            (
                if compatible {
                    expected.kind
                } else {
                    actual_kind
                },
                actual_media.to_owned(),
                true,
                !compatible,
            )
        }
        (Some(expected), None) => (expected.kind, expected.media_type.to_owned(), false, false),
        (None, Some((kind, media_type))) => (kind, media_type.to_owned(), true, false),
        (None, None) if looks_like_utf8_text(prefix) => {
            (ContentKind::Text, "text/plain".to_owned(), false, false)
        }
        (None, None) => (
            ContentKind::Unknown,
            "application/octet-stream".to_owned(),
            false,
            false,
        ),
    };

    FileTypeDetection {
        extension,
        content_kind,
        detected_content_type,
        magic_confirmed,
        mismatch,
    }
}

fn normalize_extension(extension: Option<&str>) -> Option<String> {
    extension
        .map(str::trim)
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty() && value.len() <= 32)
}

fn expected_type_for_extension(extension: &str) -> Option<ExpectedType> {
    let value = match extension {
        "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "xml" | "log" => ExpectedType {
            kind: ContentKind::Text,
            media_type: "text/plain",
        },
        "pdf" => ExpectedType {
            kind: ContentKind::Pdf,
            media_type: "application/pdf",
        },
        "docx" => ExpectedType {
            kind: ContentKind::Office(OfficeKind::Docx),
            media_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        },
        "xlsx" => ExpectedType {
            kind: ContentKind::Office(OfficeKind::Xlsx),
            media_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        },
        "pptx" => ExpectedType {
            kind: ContentKind::Office(OfficeKind::Pptx),
            media_type: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        },
        "zip" => ExpectedType {
            kind: ContentKind::Zip,
            media_type: "application/zip",
        },
        "png" => ExpectedType {
            kind: ContentKind::Image,
            media_type: "image/png",
        },
        "jpg" | "jpeg" => ExpectedType {
            kind: ContentKind::Image,
            media_type: "image/jpeg",
        },
        "webp" => ExpectedType {
            kind: ContentKind::Image,
            media_type: "image/webp",
        },
        "tif" | "tiff" => ExpectedType {
            kind: ContentKind::Image,
            media_type: "image/tiff",
        },
        "gif" => ExpectedType {
            kind: ContentKind::Image,
            media_type: "image/gif",
        },
        "bmp" => ExpectedType {
            kind: ContentKind::Image,
            media_type: "image/bmp",
        },
        "mp4" => ExpectedType {
            kind: ContentKind::Video,
            media_type: "video/mp4",
        },
        "mov" => ExpectedType {
            kind: ContentKind::Video,
            media_type: "video/quicktime",
        },
        "webm" => ExpectedType {
            kind: ContentKind::Video,
            media_type: "video/webm",
        },
        "mkv" | "avi" => ExpectedType {
            kind: ContentKind::Video,
            media_type: "video/unknown",
        },
        "doc" | "xls" | "ppt" => ExpectedType {
            kind: ContentKind::LegacyOffice,
            media_type: "application/x-ole-storage",
        },
        "exe" | "dll" | "com" | "msi" | "app" | "dmg" | "so" | "dylib" => ExpectedType {
            kind: ContentKind::Executable,
            media_type: "application/x-executable",
        },
        _ => return None,
    };
    Some(value)
}

fn magic_type(prefix: &[u8]) -> Option<(ContentKind, &'static str)> {
    if prefix.starts_with(b"%PDF-") {
        return Some((ContentKind::Pdf, "application/pdf"));
    }
    if prefix.starts_with(&[0x50, 0x4b, 0x03, 0x04])
        || prefix.starts_with(&[0x50, 0x4b, 0x05, 0x06])
        || prefix.starts_with(&[0x50, 0x4b, 0x07, 0x08])
    {
        return Some((ContentKind::Zip, "application/zip"));
    }
    if prefix.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
        return Some((ContentKind::LegacyOffice, "application/x-ole-storage"));
    }
    if prefix.starts_with(b"MZ")
        || prefix.starts_with(&[0x7f, b'E', b'L', b'F'])
        || is_mach_o(prefix)
    {
        return Some((ContentKind::Executable, "application/x-executable"));
    }

    infer::get(prefix).map(|kind| {
        let media_type = kind.mime_type();
        let content_kind = if media_type.starts_with("image/") {
            ContentKind::Image
        } else if media_type.starts_with("video/") {
            ContentKind::Video
        } else if media_type == "application/pdf" {
            ContentKind::Pdf
        } else if media_type == "application/zip" {
            ContentKind::Zip
        } else if is_executable_media_type(media_type) {
            ContentKind::Executable
        } else {
            ContentKind::Unknown
        };
        (content_kind, media_type)
    })
}

fn kinds_are_compatible(expected: ContentKind, actual: ContentKind) -> bool {
    matches!(
        (expected, actual),
        (ContentKind::Text, ContentKind::Text)
            | (ContentKind::Pdf, ContentKind::Pdf)
            | (ContentKind::Zip, ContentKind::Zip)
            | (ContentKind::Image, ContentKind::Image)
            | (ContentKind::Video, ContentKind::Video)
            | (ContentKind::LegacyOffice, ContentKind::LegacyOffice)
            | (ContentKind::Executable, ContentKind::Executable)
    )
}

fn media_types_are_compatible(expected: &str, actual: &str) -> bool {
    expected == actual
        || (expected == "text/plain" && actual.starts_with("text/"))
        || (expected == "video/unknown" && actual.starts_with("video/"))
        || (expected == "application/x-executable" && is_executable_media_type(actual))
}

fn looks_like_utf8_text(prefix: &[u8]) -> bool {
    if prefix.is_empty() || prefix.contains(&0) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(prefix) else {
        return false;
    };
    let controls = text
        .chars()
        .filter(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        .count();
    controls.saturating_mul(100) <= text.chars().count().max(1)
}

fn is_executable_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/x-executable"
            | "application/x-mach-binary"
            | "application/vnd.microsoft.portable-executable"
            | "application/x-msdownload"
    )
}

fn is_mach_o(prefix: &[u8]) -> bool {
    matches!(
        prefix.get(..4),
        Some([0xfe, 0xed, 0xfa, 0xce])
            | Some([0xce, 0xfa, 0xed, 0xfe])
            | Some([0xfe, 0xed, 0xfa, 0xcf])
            | Some([0xcf, 0xfa, 0xed, 0xfe])
            | Some([0xca, 0xfe, 0xba, 0xbe])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_disguised_as_pdf_is_a_mismatch() {
        let detection = detect_file_type(Some("pdf"), b"MZ\x90\x00");
        assert_eq!(detection.content_kind, ContentKind::Executable);
        assert!(detection.mismatch);
    }

    #[test]
    fn extensionless_utf8_is_detected_as_text() {
        let detection = detect_file_type(None, "hello\nworld".as_bytes());
        assert_eq!(detection.content_kind, ContentKind::Text);
        assert!(!detection.magic_confirmed);
    }

    #[test]
    fn invalid_text_still_reaches_the_text_decoder() {
        let detection = detect_file_type(Some("txt"), &[0xff, 0xfe, 0xfd]);
        assert_eq!(detection.content_kind, ContentKind::Text);
        assert!(!detection.mismatch);
    }
}
