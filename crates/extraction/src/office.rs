use crate::{
    engine::{ContentExtractor, ExtractionContext, ExtractionInput, office_extractor_type},
    model::{
        ContentKind, ErrorCategory, ExtractionFailure, ExtractionPayload, ExtractionStatus,
        ExtractorType, OfficeKind,
    },
};
use quick_xml::{Reader, events::Event};
use std::io::{Cursor, Read, Seek};
use zip::ZipArchive;

#[derive(Debug, Default)]
pub struct OfficeExtractor;

impl ContentExtractor for OfficeExtractor {
    fn can_handle(&self, kind: ContentKind) -> bool {
        matches!(kind, ContentKind::Office(_))
    }

    fn extractor_type(&self, kind: ContentKind) -> ExtractorType {
        match kind {
            ContentKind::Office(kind) => office_extractor_type(kind),
            _ => ExtractorType::Docx,
        }
    }

    fn extract(
        &self,
        input: &ExtractionInput<'_>,
        context: &ExtractionContext<'_>,
    ) -> Result<ExtractionPayload, ExtractionFailure> {
        if input.input_truncated {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                "Office document exceeds the configured input limit",
            ));
        }
        let cursor = Cursor::new(input.bytes);
        let mut archive = ZipArchive::new(cursor).map_err(|_| {
            ExtractionFailure::failed(
                ErrorCategory::Corrupt,
                "Office document is not a readable Open XML archive",
            )
        })?;
        let names = inspect_archive(&mut archive, context)?;
        let actual_kind = office_kind(&names).ok_or_else(|| {
            ExtractionFailure::failed(
                ErrorCategory::TypeMismatch,
                "archive does not contain a supported Office document structure",
            )
        })?;
        let expected_kind = match input.detection.content_kind {
            ContentKind::Office(kind) => kind,
            _ => {
                return Err(ExtractionFailure::failed(
                    ErrorCategory::TypeMismatch,
                    "file type changed before Office extraction",
                ));
            }
        };
        if actual_kind != expected_kind {
            return Err(ExtractionFailure::failed(
                ErrorCategory::TypeMismatch,
                "Office extension does not match the document structure",
            ));
        }

        let mut uncompressed_bytes_read = 0_u64;
        match actual_kind {
            OfficeKind::Docx => {
                extract_docx(&mut archive, &names, context, &mut uncompressed_bytes_read)
            }
            OfficeKind::Xlsx => {
                extract_xlsx(&mut archive, &names, context, &mut uncompressed_bytes_read)
            }
            OfficeKind::Pptx => {
                extract_pptx(&mut archive, &names, context, &mut uncompressed_bytes_read)
            }
        }
    }
}

fn inspect_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    context: &ExtractionContext<'_>,
) -> Result<Vec<String>, ExtractionFailure> {
    if archive.len() > context.limits.max_archive_entries {
        return Err(ExtractionFailure::skipped(
            ErrorCategory::TooManyEntries,
            "Office archive contains too many entries",
        ));
    }
    let mut names = Vec::with_capacity(archive.len());
    let mut total_uncompressed = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| {
            ExtractionFailure::failed(
                ErrorCategory::Corrupt,
                "Office archive directory is malformed",
            )
        })?;
        let name = entry.name();
        if !safe_archive_path(name) {
            return Err(ExtractionFailure::failed(
                ErrorCategory::ArchiveTraversal,
                "Office archive contains an unsafe entry path",
            ));
        }
        total_uncompressed = total_uncompressed.saturating_add(entry.size());
        if total_uncompressed > context.limits.max_uncompressed_bytes {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::TooLarge,
                "Office archive exceeds the decompression safety limit",
            ));
        }
        if suspicious_ratio(
            entry.size(),
            entry.compressed_size(),
            context.limits.max_compression_ratio,
        ) {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::PotentialArchiveBomb,
                "Office archive has a suspicious compression ratio",
            ));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn office_kind(names: &[String]) -> Option<OfficeKind> {
    if names.iter().any(|name| name == "word/document.xml") {
        Some(OfficeKind::Docx)
    } else if names.iter().any(|name| name == "xl/workbook.xml")
        && names
            .iter()
            .any(|name| name.starts_with("xl/worksheets/sheet"))
    {
        Some(OfficeKind::Xlsx)
    } else if names
        .iter()
        .any(|name| is_numbered_xml(name, "ppt/slides/slide"))
    {
        Some(OfficeKind::Pptx)
    } else {
        None
    }
}

