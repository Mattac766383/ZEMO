//! NtCreateFile CreateOptions contracts.
//!
//! Values match the WDK (`ntifs.h` / `wdm.h`). `FILE_DIRECTORY_FILE` is valid
//! only with a documented subset of CreateOptions; any other bit, including
//! `FILE_OPEN_NO_RECALL`, is `STATUS_INVALID_PARAMETER` (Win32 87).

/// `FILE_DIRECTORY_FILE`
pub const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
/// `FILE_SYNCHRONOUS_IO_ALERT`
pub const FILE_SYNCHRONOUS_IO_ALERT: u32 = 0x0000_0010;
/// `FILE_SYNCHRONOUS_IO_NONALERT`
pub const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
/// `FILE_NON_DIRECTORY_FILE`
pub const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
/// `FILE_COMPLETE_IF_OPLOCKED`
pub const FILE_COMPLETE_IF_OPLOCKED: u32 = 0x0000_0100;
/// `FILE_DELETE_ON_CLOSE`
pub const FILE_DELETE_ON_CLOSE: u32 = 0x0000_1000;
/// `FILE_OPEN_BY_FILE_ID`
pub const FILE_OPEN_BY_FILE_ID: u32 = 0x0000_2000;
/// `FILE_OPEN_FOR_BACKUP_INTENT`
pub const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x0000_4000;
/// `FILE_WRITE_THROUGH`
pub const FILE_WRITE_THROUGH: u32 = 0x0000_0002;
/// `FILE_OPEN_FOR_FREE_SPACE_QUERY`
pub const FILE_OPEN_FOR_FREE_SPACE_QUERY: u32 = 0x0080_0000;
/// `FILE_OPEN_REPARSE_POINT`
pub const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
/// `FILE_OPEN_NO_RECALL` — valid for files, **not** with `FILE_DIRECTORY_FILE`.
pub const FILE_OPEN_NO_RECALL: u32 = 0x0040_0000;

/// Ionescu/ReactOS `IopCreateFile` mask: CreateOptions allowed with
/// `FILE_DIRECTORY_FILE`. `FILE_OPEN_NO_RECALL` is intentionally absent.
pub const DIRECTORY_CREATE_OPTION_MASK: u32 = FILE_DIRECTORY_FILE
    | FILE_SYNCHRONOUS_IO_ALERT
    | FILE_SYNCHRONOUS_IO_NONALERT
    | FILE_WRITE_THROUGH
    | FILE_COMPLETE_IF_OPLOCKED
    | FILE_OPEN_FOR_BACKUP_INTENT
    | FILE_DELETE_ON_CLOSE
    | FILE_OPEN_FOR_FREE_SPACE_QUERY
    | FILE_OPEN_BY_FILE_ID
    | FILE_OPEN_REPARSE_POINT;

/// CreateOptions for one relative component under an already-open parent.
///
/// Directory opens omit `FILE_OPEN_NO_RECALL` (STATUS_INVALID_PARAMETER / 87
/// when combined with `FILE_DIRECTORY_FILE`). File opens keep it so cloud/HSM
/// content is not recalled. `FILE_OPEN_REPARSE_POINT` opens a reparse leaf
/// without following it; the caller must then inspect and reject.
#[must_use]
pub const fn anchored_create_options(directory: bool) -> u32 {
    if directory {
        FILE_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_FOR_BACKUP_INTENT
            | FILE_OPEN_REPARSE_POINT
    } else {
        FILE_NON_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_FOR_BACKUP_INTENT
            | FILE_OPEN_REPARSE_POINT
            | FILE_OPEN_NO_RECALL
    }
}

#[must_use]
pub const fn directory_create_options_are_legal(options: u32) -> bool {
    options & FILE_DIRECTORY_FILE != 0 && options & !DIRECTORY_CREATE_OPTION_MASK == 0
}

/// NtCreateFile ObjectName when RootDirectory is a handle must be a single
/// relative leaf. An absolute Win32/NT path combined with a root handle is
/// STATUS_OBJECT_PATH_SYNTAX_BAD or STATUS_INVALID_PARAMETER.
#[must_use]
pub fn relative_object_name_is_legal(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !name.contains(['\\', '/', ':'])
        && !name.starts_with(r"\\?\")
        && name != "."
        && name != ".."
}

#[cfg(test)]
mod tests {
    use super::{
        DIRECTORY_CREATE_OPTION_MASK, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE,
        FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, anchored_create_options, directory_create_options_are_legal,
    };

    #[test]
    fn directory_options_exclude_file_open_no_recall() {
        let options = anchored_create_options(true);
        assert_eq!(options & FILE_OPEN_NO_RECALL, 0);
        assert_eq!(options & FILE_DIRECTORY_FILE, FILE_DIRECTORY_FILE);
        assert_eq!(options & FILE_NON_DIRECTORY_FILE, 0);
        assert_eq!(
            options & FILE_OPEN_REPARSE_POINT,
            FILE_OPEN_REPARSE_POINT,
            "reparse leaves must be opened, not followed"
        );
        assert_eq!(
            options & FILE_OPEN_FOR_BACKUP_INTENT,
            FILE_OPEN_FOR_BACKUP_INTENT
        );
        assert_eq!(
            options & FILE_SYNCHRONOUS_IO_NONALERT,
            FILE_SYNCHRONOUS_IO_NONALERT
        );
        assert!(directory_create_options_are_legal(options));
        assert_eq!(options & !DIRECTORY_CREATE_OPTION_MASK, 0);
    }

    #[test]
    fn file_options_keep_no_recall_and_are_not_directory() {
        let options = anchored_create_options(false);
        assert_eq!(options & FILE_DIRECTORY_FILE, 0);
        assert_eq!(options & FILE_NON_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE);
        assert_eq!(options & FILE_OPEN_NO_RECALL, FILE_OPEN_NO_RECALL);
        assert_eq!(options & FILE_OPEN_REPARSE_POINT, FILE_OPEN_REPARSE_POINT);
        assert!(
            !directory_create_options_are_legal(options),
            "file options must not be passed with FILE_DIRECTORY_FILE"
        );
    }

    #[test]
    fn file_open_no_recall_with_directory_file_is_illegal() {
        let illegal = FILE_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_REPARSE_POINT
            | FILE_OPEN_NO_RECALL;
        assert!(
            !directory_create_options_are_legal(illegal),
            "this combination is STATUS_INVALID_PARAMETER / ERROR 87"
        );
    }

    #[test]
    fn object_name_with_root_directory_must_be_a_relative_leaf() {
        use super::relative_object_name_is_legal;
        assert!(relative_object_name_is_legal("a"));
        assert!(relative_object_name_is_legal("_temp"));
        assert!(relative_object_name_is_legal(
            "zemo-windows-qualification-diag"
        ));
        assert!(!relative_object_name_is_legal(r"\a"));
        assert!(!relative_object_name_is_legal(r"D:\a\_temp"));
        assert!(!relative_object_name_is_legal(r"\\?\D:\a\_temp"));
        assert!(!relative_object_name_is_legal(r"a\_temp"));
        assert!(!relative_object_name_is_legal(""));
        assert!(!relative_object_name_is_legal("."));
        assert!(!relative_object_name_is_legal(".."));
    }
}
