//! Packaged macOS Apply qualification (M18 Step 2).
//!
//! These tests speak to the **bundled** `operation-executor` sidecar inside a
//! release `.app`. They never scan Documents/Desktop/Downloads and they never
//! compile mutation into this desktop crate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::items_after_statements,
    clippy::used_underscore_binding,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unnecessary_debug_formatting,
    clippy::map_unwrap_or
)]

use crate::executor_client::{ProcessApprovedExecutorClient, resolve_packaged_sidecar};
use application::{
    ApprovedExecutorClient, ExecutionApplicationService, ExecutionConsentAuthorityKey,
    ScannerApplicationService,
};
use domain::{
    OrganizationProposal, OrganizationProposalDiff, OrganizationProposalOperation,
    OrganizationProposalStatus, OrganizationProposalSummary, OrganizationReason,
    OrganizationRevisionId, ProposalConfidenceLevel, ProposalConflictState, ProposalId,
    ProposalItemId, ProposalOperationKind, ProposalSourceSnapshot,
};
use ipc_contracts::executor_v2::ROOT_AUTHORITY_SECRET_SERVICE;
use operations::{ApplyGate, ExecutionSafetyPolicy, FileJournal, JournalKey};
use persistence::{Database, DatabaseKey, ProposalSourceFileRecord, ProposalWorkspaceSourceRecord};
use platform::ReadOnlyPlatform;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tempfile::{Builder, TempDir};

const QUALIFICATION_TIMESTAMP: &str = "2026-08-14T00:00:00Z";

fn packaged_app() -> PathBuf {
    let raw = std::env::var("WORKING_NAME_PACKAGED_APP")
        .unwrap_or_else(|_| panic!("WORKING_NAME_PACKAGED_APP must point at the release ZEMO.app"));
    let path = PathBuf::from(raw);
    assert!(
        path.extension().is_some_and(|ext| ext == "app"),
        "WORKING_NAME_PACKAGED_APP must be a .app bundle: {path:?}"
    );
    path.canonicalize()
        .unwrap_or_else(|error| panic!("packaged app should canonicalize: {error}"))
}

fn packaged_sidecar() -> PathBuf {
    let app = packaged_app();
    let macos = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    let executable = ["desktop", "ZEMO", "Working Name"]
        .into_iter()
        .map(|name| macos.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| macos.join("desktop"));
    resolve_packaged_sidecar(&resources, &executable)
        .unwrap_or_else(|error| panic!("packaged sidecar should resolve inside the .app: {error}"))
}

fn executor_root_authority() -> [u8; 32] {
    // Qualification must not depend on Keychain ACL sharing between the cargo
    // test binary and the ad-hoc sidecar. Persist a fresh root to the same
    // 0600 application-support file the packaged helper reads first.
    let mut root = [0_u8; 32];
    getrandom::fill(&mut root).unwrap_or_else(|error| panic!("executor root: {error}"));
    privacy::persist_shared_executor_root(ROOT_AUTHORITY_SECRET_SERVICE, &root)
        .unwrap_or_else(|error| panic!("shared executor root file: {error}"));
    let loaded = privacy::load_shared_executor_root(ROOT_AUTHORITY_SECRET_SERVICE)
        .unwrap_or_else(|error| panic!("shared executor root reload: {error}"))
        .unwrap_or_else(|| panic!("shared executor root missing after persist"));
    assert_eq!(
        loaded.as_slice(),
        root.as_slice(),
        "shared executor root round-trip"
    );
    root
}

fn assert_isolated_sandbox(root: &Path) {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    assert!(
        name.starts_with("supremacy-m18-step2-sandbox-"),
        "qualification root is not an M18 Step 2 sandbox: {root:?}"
    );
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|error| panic!("sandbox should canonicalize: {error}"));
    for (label, folder) in [
        ("Documents", dirs::document_dir()),
        ("Desktop", dirs::desktop_dir()),
        ("Downloads", dirs::download_dir()),
    ] {
        if let Some(folder) = folder {
            assert!(
                !canonical.starts_with(&folder),
                "sandbox must not live under {label}: {canonical:?}"
            );
        }
    }
}

struct MutationSandbox {
    directory: TempDir,
}

impl MutationSandbox {
    fn new() -> Self {
        let directory = Builder::new()
            .prefix("supremacy-m18-step2-sandbox-")
            .tempdir()
            .unwrap_or_else(|error| panic!("sandbox should be created: {error}"));
        assert_isolated_sandbox(directory.path());
        Self { directory }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path().join(relative);
        fs::create_dir_all(path.parent().unwrap_or(self.path()))
            .unwrap_or_else(|error| panic!("fixture parent: {error}"));
        fs::write(&path, bytes).unwrap_or_else(|error| panic!("fixture write: {error}"));
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot_tree(self.path())
    }
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    fn walk(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap_or_else(|error| panic!("read_dir: {error}")) {
            let entry = entry.unwrap_or_else(|error| panic!("dirent: {error}"));
            let path = entry.path();
            let file_type = entry
                .file_type()
                .unwrap_or_else(|error| panic!("file type: {error}"));
            if file_type.is_dir() {
                walk(root, &path, files);
            } else if file_type.is_file() {
                let bytes = fs::read(&path).unwrap_or_else(|error| panic!("read: {error}"));
                files.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|_| panic!("relative"))
                        .to_path_buf(),
                    bytes,
                );
            }
        }
    }
    walk(root, root, &mut files);
    files
}

struct CatalogFixture {
    database: Arc<Database>,
    platform: Arc<dyn ReadOnlyPlatform>,
    source: ProposalWorkspaceSourceRecord,
    source_by_path: BTreeMap<String, ProposalSourceFileRecord>,
}