fn extract_docx<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &[String],
    context: &ExtractionContext<'_>,
    bytes_read: &mut u64,
) -> Result<ExtractionPayload, ExtractionFailure> {
    let mut selected = names
        .iter()
        .filter(|name| {
            name.as_str() == "word/document.xml"
                || is_numbered_xml(name, "word/header")
                || is_numbered_xml(name, "word/footer")
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by_key(|name| {
        if name == "word/document.xml" {
            0
        } else if name.starts_with("word/header") {
            1
        } else {
            2
        }
    });

    let mut text = String::new();
    for name in selected {
        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "Office extraction was cancelled",
            ));
        }
        let xml = read_entry(archive, &name, context, bytes_read)?;
        append_section(&mut text, &extract_word_text(&xml)?);
    }
    let mut payload = ExtractionPayload::success(ExtractorType::Docx);
    payload.text = text;
    payload.metadata = serde_json::json!({
        "format": "docx",
        "uncompressedBytesRead": *bytes_read,
        "macrosExecuted": false,
        "embeddedObjectsExecuted": false,
        "network": false
    });
    Ok(payload)
}

fn extract_xlsx<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &[String],
    context: &ExtractionContext<'_>,
    bytes_read: &mut u64,
) -> Result<ExtractionPayload, ExtractionFailure> {
    let shared_strings = if names.iter().any(|name| name == "xl/sharedStrings.xml") {
        let xml = read_entry(archive, "xl/sharedStrings.xml", context, bytes_read)?;
        parse_shared_strings(&xml)?
    } else {
        Vec::new()
    };
    let sheet_names = if names.iter().any(|name| name == "xl/workbook.xml") {
        let xml = read_entry(archive, "xl/workbook.xml", context, bytes_read)?;
        parse_sheet_names(&xml)?
    } else {
        Vec::new()
    };
    let mut worksheet_entries = names
        .iter()
        .filter(|name| is_numbered_xml(name, "xl/worksheets/sheet"))
        .cloned()
        .collect::<Vec<_>>();
    worksheet_entries.sort_by_key(|name| numbered_xml_index(name).unwrap_or(u32::MAX));

    let mut text = String::new();
    let mut cells_seen = 0_usize;
    let mut formulas_skipped = 0_usize;
    let mut truncated = false;
    for (sheet_index, entry_name) in worksheet_entries.iter().enumerate() {
        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "spreadsheet extraction was cancelled",
            ));
        }
        let xml = read_entry(archive, entry_name, context, bytes_read)?;
        let remaining = context
            .limits
            .max_spreadsheet_cells
            .saturating_sub(cells_seen);
        let sheet = parse_worksheet(&xml, &shared_strings, remaining)?;
        let display_name = sheet_names
            .get(sheet_index)
            .cloned()
            .unwrap_or_else(|| format!("Sheet {}", sheet_index + 1));
        append_section(&mut text, &format!("[Sheet: {display_name}]"));
        append_section(&mut text, &sheet.values.join("\n"));
        cells_seen = cells_seen.saturating_add(sheet.cells_seen);
        formulas_skipped = formulas_skipped.saturating_add(sheet.formulas_skipped);
        if sheet.truncated {
            truncated = true;
            break;
        }
    }

    let mut payload = ExtractionPayload::success(ExtractorType::Xlsx);
    payload.text = text;
    payload.sheet_count = Some(u32::try_from(worksheet_entries.len()).unwrap_or(u32::MAX));
    payload.metadata = serde_json::json!({
        "format": "xlsx",
        "sheetNames": sheet_names,
        "cellCount": cells_seen,
        "formulasSkipped": formulas_skipped,
        "uncompressedBytesRead": *bytes_read,
        "formulasExecuted": false,
        "macrosExecuted": false,
        "network": false
    });
    if truncated {
        payload.status = ExtractionStatus::Partial;
        payload.truncated = true;
        payload.error_category = Some(ErrorCategory::TooManyCells);
        payload.error_message = Some(format!(
            "spreadsheet extraction stopped at the configured {}-cell limit",
            context.limits.max_spreadsheet_cells
        ));
    }
    Ok(payload)
}

