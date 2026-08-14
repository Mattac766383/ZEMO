use std::collections::HashSet;

pub const DEFAULT_MAX_PATH_UTF16: usize = 240;
pub const DEFAULT_MAX_SEGMENT_UTF16: usize = 80;
pub const DEFAULT_MAX_FILENAME_UTF16: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualPathPolicy {
    pub maximum_depth: usize,
    pub maximum_path_utf16: usize,
    pub maximum_segment_utf16: usize,
    pub maximum_filename_utf16: usize,
}

impl Default for VirtualPathPolicy {
    fn default() -> Self {
        Self {
            maximum_depth: 6,
            maximum_path_utf16: DEFAULT_MAX_PATH_UTF16,
            maximum_segment_utf16: DEFAULT_MAX_SEGMENT_UTF16,
            maximum_filename_utf16: DEFAULT_MAX_FILENAME_UTF16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VirtualPathError {
    #[error("a virtual path segment is empty")]
    Empty,
    #[error("a virtual path segment contains traversal or a separator")]
    TraversalOrSeparator,
    #[error("a virtual path contains an absolute or drive-prefixed segment")]
    AbsoluteOrDrivePrefix,
    #[error("a virtual path segment contains a Windows-invalid character")]
    InvalidCharacter,
    #[error("a virtual path segment uses a reserved Windows device name")]
    ReservedWindowsName,
    #[error("the virtual path is deeper than policy permits")]
    TooDeep,
    #[error("the virtual path is longer than policy permits")]
    TooLong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedValue {
    pub value: String,
    pub changed: bool,
}

impl VirtualPathPolicy {
    #[must_use]
    pub fn sanitize_machine_segment(&self, input: &str) -> SanitizedValue {
        sanitize_component(input, self.maximum_segment_utf16, false)
    }

    #[must_use]
    pub fn sanitize_machine_filename(&self, input: &str) -> SanitizedValue {
        sanitize_component(input, self.maximum_filename_utf16, true)
    }

    pub fn validate_user_destination(&self, segments: &[String]) -> Result<(), VirtualPathError> {
        if segments.len() > self.maximum_depth {
            return Err(VirtualPathError::TooDeep);
        }
        for segment in segments {
            validate_component(segment, self.maximum_segment_utf16)?;
        }
        Ok(())
    }

    pub fn validate_user_filename(&self, filename: &str) -> Result<(), VirtualPathError> {
        validate_component(filename, self.maximum_filename_utf16)
    }

    #[must_use]
    pub fn path_length_utf16(&self, destination: &[String], filename: &str) -> usize {
        destination
            .iter()
            .map(|segment| segment.encode_utf16().count())
            .sum::<usize>()
            .saturating_add(filename.encode_utf16().count())
            .saturating_add(destination.len())
    }

    #[must_use]
    pub fn fit_machine_path(
        &self,
        destination: &[String],
        filename: &str,
    ) -> (Vec<String>, String, bool, bool) {
        let mut changed = false;
        let mut safe_destination = destination
            .iter()
            .map(|segment| {
                let sanitized = self.sanitize_machine_segment(segment);
                changed |= sanitized.changed;
                sanitized.value
            })
            .collect::<Vec<_>>();
        if safe_destination.len() > self.maximum_depth {
            safe_destination.truncate(self.maximum_depth);
            changed = true;
        }
        let sanitized_name = self.sanitize_machine_filename(filename);
        changed |= sanitized_name.changed;
        let mut safe_name = sanitized_name.value;

        if self.path_length_utf16(&safe_destination, &safe_name) > self.maximum_path_utf16 {
            for index in (0..safe_destination.len()).rev() {
                if self.path_length_utf16(&safe_destination, &safe_name) <= self.maximum_path_utf16
                {
                    break;
                }
                safe_destination[index] = truncate_utf16(&safe_destination[index], 32);
                changed = true;
            }
        }
        if self.path_length_utf16(&safe_destination, &safe_name) > self.maximum_path_utf16 {
            let destination_length = safe_destination
                .iter()
                .map(|segment| segment.encode_utf16().count())
                .sum::<usize>()
                .saturating_add(safe_destination.len());
            let remaining = self
                .maximum_path_utf16
                .saturating_sub(destination_length)
                .max(16);
            safe_name = truncate_filename_preserving_extension(&safe_name, remaining);
            changed = true;
        }
        let valid =
            self.path_length_utf16(&safe_destination, &safe_name) <= self.maximum_path_utf16;
        (safe_destination, safe_name, changed, valid)
    }
}

pub fn validate_component(input: &str, maximum_utf16: usize) -> Result<(), VirtualPathError> {
    if input.is_empty() || input.trim().is_empty() {
        return Err(VirtualPathError::Empty);
    }
    if input == "." || input == ".." || input.contains('/') || input.contains('\\') {
        return Err(VirtualPathError::TraversalOrSeparator);
    }
    if input.starts_with('/') || input.starts_with('\\') || has_drive_prefix(input) {
        return Err(VirtualPathError::AbsoluteOrDrivePrefix);
    }
    if input.chars().any(is_windows_invalid) || input.ends_with([' ', '.']) {
        return Err(VirtualPathError::InvalidCharacter);
    }
    if is_reserved_windows_name(input) {
        return Err(VirtualPathError::ReservedWindowsName);
    }
    if input.encode_utf16().count() > maximum_utf16 {
        return Err(VirtualPathError::TooLong);
    }
    Ok(())
}

#[must_use]
pub fn collision_key(destination: &[String], filename: &str) -> String {
    destination
        .iter()
        .chain(std::iter::once(&filename.to_owned()))
        .map(|component| component.to_lowercase())
        .collect::<Vec<_>>()
        .join("\\")
}

#[must_use]
pub fn collision_name(filename: &str, ordinal: usize, maximum_utf16: usize) -> String {
    let (stem, extension) = split_extension(filename);
    let suffix = format!("_{ordinal}");
    let extension_length = extension
        .as_ref()
        .map_or(0, |value| value.encode_utf16().count().saturating_add(1));
    let stem_limit = maximum_utf16
        .saturating_sub(suffix.encode_utf16().count())
        .saturating_sub(extension_length)
        .max(1);
    let stem = truncate_utf16(stem, stem_limit);
    match extension {
        Some(extension) => format!("{stem}{suffix}.{extension}"),
        None => format!("{stem}{suffix}"),
    }
}

fn sanitize_component(
    input: &str,
    maximum_utf16: usize,
    preserve_extension: bool,
) -> SanitizedValue {
    let trimmed = input.trim();
    let mut output = String::with_capacity(trimmed.len().min(maximum_utf16));
    let mut previous_replacement = false;
    for character in trimmed.chars() {
        let replacement = is_windows_invalid(character) || matches!(character, '/' | '\\');
        if replacement {
            if !previous_replacement {
                output.push('_');
            }
        } else {
            output.push(character);
        }
        previous_replacement = replacement;
    }
    while output.ends_with([' ', '.']) {
        output.pop();
    }
    if output.is_empty() || output == "." || output == ".." {
        output = "Unclassified".to_owned();
    }
    if has_drive_prefix(&output) {
        output = output.replacen(':', "_", 1);
    }
    if is_reserved_windows_name(&output) {
        output.insert(0, '_');
    }
    let before_truncation = output.clone();
    output = if preserve_extension {
        truncate_filename_preserving_extension(&output, maximum_utf16)
    } else {
        truncate_utf16(&output, maximum_utf16)
    };
    SanitizedValue {
        changed: output != input || output != before_truncation,
        value: output,
    }
}

fn truncate_filename_preserving_extension(input: &str, maximum_utf16: usize) -> String {
    if input.encode_utf16().count() <= maximum_utf16 {
        return input.to_owned();
    }
    let (stem, extension) = split_extension(input);
    let Some(extension) = extension else {
        return truncate_utf16(input, maximum_utf16);
    };
    let extension_length = extension.encode_utf16().count().saturating_add(1);
    if extension_length >= maximum_utf16 {
        return truncate_utf16(input, maximum_utf16);
    }
    format!(
        "{}.{}",
        truncate_utf16(stem, maximum_utf16 - extension_length),
        extension
    )
}

fn truncate_utf16(input: &str, maximum: usize) -> String {
    let mut used = 0_usize;
    input
        .chars()
        .take_while(|character| {
            let units = character.len_utf16();
            if used.saturating_add(units) > maximum {
                false
            } else {
                used += units;
                true
            }
        })
        .collect()
}

fn split_extension(filename: &str) -> (&str, Option<&str>) {
    let Some((stem, extension)) = filename.rsplit_once('.') else {
        return (filename, None);
    };
    if stem.is_empty() || extension.is_empty() || extension.encode_utf16().count() > 16 {
        (filename, None)
    } else {
        (stem, Some(extension))
    }
}

fn has_drive_prefix(input: &str) -> bool {
    let mut characters = input.chars();
    matches!(
        (characters.next(), characters.next()),
        (Some(first), Some(':')) if first.is_ascii_alphabetic()
    )
}

fn is_windows_invalid(character: char) -> bool {
    character == '\0'
        || character.is_control()
        || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

fn is_reserved_windows_name(input: &str) -> bool {
    let stem = input
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    let fixed = HashSet::from(["CON", "PRN", "AUX", "NUL"]);
    fixed.contains(stem.as_str())
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_user_traversal_and_drive_prefixes() {
        let policy = VirtualPathPolicy::default();
        assert_eq!(
            policy.validate_user_destination(&["..".to_owned()]),
            Err(VirtualPathError::TraversalOrSeparator)
        );
        assert_eq!(
            policy.validate_user_destination(&["C:".to_owned()]),
            Err(VirtualPathError::AbsoluteOrDrivePrefix)
        );
    }

    #[test]
    fn machine_names_are_windows_safe_and_keep_extensions() {
        let policy = VirtualPathPolicy::default();
        let sanitized = policy.sanitize_machine_filename("CON<report>.pdf");
        assert_eq!(sanitized.value, "CON_report_.pdf");
        assert!(sanitized.changed);
        assert!(validate_component(&sanitized.value, DEFAULT_MAX_FILENAME_UTF16).is_ok());

        let reserved = policy.sanitize_machine_filename("NUL.txt");
        assert_eq!(reserved.value, "_NUL.txt");
    }

    #[test]
    fn collision_keys_are_windows_case_insensitive() {
        assert_eq!(
            collision_key(&["Business".into()], "Invoice.pdf"),
            collision_key(&["business".into()], "invoice.PDF")
        );
    }

    #[test]
    fn machine_paths_are_shortened_to_the_conservative_windows_budget() {
        let policy = VirtualPathPolicy::default();
        let destination = (0..10)
            .map(|index| format!("{index}_{}", "Long folder ".repeat(30)))
            .collect::<Vec<_>>();
        let filename = format!("{}.pdf", "Long semantic filename ".repeat(30));
        let (destination, filename, changed, valid) =
            policy.fit_machine_path(&destination, &filename);

        assert!(changed);
        assert!(valid);
        assert!(destination.len() <= policy.maximum_depth);
        assert!(policy.path_length_utf16(&destination, &filename) <= policy.maximum_path_utf16);
        assert!(
            destination.iter().all(|segment| validate_component(
                segment,
                policy.maximum_segment_utf16
            )
            .is_ok())
        );
        assert!(validate_component(&filename, policy.maximum_filename_utf16).is_ok());
        assert!(filename.ends_with(".pdf"));
    }
}