impl CatalogFixture {
    fn scan(sandbox: &MutationSandbox, expected_files: usize, name: &str) -> Self {
        let database = Arc::new(
            Database::open_in_memory(&DatabaseKey::from_bytes([18; 32]))
                .unwrap_or_else(|error| panic!("qualification database: {error}")),
        );
        let platform: Arc<dyn ReadOnlyPlatform> = Arc::new(platform_macos::MacOsPlatform);
        let scanner = ScannerApplicationService::new(database.clone(), platform.clone());
        let workspace = scanner
            .create_workspace(name)
            .unwrap_or_else(|error| panic!("workspace: {error}"));
        let root = scanner
            .register_root(workspace.id, sandbox.path())
            .unwrap_or_else(|error| panic!("register root: {error}"));
        let scan = scanner
            .scan_workspace(workspace.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("scan: {error}"));
        assert_eq!(scan.indexed_count, expected_files as u64);
        let source = database
            .organization_source_for_root(workspace.id, root.id)
            .unwrap_or_else(|error| panic!("proposal source: {error}"));
        let source_by_path = source
            .files
            .iter()
            .cloned()
            .map(|file| (normalized_relative(&file.relative_path), file))
            .collect();
        Self {
            database,
            platform,
            source,
            source_by_path,
        }
    }

    fn persist_proposal(&self, mappings: &[(String, String)]) -> OrganizationProposal {
        let operations = mappings
            .iter()
            .map(|(source, destination)| self.operation(source, destination))
            .collect::<Vec<_>>();
        let proposed_moves = operations
            .iter()
            .filter(|operation| operation.operation_kind == ProposalOperationKind::MoveProposal)
            .count() as u64;
        let proposal = OrganizationProposal {
            id: ProposalId::new(),
            revision_id: OrganizationRevisionId::new(),
            workspace_id: self.source.workspace_id,
            root_id: self.source.root_id,
            source_scan_id: self.source.scan_id,
            revision: 1,
            status: OrganizationProposalStatus::ApprovedForFutureApply,
            engine_version: "m18-step2-packaged-v1".to_owned(),
            policy_version: "m8-strict-no-overwrite-v1".to_owned(),
            source_semantic_version: self.source.semantic_version.clone(),
            source_relationship_version: self.source.relationship_version.clone(),
            created_at: QUALIFICATION_TIMESTAMP.to_owned(),
            updated_at: QUALIFICATION_TIMESTAMP.to_owned(),
            summary: OrganizationProposalSummary {
                files_analyzed: operations.len() as u64,
                proposed_moves,
                proposed_renames: operations.len() as u64 - proposed_moves,
                unchanged: 0,
                needs_review: 0,
                unresolved: 0,
                conflicts: 0,
                high_confidence: operations.len() as u64,
                medium_confidence: 0,
                low_confidence: 0,
                duplicate_no_action: 0,
                average_depth: 1.0,
                maximum_depth: 2,
            },
            diff: OrganizationProposalDiff::default(),
            nodes: Vec::new(),
            operations,
        };
        self.database
            .persist_organization_proposal(&proposal, "initial")
            .unwrap_or_else(|error| panic!("proposal persist: {error}"));
        proposal
    }

    fn operation(
        &self,
        source_path: &str,
        destination_path: &str,
    ) -> OrganizationProposalOperation {
        let source = self
            .source_by_path
            .get(&normalized_relative(source_path))
            .unwrap_or_else(|| panic!("missing scanned source {source_path}"));
        let destination = path_segments(destination_path);
        let (proposed_name, proposed_destination) = destination
            .split_last()
            .map(|(name, parents)| (name.clone(), parents.to_vec()))
            .unwrap_or_else(|| panic!("destination"));
        let source_parent = path_segments(&source.relative_path);
        let source_parent = &source_parent[..source_parent.len().saturating_sub(1)];
        let operation_kind = if source_parent == proposed_destination.as_slice() {
            ProposalOperationKind::RenameProposal
        } else {
            ProposalOperationKind::MoveProposal
        };
        OrganizationProposalOperation {
            id: ProposalItemId::new(),
            file_id: source.file_id.parse().unwrap_or_else(|_| panic!("file id")),
            file_version_id: source
                .file_version_id
                .parse()
                .unwrap_or_else(|_| panic!("file version")),
            source: ProposalSourceSnapshot {
                relative_path: source.relative_path.clone(),
                content_hash: source.content_hash.clone(),
                byte_size: source.byte_size,
                modified_at: source.modified_at.clone(),
            },
            source_name: source.filename.clone(),
            machine_destination: proposed_destination.clone(),
            machine_name: proposed_name.clone(),
            proposed_destination,
            proposed_name,
            operation_kind,
            confidence_score: 1.0,
            confidence_level: ProposalConfidenceLevel::VeryHigh,
            reasons: vec![OrganizationReason {
                code: "m18_step2".to_owned(),
                explanation: "Packaged macOS Apply qualification mapping.".to_owned(),
                evidence_references: vec![source.relative_path.clone()],
            }],
            conflict_state: ProposalConflictState::None,
            needs_review: false,
            stale: false,
            user_override: true,
            disruption_score: 0.1,
            proposed_path_length: normalized_relative(destination_path).encode_utf16().count(),
            proposed_depth: path_segments(destination_path).len(),
            semantic_context: "unknown".to_owned(),
            document_type: "qualification_fixture".to_owned(),
            customer_name: None,
            supplier_name: None,
            project_name: None,
            duplicate_group_id: None,
            duplicate_canonical: true,
        }
    }
}

fn packaged_service(fixture: &CatalogFixture, journal_path: &Path) -> ExecutionApplicationService {
    packaged_service_with(fixture, journal_path, executor_root_authority(), None)
}

