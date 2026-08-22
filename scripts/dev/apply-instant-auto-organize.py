from pathlib import Path


def replace_one(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:100]!r}")
    file.write_text(text.replace(old, new, 1), encoding="utf-8")
    print(f"patched {path}")


# -----------------------------------------------------------------------------
# Persistence: operational_mode already exists in the encrypted schema; expose
# one narrow setter instead of adding another preferences store.
# -----------------------------------------------------------------------------
replace_one(
    "crates/persistence/src/monitoring.rs",
    '''    pub fn configure_root_monitoring(\n''',
    '''    pub fn set_workspace_monitoring_mode(\n        &self,\n        workspace_id: WorkspaceId,\n        mode: MonitoringMode,\n    ) -> Result<WorkspaceMonitoringStateRecord, PersistenceError> {\n        let connection = self.lock()?;\n        let transaction = connection.unchecked_transaction()?;\n        ensure_workspace_state(&transaction, workspace_id)?;\n        transaction.execute(\n            "UPDATE workspace_monitoring_state\n             SET operational_mode = ?2,\n                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')\n             WHERE workspace_id = ?1",\n            params![workspace_id.to_string(), mode.database_name()],\n        )?;\n        let state = workspace_monitoring_state_from_connection(&transaction, workspace_id)?;\n        transaction.commit()?;\n        Ok(state)\n    }\n\n    pub fn configure_root_monitoring(\n''',
)

# -----------------------------------------------------------------------------
# Application monitoring: Automatic stays opt-in, uses a 92% threshold, and
# claims one due job per cycle so one ambiguous file never drags other files
# into the same physical execution.
# -----------------------------------------------------------------------------
replace_one(
    "crates/application/src/monitoring.rs",
    '''const MAXIMUM_JOB_BATCH: usize = 500;\nconst ACTIVITY_LIMIT: usize = 40;\n''',
    '''const MAXIMUM_JOB_BATCH: usize = 500;\nconst AUTOMATIC_JOB_BATCH: usize = 1;\npub(crate) const AUTOMATIC_CONFIDENCE_THRESHOLD: f32 = 0.92;\nconst ACTIVITY_LIMIT: usize = 40;\n''',
)

replace_one(
    "crates/application/src/monitoring.rs",
    '''        let state = self.database.get_workspace_monitoring_state(workspace_id)?;\n        let roots = self.database.list_monitored_roots(workspace_id)?;\n''',
    '''        let state = self.database.get_workspace_monitoring_state(workspace_id)?;\n        let proposal_only = state.mode != MonitoringMode::Automatic;\n        let roots = self.database.list_monitored_roots(workspace_id)?;\n''',
)
replace_one(
    "crates/application/src/monitoring.rs",
    '''            local_only: true,\n            proposal_only: true,\n''',
    '''            local_only: true,\n            proposal_only,\n''',
)

replace_one(
    "crates/application/src/monitoring.rs",
    '''    pub fn pause_monitoring(\n''',
    '''    pub fn set_monitoring_mode(\n        &self,\n        workspace_id: WorkspaceId,\n        mode: MonitoringMode,\n    ) -> Result<MonitoringDashboard, ApplicationError> {\n        self.ensure_monitoring_configuration(workspace_id)?;\n        self.database\n            .set_workspace_monitoring_mode(workspace_id, mode)?;\n        self.monitoring_dashboard(workspace_id)\n    }\n\n    pub fn pause_monitoring(\n''',
)

replace_one(
    "crates/application/src/monitoring.rs",
    '''        if state.mode != MonitoringMode::Prudent || state.paused || is_cancelled() {\n            return self.monitoring_dashboard(workspace_id);\n        }\n''',
    '''        if state.paused || is_cancelled() {\n            return self.monitoring_dashboard(workspace_id);\n        }\n''',
)

replace_one(
    "crates/application/src/monitoring.rs",
    '''        let due_ids = self\n            .database\n            .list_due_monitoring_jobs_for_workspace(workspace_id, now, MAXIMUM_JOB_BATCH)?\n''',
    '''        let mode = self.database.get_workspace_monitoring_state(workspace_id)?.mode;\n        let batch_limit = if mode == MonitoringMode::Automatic {\n            AUTOMATIC_JOB_BATCH\n        } else {\n            MAXIMUM_JOB_BATCH\n        };\n        let due_ids = self\n            .database\n            .list_due_monitoring_jobs_for_workspace(workspace_id, now, batch_limit)?\n''',
)