fn extract_pptx<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &[String],
    context: &ExtractionContext<'_>,
    bytes_read: &mut u64,
) -> Result<ExtractionPayload, ExtractionFailure> {
    let mut slides = names
        .iter()
        .filter(|name| is_numbered_xml(name, "ppt/slides/slide"))
        .cloned()
        .collect::<Vec<_>>();
    slides.sort_by_key(|name| numbered_xml_index(name).unwrap_or(u32::MAX));
    let total_slides = u32::try_from(slides.len()).unwrap_or(u32::MAX);
    let take_count = usize::try_from(context.limits.max_pages).unwrap_or(usize::MAX);
    let mut text = String::new();
    for (index, name) in slides.iter().take(take_count).enumerate() {
        if (context.is_cancelled)() {
            return Err(ExtractionFailure::skipped(
                ErrorCategory::Cancelled,
                "presentation extraction was cancelled",
            ));
        }
        let xml = read_entry(archive, name, context, bytes_read)?;
        append_section(&mut text, &format!("[Slide {}]", index + 1));
        append_section(&mut text, &extract_word_text(&xml)?);
    }

    let mut payload = ExtractionPayload::success(ExtractorType::Pptx);
    payload.text = text;
    payload.slide_count = Some(total_slides);
    payload.metadata = serde_json::json!({
        "format": "pptx",
        "slideCount": total_slides,
        "uncompressedBytesRead": *bytes_read,
        "embeddedObjectsExecuted": false,
        "network": false
    });
    if total_slides > context.limits.max_pages {
        payload.status = ExtractionStatus::Partial;
        payload.truncated = true;
        payload.error_category = Some(ErrorCategory::TooManyPages);
        payload.error_message = Some(format!(
            "presentation extraction stopped at the configured {}-slide limit",
            context.limits.max_pages
        ));
    }
    Ok(payload)
}

fn read_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    context: &ExtractionContext<'_>,
    total_read: &mut u64,
) -> Result<Vec<u8>, ExtractionFailure> {
    let mut entry = archive.by_name(name).map_err(|_| {
        ExtractionFailure::failed(
            ErrorCategory::Corrupt,
            "required Office document part is missing",
        )
    })?;
    let next_total = total_read.saturating_add(entry.size());
    if next_total > context.limits.max_uncompressed_bytes {
        return Err(ExtractionFailure::skipped(
            ErrorCategory::TooLarge,
            "Office XML parts exceed the decompression safety limit",
        ));
    }
    let declared_size = entry.size();
    let capacity = usize::try_from(declared_size).map_err(|_| {
        ExtractionFailure::skipped(
            ErrorCategory::TooLarge,
            "Office XML part is too large for this platform",
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    entry
        .by_ref()
        .take(declared_size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            ExtractionFailure::failed(
                ErrorCategory::Corrupt,
                "Office XML part could not be decompressed",
            )
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > declared_size {
        return Err(ExtractionFailure::failed(
            ErrorCategory::Corrupt,
            "Office XML part expanded beyond its declared size",
        ));
    }
    *total_read = next_total;
    Ok(bytes)
}

fn extract_word_text(xml: &[u8]) -> Result<String, ExtractionFailure> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut output = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if local_name(event.name().as_ref()) == b"p" => {
                if !output.ends_with('\n') && !output.is_empty() {
                    output.push('\n');
                }
            }
            Ok(Event::Empty(event))
                if matches!(local_name(event.name().as_ref()), b"br" | b"tab") =>
            {
                output.push(if local_name(event.name().as_ref()) == b"tab" {
                    '\t'
                } else {
                    '\n'
                });
            }
            Ok(Event::Text(value)) => {
                let decoded = value.decode().map_err(|_| malformed_xml())?;
                let unescaped =
                    quick_xml::escape::unescape(&decoded).map_err(|_| malformed_xml())?;
                if !unescaped.trim().is_empty() {
                    if !output.is_empty()
                        && !output.ends_with([' ', '\n', '\t'])
                        && !unescaped.starts_with([' ', '\n', '\t'])
                    {
                        output.push(' ');
                    }
                    output.push_str(&unescaped);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(malformed_xml()),
        }
    }
    Ok(output.trim().to_owned())
}

fn parse_shared_strings(xml: &[u8]) -> Result<Vec<String>, ExtractionFailure> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut values = Vec::new();
    let mut current = String::new();
    let mut inside_item = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if local_name(event.name().as_ref()) == b"si" => {
                current.clear();
                inside_item = true;
            }
            Ok(Event::Text(value)) if inside_item => {
                let decoded = value.decode().map_err(|_| malformed_xml())?;
                let unescaped =
                    quick_xml::escape::unescape(&decoded).map_err(|_| malformed_xml())?;
                current.push_str(&unescaped);
            }
            Ok(Event::End(event)) if local_name(event.name().as_ref()) == b"si" => {
                values.push(current.clone());
                inside_item = false;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(malformed_xml()),
        }
    }
    Ok(values)
}

fn parse_sheet_names(xml: &[u8]) -> Result<Vec<String>, ExtractionFailure> {
    let mut reader = Reader::from_reader(xml);
    let mut names = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event) | Event::Empty(event))
                if local_name(event.name().as_ref()) == b"sheet" =>
            {
                for attribute in event.attributes().flatten() {
                    if local_name(attribute.key.as_ref()) == b"name" {
                        let raw = String::from_utf8_lossy(attribute.value.as_ref());
                        let value =
                            quick_xml::escape::unescape(&raw).map_err(|_| malformed_xml())?;
                        names.push(value.into_owned());
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(malformed_xml()),
        }
    }
    Ok(names)
}