fn packaged_service_with(
    fixture: &CatalogFixture,
    journal_path: &Path,
    root: [u8; 32],
    qualification_crash: Option<&'static str>,
) -> ExecutionApplicationService {
    let sidecar = packaged_sidecar();
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(match qualification_crash {
        Some(phase) => {
            ProcessApprovedExecutorClient::with_qualification_crash(sidecar, root, phase)
                .unwrap_or_else(|error| panic!("packaged crash client: {error}"))
        }
        None => ProcessApprovedExecutorClient::new(sidecar, root)
            .unwrap_or_else(|error| panic!("packaged executor client: {error}")),
    });
    let journal_key = JournalKey::from_bytes([18; 32]);
    let journal = FileJournal::open_or_locked(journal_path, journal_key, 1);
    let mut policy = ExecutionSafetyPolicy::default();
    policy.allow_qualified_case_only_rename = true;
    ExecutionApplicationService::new(
        fixture.database.clone(),
        fixture.platform.clone(),
        executor,
        journal,
        ApplyGate {
            enabled: true,
            reason: "packaged macos apply qualification".to_owned(),
        },
        policy,
        ExecutionConsentAuthorityKey::derive(&root),
    )
    .unwrap_or_else(|error| panic!("execution service: {error}"))
}

fn display_leaf(root: &Path, relative_parent: &str, ignore_ascii: &str) -> Option<String> {
    fs::read_dir(root.join(relative_parent))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.eq_ignore_ascii_case(ignore_ascii))
}