old_proposal_block = '''        let has_current = self\n            .database\n            .current_organization_proposal_id_for_root(root.workspace_id, root.root_id)?\n            .is_some();\n        let proposal =\n            if has_current && (!dirty_file_ids.is_empty() || !deleted_file_ids.is_empty()) {\n                self.update_organization_proposal_incrementally(\n                    root.workspace_id,\n                    root.root_id,\n                    &dirty_file_ids,\n                    &deleted_file_ids,\n                    is_cancelled,\n                    &mut |_| {},\n                )?\n                .proposal\n            } else {\n                self.generate_organization_proposal_for_root(\n                    root.workspace_id,\n                    root.root_id,\n                    has_current,\n                    is_cancelled,\n                    &mut |_| {},\n                )?\n            };\n'''
new_proposal_block = '''        let monitoring_mode = self\n            .database\n            .get_workspace_monitoring_state(root.workspace_id)?\n            .mode;\n        let has_current = self\n            .database\n            .current_organization_proposal_id_for_root(root.workspace_id, root.root_id)?\n            .is_some();\n        let proposal = if monitoring_mode == MonitoringMode::Automatic\n            && !dirty_file_ids.is_empty()\n        {\n            self.generate_automatic_monitoring_proposal_for_files(\n                root.workspace_id,\n                root.root_id,\n                &dirty_file_ids,\n                is_cancelled,\n                &mut |_| {},\n            )?\n        } else if has_current && (!dirty_file_ids.is_empty() || !deleted_file_ids.is_empty()) {\n            self.update_organization_proposal_incrementally(\n                root.workspace_id,\n                root.root_id,\n                &dirty_file_ids,\n                &deleted_file_ids,\n                is_cancelled,\n                &mut |_| {},\n            )?\n            .proposal\n        } else {\n            self.generate_organization_proposal_for_root(\n                root.workspace_id,\n                root.root_id,\n                has_current,\n                is_cancelled,\n                &mut |_| {},\n            )?\n        };\n'''
replace_one("crates/application/src/monitoring.rs", old_proposal_block, new_proposal_block)

replace_one(
    "crates/application/src/monitoring.rs",
    '''        let ready_from_proposal = proposal\n            .operations\n''',
    '''        let confidence_threshold = if monitoring_mode == MonitoringMode::Automatic {\n            AUTOMATIC_CONFIDENCE_THRESHOLD\n        } else {\n            0.80\n        };\n        let ready_from_proposal = proposal\n            .operations\n''',
)
replace_one(
    "crates/application/src/monitoring.rs",
    '''                    && !operation.needs_review\n                    && operation.confidence_score >= 0.80\n''',
    '''                    && !operation.needs_review\n                    && operation.confidence_score >= confidence_threshold\n''',
)
replace_one(
    "crates/application/src/monitoring.rs",
    '''                    && (operation.needs_review || operation.confidence_score < 0.80)\n''',
    '''                    && (operation.needs_review\n                        || operation.confidence_score < confidence_threshold)\n''',
)

