#![cfg(any(target_os = "macos", target_os = "windows"))]

use application::{ScannerApplicationService, SemanticCorrectionAction};
use domain::{
    LocalRuleInput, OrganizationPreferences, ProposalOverrideAction, RuleAction, RuleCondition,
    RuleField, RuleOperator, SemanticRuleField,
};
use persistence::{Database, DatabaseKey};
use platform::ReadOnlyPlatform;
use search::{SearchQuery, SearchSort};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::TempDir;

#[cfg(target_os = "macos")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_macos::MacOsPlatform)
}

#[cfg(target_os = "windows")]
fn native_platform() -> Arc<dyn ReadOnlyPlatform> {
    Arc::new(platform_windows::WindowsPlatform)
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    assert!(target.starts_with(root) && target != root);
    fs::create_dir_all(
        target
            .parent()
            .unwrap_or_else(|| panic!("fixture should have a parent")),
    )
    .unwrap_or_else(|error| panic!("fixture parent should be created: {error}"));
    fs::write(target, content).unwrap_or_else(|error| panic!("fixture should be written: {error}"));
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SnapshotEntry {
    Directory,
    File { size: u64, hash: blake3::Hash },
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    let mut output = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("fixture should be readable: {error}"))
        {
            let entry = entry.unwrap_or_else(|error| panic!("entry should be readable: {error}"));
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("metadata should be readable: {error}"));
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                output.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|_| panic!("fixture path should remain scoped"))
                        .to_path_buf(),
                    SnapshotEntry::Directory,
                );
                pending.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("fixture bytes should be readable: {error}"));
                output.insert(
                    path.strip_prefix(root)
                        .unwrap_or_else(|_| panic!("fixture path should remain scoped"))
                        .to_path_buf(),
                    SnapshotEntry::File {
                        size: metadata.len(),
                        hash: blake3::hash(&bytes),
                    },
                );
            }
        }
    }
    output
}

fn file_id(
    service: &ScannerApplicationService,
    workspace_id: domain::WorkspaceId,
    filename: &str,
) -> String {
    service
        .search_files(
            workspace_id,
            SearchQuery {
                text: filename.to_owned(),
                sort: SearchSort::Filename,
                page_size: 100,
                ..SearchQuery::default()
            },
        )
        .unwrap_or_else(|error| panic!("local search should succeed: {error}"))
        .results
        .into_iter()
        .find(|result| result.filename == filename)
        .unwrap_or_else(|| panic!("fixture {filename} should be indexed"))
        .file_id
}

fn destination_rule(name: &str, path: &str, destination: &[&str]) -> LocalRuleInput {
    LocalRuleInput {
        name: name.to_owned(),
        explanation: format!("{name} is an explicit test rule."),
        enabled: true,
        conditions: vec![RuleCondition {
            field: RuleField::SourcePath,
            operator: RuleOperator::StartsWith,
            value: Some(path.to_owned()),
        }],
        action: RuleAction::SetDestination {
            segments: destination
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        },
    }
}