fn staging_artifacts(root: &Path) -> Vec<PathBuf> {
    let staging = root.join(".supremacy-staging");
    if !staging.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    fn walk(current: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    walk(&staging, &mut files);
    files
}

fn attest(
    service: &ExecutionApplicationService,
    execution_id: domain::ExecutionId,
    phrase: Option<&str>,
) -> domain::ExecutionDetail {
    let challenge = service
        .create_execution_consent_challenge(execution_id, phrase)
        .unwrap_or_else(|error| panic!("consent challenge: {error}"));
    service
        .finalize_execution_consent(challenge)
        .unwrap_or_else(|error| panic!("consent attest: {error}"))
}

fn normalized_relative(value: &str) -> String {
    value
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn path_segments(value: &str) -> Vec<String> {
    normalized_relative(value)
        .split('/')
        .map(str::to_owned)
        .collect()
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[test]
fn renderer_capabilities_exclude_filesystem_mutation_plugins() {
    let capabilities = include_str!("../capabilities/default.json");
    assert!(capabilities.contains("core:default"));
    assert!(!capabilities.contains("fs:"));
    assert!(!capabilities.contains("shell:"));
    let lib = include_str!("lib.rs");
    assert!(!lib.contains("std::fs::rename"));
    assert!(!lib.contains("std::fs::remove_file"));
    assert!(!lib.contains("std::fs::copy("));
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_sidecar_is_present_regular_and_authenticated() {
    let sidecar = packaged_sidecar();
    let metadata =
        fs::symlink_metadata(&sidecar).unwrap_or_else(|error| panic!("sidecar metadata: {error}"));
    assert!(metadata.is_file());
    assert!(!metadata.file_type().is_symlink());
    assert!(
        sidecar.starts_with(packaged_app().join("Contents")),
        "sidecar escaped the .app: {sidecar:?}"
    );
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/hello.txt", b"packaged-auth");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 sidecar auth");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/hello.txt".to_owned(),
        "Organized/hello.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("auth preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("authenticated apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    assert_eq!(
        fs::read(sandbox.path().join("Organized/hello.txt"))
            .unwrap_or_else(|error| panic!("read: {error}")),
        b"packaged-auth"
    );
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn missing_sidecar_fails_closed() {
    let temp = Builder::new()
        .prefix("supremacy-m18-step2-empty-app-")
        .tempdir()
        .unwrap_or_else(|error| panic!("empty app: {error}"));
    let macos = temp.path().join("Contents/MacOS");
    let resources = temp.path().join("Contents/Resources");
    fs::create_dir_all(&macos).unwrap_or_else(|error| panic!("macos dir: {error}"));
    fs::create_dir_all(&resources).unwrap_or_else(|error| panic!("resources dir: {error}"));
    let fake_exe = macos.join("Working Name");
    fs::write(&fake_exe, b"not-an-executor").unwrap_or_else(|error| panic!("fake exe: {error}"));
    let error = resolve_packaged_sidecar(&resources, &fake_exe)
        .expect_err("missing sidecar must fail closed");
    let text = error.to_string();
    assert!(
        text.contains("not found") || text.contains("unavailable"),
        "missing sidecar error: {text}"
    );
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_apply_move_rename_undo_and_journal_stay_in_app_data() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/report.txt", b"move-me");
    sandbox.write("incoming/Invoice.pdf", b"rename-me");
    sandbox.write("incoming/notes.txt", b"both");
    let initial = sandbox.snapshot();
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let journal_path = app_data.path().join("operation-recovery.jsonl.enc");
    let fixture = CatalogFixture::scan(&sandbox, 3, "M18 packaged matrix");
    let service = packaged_service(&fixture, &journal_path);
    let proposal = fixture.persist_proposal(&[
        (
            "incoming/report.txt".to_owned(),
            "Organized/Reviewed/report-final.txt".to_owned(),
        ),
        (
            "incoming/notes.txt".to_owned(),
            "Organized/notes-renamed.txt".to_owned(),
        ),
    ]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    assert!(!sandbox.path().join("incoming/report.txt").exists());
    assert_eq!(
        fs::read(sandbox.path().join("Organized/Reviewed/report-final.txt")).unwrap(),
        b"move-me"
    );
    assert_eq!(
        fs::read(sandbox.path().join("Organized/notes-renamed.txt")).unwrap(),
        b"both"
    );
    assert_eq!(
        fs::read(sandbox.path().join("incoming/Invoice.pdf")).unwrap(),
        b"rename-me"
    );
    assert!(journal_path.is_file());
    assert!(!sandbox.path().join("operation-recovery.jsonl.enc").exists());
    let body = fs::read(&journal_path).unwrap_or_else(|error| panic!("journal read: {error}"));
    let haystack = String::from_utf8_lossy(&body);
    assert!(
        !haystack.contains("move-me") && !haystack.contains("report-final"),
        "recovery journal must not contain plaintext file payloads"
    );
    let rolled = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("undo: {error}"));
    assert_eq!(
        rolled.session.status,
        domain::OrganizationExecutionStatus::RolledBack
    );
    assert_eq!(sandbox.snapshot(), initial);

    let fixture = CatalogFixture::scan(&sandbox, 3, "M18 packaged case-only");
    let service = packaged_service(&fixture, &app_data.path().join("journal-case.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/Invoice.pdf".to_owned(),
        "incoming/invoice.pdf".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("case-only preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("case-only apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    let invoice_leaf = fs::read_dir(sandbox.path().join("incoming"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .find(|name| name.to_string_lossy().eq_ignore_ascii_case("invoice.pdf"));
    assert_eq!(
        invoice_leaf
            .as_ref()
            .map(|name| name.to_string_lossy().into_owned()),
        Some("invoice.pdf".to_owned())
    );
    let rolled = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("case-only undo: {error}"));
    eprintln!(
        "M18 packaged case-only undo status={:?} rollback_blocked={} failed={}",
        rolled.session.status,
        rolled.session.summary.rollback_blocked,
        rolled.session.summary.failed
    );
    assert!(
        sandbox
            .path()
            .join("incoming")
            .read_dir()
            .unwrap()
            .any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("invoice.pdf")
            })
    );
    assert_eq!(
        fs::read(
            sandbox
                .path()
                .join("incoming")
                .read_dir()
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("invoice.pdf")))
                .unwrap()
        )
        .unwrap(),
        b"rename-me"
    );
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_case_only_undo_reproduces_apfs_collision() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/Facture.pdf", b"case-only-bytes");
    let initial = sandbox.snapshot();
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 case-only reproduce");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal-case.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/Facture.pdf".to_owned(),
        "incoming/facture.pdf".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("case-only preflight: {error}"));
    eprintln!(
        "M18.1 case-only plan: {:?}",
        prepared
            .operations
            .iter()
            .map(|operation| (
                operation.kind,
                operation.source_relative_path.clone(),
                operation.destination_relative_path.clone(),
                operation.status
            ))
            .collect::<Vec<_>>()
    );
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("case-only apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    let rolled = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("case-only undo: {error}"));
    eprintln!(
        "M18.1 case-only undo status={:?} blocked={} failed={} ops={:?}",
        rolled.session.status,
        rolled.session.summary.rollback_blocked,
        rolled.session.summary.failed,
        rolled
            .operations
            .iter()
            .map(|operation| (
                operation.kind,
                operation.status,
                operation.source_relative_path.clone(),
                operation.destination_relative_path.clone(),
                operation.error_code.clone(),
                operation.error_message.clone()
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rolled.session.status,
        domain::OrganizationExecutionStatus::RolledBack,
        "case-only Apply→Undo must restore Facture.pdf without treating APFS same-inode occupancy as a collision"
    );
    assert_eq!(sandbox.snapshot(), initial);
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_case_only_round_trip_matrix_and_external_conflicts() {
    let cases = [
        (
            "incoming/File.pdf",
            "incoming/file.pdf",
            b"ascii-mixed".as_slice(),
        ),
        (
            "incoming/FILE.pdf",
            "incoming/File.pdf",
            b"ascii-upper".as_slice(),
        ),
        (
            "incoming/Facture Été.pdf",
            "incoming/facture été.pdf",
            b"accented".as_slice(),
        ),
        (
            "incoming/My Invoice.pdf",
            "incoming/my invoice.pdf",
            b"spaces".as_slice(),
        ),
        (
            "incoming/发票-Résumé.pdf",
            "incoming/发票-résumé.pdf",
            b"unicode".as_slice(),
        ),
    ];
    for (source, destination, bytes) in cases {
        let sandbox = MutationSandbox::new();
        sandbox.write(source, bytes);
        let initial = sandbox.snapshot();
        let fixture = CatalogFixture::scan(&sandbox, 1, &format!("M18.1 case {source}"));
        let app_data = Builder::new()
            .prefix("supremacy-m18-step2-appdata-")
            .tempdir()
            .unwrap_or_else(|error| panic!("app data: {error}"));
        let service = packaged_service(&fixture, &app_data.path().join("journal.jsonl.enc"));
        let proposal = fixture.persist_proposal(&[(source.to_owned(), destination.to_owned())]);
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("{source} preflight: {error}"));
        let approved = attest(&service, prepared.session.id, None);
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("{source} apply: {error}"));
        assert_eq!(completed.session.summary.failed, 0, "{source}");
        let parent = Path::new(source).parent().unwrap().to_string_lossy();
        let dest_name = Path::new(destination)
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert_eq!(
            display_leaf(sandbox.path(), &parent, &dest_name).as_deref(),
            Some(dest_name.as_ref()),
            "{source} apply leaf"
        );
        let rolled = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("{source} undo: {error}"));
        assert_eq!(
            rolled.session.status,
            domain::OrganizationExecutionStatus::RolledBack,
            "{source} undo"
        );
        assert_eq!(rolled.session.summary.rollback_blocked, 0, "{source}");
        assert_eq!(sandbox.snapshot(), initial, "{source} snapshot");
        assert!(
            staging_artifacts(sandbox.path()).is_empty(),
            "{source} staging"
        );
    }

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/Facture.pdf", b"original-identity");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 external replace");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal-ext.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/Facture.pdf".to_owned(),
        "incoming/facture.pdf".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("external preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("external apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    let applied = sandbox
        .path()
        .join("incoming")
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("facture.pdf"))
        })
        .unwrap();
    fs::write(&applied, b"user-replaced-bytes").unwrap();
    let rolled = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("external undo: {error}"));
    assert_eq!(
        rolled.session.status,
        domain::OrganizationExecutionStatus::RollbackPartial
    );
    assert!(rolled.session.summary.rollback_blocked >= 1);
    assert_eq!(fs::read(&applied).unwrap(), b"user-replaced-bytes");

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/Facture.pdf", b"keep-me");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 genuine collision");
    let service = packaged_service(&fixture, &app_data.path().join("journal-col.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/Facture.pdf".to_owned(),
        "incoming/facture.pdf".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("collision preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("collision apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    let applied = sandbox
        .path()
        .join("incoming")
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("facture.pdf"))
        })
        .unwrap();
    let aside = sandbox.path().join("incoming/aside-original.pdf");
    fs::rename(&applied, &aside).unwrap();
    sandbox.write("incoming/Facture.pdf", b"different-file");
    let rolled = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("collision undo: {error}"));
    assert_eq!(
        rolled.session.status,
        domain::OrganizationExecutionStatus::RollbackPartial
    );
    assert!(rolled.session.summary.rollback_blocked >= 1);
    assert_eq!(fs::read(&aside).unwrap(), b"keep-me");
    assert_eq!(
        fs::read(sandbox.path().join("incoming/Facture.pdf")).unwrap(),
        b"different-file"
    );
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_controlled_crash_before_after_and_stage_recover_on_relaunch() {
    let root = executor_root_authority();

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.txt", b"before-mutation");
    let initial = sandbox.snapshot();
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 crash before");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let journal = app_data.path().join("journal-before.jsonl.enc");
    let service = packaged_service_with(&fixture, &journal, root, Some("before_mutation"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.txt".to_owned(),
        "Organized/item.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("crash-before preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let started = service.start_execution(approved.session.id, &mut |_| {});
    assert!(
        started.is_err()
            || started.as_ref().is_ok_and(|detail| {
                matches!(
                    detail.session.status,
                    domain::OrganizationExecutionStatus::RecoveryRequired
                        | domain::OrganizationExecutionStatus::RecoveryAmbiguous
                        | domain::OrganizationExecutionStatus::RecoveryAvailable
                )
            }),
        "crash before mutation must not complete: {started:?}"
    );
    assert_eq!(
        fs::read(sandbox.path().join("incoming/item.txt")).unwrap(),
        b"before-mutation"
    );
    assert!(!sandbox.path().join("Organized/item.txt").exists());
    drop(service);
    let recovered_service = packaged_service_with(&fixture, &journal, root, None);
    let recovered = recovered_service
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("crash-before recover: {error}"));
    assert_eq!(sandbox.snapshot(), initial);
    assert!(!sandbox.path().join("Organized/item.txt").exists());
    assert_ne!(
        recovered.state,
        domain::ExecutionRecoveryState::RecoveryNotRequired
    );
    eprintln!("M18.1 crash before mutation recovery={recovered:?}");

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.txt", b"after-mutation");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 crash after");
    let journal = app_data.path().join("journal-after.jsonl.enc");
    let service = packaged_service_with(&fixture, &journal, root, Some("after_mutation"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.txt".to_owned(),
        "Organized/item.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("crash-after preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let started = service.start_execution(approved.session.id, &mut |_| {});
    assert!(
        started.is_err()
            || started.as_ref().is_ok_and(|detail| {
                matches!(
                    detail.session.status,
                    domain::OrganizationExecutionStatus::RecoveryRequired
                        | domain::OrganizationExecutionStatus::RecoveryAmbiguous
                        | domain::OrganizationExecutionStatus::RecoveryAvailable
                )
            }),
        "crash after mutation must not acknowledge completion: {started:?}"
    );
    assert_eq!(
        fs::read(sandbox.path().join("Organized/item.txt")).unwrap(),
        b"after-mutation"
    );
    assert!(!sandbox.path().join("incoming/item.txt").exists());
    drop(service);
    let recovered_service = packaged_service_with(&fixture, &journal, root, None);
    let recovered = recovered_service
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("crash-after recover: {error}"));
    assert_eq!(
        fs::read(sandbox.path().join("Organized/item.txt")).unwrap(),
        b"after-mutation"
    );
    assert!(!sandbox.path().join("incoming/item.txt").exists());
    assert!(
        recovered_service
            .start_execution(approved.session.id, &mut |_| {})
            .is_err(),
        "relaunch must not re-apply a mutated operation"
    );
    eprintln!("M18.1 crash after mutation recovery={recovered:?}");

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/Facture.pdf", b"stage-crash");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 crash stage");
    let journal = app_data.path().join("journal-stage.jsonl.enc");
    let service = packaged_service_with(&fixture, &journal, root, Some("after_stage"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/Facture.pdf".to_owned(),
        "incoming/facture.pdf".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("stage-crash preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let started = service.start_execution(approved.session.id, &mut |_| {});
    assert!(
        started.is_err()
            || started.as_ref().is_ok_and(|detail| {
                detail.session.status != domain::OrganizationExecutionStatus::Completed
            }),
        "case-only staging crash must not complete: {started:?}"
    );
    let staged = staging_artifacts(sandbox.path());
    assert_eq!(staged.len(), 1, "exactly one staging artifact: {staged:?}");
    assert_eq!(fs::read(&staged[0]).unwrap(), b"stage-crash");
    assert!(
        display_leaf(sandbox.path(), "incoming", "facture.pdf").is_none(),
        "final case-only name must not appear as completed output"
    );
    drop(service);
    let recovered_service = packaged_service_with(&fixture, &journal, root, None);
    let recovered = recovered_service
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("stage-crash recover: {error}"));
    let copies = snapshot_tree(sandbox.path())
        .into_iter()
        .filter(|(path, _)| {
            !path.starts_with(".supremacy-staging")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("facture.pdf"))
        })
        .count();
    assert!(
        copies <= 1,
        "recovery must not duplicate the case-only file"
    );
    assert_eq!(
        snapshot_tree(sandbox.path())
            .values()
            .filter(|bytes| bytes.as_slice() == b"stage-crash")
            .count(),
        1,
        "bytes must exist exactly once after staging crash recovery"
    );
    eprintln!(
        "M18.1 case-only staging crash recovery={recovered:?} staging={:?}",
        staging_artifacts(sandbox.path())
    );
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_crash_after_ack_before_coordinator_persist() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CrashAfterAck {
        inner: Arc<dyn ApprovedExecutorClient>,
        seen: Arc<AtomicUsize>,
    }
    impl ApprovedExecutorClient for CrashAfterAck {
        fn open_session(
            &self,
            envelope: ipc_contracts::executor_v2::ImmutableExecutionEnvelope,
            authorization: ipc_contracts::executor_v2::SessionAuthorization,
        ) -> Result<Box<dyn application::ApprovedExecutorSession>, application::ApprovedExecutorError>
        {
            Ok(Box::new(CrashAfterAckSession {
                inner: self.inner.open_session(envelope, authorization)?,
                seen: Arc::clone(&self.seen),
            }))
        }
    }
    struct CrashAfterAckSession {
        inner: Box<dyn application::ApprovedExecutorSession>,
        seen: Arc<AtomicUsize>,
    }
    impl application::ApprovedExecutorSession for CrashAfterAckSession {
        fn identity(&self) -> &domain::ExecutorSessionIdentity {
            self.inner.identity()
        }
        fn prepare_operation(
            &mut self,
            operation_id: domain::OperationStepId,
            direction: ipc_contracts::executor_v2::OperationDirection,
        ) -> Result<domain::ExecutorRequestIdentity, application::ApprovedExecutorError> {
            self.inner.prepare_operation(operation_id, direction)
        }
        fn dispatch_prepared(
            &mut self,
            request: domain::ExecutorRequestIdentity,
            journal_intent: ipc_contracts::executor_v2::CommittedJournalEventBinding,
        ) -> Result<application::ExecutorDispatchResult, application::ApprovedExecutorError>
        {
            let dispatched = self.inner.dispatch_prepared(request, journal_intent)?;
            if self.seen.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(application::ApprovedExecutorError::Ambiguous(
                    "qualification crash after executor acknowledgement".to_owned(),
                ));
            }
            Ok(dispatched)
        }
    }

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.txt", b"ack-boundary");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18.1 crash ack");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let root = executor_root_authority();
    let inner: Arc<dyn ApprovedExecutorClient> = Arc::new(
        ProcessApprovedExecutorClient::new(packaged_sidecar(), root)
            .unwrap_or_else(|error| panic!("ack client: {error}")),
    );
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(CrashAfterAck {
        inner,
        seen: Arc::new(AtomicUsize::new(0)),
    });
    let mut policy = ExecutionSafetyPolicy::default();
    policy.allow_qualified_case_only_rename = true;
    let journal = app_data.path().join("journal-ack.jsonl.enc");
    let service = ExecutionApplicationService::new(
        fixture.database.clone(),
        fixture.platform.clone(),
        executor,
        FileJournal::open_or_locked(&journal, JournalKey::from_bytes([18; 32]), 1),
        ApplyGate {
            enabled: true,
            reason: "packaged ack-boundary crash".to_owned(),
        },
        policy,
        ExecutionConsentAuthorityKey::derive(&root),
    )
    .unwrap_or_else(|error| panic!("ack service: {error}"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.txt".to_owned(),
        "Organized/item.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("ack preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let started = service.start_execution(approved.session.id, &mut |_| {});
    assert!(started.is_err(), "ack-boundary crash must fail closed");
    drop(service);
    let recovered_service = packaged_service_with(&fixture, &journal, root, None);
    let recovered = recovered_service
        .recover_execution(approved.session.id)
        .unwrap_or_else(|error| panic!("ack recover: {error}"));
    let source = sandbox.path().join("incoming/item.txt").exists();
    let destination = sandbox.path().join("Organized/item.txt").exists();
    assert!(
        source ^ destination,
        "ack-boundary recovery must not duplicate: source={source} dest={destination}"
    );
    assert_eq!(
        if destination {
            fs::read(sandbox.path().join("Organized/item.txt")).unwrap()
        } else {
            fs::read(sandbox.path().join("incoming/item.txt")).unwrap()
        },
        b"ack-boundary"
    );
    eprintln!("M18.1 ack-boundary crash recovery={recovered:?} dest={destination}");
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_no_overwrite_drift_symlink_permission_and_blocked_undo() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/keep.txt", b"source");
    sandbox.write("incoming/drift.txt", b"before");
    let fixture = CatalogFixture::scan(&sandbox, 2, "M18 packaged safety");
    fs::create_dir_all(sandbox.path().join("Organized")).unwrap();
    sandbox.write("Organized/keep.txt", b"already-here");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/keep.txt".to_owned(),
        "Organized/keep.txt".to_owned(),
    )]);
    let prepared = service.prepare_execution(proposal.id, proposal.revision);
    assert!(
        prepared.is_err()
            || prepared.is_ok_and(|detail| detail.operations.iter().any(|operation| {
                operation.error_code.as_deref() == Some("destination_exists")
                    || operation.status != domain::ExecutionOperationStatus::PreflightOk
            }))
    );
    assert_eq!(
        fs::read(sandbox.path().join("Organized/keep.txt")).unwrap(),
        b"already-here"
    );
    assert_eq!(
        fs::read(sandbox.path().join("incoming/keep.txt")).unwrap(),
        b"source"
    );

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/drift.txt", b"before");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 packaged drift");
    let proposal = fixture.persist_proposal(&[(
        "incoming/drift.txt".to_owned(),
        "Organized/drift.txt".to_owned(),
    )]);
    sandbox.write("incoming/drift.txt", b"changed-after-scan");
    let service = packaged_service(&fixture, &app_data.path().join("journal-drift.jsonl.enc"));
    let prepared = service.prepare_execution(proposal.id, proposal.revision);
    assert!(
        prepared.is_err()
            || prepared.is_ok_and(|detail| detail.operations.iter().any(|operation| {
                operation
                    .error_code
                    .as_deref()
                    .is_some_and(|code| code.contains("drift") || code.contains("hash"))
                    || operation.status != domain::ExecutionOperationStatus::PreflightOk
            }))
    );
    assert_eq!(
        fs::read(sandbox.path().join("incoming/drift.txt")).unwrap(),
        b"changed-after-scan"
    );
    assert!(!sandbox.path().join("Organized/drift.txt").exists());

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/real.txt", b"real");
    let outside = Builder::new()
        .prefix("supremacy-m18-step2-outside-")
        .tempdir()
        .unwrap_or_else(|error| panic!("outside: {error}"));
    fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), sandbox.path().join("incoming/escape"))
        .unwrap_or_else(|error| panic!("symlink: {error}"));
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 packaged symlink");
    let service = packaged_service(&fixture, &app_data.path().join("journal-link.jsonl.enc"));
    if fixture
        .source_by_path
        .contains_key("incoming/escape/secret.txt")
    {
        panic!("symlink traversal was indexed as a source");
    }
    let proposal = fixture.persist_proposal(&[(
        "incoming/real.txt".to_owned(),
        "incoming/escape/real.txt".to_owned(),
    )]);
    let prepared = service.prepare_execution(proposal.id, proposal.revision);
    assert!(
        prepared.is_err()
            || prepared.is_ok_and(|detail| {
                detail.session.summary.failed + detail.session.summary.blocked > 0
                    || detail.operations.iter().any(|operation| {
                        operation.status != domain::ExecutionOperationStatus::PreflightOk
                    })
            })
    );
    assert_eq!(
        fs::read(outside.path().join("secret.txt")).unwrap(),
        b"secret"
    );

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/locked.txt", b"locked");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 packaged permission");
    let path = sandbox.path().join("incoming/locked.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let service = packaged_service(&fixture, &app_data.path().join("journal-perm.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/locked.txt".to_owned(),
        "Organized/locked.txt".to_owned(),
    )]);
    let prepared = service.prepare_execution(proposal.id, proposal.revision);
    if let Ok(prepared) = prepared {
        let approved = attest(&service, prepared.session.id, None);
        let completed = service.start_execution(approved.session.id, &mut |_| {});
        assert!(
            completed.is_err()
                || completed.is_ok_and(|detail| detail.session.summary.failed
                    + detail.session.summary.blocked
                    > 0)
        );
    }
    assert_eq!(fs::read(&path).unwrap(), b"locked");
    assert!(!sandbox.path().join("Organized/locked.txt").exists());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/report.txt", b"original");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 packaged blocked undo");
    let service = packaged_service(&fixture, &app_data.path().join("journal-undo.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/report.txt".to_owned(),
        "Organized/report.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("apply: {error}"));
    let destination = sandbox.path().join("Organized/report.txt");
    fs::write(&destination, b"user-edited-after-apply").unwrap();
    let rolled = service
        .rollback_execution(completed.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("blocked undo: {error}"));
    assert_eq!(
        rolled.session.status,
        domain::OrganizationExecutionStatus::RollbackPartial
    );
    assert!(rolled.session.summary.rollback_blocked >= 1);
    assert_eq!(fs::read(&destination).unwrap(), b"user-edited-after-apply");
    assert!(!sandbox.path().join("incoming/report.txt").exists());
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_unicode_and_busy_file_behavior() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/facture été 发票.txt", b"unicode");
    sandbox.write("incoming/space name.txt", b"spaces");
    sandbox.write("incoming/emoji-📄.txt", b"emoji");
    let fixture = CatalogFixture::scan(&sandbox, 3, "M18 packaged unicode");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[
        (
            "incoming/facture été 发票.txt".to_owned(),
            "Organized/facture été 发票.txt".to_owned(),
        ),
        (
            "incoming/space name.txt".to_owned(),
            "Organized/space name.txt".to_owned(),
        ),
        (
            "incoming/emoji-📄.txt".to_owned(),
            "Organized/emoji-📄.txt".to_owned(),
        ),
    ]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("unicode preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service
        .start_execution(approved.session.id, &mut |_| {})
        .unwrap_or_else(|error| panic!("unicode apply: {error}"));
    assert_eq!(completed.session.summary.failed, 0);
    assert_eq!(
        fs::read(sandbox.path().join("Organized/facture été 发票.txt")).unwrap(),
        b"unicode"
    );
    assert_eq!(
        fs::read(sandbox.path().join("Organized/space name.txt")).unwrap(),
        b"spaces"
    );
    assert_eq!(
        fs::read(sandbox.path().join("Organized/emoji-📄.txt")).unwrap(),
        b"emoji"
    );

    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/open.txt", b"busy");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 packaged busy");
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(sandbox.path().join("incoming/open.txt"))
        .unwrap_or_else(|error| panic!("open held: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal-busy.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/open.txt".to_owned(),
        "Organized/open.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("busy preflight: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    let completed = service.start_execution(approved.session.id, &mut |_| {});
    eprintln!(
        "M18 packaged busy-file (open writable handle) outcome: {:?}",
        completed
            .as_ref()
            .map(|detail| (
                detail.session.status,
                detail.session.summary.failed,
                detail.session.summary.blocked
            ))
            .map_err(|error| error.to_string())
    );
    drop(held);
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_revoked_access_after_preview_fails_closed() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/item.txt", b"revoke");
    let fixture = CatalogFixture::scan(&sandbox, 1, "M18 packaged revoke");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let service = packaged_service(&fixture, &app_data.path().join("journal.jsonl.enc"));
    let proposal = fixture.persist_proposal(&[(
        "incoming/item.txt".to_owned(),
        "Organized/item.txt".to_owned(),
    )]);
    let prepared = service
        .prepare_execution(proposal.id, proposal.revision)
        .unwrap_or_else(|error| panic!("preflight before revoke: {error}"));
    let approved = attest(&service, prepared.session.id, None);
    struct RestorePermissions<'a> {
        path: &'a Path,
    }
    impl Drop for RestorePermissions<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.path, fs::Permissions::from_mode(0o755));
        }
    }
    let _restore = RestorePermissions {
        path: sandbox.path(),
    };
    fs::set_permissions(sandbox.path(), fs::Permissions::from_mode(0o000))
        .unwrap_or_else(|error| panic!("revoke permissions: {error}"));
    let started = service.start_execution(approved.session.id, &mut |_| {});
    drop(_restore);
    assert!(
        started.is_err()
            || started.is_ok_and(|detail| detail.session.summary.failed
                + detail.session.summary.blocked
                > 0
                || !sandbox.path().join("Organized/item.txt").exists())
    );
    assert_eq!(
        fs::read(sandbox.path().join("incoming/item.txt")).unwrap(),
        b"revoke"
    );
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_relaunch_journal_remains_coherent() {
    let sandbox = MutationSandbox::new();
    sandbox.write("incoming/keep.txt", b"relaunch");
    let app_data = Builder::new()
        .prefix("supremacy-m18-step2-appdata-")
        .tempdir()
        .unwrap_or_else(|error| panic!("app data: {error}"));
    let db_path = app_data.path().join("catalog.db");
    let journal_path = app_data.path().join("operation-recovery.jsonl.enc");
    let key = DatabaseKey::from_bytes([18; 32]);
    let database =
        Arc::new(Database::open(&db_path, &key).unwrap_or_else(|error| panic!("db open: {error}")));
    let platform: Arc<dyn ReadOnlyPlatform> = Arc::new(platform_macos::MacOsPlatform);
    let scanner = ScannerApplicationService::new(database.clone(), platform.clone());
    let workspace = scanner.create_workspace("relaunch").unwrap();
    scanner.register_root(workspace.id, sandbox.path()).unwrap();
    scanner
        .scan_workspace(workspace.id, &|| false, &mut |_| {})
        .unwrap();
    drop(scanner);
    drop(database);
    let database = Arc::new(
        Database::open(&db_path, &key).unwrap_or_else(|error| panic!("db reopen: {error}")),
    );
    let fixture_platform: Arc<dyn ReadOnlyPlatform> = Arc::new(platform_macos::MacOsPlatform);
    let root = executor_root_authority();
    let executor: Arc<dyn ApprovedExecutorClient> = Arc::new(
        ProcessApprovedExecutorClient::new(packaged_sidecar(), root)
            .unwrap_or_else(|error| panic!("client: {error}")),
    );
    let mut policy = ExecutionSafetyPolicy::default();
    policy.allow_qualified_case_only_rename = true;
    let service = ExecutionApplicationService::new(
        database,
        fixture_platform,
        executor,
        FileJournal::open_or_locked(&journal_path, JournalKey::from_bytes([18; 32]), 1),
        ApplyGate {
            enabled: true,
            reason: "relaunch".to_owned(),
        },
        policy,
        ExecutionConsentAuthorityKey::derive(&root),
    )
    .unwrap_or_else(|error| panic!("reopen service: {error}"));
    assert!(
        !service
            .system_status()
            .unwrap_or_else(|error| panic!("status: {error}"))
            .journal_locked
    );
    assert!(journal_path.is_file() || !journal_path.exists());
}