# -----------------------------------------------------------------------------
# Proposal engine: build a fresh proposal containing only the files from the
# current automatic monitoring batch. This prevents old pending suggestions
# from being swept into an automatic Apply.
# -----------------------------------------------------------------------------
proposal_method = '''    pub fn generate_automatic_monitoring_proposal_for_files(\n        &self,\n        workspace_id: WorkspaceId,\n        root_id: RootId,\n        file_ids: &[FileId],\n        is_cancelled: &(dyn Fn() -> bool + Sync),\n        on_progress: &mut dyn FnMut(ProposalBuildProgress),\n    ) -> Result<OrganizationProposal, ApplicationError> {\n        if file_ids.is_empty() {\n            return Err(ApplicationError::InvalidOrganizationProposal);\n        }\n        let source = self\n            .database\n            .organization_source_for_files(workspace_id, root_id, file_ids)?;\n        let mut preferences = self.database.organization_preferences(workspace_id)?;\n        preferences.review_threshold = preferences\n            .review_threshold\n            .max(crate::monitoring::AUTOMATIC_CONFIDENCE_THRESHOLD);\n        let rules = self.database.rules(workspace_id)?;\n        let now = now_iso();\n        let inputs = source\n            .files\n            .into_iter()\n            .map(|source| source_input(source, &rules))\n            .collect::<Result<Vec<_>, _>>()?;\n        let matches = inputs\n            .iter()\n            .flat_map(|input| {\n                input\n                    .rule_evaluation\n                    .matched_rules\n                    .iter()\n                    .map(move |matched| RuleFileMatch {\n                        rule_id: matched.id,\n                        workspace_id,\n                        file_id: input.file_id,\n                        boost: 0.15,\n                        explanation: format!(\"Matched your rule: {}\", matched.explanation.trim()),\n                    })\n            })\n            .collect::<Vec<_>>();\n        self.database.replace_rule_file_matches_for_files(\n            workspace_id,\n            file_ids,\n            &matches,\n        )?;\n        let outcome = LocalOrganizationProposalEngine.build_with_mode(\n            OrganizationBuildRequest {\n                proposal_id: ProposalId::new(),\n                revision_id: OrganizationRevisionId::new(),\n                workspace_id,\n                root_id: source.root_id,\n                source_scan_id: source.scan_id,\n                revision: 1,\n                created_at: now.clone(),\n                updated_at: now,\n                source_semantic_version: source.semantic_version,\n                source_relationship_version: source.relationship_version,\n                preferences,\n                inputs,\n                overrides: Vec::new(),\n                previous_operations: Vec::new(),\n                consumer_mode: false,\n                consumer_root_kind: consumer_root_kind(self, workspace_id, root_id),\n            },\n            is_cancelled,\n            on_progress,\n        );\n        self.database.persist_organization_proposal_with_meta(\n            &outcome.proposal,\n            \"automatic_monitoring\",\n            outcome.rebuild_mode.database_name(),\n            Some(\"single_new_file_batch\"),\n            outcome.dirty_file_count,\n        )?;\n        Ok(outcome.proposal)\n    }\n\n'''
replace_one(
    "crates/application/src/proposal.rs",
    '''    pub fn latest_organization_proposal(\n''',
    proposal_method + '''    pub fn latest_organization_proposal(\n''',
)

# -----------------------------------------------------------------------------
# Tauri: explicit one-time mode activation dialog, dashboard truth, and an
# automatic execution path that still goes through proposal approval,
# preflight, authenticated consent, executor sidecar, journal and rollback.
# -----------------------------------------------------------------------------
replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''    OrganizationProposal, OrganizationProposalOperation, OrganizationProposalStatus, ProposalId,\n''',
    '''    OrganizationProposal, OrganizationProposalOperation, OrganizationProposalStatus,\n    ProposalId, ProposalOperationKind,\n''',
)
replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''    IdentitySummaryRecord, InventorySort, MonitoringExclusionKind, ReviewAction, ReviewItemRecord,\n''',
    '''    IdentitySummaryRecord, InventorySort, MonitoringExclusionKind, MonitoringMode, ReviewAction,\n    ReviewItemRecord,\n''',
)

mode_command = '''#[tauri::command(rename_all = \"camelCase\")]\nasync fn set_monitoring_mode(\n    state: State<'_, ManagedScanner>,\n    workspace_id: String,\n    mode: String,\n) -> Result<MonitoringDashboardDto, String> {\n    let workspace_id = parse_workspace_id(&workspace_id).map_err(command_error)?;\n    let requested = match mode.as_str() {\n        \"PRUDENT\" => MonitoringMode::Prudent,\n        \"AUTOMATIC\" => MonitoringMode::Automatic,\n        _ => return Err(\"Mode de surveillance inconnu.\".to_owned()),\n    };\n\n    if requested == MonitoringMode::Automatic {\n        let dashboard = state\n            .service\n            .monitoring_dashboard(workspace_id)\n            .map_err(command_error)?;\n        if dashboard.state.startup_reconciliation_pending || dashboard.counts.pending_jobs > 0 {\n            return Err(\n                \"Terminez d’abord la mise à jour des dossiers surveillés avant d’activer le rangement automatique.\"\n                    .to_owned(),\n            );\n        }\n        let execution = state.execution_service.system_status().map_err(command_error)?;\n        if !execution.apply_gate.enabled || execution.recovery_required || execution.journal_locked {\n            return Err(\n                \"Le rangement automatique reste désactivé tant que l’application sécurisée ou la récupération n’est pas prête.\"\n                    .to_owned(),\n            );\n        }\n        if !show_native_automatic_monitoring_confirmation() {\n            return Ok(monitoring_dashboard_dto(dashboard));\n        }\n    }\n\n    state\n        .service\n        .set_monitoring_mode(workspace_id, requested)\n        .map(monitoring_dashboard_dto)\n        .map_err(command_error)\n}\n\nfn show_native_automatic_monitoring_confirmation() -> bool {\n    rfd::MessageDialog::new()\n        .set_level(rfd::MessageLevel::Warning)\n        .set_title(\"Activer le rangement automatique ?\")\n        .set_description(\n            \"ZEMO pourra déplacer ou renommer automatiquement les nouveaux fichiers seulement lorsque la confiance est d’au moins 92 %.\\n\\nLes fichiers ambigus, instables ou en conflit restent dans À vérifier. Aucun fichier existant n’est remplacé. Chaque Apply reste journalisé et peut être annulé lorsque le rollback est disponible.\",\n        )\n        .set_buttons(rfd::MessageButtons::YesNo)\n        .show()\n        == rfd::MessageDialogResult::Yes\n}\n\n'''
replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''#[tauri::command(rename_all = "camelCase")]\nasync fn pause_monitoring(\n''',
    mode_command + '''#[tauri::command(rename_all = "camelCase")]\nasync fn pause_monitoring(\n''',
)

replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''        automatic_execution_enabled: false,\n''',
    '''        automatic_execution_enabled: !dashboard.proposal_only,\n''',
)

# Add automatic execution helpers before the monitoring loop.
auto_helpers = '''const AUTOMATIC_EXECUTION_THRESHOLD: f32 = 0.92;\n\nfn automatic_proposal_is_eligible(proposal: &OrganizationProposal) -> bool {\n    if proposal.status != OrganizationProposalStatus::ReadyForReview {\n        return false;\n    }\n    let candidates = proposal\n        .operations\n        .iter()\n        .filter(|operation| {\n            matches!(\n                operation.operation_kind,\n                ProposalOperationKind::MoveProposal | ProposalOperationKind::RenameProposal\n            )\n        })\n        .collect::<Vec<_>>();\n    if candidates.len() != 1 {\n        return false;\n    }\n    let operation = candidates[0];\n    operation.confidence_score >= AUTOMATIC_EXECUTION_THRESHOLD\n        && !operation.needs_review\n        && !operation.stale\n        && !operation.conflict_state.requires_review()\n}\n\nfn try_run_automatic_execution(\n    scanner: &ScannerApplicationService,\n    execution: &ExecutionApplicationService,\n    proposal: OrganizationProposal,\n) -> Result<(), ApplicationError> {\n    if !automatic_proposal_is_eligible(&proposal) {\n        return Ok(());\n    }\n    let status = execution.system_status()?;\n    if !status.apply_gate.enabled || status.recovery_required || status.journal_locked {\n        return Ok(());\n    }\n\n    let approved = scanner.set_organization_proposal_status(\n        proposal.id,\n        OrganizationProposalStatus::ApprovedForFutureApply,\n    )?;\n    let prepared = execution.prepare_execution(approved.id, approved.revision)?;\n    let execution_id = prepared.session.id;\n    let result = (|| {\n        let challenge = execution.create_execution_consent_challenge(execution_id, None)?;\n        execution.finalize_execution_consent(challenge)?;\n        execution.start_execution(execution_id, &mut |_| {})?;\n        Ok(())\n    })();\n    if result.is_err() {\n        let _ = execution.cancel_execution(execution_id);\n    }\n    result\n}\n\n'''
replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''fn start_monitoring_loop(\n''',
    auto_helpers + '''fn start_monitoring_loop(\n''',
)

replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''fn start_monitoring_loop(\n    service: Arc<ScannerApplicationService>,\n    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,\n) -> Result<(), io::Error> {\n''',
    '''fn start_monitoring_loop(\n    service: Arc<ScannerApplicationService>,\n    execution: Arc<ExecutionApplicationService>,\n    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,\n) -> Result<(), io::Error> {\n''',
)

old_cycle = '''                    let _ = service.run_monitoring_cycle(workspace_id, &|| {\n                        cancellation.load(Ordering::Relaxed)\n                    });\n                    if let Ok(mut registry) = cancellations.lock() {\n'''
new_cycle = '''                    let before = service\n                        .latest_organization_proposal(workspace_id)\n                        .ok()\n                        .map(|proposal| proposal.id);\n                    let startup_was_pending = service\n                        .monitoring_dashboard(workspace_id)\n                        .ok()\n                        .is_some_and(|dashboard| dashboard.state.startup_reconciliation_pending);\n                    let cycle = service.run_monitoring_cycle(workspace_id, &|| {\n                        cancellation.load(Ordering::Relaxed)\n                    });\n                    if let Ok(dashboard) = cycle\n                        && dashboard.state.mode == MonitoringMode::Automatic\n                        && !startup_was_pending\n                        && !cancellation.load(Ordering::Relaxed)\n                        && let Ok(proposal) = service.latest_organization_proposal(workspace_id)\n                        && Some(proposal.id) != before\n                    {\n                        let _ = try_run_automatic_execution(&service, &execution, proposal);\n                    }\n                    if let Ok(mut registry) = cancellations.lock() {\n'''
replace_one("apps/desktop/src-tauri/src/lib.rs", old_cycle, new_cycle)

replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''            start_monitoring_loop(services.scanner.clone(), monitoring_cancellations.clone())?;\n''',
    '''            start_monitoring_loop(\n                services.scanner.clone(),\n                services.execution.clone(),\n                monitoring_cancellations.clone(),\n            )?;\n''',
)

replace_one(
    "apps/desktop/src-tauri/src/lib.rs",
    '''            get_monitoring_dashboard,\n            pause_monitoring,\n''',
    '''            get_monitoring_dashboard,\n            set_monitoring_mode,\n            pause_monitoring,\n''',
)

# -----------------------------------------------------------------------------
# Renderer API/types + monitoring UI. Prudent keeps all existing wording and
# tests; Automatic gets its own explicit status/undo language.
# -----------------------------------------------------------------------------
replace_one(
    "apps/desktop/src/types.ts",
    '''  mode: "PRUDENT" | (string & Record<never, never>);\n  paused: boolean;\n  startupReconciliationPending: boolean;\n  automaticExecutionEnabled: false;\n''',
    '''  mode: "PRUDENT" | "AUTOMATIC" | "RULES" | (string & Record<never, never>);\n  paused: boolean;\n  startupReconciliationPending: boolean;\n  automaticExecutionEnabled: boolean;\n''',
)

replace_one(
    "apps/desktop/src/api.ts",
    '''export function pauseMonitoring(workspaceId: string): Promise<void> {\n''',
    '''export function setMonitoringMode(\n  workspaceId: string,\n  mode: "PRUDENT" | "AUTOMATIC",\n): Promise<MonitoringDashboard> {\n  return invoke<MonitoringDashboard>("set_monitoring_mode", { workspaceId, mode });\n}\n\nexport function pauseMonitoring(workspaceId: string): Promise<void> {\n''',
)

replace_one(
    "apps/desktop/src/MonitoringView.tsx",
    '''  runMonitoringCycle,\n  setMonitoredFolderEnabled,\n''',
    '''  runMonitoringCycle,\n  setMonitoringMode,\n  setMonitoredFolderEnabled,\n''',
)

replace_one(
    "apps/desktop/src/MonitoringView.tsx",
    '''  const safetyInvariantFailed =\n    visibleDashboard?.automaticExecutionEnabled !== undefined &&\n    visibleDashboard.automaticExecutionEnabled !== false;\n''',
    '''  const automaticMode = visibleDashboard?.mode === "AUTOMATIC";\n  const safetyInvariantFailed =\n    visibleDashboard?.mode === "PRUDENT" &&\n    visibleDashboard.automaticExecutionEnabled !== false;\n''',
)

replace_one(
    "apps/desktop/src/MonitoringView.tsx",
    '''          <p>\n            Détecte les nouveaux fichiers dans les dossiers choisis et prépare\n            de nouvelles propositions. Les fichiers ne sont pas déplacés\n            automatiquement.\n          </p>\n''',
    '''          <p>\n            {automaticMode\n              ? "Détecte chaque nouveau fichier et le range automatiquement uniquement quand ZEMO est très sûr de son choix."\n              : "Détecte les nouveaux fichiers dans les dossiers choisis et prépare de nouvelles propositions. Les fichiers ne sont pas déplacés automatiquement."}\n          </p>\n''',
)

old_safety = '''      <div className="monitoring-safety" role="status">\n        <strong>Surveillance = propositions uniquement</strong>\n        <p>\n          La surveillance prépare des propositions d’organisation. Elle ne\n          déplace, ne renomme ni ne supprime jamais de fichiers automatiquement.\n        </p>\n      </div>\n'''
new_safety = '''      <div className="monitoring-safety" role="status">\n        {automaticMode ? (\n          <>\n            <strong>Rangement automatique actif</strong>\n            <p>\n              Seuls les nouveaux fichiers à au moins 92 % de confiance, stables\n              et sans conflit sont rangés automatiquement. Les autres restent\n              dans À vérifier. Chaque Apply passe par le journal sécurisé et\n              reste annulable lorsque le rollback est disponible.\n            </p>\n          </>\n        ) : (\n          <>\n            <strong>Surveillance = propositions uniquement</strong>\n            <p>\n              La surveillance prépare des propositions d’organisation. Elle ne\n              déplace, ne renomme ni ne supprime jamais de fichiers automatiquement.\n            </p>\n          </>\n        )}\n      </div>\n'''
replace_one("apps/desktop/src/MonitoringView.tsx", old_safety, new_safety)

replace_one(
    "apps/desktop/src/MonitoringView.tsx",
    '''          <div className="monitoring-controls">\n            <button\n              type="button"\n              disabled={busy !== null}\n              onClick={() =>\n                void performAction(\n                  visibleDashboard.paused ? "resume" : "pause",\n''',
    '''          <div className="monitoring-controls">\n            <button\n              type="button"\n              className={automaticMode ? undefined : "primary-action"}\n              disabled={busy !== null || visibleDashboard.startupReconciliationPending}\n              onClick={() =>\n                void performAction("mode", () =>\n                  setMonitoringMode(\n                    workspaceId,\n                    automaticMode ? "PRUDENT" : "AUTOMATIC",\n                  ),\n                )\n              }\n            >\n              {busy === "mode"\n                ? "Mise à jour…"\n                : automaticMode\n                  ? "Revenir au mode prudent"\n                  : "Activer le rangement automatique"}\n            </button>\n            <button\n              type="button"\n              disabled={busy !== null}\n              onClick={() =>\n                void performAction(\n                  visibleDashboard.paused ? "resume" : "pause",\n''',
)

# Frontend tests: mock new command and verify opt-in/92% semantics.
replace_one(
    "apps/desktop/src/Milestone10.test.tsx",
    '''  runMonitoringCycle: vi.fn(),\n  setMonitoredFolderEnabled: vi.fn(),\n''',
    '''  runMonitoringCycle: vi.fn(),\n  setMonitoringMode: vi.fn(),\n  setMonitoredFolderEnabled: vi.fn(),\n''',
)
replace_one(
    "apps/desktop/src/Milestone10.test.tsx",
    '''    vi.mocked(api.runMonitoringCycle).mockResolvedValue(copyDashboard());\n    vi.mocked(api.cancelMonitoring).mockResolvedValue(undefined);\n''',
    '''    vi.mocked(api.runMonitoringCycle).mockResolvedValue(copyDashboard());\n    vi.mocked(api.setMonitoringMode).mockResolvedValue(copyDashboard());\n    vi.mocked(api.cancelMonitoring).mockResolvedValue(undefined);\n''',
)

auto_test = '''\n  it("enables automatic organization explicitly and explains the 92 percent gate", async () => {\n    const automatic = copyDashboard({\n      mode: "AUTOMATIC",\n      automaticExecutionEnabled: true,\n      counts: { pendingJobs: 0 },\n    });\n    vi.mocked(api.setMonitoringMode).mockResolvedValue(automatic);\n\n    render(<MonitoringView workspaceId="workspace-10" />);\n    fireEvent.click(\n      await screen.findByRole("button", {\n        name: "Activer le rangement automatique",\n      }),\n    );\n\n    await waitFor(() => {\n      expect(api.setMonitoringMode).toHaveBeenCalledWith(\n        "workspace-10",\n        "AUTOMATIC",\n      );\n    });\n    expect(await screen.findByText("Rangement automatique actif")).toBeTruthy();\n    expect(screen.getByText(/92 % de confiance/i)).toBeTruthy();\n    expect(\n      screen.getByRole("button", { name: "Revenir au mode prudent" }),\n    ).toBeTruthy();\n  });\n'''
replace_one(
    "apps/desktop/src/Milestone10.test.tsx",
    '''  it("pauses and resumes global monitoring", async () => {\n''',
    auto_test + '''\n  it("pauses and resumes global monitoring", async () => {\n''',
)

print("instant auto-organize patch complete")
