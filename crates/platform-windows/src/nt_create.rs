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

/// `FILE_RENAME_REPLACE_IF_EXISTS` — must stay unset. No overwrite.
pub const FILE_RENAME_REPLACE_IF_EXISTS: u32 = 0x0000_0001;
/// `FILE_ADD_FILE` — required on the destination parent handle used as
/// `FILE_RENAME_INFORMATION.RootDirectory`.
pub const FILE_ADD_FILE: u32 = 0x0000_0002;
/// `FILE_TRAVERSE`
pub const FILE_TRAVERSE: u32 = 0x0000_0020;
/// `FILE_READ_ATTRIBUTES`
pub const FILE_READ_ATTRIBUTES: u32 = 0x0080;
/// Destination-parent access for a no-replace NT rename.
pub const DESTINATION_PARENT_RENAME_ACCESS: u32 =
    FILE_ADD_FILE | FILE_TRAVERSE | FILE_READ_ATTRIBUTES;

/// Win32 `SetFileInformationByHandle(FileRenameInfo/Ex)` requires
/// `RootDirectory = NULL` and a NUL-terminated Win32 path. Combining a parent
/// handle with a relative leaf through that wrapper is ERROR 87 on Windows 11.
/// The NT `NtSetInformationFile(FileRenameInformation)` contract is the opposite:
/// `RootDirectory` = parent directory handle and `FileName` = one relative leaf.
#[must_use]
pub const fn rename_flags_are_no_replace(flags: u32) -> bool {
    flags & FILE_RENAME_REPLACE_IF_EXISTS == 0
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
        DESTINATION_PARENT_RENAME_ACCESS, DIRECTORY_CREATE_OPTION_MASK, FILE_ADD_FILE,
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_FOR_BACKUP_INTENT,
        FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_RENAME_REPLACE_IF_EXISTS, FILE_SYNCHRONOUS_IO_NONALERT, FILE_TRAVERSE,
        anchored_create_options, directory_create_options_are_legal, relative_object_name_is_legal,
        rename_flags_are_no_replace,
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

    #[test]
    fn rename_must_use_relative_leaf_and_never_replace() {
        assert!(rename_flags_are_no_replace(0));
        assert!(!rename_flags_are_no_replace(FILE_RENAME_REPLACE_IF_EXISTS));
        assert!(relative_object_name_is_legal("committed.txt"));
        assert!(relative_object_name_is_legal("facture-été.txt"));
        assert!(!relative_object_name_is_legal(r"D:\temp\root\file.txt"));
        assert!(!relative_object_name_is_legal(r"\\?\D:\temp\root\file.txt"));
        assert!(!relative_object_name_is_legal(r"child\file.txt"));
        assert_eq!(
            DESTINATION_PARENT_RENAME_ACCESS & FILE_ADD_FILE,
            FILE_ADD_FILE,
            "NtSetInformationFile rename requires FILE_ADD_FILE on RootDirectory"
        );
        assert_eq!(
            DESTINATION_PARENT_RENAME_ACCESS & FILE_TRAVERSE,
            FILE_TRAVERSE
        );
        assert_eq!(
            DESTINATION_PARENT_RENAME_ACCESS & FILE_READ_ATTRIBUTES,
            FILE_READ_ATTRIBUTES
        );
        assert_eq!(
            DESTINATION_PARENT_RENAME_ACCESS & FILE_RENAME_REPLACE_IF_EXISTS,
            0
        );
    }
}