#[test]
#[ignore = "requires WORKING_NAME_PACKAGED_APP release bundle"]
fn packaged_batch_timings_10_100_1000() {
    for count in [10_usize, 100, 1000] {
        let sandbox = MutationSandbox::new();
        let mut mappings = Vec::with_capacity(count);
        for index in 0..count {
            let source = format!("incoming/item-{index:04}.txt");
            let destination = format!("Organized/item-{index:04}.txt");
            sandbox.write(&source, format!("batch-{count}-{index}").as_bytes());
            mappings.push((source, destination));
        }
        let fixture = CatalogFixture::scan(&sandbox, count, &format!("M18 batch {count}"));
        let app_data = Builder::new()
            .prefix("supremacy-m18-step2-appdata-")
            .tempdir()
            .unwrap_or_else(|error| panic!("app data: {error}"));
        let service = packaged_service(&fixture, &app_data.path().join("journal.jsonl.enc"));
        let proposal = fixture.persist_proposal(&mappings);
        let preflight_started = Instant::now();
        let prepared = service
            .prepare_execution(proposal.id, proposal.revision)
            .unwrap_or_else(|error| panic!("batch {count} preflight: {error}"));
        let preflight = preflight_started.elapsed();
        let phrase = (count >= 1000).then_some("ORGANIZE");
        let approved = attest(&service, prepared.session.id, phrase);
        eprintln!(
            "M18 packaged batch {count} attested status={:?} recovery={:?}",
            approved.session.status, approved.session.recovery_state
        );
        let apply_started = Instant::now();
        let completed = service
            .start_execution(approved.session.id, &mut |_| {})
            .unwrap_or_else(|error| {
                panic!(
                    "batch {count} apply: {error} status={:?} recovery={:?}",
                    approved.session.status, approved.session.recovery_state
                )
            });
        let apply = apply_started.elapsed();
        assert_eq!(completed.session.summary.failed, 0, "batch {count}");
        let undo_started = Instant::now();
        let rolled = service
            .rollback_execution(completed.session.id, &mut |_| {})
            .unwrap_or_else(|error| panic!("batch {count} undo: {error}"));
        let undo = undo_started.elapsed();
        assert_eq!(
            rolled.session.status,
            domain::OrganizationExecutionStatus::RolledBack
        );
        eprintln!(
            "M18 packaged batch {count}: preflight_ms={} apply_ms={} undo_ms={} total_ms={}",
            duration_ms(preflight),
            duration_ms(apply),
            duration_ms(undo),
            duration_ms(preflight + apply + undo)
        );
    }
}