#[test]
fn local_rules_are_explicit_persistent_explainable_and_proposal_only() {
    let fixture = TempDir::new().expect("fixture should exist");
    let database_directory = TempDir::new().expect("database directory should exist");
    let database_path = database_directory.path().join("milestone11.db");
    let key_bytes = [111_u8; 32];
    let files = [
        (
            "Preferred/alpha.txt",
            "INVOICE\nQuarterly services\nSupplier: Point P\nInvoice number: Q-1\nDate: 2026-08-01\nTotal: 100 EUR",
        ),
        (
            "Other/beta.txt",
            "INVOICE\nQuarterly services\nSupplier: Point P\nInvoice number: Q-2\nDate: 2026-08-02\nTotal: 200 EUR",
        ),
        (
            "Other/gamma.txt",
            "INVOICE\nSupplier: Northwind\nInvoice number: G-3\nDate: 2026-08-03\nTotal: 300 EUR",
        ),
        (
            "Downloads/manual.txt",
            "INVOICE\nSupplier: Point P\nProject: Project Bordeaux\nInvoice number: M-4\nDate: 2026-08-04\nTotal: 400 EUR",
        ),
        (
            "Downloads/tax.txt",
            "INVOICE\nSupplier: Point P\nInvoice number: T-5\nDate: 2026-08-05\nTotal: 500 EUR",
        ),
    ];
    for (path, content) in files {
        write_file(fixture.path(), path, content);
    }
    let before = snapshot(fixture.path());

    let workspace_id;
    {
        let database = Arc::new(
            Database::open(&database_path, &DatabaseKey::from_bytes(key_bytes))
                .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
        );
        let service = ScannerApplicationService::new(database, native_platform());
        let workspace = service
            .create_workspace("Milestone 11 local rules")
            .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
        workspace_id = workspace.id;
        assert!(service.system_status().network_disabled);
        service
            .register_root(workspace_id, fixture.path())
            .unwrap_or_else(|error| panic!("root should register: {error}"));
        let scan = service
            .scan_workspace(workspace_id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("scan should succeed: {error}"));
        service
            .analyze_scan_content(scan.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("extraction should succeed: {error}"));
        service
            .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("semantic analysis should succeed: {error}"));

        let search_rule = service
            .create_local_rule(
                workspace_id,
                &LocalRuleInput {
                    name: "Prefer reviewed quarterly files".to_owned(),
                    explanation:
                        "Files already reviewed under Preferred receive a bounded search boost."
                            .to_owned(),
                    enabled: true,
                    conditions: vec![RuleCondition {
                        field: RuleField::SourcePath,
                        operator: RuleOperator::StartsWith,
                        value: Some("Preferred".to_owned()),
                    }],
                    action: RuleAction::PreserveSubtree,
                },
            )
            .unwrap_or_else(|error| panic!("search rule should save: {error}"));
        let search = service
            .search_files(
                workspace_id,
                SearchQuery {
                    text: "quarterly services".to_owned(),
                    sort: SearchSort::Relevance,
                    page_size: 10,
                    ..SearchQuery::default()
                },
            )
            .unwrap_or_else(|error| panic!("rule-aware search should succeed: {error}"));
        assert_eq!(search.results[0].filename, "alpha.txt");
        assert!(search.results[0].why_matched.iter().any(|reason| {
            reason.contains("Matched your rule") && reason.contains("bounded search boost")
        }));

        let first = service
            .create_local_rule(
                workspace_id,
                &destination_rule(
                    "First manual destination",
                    "Downloads/manual.txt",
                    &["Business", "First"],
                ),
            )
            .unwrap_or_else(|error| panic!("first rule should save: {error}"));
        let second = service
            .create_local_rule(
                workspace_id,
                &destination_rule(
                    "Second manual destination",
                    "Downloads/manual.txt",
                    &["Business", "Second"],
                ),
            )
            .unwrap_or_else(|error| panic!("second rule should save: {error}"));
        let proposal = service
            .generate_organization_proposal(workspace_id, false, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("proposal should build: {error}"));
        let manual_file = proposal
            .operations
            .iter()
            .find(|operation| operation.source_name == "manual.txt")
            .unwrap_or_else(|| panic!("manual fixture should be proposed"));
        assert_eq!(manual_file.proposed_destination, ["Business", "First"]);
        assert!(manual_file.reasons.iter().any(|reason| {
            reason.code == "user_rule"
                && reason
                    .explanation
                    .starts_with("Placed here because of your rule:")
        }));

        service
            .set_local_rule_enabled(workspace_id, first.id, false)
            .unwrap_or_else(|error| panic!("first rule should disable: {error}"));
        let disabled = service
            .recompute_after_rule_change(workspace_id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("disabled-rule recomputation should succeed: {error}"))
            .unwrap_or_else(|| panic!("current proposal should exist"));
        assert_eq!(
            disabled
                .operations
                .iter()
                .find(|operation| operation.source_name == "manual.txt")
                .unwrap_or_else(|| panic!("manual fixture should remain"))
                .proposed_destination,
            ["Business", "Second"]
        );
        service
            .delete_local_rule(workspace_id, second.id)
            .unwrap_or_else(|error| panic!("second rule should delete: {error}"));
        service
            .set_local_rule_enabled(workspace_id, first.id, true)
            .unwrap_or_else(|error| panic!("first rule should re-enable: {error}"));
        assert!(
            service
                .rules_preferences_state(workspace_id)
                .unwrap_or_else(|error| panic!("state should load: {error}"))
                .rules
                .iter()
                .all(|rule| rule.id != second.id)
        );

        let beta = disabled
            .operations
            .iter()
            .find(|operation| operation.source_name == "beta.txt")
            .unwrap_or_else(|| panic!("beta fixture should be proposed"));
        let beta_file_id = beta.file_id;
        service
            .set_organization_proposal_override(
                disabled.id,
                beta_file_id,
                ProposalOverrideAction::Destination,
                Some(vec!["Business".into(), "Chosen".into()]),
                None,
                Some("unrelated direct file decision".into()),
                &|| false,
                &mut |_| {},
            )
            .unwrap_or_else(|error| panic!("manual override should save: {error}"));
        service
            .update_local_rule(
                workspace_id,
                first.id,
                &destination_rule(
                    "Updated manual destination",
                    "Downloads/manual.txt",
                    &["Business", "Updated"],
                ),
            )
            .unwrap_or_else(|error| panic!("first rule should update: {error}"));
        let recomputed = service
            .recompute_after_rule_change(workspace_id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("rule recomputation should succeed: {error}"))
            .unwrap_or_else(|| panic!("current proposal should exist"));
        assert_eq!(
            recomputed
                .operations
                .iter()
                .find(|operation| operation.file_id == beta_file_id)
                .unwrap_or_else(|| panic!("overridden beta should remain"))
                .proposed_destination,
            ["Business", "Chosen"]
        );

        let semantic_rule = service
            .create_local_rule(
                workspace_id,
                &LocalRuleInput {
                    name: "Tax file semantic override".to_owned(),
                    explanation: "The tax fixture is explicitly interpreted as a tax document."
                        .to_owned(),
                    enabled: true,
                    conditions: vec![RuleCondition {
                        field: RuleField::SourcePath,
                        operator: RuleOperator::StartsWith,
                        value: Some("Downloads/tax.txt".to_owned()),
                    }],
                    action: RuleAction::SetSemanticField {
                        field: SemanticRuleField::DocumentType,
                        value: "tax_document".to_owned(),
                    },
                },
            )
            .unwrap_or_else(|error| panic!("semantic rule should save: {error}"));
        service
            .create_local_rule(
                workspace_id,
                &LocalRuleInput {
                    name: "Add an explicit project interpretation".to_owned(),
                    explanation:
                        "A rule-provided semantic field remains visible without machine evidence."
                            .to_owned(),
                    enabled: true,
                    conditions: vec![RuleCondition {
                        field: RuleField::SourcePath,
                        operator: RuleOperator::StartsWith,
                        value: Some("Downloads/tax.txt".to_owned()),
                    }],
                    action: RuleAction::SetSemanticField {
                        field: SemanticRuleField::Project,
                        value: "Tax Archive".to_owned(),
                    },
                },
            )
            .unwrap_or_else(|error| panic!("project semantic rule should save: {error}"));
        let tax_id = file_id(&service, workspace_id, "tax.txt");
        let detail = service
            .file_detail(&tax_id)
            .unwrap_or_else(|error| panic!("rule-overlaid detail should load: {error}"));
        let document_type = detail
            .semantic_analysis
            .as_ref()
            .and_then(|analysis| {
                analysis
                    .fields
                    .iter()
                    .find(|field| field.field_key == "document_type")
            })
            .unwrap_or_else(|| panic!("document type should exist"));
        assert_eq!(document_type.display_value.as_deref(), Some("tax_document"));
        assert_eq!(document_type.value_source, "user_rule");
        assert!(document_type.machine_display_value.is_some());
        let project = detail
            .semantic_analysis
            .as_ref()
            .and_then(|analysis| {
                analysis
                    .fields
                    .iter()
                    .find(|field| field.field_key == "project_reference_candidate")
            })
            .unwrap_or_else(|| panic!("a rule-only semantic field should be visible"));
        assert_eq!(project.display_value.as_deref(), Some("Tax Archive"));
        assert_eq!(project.value_source, "user_rule");
        assert!(project.machine_display_value.is_none());

        service
            .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("reanalysis should succeed: {error}"));
        let reanalyzed = service
            .file_detail(&tax_id)
            .unwrap_or_else(|error| panic!("reanalyzed detail should load: {error}"));
        assert_eq!(
            reanalyzed
                .semantic_analysis
                .as_ref()
                .and_then(|analysis| {
                    analysis
                        .fields
                        .iter()
                        .find(|field| field.field_key == "document_type")
                })
                .and_then(|field| field.display_value.as_deref()),
            Some("tax_document")
        );

        let rule_count_before_corrections = service
            .rules_preferences_state(workspace_id)
            .unwrap_or_else(|error| panic!("state should load: {error}"))
            .rules
            .len();
        let corrected_files = [
            file_id(&service, workspace_id, "alpha.txt"),
            file_id(&service, workspace_id, "beta.txt"),
            tax_id.clone(),
        ];
        for corrected_file in &corrected_files {
            service
                .store_semantic_correction(
                    corrected_file,
                    "document_type",
                    SemanticCorrectionAction::Correct,
                    Some("receipt"),
                )
                .unwrap_or_else(|error| panic!("correction should save: {error}"));
        }
        let suggested = service
            .rules_preferences_state(workspace_id)
            .unwrap_or_else(|error| panic!("suggestions should load: {error}"));
        assert_eq!(suggested.rules.len(), rule_count_before_corrections);
        let suggestion = suggested
            .suggestions
            .iter()
            .find(|suggestion| suggestion.status == domain::RuleSuggestionStatus::Pending)
            .unwrap_or_else(|| panic!("three repeated corrections should only suggest"));
        assert_eq!(suggestion.evidence_count, 3);
        let accepted = service
            .accept_local_rule_suggestion(workspace_id, suggestion.id)
            .unwrap_or_else(|error| panic!("explicit acceptance should create a rule: {error}"));
        assert_eq!(accepted.origin, domain::RuleOrigin::AcceptedSuggestion);
        let corrected_detail = service
            .file_detail(&tax_id)
            .unwrap_or_else(|error| panic!("corrected detail should load: {error}"));
        assert_eq!(
            corrected_detail
                .semantic_analysis
                .as_ref()
                .and_then(|analysis| {
                    analysis
                        .fields
                        .iter()
                        .find(|field| field.field_key == "document_type")
                })
                .and_then(|field| field.display_value.as_deref()),
            Some("tax_document"),
            "the earlier explicit rule must beat the confirmed correction and later suggestion"
        );

        let preferences = OrganizationPreferences {
            personal_root_name: "Private".to_owned(),
            business_root_name: "Company".to_owned(),
            naming_language: "fr".to_owned(),
            maximum_depth: 5,
            include_year_folders: false,
            client_first: false,
            keep_photos_inside_projects: true,
            supplier_invoices_inside_projects: true,
            preserve_existing_folders: false,
            rename_template: "{date}_{party}_{document_type}_{identifier}".to_owned(),
            review_threshold: 0.72,
            ..OrganizationPreferences::default()
        };
        service
            .store_local_organization_preferences(workspace_id, &preferences)
            .unwrap_or_else(|error| panic!("preferences should save: {error}"));
        let preference_proposal = service
            .recompute_after_rule_change(workspace_id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("preference recomputation should succeed: {error}"))
            .unwrap_or_else(|| panic!("current proposal should exist"));
        assert_eq!(
            preference_proposal
                .operations
                .iter()
                .find(|operation| operation.source_name == "gamma.txt")
                .and_then(|operation| operation.proposed_destination.first())
                .map(String::as_str),
            Some("Company")
        );
        assert_eq!(before, snapshot(fixture.path()));
        assert!(service.system_status().network_disabled);
        assert!(
            service
                .rules_preferences_state(workspace_id)
                .unwrap_or_else(|error| panic!("final state should load: {error}"))
                .rules
                .iter()
                .any(|rule| rule.id == semantic_rule.id)
        );
        assert_ne!(search_rule.id, first.id);
    }

    let reopened = Arc::new(
        Database::open(&database_path, &DatabaseKey::from_bytes(key_bytes))
            .unwrap_or_else(|error| panic!("encrypted database should reopen: {error}")),
    );
    let reopened_service = ScannerApplicationService::new(reopened, native_platform());
    let state = reopened_service
        .rules_preferences_state(workspace_id)
        .unwrap_or_else(|error| panic!("rules and preferences should survive reopen: {error}"));
    assert!(state.rules.len() >= 4);
    assert_eq!(state.preferences.personal_root_name, "Private");
    assert_eq!(state.preferences.business_root_name, "Company");
    assert_eq!(state.preferences.naming_language, "fr");
    assert_eq!(before, snapshot(fixture.path()));
    assert!(reopened_service.system_status().network_disabled);
}
