use application::ScannerApplicationService;
use domain::{
    FileFingerprint, NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity,
};
use persistence::{Database, DatabaseKey};
use platform::{
    EnumerationProgress, PlatformError, ReadOnlyEntry, ReadOnlyEnumeration, ReadOnlyPlatform,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
struct ReadOnlyFixture;

impl ReadOnlyFixture {
    fn volume() -> VolumeIdentity {
        VolumeIdentity {
            platform: PlatformKind::Windows,
            stable_identifier: "volume-serial:00000001".to_owned(),
            filesystem_type: Some("NTFS".to_owned()),
            case_sensitive: false,
            removable: false,
            local: true,
        }
    }

    fn native(value: &str) -> NativePath {
        NativePath {
            encoding: PathEncoding::WindowsUtf16Le,
            bytes: value.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        }
    }

    fn identity() -> NativeFileIdentity {
        NativeFileIdentity {
            volume: Self::volume(),
            object_key: vec![1; 16],
            parent_key: vec![2; 16],
            leaf_name: Self::native("invoice.txt"),
            link_count: 1,
            reparse_tag: None,
        }
    }
}

impl ReadOnlyPlatform for ReadOnlyFixture {
    fn inspect_volume(&self, _root: &Path) -> Result<VolumeIdentity, PlatformError> {
        Ok(Self::volume())
    }

    fn enumerate_regular_files(
        &self,
        root: &Path,
        _max_entries: usize,
        _is_cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(EnumerationProgress),
    ) -> Result<ReadOnlyEnumeration, PlatformError> {
        let progress = EnumerationProgress {
            entries_discovered: 1,
            files_discovered: 1,
            directories_discovered: 1,
            bytes_discovered: 73,
            errors: 0,
            skipped_items: 0,
        };
        on_progress(progress);
        Ok(ReadOnlyEnumeration {
            files: vec![ReadOnlyEntry {
                absolute_path: root.join("invoice.txt"),
                relative_path: Self::native("invoice.txt"),
                identity: Self::identity(),
                byte_size: 73,
                modified_at_ns: Some(1),
                created_at_ns: Some(1),
                accessed_at_ns: Some(1),
                attributes: 0,
                read_only: false,
                hidden: false,
                cloud_placeholder: false,
                encrypted: false,
            }],
            issues: Vec::new(),
            progress,
            truncated: false,
            cancelled: false,
        })
    }

    fn read_bounded(&self, _path: &Path, _max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
        Ok(b"Facture N INV-2026-001\nClient: ACME SAS\nTotal 1250,00 EUR\n2026-08-09".to_vec())
    }

    fn read_prefix(&self, _path: &Path, _max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
        Ok(b"Facture N INV-2026-001".to_vec())
    }

    fn fingerprint(
        &self,
        _path: &Path,
        include_content_digest: bool,
        _max_bytes: u64,
    ) -> Result<FileFingerprint, PlatformError> {
        Ok(FileFingerprint {
            native_identity: Self::identity(),
            byte_size: 73,
            modified_at_ns: Some(1),
            created_at_ns: Some(1),
            attributes: 0,
            quick_digest: None,
            content_digest: include_content_digest.then_some([9; 32]),
        })
    }
}

#[test]
fn scanner_service_persists_inventory_without_a_mutation_capability() {
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([3; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}")),
    );
    let platform = Arc::new(ReadOnlyFixture);
    let application = ScannerApplicationService::new(database, platform);
    let workspace = application
        .create_workspace("Dossier test")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    application
        .register_root(workspace.id, &PathBuf::from("/fixture"))
        .unwrap_or_else(|error| panic!("root should be registered: {error}"));

    let scan = application
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("scan should complete: {error}"));
    assert_eq!(scan.discovered_count, 1);
    assert_eq!(scan.indexed_count, 1);
    assert_eq!(scan.hashed_count, 0);
    let files = application
        .scan_files(scan.id, persistence::InventorySort::Filename, false, 100, 0)
        .unwrap_or_else(|error| panic!("inventory should load: {error}"));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "invoice.txt");
}
