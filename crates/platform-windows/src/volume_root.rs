//! Win32 volume-root strings.
//!
//! `GetVolumeInformationW`, `GetVolumeNameForVolumeMountPointW`, `GetDriveTypeW`,
//! and `CreateFileW` on a drive root require a mount point that ends with exactly
//! one backslash (`D:\` or `\\?\D:\`). `PathBuf::push("\\")` after a verbatim
//! prefix (`\\?\D:`) can produce `\\?\D:` or `\\?\D:\\`, both of which yield
//! Win32 error 87 (ERROR_INVALID_PARAMETER).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWindowsDrivePrefix {
    pub letter: u8,
    pub verbatim: bool,
    pub win32_root: String,
}

#[must_use]
pub fn parse_windows_drive_prefix(path: &str) -> Option<ParsedWindowsDrivePrefix> {
    let verbatim = path.starts_with("\\\\?\\");
    let rest = if verbatim {
        path.strip_prefix("\\\\?\\")?
    } else {
        path
    };
    let mut chars = rest.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() || chars.next() != Some(':') {
        return None;
    }
    match chars.next() {
        Some('\\' | '/') => {}
        _ => return None,
    }
    let letter = u8::try_from(letter.to_ascii_uppercase()).ok()?;
    let win32_root = format_win32_drive_root(letter, verbatim)?;
    Some(ParsedWindowsDrivePrefix {
        letter,
        verbatim,
        win32_root,
    })
}

#[must_use]
pub fn format_win32_drive_root(letter: u8, verbatim: bool) -> Option<String> {
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let letter = char::from(letter).to_ascii_uppercase();
    Some(if verbatim {
        format!("\\\\?\\{letter}:\\")
    } else {
        format!("{letter}:\\")
    })
}

#[must_use]
pub fn is_legal_win32_mount_point(mount: &str) -> bool {
    let bytes = mount.as_bytes();
    if bytes.len() < 3 || !mount.ends_with('\\') || mount.ends_with("\\\\") {
        return false;
    }
    if mount.len() == 3 {
        return bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    }
    mount
        .strip_prefix("\\\\?\\")
        .is_some_and(|rest| rest.len() == 3 && is_legal_win32_mount_point(rest))
}

#[cfg(test)]
mod tests {
    use super::{format_win32_drive_root, is_legal_win32_mount_point, parse_windows_drive_prefix};

    #[test]
    fn github_runner_verbatim_root_is_legal() {
        let root =
            format_win32_drive_root(b'D', true).unwrap_or_else(|| panic!("D: should format"));
        assert_eq!(root, "\\\\?\\D:\\");
        assert!(is_legal_win32_mount_point(&root));
        assert!(!root.ends_with("\\\\"));
    }

    #[test]
    fn dos_drive_root_is_legal() {
        let root =
            format_win32_drive_root(b'd', false).unwrap_or_else(|| panic!("d: should format"));
        assert_eq!(root, "D:\\");
        assert!(is_legal_win32_mount_point(&root));
    }

    #[test]
    fn rejects_missing_or_double_trailing_slash() {
        assert!(!is_legal_win32_mount_point("\\\\?\\D:"));
        assert!(!is_legal_win32_mount_point("\\\\?\\D:\\\\"));
        assert!(!is_legal_win32_mount_point("D:"));
        assert!(!is_legal_win32_mount_point("D:\\\\"));
        assert!(!is_legal_win32_mount_point(""));
    }

    #[test]
    fn parses_dos_and_verbatim_file_paths() {
        let dos = parse_windows_drive_prefix(r"D:\folder\file.txt")
            .unwrap_or_else(|| panic!("DOS path should parse"));
        assert_eq!(dos.letter, b'D');
        assert!(!dos.verbatim);
        assert_eq!(dos.win32_root, "D:\\");
        assert!(is_legal_win32_mount_point(&dos.win32_root));

        let verbatim = parse_windows_drive_prefix(r"\\?\D:\folder\file.txt")
            .unwrap_or_else(|| panic!("verbatim path should parse"));
        assert_eq!(verbatim.letter, b'D');
        assert!(verbatim.verbatim);
        assert_eq!(verbatim.win32_root, "\\\\?\\D:\\");
        assert!(is_legal_win32_mount_point(&verbatim.win32_root));
        assert!(!verbatim.win32_root.ends_with("\\\\"));
    }

    #[test]
    fn parses_mixed_case_drive_and_unicode_leaf() {
        let mixed = parse_windows_drive_prefix(r"d:\Folder\File.txt")
            .unwrap_or_else(|| panic!("mixed-case drive should parse"));
        assert_eq!(mixed.letter, b'D');
        assert_eq!(mixed.win32_root, "D:\\");

        let unicode = parse_windows_drive_prefix(r"D:\dossier\facture-été.txt")
            .unwrap_or_else(|| panic!("unicode path should parse"));
        assert_eq!(unicode.win32_root, "D:\\");
        assert!(is_legal_win32_mount_point(&unicode.win32_root));

        let emoji = parse_windows_drive_prefix(r"D:\inbox\facture-🎉.txt")
            .unwrap_or_else(|| panic!("emoji path should parse"));
        assert_eq!(emoji.letter, b'D');
        assert_eq!(emoji.win32_root, "D:\\");
        assert!(is_legal_win32_mount_point(&emoji.win32_root));
        assert!(!emoji.win32_root.ends_with("\\\\"));
    }
}