#[derive(Debug, Default)]
struct WorksheetData {
    values: Vec<String>,
    cells_seen: usize,
    formulas_skipped: usize,
    truncated: bool,
}

fn parse_worksheet(
    xml: &[u8],
    shared_strings: &[String],
    max_cells: usize,
) -> Result<WorksheetData, ExtractionFailure> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut output = WorksheetData::default();
    let mut inside_cell = false;
    let mut inside_value = false;
    let mut has_formula = false;
    let mut cell_type = None::<String>;
    let mut value = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if local_name(event.name().as_ref()) == b"c" => {
                if output.cells_seen >= max_cells {
                    output.truncated = true;
                    break;
                }
                output.cells_seen = output.cells_seen.saturating_add(1);
                inside_cell = true;
                has_formula = false;
                value.clear();
                cell_type = event.attributes().flatten().find_map(|attribute| {
                    (local_name(attribute.key.as_ref()) == b"t")
                        .then(|| String::from_utf8_lossy(attribute.value.as_ref()).into_owned())
                });
            }
            Ok(Event::Empty(event)) if local_name(event.name().as_ref()) == b"c" => {
                if output.cells_seen >= max_cells {
                    output.truncated = true;
                    break;
                }
                output.cells_seen = output.cells_seen.saturating_add(1);
            }
            Ok(Event::Start(event))
                if inside_cell && matches!(local_name(event.name().as_ref()), b"v" | b"t") =>
            {
                inside_value = true;
            }
            Ok(Event::Start(event)) if inside_cell && local_name(event.name().as_ref()) == b"f" => {
                has_formula = true;
                output.formulas_skipped = output.formulas_skipped.saturating_add(1);
            }
            Ok(Event::Text(text)) if inside_cell && inside_value => {
                let decoded = text.decode().map_err(|_| malformed_xml())?;
                let unescaped =
                    quick_xml::escape::unescape(&decoded).map_err(|_| malformed_xml())?;
                value.push_str(&unescaped);
            }
            Ok(Event::End(event)) if matches!(local_name(event.name().as_ref()), b"v" | b"t") => {
                inside_value = false;
            }
            Ok(Event::End(event)) if local_name(event.name().as_ref()) == b"c" => {
                if !has_formula && !value.is_empty() {
                    let rendered = match cell_type.as_deref() {
                        Some("s") => value
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| shared_strings.get(index))
                            .cloned()
                            .unwrap_or_default(),
                        Some("b") => {
                            if value == "1" {
                                "TRUE".to_owned()
                            } else {
                                "FALSE".to_owned()
                            }
                        }
                        _ => value.clone(),
                    };
                    if !rendered.is_empty() {
                        output.values.push(rendered);
                    }
                }
                inside_cell = false;
                inside_value = false;
                value.clear();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(malformed_xml()),
        }
    }
    Ok(output)
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn append_section(output: &mut String, section: &str) {
    let section = section.trim();
    if section.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(section);
}

fn malformed_xml() -> ExtractionFailure {
    ExtractionFailure::failed(ErrorCategory::Corrupt, "Office XML content is malformed")
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

fn is_numbered_xml(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_suffix(".xml"))
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn numbered_xml_index(name: &str) -> Option<u32> {
    let digits = name
        .strip_suffix(".xml")?
        .rsplit(|character: char| !character.is_ascii_digit())
        .next()?;
    digits.parse().ok()
}

#[cfg(test)]
pub(crate) fn archive_path_is_safe(name: &str) -> bool {
    safe_archive_path(name)
}
