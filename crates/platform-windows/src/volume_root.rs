//! Win32 volume-root strings.
//!
//! `GetVolumeInformationW`, `GetVolumeNameForVolumeMountPointW`, `GetDriveTypeW`,
//! and `CreateFileW` on a drive root require a mount point that ends with exactly
//! one backslash (`D:\` or `\\?\D:\`). `PathBuf::push("\\")` after a verbatim
//! prefix (`\\?\D:`) can produce `\\?\D:` or `\\?\D:\\`, both of which yield
//! Win32 error 87 (ERROR_INVALID_PARAMETER).

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
    use super::{format_win32_drive_root, is_legal_win32_mount_point};

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
}
