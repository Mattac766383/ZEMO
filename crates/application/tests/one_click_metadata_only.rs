use application::ScannerApplicationService;
use domain::{NativeFileIdentity, NativePath, PathEncoding, PlatformKind, VolumeIdentity};
use persistence::{Database, DatabaseKey, InventorySort};
use platform::{
    EnumerationProgress, PlatformError, ReadOnlyEntry, ReadOnlyEnumeration, ReadOnlyPlatform,
};
use std::{fs, path::Path, sync::Arc};

#[derive(Debug, Default)]
struct MetadataOnlyPlatform;

impl MetadataOnlyPlatform {
    fn volume() -> VolumeIdentity {
        VolumeIdentity {
            platform: PlatformKind::MacOs,
            stable_identifier: "metadata-only-test".to_owned(),
            filesystem_type: Some("apfs".to_owned()),
            case_sensitive: false,
            removable: false,
            local: true,
        }
    }
}

impl ReadOnlyPlatform for MetadataOnlyPlatform {
    fn inspect_volume(&self, _root: &Path) -> Result<VolumeIdentity, PlatformError> {
        Ok(Self::volume())
    }

    fn enumerate_regular_files(
        &self,
        _root: &Path,
        _max_entries: usize,
        _is_cancelled: &dyn Fn() -> bool,
        _on_progress: &mut dyn FnMut(EnumerationProgress),
    ) -> Result<ReadOnlyEnumeration, PlatformError> {
        panic!("consumer one-click must never recurse through the generic enumerator")
    }

    fn inspect_regular_file(
        &self,
        root: &Path,
        relative_path: &Path,
    ) -> Result<ReadOnlyEntry, PlatformError> {
        let absolute_path = root.join(relative_path);
        let metadata = fs::symlink_metadata(&absolute_path)?;
        if metadata.file_type().is_symlink() {
            return Err(PlatformError::ReparsePoint);
        }
        if !metadata.is_file() {
            return Err(PlatformError::Unsupported(
                "test entry is not a regular file".to_owned(),
            ));
        }
        let relative_bytes = relative_path.to_string_lossy().as_bytes().to_vec();
        let leaf_bytes = relative_path
            .file_name()
            .map(|value| value.to_string_lossy().as_bytes().to_vec())
            .ok_or(PlatformError::OutsideRoot)?;
        let size = metadata.len();
        Ok(ReadOnlyEntry {
            absolute_path,
            relative_path: NativePath {
                encoding: PathEncoding::UnixBytes,
                bytes: relative_bytes,
            },
            identity: NativeFileIdentity {
                volume: Self::volume(),
                object_key: format!("object:{relative_path:?}:{size}").into_bytes(),
                parent_key: b"parent".to_vec(),
                leaf_name: NativePath {
                    encoding: PathEncoding::UnixBytes,
                    bytes: leaf_bytes,
                },
                link_count: 1,
                reparse_tag: None,
            },
            byte_size: size,
            modified_at_ns: None,
            created_at_ns: None,
            accessed_at_ns: None,
            attributes: 0,
            read_only: false,
            hidden: false,
            cloud_placeholder: false,
            encrypted: false,
        })
    }

    fn read_bounded(&self, _path: &Path, _max_bytes: u64) -> Result<Vec<u8>, PlatformError> {
        panic!("consumer one-click must not read file contents")
    }

    fn read_prefix(&self, _path: &Path, _max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
        panic!("consumer one-click must not read file prefixes")
    }

    fn fingerprint(
        &self,
        _path: &Path,
        _include_content_digest: bool,
        _max_bytes: u64,
    ) -> Result<domain::FileFingerprint, PlatformError> {
        panic!("consumer one-click must not hash file contents")
    }
}

#[test]
fn consumer_scan_is_top_level_metadata_only_and_idempotent() {
    let temp = tempfile::tempdir().expect("temp dir");
    fs::write(temp.path().join("invoice.pdf"), b"same-size-a").expect("fixture file");
    fs::write(temp.path().join("photo.jpg"), b"same-size-b").expect("fixture file");
    fs::create_dir(temp.path().join("AlreadySorted")).expect("nested dir");
    fs::write(
        temp.path().join("AlreadySorted").join("nested.txt"),
        b"must not be scanned",
    )
    .expect("nested fixture");

    let database =
        Arc::new(Database::open_in_memory(&DatabaseKey::from_bytes([61; 32])).expect("database"));
    let service = ScannerApplicationService::new(database, Arc::new(MetadataOnlyPlatform));
    let workspace = service
        .create_workspace("metadata-only")
        .expect("workspace");
    let first_root = service
        .register_root(workspace.id, temp.path())
        .expect("first registration");
    let second_root = service
        .register_root(workspace.id, temp.path())
        .expect("idempotent registration");
    assert_eq!(
        first_root.id, second_root.id,
        "same path must reuse the root"
    );

    let mut progress_events = 0_u64;
    let scan = service
        .scan_workspace_consumer(workspace.id, &|| false, &mut |_| {
            progress_events = progress_events.saturating_add(1);
        })
        .expect("consumer metadata scan");

    assert_eq!(scan.indexed_count, 2, "nested file must not be scanned");
    assert_eq!(
        scan.hashed_count, 0,
        "one-click must not hash during discovery"
    );
    assert!(
        !scan.truncated,
        "small fixture must finish inside the bound"
    );
    assert!(progress_events >= 4, "scan must visibly report progress");

    let files = service
        .scan_files(scan.id, InventorySort::Filename, false, 100, 0)
        .expect("scan files");
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .all(|file| file.hashing_status == "not_candidate")
    );
    assert!(
        files
            .iter()
            .all(|file| !file.relative_path.contains("AlreadySorted"))
    );
}
