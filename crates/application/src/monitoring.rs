use crate::{ApplicationError, ScannerApplicationService};
use catalog::{
    CatalogScanner, HashingStatus, ReadabilityStatus, ScanItemStatus, ScanOutput, ScanPolicy,
    ScanProgress,
};
use domain::{FileId, OrganizationProposalStatus, RootId, ScanId, WorkspaceId};
use parking_lot::Mutex;
use persistence::{
    DuplicateGroupInput, MonitoredRootRecord, MonitoringActivityInput, MonitoringActivityRecord,
    MonitoringDashboardCountsRecord, MonitoringExclusionKind, MonitoringExclusionRecord,
    MonitoringJobRecord, MonitoringJobStage, MonitoringJobStatus, MonitoringMode,
    MonitoringRootStatus, MonitoringStabilitySample, PersistedScan, RootMonitoringConfiguration,
    RootRecord, ScanCompletionInput, ScanFileInput, ScanIssueInput, ScanKind, ScanRecord,
    WatchBackend, WatchEventInput, WatchEventKind, WatchEventScope, WatchRegistrationRecord,
    WatchRegistrationStatus, WorkspaceMonitoringStateRecord, WorkspaceRecord,
};
use platform::{
    ChangeHint, ChangeMonitor, ChangeScope, LocalChangeMonitor, LocalEventKind, PlatformError,
    PollingChangeMonitor,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const DEFAULT_SIZE_THRESHOLD_BYTES: u64 = 512 * 1_024 * 1_024;
const DEFAULT_STARTUP_ENTRY_LIMIT: u32 = 100_000;
const EVENT_DEBOUNCE_MS: i64 = 750;
const STABILITY_RECHECK_MS: i64 = 1_000;
const RETRY_BASE_MS: i64 = 1_000;
const MAXIMUM_ATTEMPTS: u8 = 8;
const MAXIMUM_JOB_BATCH: usize = 500;
const ACTIVITY_LIMIT: usize = 40;

/// Runtime-only handles. Durable monitoring state and queued work always live
/// in the encrypted database, so losing this value cannot lose events.
pub(crate) struct MonitoringRuntime {
    monitors: Mutex<HashMap<RootId, Arc<dyn ChangeMonitor>>>,
    cycle: Mutex<()>,
}

impl Default for MonitoringRuntime {
    fn default() -> Self {
        Self {
            monitors: Mutex::new(HashMap::new()),
            cycle: Mutex::new(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitoringDashboard {
    pub state: WorkspaceMonitoringStateRecord,
    pub roots: Vec<MonitoredRootRecord>,
    pub counts: MonitoringDashboardCountsRecord,
    pub activity: Vec<MonitoringActivityRecord>,
    pub exclusions: Vec<MonitoringExclusionRecord>,
    pub local_only: bool,
    pub proposal_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredWorkspaceSession {
    pub workspace: WorkspaceRecord,
    pub root: Option<RootRecord>,
    pub latest_scan: Option<ScanRecord>,
    pub safe_read_only: bool,
    pub filesystem_execution_resumed: bool,
}

#[derive(Debug)]
struct PreparedJob {
    job: MonitoringJobRecord,
    current_path: Option<PathBuf>,
    missing_path: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct BatchOutcome {
    analyzed: u64,
    ready: u64,
    review: u64,
    failed: u64,
}

impl ScannerApplicationService {
    /// Restore UI context only. Filesystem execution is deliberately never
    /// resumed by this code path.
    pub fn restore_workspace_session(
        &self,
    ) -> Result<Option<RestoredWorkspaceSession>, ApplicationError> {
        let Some(workspace) = self.database.restore_current_workspace()? else {
            return Ok(None);
        };
        let root = self.database.restore_current_root(workspace.id)?;
        let latest_scan = root
            .as_ref()
            .map(|root| self.database.latest_scan_for_root(root.id))
            .transpose()?
            .flatten();
        Ok(Some(RestoredWorkspaceSession {
            workspace,
            root,
            latest_scan,
            safe_read_only: true,
            filesystem_execution_resumed: false,
        }))
    }

    pub fn set_current_workspace(&self, workspace_id: WorkspaceId) -> Result<(), ApplicationError> {
        self.database.set_current_workspace(workspace_id)?;
        self.ensure_monitoring_configuration(workspace_id)?;
        Ok(())
    }

    /// Rebuild runtime watcher handles after restart. Interrupted work remains
    /// durable and is requeued; no proposal is ever applied automatically.
    pub fn restore_monitoring_runtime(&self) -> Result<(), ApplicationError> {
        for workspace in self.database.list_workspaces()? {
            self.database
                .normalize_interrupted_monitoring_jobs(workspace.id)?;
            self.ensure_monitoring_configuration(workspace.id)?;
            let state = self.database.get_workspace_monitoring_state(workspace.id)?;
            if !state.paused {
                self.start_workspace_monitors(workspace.id)?;
            }
        }
        Ok(())
    }

    pub fn monitoring_dashboard(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MonitoringDashboard, ApplicationError> {
        self.ensure_monitoring_configuration(workspace_id)?;
        let state = self.database.get_workspace_monitoring_state(workspace_id)?;
        let roots = self.database.list_monitored_roots(workspace_id)?;
        let counts = self.database.monitoring_dashboard_counts(workspace_id)?;
        let activity = self
            .database
            .list_monitoring_activity(workspace_id, ACTIVITY_LIMIT)?;
        let exclusions = self.all_monitoring_exclusions(workspace_id, &roots)?;
        Ok(MonitoringDashboard {
            state,
            roots,
            counts,
            activity,
            exclusions,
            local_only: true,
            proposal_only: true,
        })
    }

    pub fn pause_monitoring(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MonitoringDashboard, ApplicationError> {
        self.ensure_monitoring_configuration(workspace_id)?;
        self.database
            .set_global_monitoring_pause(workspace_id, true)?;
        for root in self.database.list_monitored_roots(workspace_id)? {
            self.stop_root_monitor(root.root_id)?;
            if root.enabled {
                self.database.set_root_monitoring_status(
                    root.root_id,
                    MonitoringRootStatus::Paused,
                    None,
                    None,
                )?;
            }
        }
        self.monitoring_dashboard(workspace_id)
    }

    pub fn resume_monitoring(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MonitoringDashboard, ApplicationError> {
        self.ensure_monitoring_configuration(workspace_id)?;
        self.database
            .set_global_monitoring_pause(workspace_id, false)?;
        self.database
            .mark_startup_reconciliation_pending(workspace_id)?;
        self.start_workspace_monitors(workspace_id)?;
        self.monitoring_dashboard(workspace_id)
    }

    pub fn set_monitored_root_enabled(
        &self,
        root_id: RootId,
        enabled: bool,
    ) -> Result<MonitoringDashboard, ApplicationError> {
        let root = self.find_monitored_root(root_id)?;
        let workspace_id = root.workspace_id;
        self.ensure_monitoring_configuration(workspace_id)?;
        self.database
            .set_root_monitoring_enabled(root_id, enabled)?;
        if enabled {
            self.database
                .mark_startup_reconciliation_pending(workspace_id)?;
            let state = self.database.get_workspace_monitoring_state(workspace_id)?;
            if !state.paused {
                let mut root = root;
                root.enabled = true;
                self.start_root_monitor(&root)?;
            }
        } else {
            self.stop_root_monitor(root_id)?;
        }
        self.monitoring_dashboard(workspace_id)
    }

    pub fn add_monitoring_exclusion(
        &self,
        workspace_id: WorkspaceId,
        root_id: Option<RootId>,
        kind: MonitoringExclusionKind,
        value: &str,
    ) -> Result<MonitoringDashboard, ApplicationError> {
        self.database
            .upsert_monitoring_exclusion(workspace_id, root_id, kind, value, true)?;
        self.monitoring_dashboard(workspace_id)
    }

    pub fn remove_monitoring_exclusion(&self, exclusion_id: &str) -> Result<(), ApplicationError> {
        if !self.database.remove_monitoring_exclusion(exclusion_id)? {
            return Err(ApplicationError::Persistence(
                persistence::PersistenceError::NotFound,
            ));
        }
        Ok(())
    }

    pub fn monitoring_workspace_ids(&self) -> Result<Vec<WorkspaceId>, ApplicationError> {
        self.database
            .list_workspaces()
            .map(|workspaces| {
                workspaces
                    .into_iter()
                    .map(|workspace| workspace.id)
                    .collect()
            })
            .map_err(ApplicationError::Persistence)
    }

    /// Persist already-coalesced watcher hints. This boundary is public so
    /// deterministic tests and future native backends can feed the same queue.
    pub fn record_monitoring_hints(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        hints: &[ChangeHint],
    ) -> Result<u64, ApplicationError> {
        let root = self
            .database
            .list_monitored_roots(workspace_id)?
            .into_iter()
            .find(|root| root.root_id == root_id)
            .ok_or(ApplicationError::Persistence(
                persistence::PersistenceError::NotFound,
            ))?;
        let registration = self.ensure_watch_registration(&root)?;
        self.persist_change_hints(&registration, hints)
    }

    /// One bounded local cycle: drain hints, perform startup gap detection,
    /// sample stability, and incrementally refresh catalog understanding,
    /// proposal, and search records.
    pub fn run_monitoring_cycle(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<MonitoringDashboard, ApplicationError> {
        let _cycle = self.monitoring.cycle.lock();
        self.ensure_monitoring_configuration(workspace_id)?;
        let state = self.database.get_workspace_monitoring_state(workspace_id)?;
        if state.mode != MonitoringMode::Prudent || state.paused || is_cancelled() {
            return self.monitoring_dashboard(workspace_id);
        }

        self.start_workspace_monitors(workspace_id)?;
        self.drain_workspace_hints(workspace_id, is_cancelled)?;

        if state.startup_reconciliation_pending && !is_cancelled() {
            self.run_startup_reconciliation(workspace_id, is_cancelled)?;
        }
        if !is_cancelled()
            && let Err(error) = self.process_due_monitoring_jobs(workspace_id, is_cancelled)
        {
            let retry_at = now_unix_ms()?.saturating_add(STABILITY_RECHECK_MS);
            self.database.recover_processing_jobs_for_workspace(
                workspace_id,
                retry_at,
                "monitoring_cycle_interrupted",
            )?;
            return Err(error);
        }
        self.monitoring_dashboard(workspace_id)
    }

    pub(crate) fn register_root_for_monitoring(
        &self,
        root: &RootRecord,
    ) -> Result<(), ApplicationError> {
        let volume = self
            .read_only_platform
            .inspect_volume(&root.absolute_path_native)?;
        if !volume.local {
            return Err(ApplicationError::InvalidMonitoringRequest);
        }
        self.database.set_current_workspace(root.workspace_id)?;
        self.database.set_current_root(root.workspace_id, root.id)?;
        let state = self
            .database
            .ensure_workspace_monitoring_state(root.workspace_id)?;
        self.database.configure_root_monitoring(
            root.id,
            RootMonitoringConfiguration {
                enabled: true,
                status: if state.paused {
                    MonitoringRootStatus::Paused
                } else {
                    MonitoringRootStatus::Starting
                },
                size_threshold_bytes: DEFAULT_SIZE_THRESHOLD_BYTES,
                startup_entry_limit: DEFAULT_STARTUP_ENTRY_LIMIT,
            },
        )?;
        self.database
            .mark_startup_reconciliation_pending(root.workspace_id)?;
        let monitored = self
            .database
            .list_monitored_roots(root.workspace_id)?
            .into_iter()
            .find(|candidate| candidate.root_id == root.id)
            .ok_or(ApplicationError::Persistence(
                persistence::PersistenceError::NotFound,
            ))?;
        if !state.paused
            && let Err(error) = self.start_root_monitor(&monitored)
        {
            self.database.set_root_monitoring_status(
                root.id,
                MonitoringRootStatus::Failed,
                Some("watch_start_failed"),
                Some(&error.to_string()),
            )?;
        }
        Ok(())
    }

    fn ensure_monitoring_configuration(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(), ApplicationError> {
        self.database.workspace(workspace_id)?;
        self.database
            .ensure_workspace_monitoring_state(workspace_id)?;
        let state = self.database.get_workspace_monitoring_state(workspace_id)?;
        let configured = self
            .database
            .list_monitored_roots(workspace_id)?
            .into_iter()
            .map(|root| root.root_id)
            .collect::<HashSet<_>>();
        for root in self.database.list_roots(workspace_id)? {
            if configured.contains(&root.id) {
                continue;
            }
            self.database.configure_root_monitoring(
                root.id,
                RootMonitoringConfiguration {
                    enabled: true,
                    status: if state.paused {
                        MonitoringRootStatus::Paused
                    } else {
                        MonitoringRootStatus::Starting
                    },
                    size_threshold_bytes: DEFAULT_SIZE_THRESHOLD_BYTES,
                    startup_entry_limit: DEFAULT_STARTUP_ENTRY_LIMIT,
                },
            )?;
        }
        Ok(())
    }

    fn find_monitored_root(
        &self,
        root_id: RootId,
    ) -> Result<MonitoredRootRecord, ApplicationError> {
        for workspace in self.database.list_workspaces()? {
            if let Some(root) = self
                .database
                .list_monitored_roots(workspace.id)?
                .into_iter()
                .find(|root| root.root_id == root_id)
            {
                return Ok(root);
            }
        }
        Err(ApplicationError::Persistence(
            persistence::PersistenceError::NotFound,
        ))
    }

    fn start_workspace_monitors(&self, workspace_id: WorkspaceId) -> Result<(), ApplicationError> {
        if self
            .database
            .get_workspace_monitoring_state(workspace_id)?
            .paused
        {
            return Ok(());
        }
        for root in self.database.list_monitored_roots(workspace_id)? {
            if !root.enabled {
                continue;
            }
            if let Err(error) = self.start_root_monitor(&root) {
                self.database.set_root_monitoring_status(
                    root.root_id,
                    MonitoringRootStatus::Failed,
                    Some("watch_start_failed"),
                    Some(&error.to_string()),
                )?;
            }
        }
        Ok(())
    }

    #[inline(never)]
    fn start_root_monitor(&self, root: &MonitoredRootRecord) -> Result<(), ApplicationError> {
        if self.monitoring.monitors.lock().contains_key(&root.root_id) {
            return Ok(());
        }
        let volume = self
            .read_only_platform
            .inspect_volume(&root.selected_path_native)?;
        if !volume.local {
            return Err(ApplicationError::InvalidMonitoringRequest);
        }
        let native: Arc<dyn ChangeMonitor> = Arc::new(LocalChangeMonitor::default());
        let (monitor, backend) = match native.start(&root.selected_path_native) {
            Ok(()) => (native, native_watch_backend()),
            Err(native_error) => {
                let polling: Arc<dyn ChangeMonitor> = Arc::new(PollingChangeMonitor::default());
                polling
                    .start(&root.selected_path_native)
                    .map_err(|polling_error| {
                        PlatformError::Unsupported(format!(
                            "native watcher failed ({native_error}); polling fallback failed ({polling_error})"
                        ))
                    })?;
                (polling, WatchBackend::Polling)
            }
        };
        let registration = self
            .database
            .ensure_watch_registration(root.root_id, backend, true)?;
        self.database.update_watch_registration(
            &registration.id,
            WatchRegistrationStatus::Active,
            None,
        )?;
        self.database.set_root_monitoring_status(
            root.root_id,
            MonitoringRootStatus::Active,
            None,
            None,
        )?;
        self.monitoring
            .monitors
            .lock()
            .insert(root.root_id, monitor);
        Ok(())
    }

    fn stop_root_monitor(&self, root_id: RootId) -> Result<(), ApplicationError> {
        if let Some(monitor) = self.monitoring.monitors.lock().remove(&root_id) {
            monitor.stop()?;
        }
        for registration in self.database.list_watch_registrations(root_id)? {
            if !matches!(
                registration.status,
                WatchRegistrationStatus::Failed | WatchRegistrationStatus::Stopped
            ) {
                self.database.update_watch_registration(
                    &registration.id,
                    WatchRegistrationStatus::Paused,
                    registration.backend_cursor.as_deref(),
                )?;
            }
        }
        Ok(())
    }

    fn ensure_watch_registration(
        &self,
        root: &MonitoredRootRecord,
    ) -> Result<WatchRegistrationRecord, ApplicationError> {
        if let Some(registration) = self
            .database
            .list_watch_registrations(root.root_id)?
            .into_iter()
            .find(|registration| {
                !matches!(
                    registration.status,
                    WatchRegistrationStatus::Failed | WatchRegistrationStatus::Stopped
                )
            })
        {
            return Ok(registration);
        }
        self.database
            .ensure_watch_registration(root.root_id, native_watch_backend(), true)
            .map_err(ApplicationError::Persistence)
    }

    fn drain_workspace_hints(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), ApplicationError> {
        let roots = self
            .database
            .list_monitored_roots(workspace_id)?
            .into_iter()
            .map(|root| (root.root_id, root))
            .collect::<HashMap<_, _>>();
        let monitors = self
            .monitoring
            .monitors
            .lock()
            .iter()
            .filter(|(root_id, _)| roots.contains_key(root_id))
            .map(|(root_id, monitor)| (*root_id, monitor.clone()))
            .collect::<Vec<_>>();
        for (root_id, monitor) in monitors {
            let Some(root) = roots.get(&root_id) else {
                continue;
            };
            match monitor.drain_hints_with_cancellation(is_cancelled) {
                Ok(hints) => {
                    let registration = self.ensure_watch_registration(root)?;
                    if let Err(error) = self.persist_change_hints(&registration, &hints) {
                        self.database
                            .mark_startup_reconciliation_pending(workspace_id)?;
                        return Err(error);
                    }
                }
                Err(error) => {
                    let status = if matches!(error, PlatformError::SourceMissing) {
                        MonitoringRootStatus::Offline
                    } else {
                        MonitoringRootStatus::Overflowed
                    };
                    self.database.set_root_monitoring_status(
                        root_id,
                        status,
                        Some("watch_drain_failed"),
                        Some(&error.to_string()),
                    )?;
                    let registration = self.ensure_watch_registration(root)?;
                    self.persist_change_hints(
                        &registration,
                        &[ChangeHint {
                            root_token: String::new(),
                            native_key: None,
                            path_after: None,
                            path_before: None,
                            kind: LocalEventKind::RescanRequired,
                            scope: ChangeScope::Unknown,
                        }],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn persist_change_hints(
        &self,
        registration: &WatchRegistrationRecord,
        hints: &[ChangeHint],
    ) -> Result<u64, ApplicationError> {
        let observed_at = now_unix_ms()?;
        let inputs = hints
            .iter()
            .map(|hint| {
                Ok(WatchEventInput {
                    registration_id: registration.id.clone(),
                    kind: watch_event_kind(hint.kind),
                    scope: watch_event_scope(hint.scope),
                    path_before: hint.path_before.clone(),
                    path_after: hint.path_after.clone(),
                    native_identity_key: hint.native_key.clone(),
                    payload_json: "{\"localOnly\":true,\"telemetry\":false}".to_owned(),
                    debounce_ready_at_unix_ms: observed_at.saturating_add(EVENT_DEBOUNCE_MS),
                    maximum_attempts: MAXIMUM_ATTEMPTS,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        for chunk in inputs.chunks(MAXIMUM_JOB_BATCH) {
            self.database.append_watch_events_and_coalesce(chunk)?;
        }
        let persisted = u64::try_from(inputs.len()).unwrap_or(u64::MAX);
        if persisted > 0 {
            self.database.record_watch_checkpoint(
                &registration.id,
                &observed_at.to_string(),
                "{\"localOnly\":true,\"telemetry\":false}",
            )?;
        }
        Ok(persisted)
    }

    fn defer_root_reconciliation(
        &self,
        root: &MonitoredRootRecord,
    ) -> Result<(), ApplicationError> {
        let registration = self.ensure_watch_registration(root)?;
        self.persist_change_hints(
            &registration,
            &[ChangeHint {
                root_token: String::new(),
                native_key: None,
                path_after: None,
                path_before: None,
                kind: LocalEventKind::RescanRequired,
                scope: ChangeScope::Unknown,
            }],
        )?;
        Ok(())
    }

    fn run_startup_reconciliation(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), ApplicationError> {
        for root in self.database.list_monitored_roots(workspace_id)? {
            if is_cancelled() {
                return Ok(());
            }
            if !root.enabled {
                continue;
            }
            self.database.set_root_monitoring_status(
                root.root_id,
                MonitoringRootStatus::Reconciling,
                None,
                None,
            )?;
            match self.reconcile_root_gap(&root, is_cancelled) {
                Ok(_) => {
                    if self.find_monitored_root(root.root_id)?.status
                        == MonitoringRootStatus::Overflowed
                    {
                        self.defer_root_reconciliation(&root)?;
                    }
                }
                Err(error) => {
                    self.database.set_root_monitoring_status(
                        root.root_id,
                        MonitoringRootStatus::Failed,
                        Some("startup_reconciliation_failed"),
                        Some(&error.to_string()),
                    )?;
                    self.defer_root_reconciliation(&root)?;
                }
            }
        }
        if !is_cancelled() {
            self.database
                .mark_startup_reconciliation_completed(workspace_id)?;
        }
        Ok(())
    }

    fn reconcile_root_gap(
        &self,
        root: &MonitoredRootRecord,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<u64, ApplicationError> {
        let root_path = root.selected_path_native.as_path();
        let mut ignored_progress = |_| {};
        let enumeration = self.read_only_platform.enumerate_regular_files(
            root_path,
            usize::try_from(root.startup_entry_limit)
                .map_err(|_| ApplicationError::InvalidMonitoringRequest)?,
            is_cancelled,
            &mut ignored_progress,
        )?;
        if enumeration.cancelled || is_cancelled() {
            return Ok(0);
        }

        let exclusions = self
            .database
            .list_monitoring_exclusions(root.workspace_id, Some(root.root_id))?;
        let reconciliation_limit = usize::try_from(root.startup_entry_limit)
            .map_err(|_| ApplicationError::InvalidMonitoringRequest)?;
        let current_snapshot = self
            .database
            .catalog_snapshot_for_root(root.root_id, reconciliation_limit)?;
        let catalog_truncated = current_snapshot.len() >= reconciliation_limit;
        let mut current = current_snapshot
            .into_iter()
            .map(|record| {
                (
                    normalized_monitoring_path(&record.current_relative_path),
                    record,
                )
            })
            .collect::<HashMap<_, _>>();
        let registration = self.ensure_watch_registration(root)?;
        let ready_at = now_unix_ms()?;
        let mut inputs = Vec::new();

        for entry in &enumeration.files {
            let relative = entry
                .absolute_path
                .strip_prefix(root_path)
                .map_err(|_| ApplicationError::Platform(PlatformError::OutsideRoot))?;
            let key = normalized_monitoring_path(relative);
            if default_excluded(relative) || user_excluded(relative, &exclusions) {
                current.remove(&key);
                continue;
            }
            let kind = match current.remove(&key) {
                Some(previous)
                    if previous.byte_size == entry.byte_size
                        && previous.modified_at_ns
                            == entry.modified_at_ns.map(|value| value.to_string()) =>
                {
                    continue;
                }
                Some(_) => WatchEventKind::Modified,
                None => WatchEventKind::Created,
            };
            inputs.push(WatchEventInput {
                registration_id: registration.id.clone(),
                kind,
                scope: WatchEventScope::File,
                path_before: None,
                path_after: Some(relative.to_path_buf()),
                native_identity_key: Some(entry.identity.object_key.clone()),
                payload_json:
                    "{\"source\":\"startup_reconciliation\",\"localOnly\":true,\"telemetry\":false}"
                        .to_owned(),
                debounce_ready_at_unix_ms: ready_at,
                maximum_attempts: MAXIMUM_ATTEMPTS,
            });
        }

        // Incomplete enumeration cannot prove absence, so it must never make a
        // catalog entry missing.
        if !enumeration.truncated && enumeration.issues.is_empty() && !catalog_truncated {
            for previous in current.into_values() {
                inputs.push(WatchEventInput {
                    registration_id: registration.id.clone(),
                    kind: WatchEventKind::Removed,
                    scope: WatchEventScope::File,
                    path_before: Some(previous.current_relative_path),
                    path_after: None,
                    native_identity_key: None,
                    payload_json:
                        "{\"source\":\"startup_reconciliation\",\"localOnly\":true,\"telemetry\":false}"
                            .to_owned(),
                    debounce_ready_at_unix_ms: ready_at,
                    maximum_attempts: MAXIMUM_ATTEMPTS,
                });
            }
        } else {
            self.database.set_root_monitoring_status(
                root.root_id,
                MonitoringRootStatus::Overflowed,
                Some("bounded_reconciliation"),
                Some("Startup reconciliation was bounded; absence was not inferred."),
            )?;
        }

        for chunk in inputs.chunks(MAXIMUM_JOB_BATCH) {
            self.database.append_watch_events_and_coalesce(chunk)?;
        }
        let event_count = u64::try_from(inputs.len()).unwrap_or(u64::MAX);
        if event_count == 0
            && !enumeration.truncated
            && enumeration.issues.is_empty()
            && !catalog_truncated
        {
            let scan_id = self.persist_empty_reconciliation(root)?;
            self.database.mark_root_reconciled(root.root_id, scan_id)?;
        } else if !enumeration.truncated && enumeration.issues.is_empty() && !catalog_truncated {
            self.database.set_root_monitoring_status(
                root.root_id,
                MonitoringRootStatus::Reconciling,
                None,
                None,
            )?;
        }
        Ok(event_count)
    }

    fn reconcile_directory_gap(
        &self,
        root: &MonitoredRootRecord,
        path_before: Option<&Path>,
        path_after: Option<&Path>,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<u64, ApplicationError> {
        if path_before.is_none() && path_after.is_none() {
            return self.reconcile_root_gap(root, is_cancelled);
        }
        let limit = usize::try_from(root.startup_entry_limit)
            .map_err(|_| ApplicationError::InvalidMonitoringRequest)?;
        let root_path = root.selected_path_native.as_path();
        let mut files = Vec::new();
        let mut complete = true;
        if let Some(after) = path_after {
            let subtree = root_path.join(after);
            let mut ignored_progress = |_| {};
            match self.read_only_platform.enumerate_regular_files(
                &subtree,
                limit,
                is_cancelled,
                &mut ignored_progress,
            ) {
                Ok(enumeration) => {
                    if enumeration.cancelled || is_cancelled() {
                        return Ok(0);
                    }
                    complete = !enumeration.truncated && enumeration.issues.is_empty();
                    files = enumeration.files;
                }
                Err(PlatformError::SourceMissing) => {}
                Err(error) => return Err(ApplicationError::Platform(error)),
            }
        }
        let scopes = [path_before, path_after]
            .into_iter()
            .flatten()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        let current_snapshot = self
            .database
            .catalog_snapshot_for_root(root.root_id, limit)?;
        if current_snapshot.len() >= limit {
            complete = false;
        }
        let mut current = current_snapshot
            .into_iter()
            .filter(|record| {
                scopes
                    .iter()
                    .any(|scope| record.current_relative_path.starts_with(scope))
            })
            .map(|record| {
                (
                    normalized_monitoring_path(&record.current_relative_path),
                    record,
                )
            })
            .collect::<HashMap<_, _>>();
        let exclusions = self
            .database
            .list_monitoring_exclusions(root.workspace_id, Some(root.root_id))?;
        let registration = self.ensure_watch_registration(root)?;
        let ready_at = now_unix_ms()?;
        let mut inputs = Vec::new();
        for entry in files {
            let relative = entry
                .absolute_path
                .strip_prefix(root_path)
                .map_err(|_| ApplicationError::Platform(PlatformError::OutsideRoot))?;
            let key = normalized_monitoring_path(relative);
            if default_excluded(relative) || user_excluded(relative, &exclusions) {
                current.remove(&key);
                continue;
            }
            let kind = match current.remove(&key) {
                Some(previous)
                    if previous.byte_size == entry.byte_size
                        && previous.modified_at_ns
                            == entry.modified_at_ns.map(|value| value.to_string()) =>
                {
                    continue;
                }
                Some(_) => WatchEventKind::Modified,
                None => WatchEventKind::Created,
            };
            inputs.push(WatchEventInput {
                registration_id: registration.id.clone(),
                kind,
                scope: WatchEventScope::File,
                path_before: None,
                path_after: Some(relative.to_path_buf()),
                native_identity_key: Some(entry.identity.object_key),
                payload_json:
                    "{\"source\":\"directory_reconciliation\",\"localOnly\":true,\"telemetry\":false}"
                        .to_owned(),
                debounce_ready_at_unix_ms: ready_at,
                maximum_attempts: MAXIMUM_ATTEMPTS,
            });
        }
        if complete {
            for previous in current.into_values() {
                inputs.push(WatchEventInput {
                    registration_id: registration.id.clone(),
                    kind: WatchEventKind::Removed,
                    scope: WatchEventScope::File,
                    path_before: Some(previous.current_relative_path),
                    path_after: None,
                    native_identity_key: None,
                    payload_json:
                        "{\"source\":\"directory_reconciliation\",\"localOnly\":true,\"telemetry\":false}"
                            .to_owned(),
                    debounce_ready_at_unix_ms: ready_at,
                    maximum_attempts: MAXIMUM_ATTEMPTS,
                });
            }
        } else {
            self.database.set_root_monitoring_status(
                root.root_id,
                MonitoringRootStatus::Overflowed,
                Some("bounded_subtree_reconciliation"),
                Some("The bounded directory reconciliation was incomplete."),
            )?;
            return Err(ApplicationError::InvalidMonitoringRequest);
        }
        for chunk in inputs.chunks(MAXIMUM_JOB_BATCH) {
            self.database.append_watch_events_and_coalesce(chunk)?;
        }
        u64::try_from(inputs.len()).map_err(|_| ApplicationError::InvalidMonitoringRequest)
    }

    fn process_due_monitoring_jobs(
        &self,
        workspace_id: WorkspaceId,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), ApplicationError> {
        let now = now_unix_ms()?;
        let roots = self
            .database
            .list_monitored_roots(workspace_id)?
            .into_iter()
            .filter(|root| root.enabled)
            .map(|root| (root.root_id, root))
            .collect::<HashMap<_, _>>();
        let exclusions_by_root = roots
            .keys()
            .map(|root_id| {
                self.database
                    .list_monitoring_exclusions(workspace_id, Some(*root_id))
                    .map(|exclusions| (*root_id, exclusions))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let due_ids = self
            .database
            .list_due_monitoring_jobs_for_workspace(workspace_id, now, MAXIMUM_JOB_BATCH)?
            .into_iter()
            .filter(|job| roots.contains_key(&job.root_id))
            .map(|job| job.id)
            .collect::<Vec<_>>();
        if due_ids.is_empty() {
            return Ok(());
        }
        let claimed = self.database.claim_monitoring_jobs(&due_ids, now)?;
        let mut prepared = HashMap::<RootId, Vec<PreparedJob>>::new();
        let mut screening_outcomes = HashMap::<RootId, BatchOutcome>::new();

        for job in claimed {
            if is_cancelled() {
                self.database.requeue_monitoring_job_after_cancellation(
                    &job.id,
                    monitoring_claim_token(&job)?,
                    now.saturating_add(STABILITY_RECHECK_MS),
                )?;
                continue;
            }
            let Some(root) = roots.get(&job.root_id) else {
                self.database.reschedule_monitoring_job(
                    &job.id,
                    monitoring_claim_token(&job)?,
                    now.saturating_add(STABILITY_RECHECK_MS),
                    Some("root_temporarily_unavailable"),
                    None,
                )?;
                continue;
            };
            if matches!(
                job.event_kind,
                WatchEventKind::Overflow | WatchEventKind::RescanRequired
            ) {
                match self.reconcile_root_gap(root, is_cancelled) {
                    Ok(_) => {
                        if is_cancelled() {
                            self.database.requeue_monitoring_job_after_cancellation(
                                &job.id,
                                monitoring_claim_token(&job)?,
                                now.saturating_add(STABILITY_RECHECK_MS),
                            )?;
                            continue;
                        }
                        if self.find_monitored_root(root.root_id)?.status
                            == MonitoringRootStatus::Overflowed
                        {
                            if self.reschedule_monitoring_job(
                                &job,
                                now,
                                "bounded_reconciliation",
                                "The bounded reconciliation remains incomplete.",
                            )? == MonitoringJobStatus::Failed
                            {
                                record_failed_screening(
                                    screening_outcomes.entry(root.root_id).or_default(),
                                );
                            }
                        } else {
                            self.database.mark_monitoring_job_completed(
                                &job.id,
                                monitoring_claim_token(&job)?,
                            )?;
                        }
                    }
                    Err(error) => {
                        if self.reschedule_monitoring_job(
                            &job,
                            now,
                            "reconciliation_failed",
                            &error.to_string(),
                        )? == MonitoringJobStatus::Failed
                        {
                            record_failed_screening(
                                screening_outcomes.entry(root.root_id).or_default(),
                            );
                        }
                    }
                }
                continue;
            }
            if job.event_scope != WatchEventScope::File {
                let reconciliation = if job.event_scope == WatchEventScope::Directory {
                    self.reconcile_directory_gap(
                        root,
                        job.path_before.as_deref(),
                        job.path_after.as_deref(),
                        is_cancelled,
                    )
                } else {
                    self.reconcile_root_gap(root, is_cancelled)
                };
                match reconciliation {
                    Ok(_) if is_cancelled() => {
                        self.database.requeue_monitoring_job_after_cancellation(
                            &job.id,
                            monitoring_claim_token(&job)?,
                            now.saturating_add(STABILITY_RECHECK_MS),
                        )?;
                    }
                    Ok(_) => {
                        self.database.mark_monitoring_job_completed(
                            &job.id,
                            monitoring_claim_token(&job)?,
                        )?;
                    }
                    Err(error) => {
                        if self.reschedule_monitoring_job(
                            &job,
                            now,
                            "directory_reconciliation_failed",
                            &error.to_string(),
                        )? == MonitoringJobStatus::Failed
                        {
                            record_failed_screening(
                                screening_outcomes.entry(root.root_id).or_default(),
                            );
                        }
                    }
                }
                continue;
            }

            let current_path = job.path_after.clone();
            let effective_path = current_path.as_deref().or(job.path_before.as_deref());
            let exclusions = exclusions_by_root
                .get(&root.root_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if effective_path
                .is_some_and(|path| default_excluded(path) || user_excluded(path, exclusions))
            {
                self.database
                    .mark_monitoring_job_excluded(&job.id, monitoring_claim_token(&job)?)?;
                continue;
            }

            if job.event_kind == WatchEventKind::Removed {
                prepared.entry(root.root_id).or_default().push(PreparedJob {
                    missing_path: job.path_before.clone(),
                    current_path: None,
                    job,
                });
                continue;
            }

            let Some(path) = current_path else {
                self.database.mark_monitoring_job_to_review(
                    &job.id,
                    monitoring_claim_token(&job)?,
                    "missing_event_path",
                )?;
                record_review_screening(screening_outcomes.entry(root.root_id).or_default());
                continue;
            };
            match self.sample_stability(root, &job, &path, now)? {
                StabilityDecision::Stable => {
                    prepared.entry(root.root_id).or_default().push(PreparedJob {
                        missing_path: moved_source_path(&job),
                        current_path: Some(path),
                        job,
                    });
                }
                StabilityDecision::Waiting => {}
                StabilityDecision::Excluded => {
                    self.database
                        .mark_monitoring_job_excluded(&job.id, monitoring_claim_token(&job)?)?;
                }
                StabilityDecision::Review(reason) => {
                    self.database.mark_monitoring_job_to_review(
                        &job.id,
                        monitoring_claim_token(&job)?,
                        reason,
                    )?;
                    record_review_screening(screening_outcomes.entry(root.root_id).or_default());
                }
                StabilityDecision::Retry(message) => {
                    if self.reschedule_monitoring_job(&job, now, "file_not_stable", &message)?
                        == MonitoringJobStatus::Failed
                    {
                        record_failed_screening(
                            screening_outcomes.entry(root.root_id).or_default(),
                        );
                    }
                }
            }
        }

        for (root_id, jobs) in prepared {
            let Some(root) = roots.get(&root_id) else {
                continue;
            };
            if let Err(error) = self.reconcile_prepared_jobs(root, &jobs, is_cancelled) {
                let mut failed = BatchOutcome::default();
                for prepared_job in &jobs {
                    if self.reschedule_monitoring_job(
                        &prepared_job.job,
                        now,
                        "incremental_pipeline_failed",
                        &error.to_string(),
                    )? == MonitoringJobStatus::Failed
                    {
                        record_failed_screening(&mut failed);
                    }
                }
                if failed.failed > 0 {
                    self.record_monitoring_activity(root, None, &failed)?;
                }
            }
        }
        for (root_id, outcome) in screening_outcomes {
            if let Some(root) = roots.get(&root_id) {
                self.record_monitoring_activity(root, None, &outcome)?;
            }
        }
        for root in self.database.list_monitored_roots(workspace_id)? {
            if root.status == MonitoringRootStatus::Reconciling && root.pending_jobs == 0 {
                self.database.set_root_monitoring_status(
                    root.root_id,
                    MonitoringRootStatus::Active,
                    None,
                    None,
                )?;
            }
        }
        Ok(())
    }

    fn sample_stability(
        &self,
        root: &MonitoredRootRecord,
        job: &MonitoringJobRecord,
        relative_path: &Path,
        now: i64,
    ) -> Result<StabilityDecision, ApplicationError> {
        match self
            .read_only_platform
            .inspect_regular_file(&root.selected_path_native, relative_path)
        {
            Ok(entry) => {
                if entry.hidden || entry.cloud_placeholder {
                    return Ok(StabilityDecision::Review("unsafe_or_remote_file"));
                }
                if entry.byte_size > root.size_threshold_bytes {
                    return Ok(StabilityDecision::Review("large_file_threshold"));
                }
                if let Err(error) = self.read_only_platform.read_prefix_scoped(
                    &root.selected_path_native,
                    relative_path,
                    1,
                ) {
                    return Ok(StabilityDecision::Retry(error.to_string()));
                }
                let modified_at = entry.modified_at_ns.map(|value| value.to_string());
                let same_sample = job.sample_byte_size == Some(entry.byte_size)
                    && job.sample_modified_at_ns == modified_at;
                if same_sample && job.stable_sample_count >= 1 {
                    return Ok(StabilityDecision::Stable);
                }
                let stable_sample_count = if same_sample {
                    job.stable_sample_count.saturating_add(1)
                } else {
                    1
                };
                self.database.update_monitoring_job_stability_sample(
                    &job.id,
                    monitoring_claim_token(job)?,
                    &MonitoringStabilitySample {
                        byte_size: entry.byte_size,
                        modified_at_ns: modified_at,
                        stable_sample_count,
                        sampled_at_unix_ms: now,
                        next_check_at_unix_ms: now.saturating_add(STABILITY_RECHECK_MS),
                    },
                )?;
                Ok(StabilityDecision::Waiting)
            }
            Err(PlatformError::CloudPlaceholder | PlatformError::ReparsePoint) => {
                Ok(StabilityDecision::Review("unsafe_or_remote_file"))
            }
            Err(PlatformError::Unsupported(_)) => Ok(StabilityDecision::Excluded),
            Err(error) => Ok(StabilityDecision::Retry(error.to_string())),
        }
    }

    fn reconcile_prepared_jobs(
        &self,
        root: &MonitoredRootRecord,
        jobs: &[PreparedJob],
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), ApplicationError> {
        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Catalog)?;
        let scan_id = ScanId::new();
        self.database.begin_scan_with_kind(
            root.workspace_id,
            root.root_id,
            scan_id,
            ScanKind::Reconciliation,
        )?;
        let mut current_paths = jobs
            .iter()
            .filter_map(|prepared| prepared.current_path.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let candidate_sizes = jobs
            .iter()
            .filter_map(|prepared| prepared.job.sample_byte_size)
            .collect::<HashSet<_>>();
        if !candidate_sizes.is_empty() {
            let candidate_limit = usize::try_from(root.startup_entry_limit)
                .map_err(|_| ApplicationError::InvalidMonitoringRequest)?;
            let candidates = self
                .database
                .catalog_snapshot_for_root(root.root_id, candidate_limit)?;
            if candidates.len() >= candidate_limit {
                self.database.set_root_monitoring_status(
                    root.root_id,
                    MonitoringRootStatus::Overflowed,
                    Some("duplicate_candidate_budget"),
                    Some("Same-root duplicate candidate discovery exceeded its bounded catalog budget."),
                )?;
                return Err(ApplicationError::InvalidMonitoringRequest);
            }
            let mut known_paths = current_paths.iter().cloned().collect::<HashSet<_>>();
            for candidate in candidates {
                if candidate_sizes.contains(&candidate.byte_size)
                    && known_paths.insert(candidate.current_relative_path.clone())
                {
                    current_paths.push(candidate.current_relative_path);
                }
            }
        }
        let scanner = CatalogScanner::new(self.read_only_platform.clone());
        let mut ignored_progress = |_: ScanProgress| {};
        let output = match scanner.scan_paths_with_id_and_control(
            scan_id,
            root.workspace_id,
            root.root_id,
            &root.selected_path_native,
            &current_paths,
            ScanPolicy {
                include_hidden: false,
                max_hash_bytes: root.size_threshold_bytes,
                ..ScanPolicy::default()
            },
            is_cancelled,
            &mut ignored_progress,
        ) {
            Ok(output) => output,
            Err(error) => {
                self.database
                    .fail_scan(scan_id, "targeted_catalog_failed")?;
                return Err(ApplicationError::Catalog(error));
            }
        };
        if output.files.iter().any(|file| {
            let Ok(relative) = file.absolute_path.strip_prefix(&root.selected_path_native) else {
                return true;
            };
            jobs.iter()
                .find(|prepared| prepared.current_path.as_deref() == Some(relative))
                .is_some_and(|prepared| {
                    prepared.job.sample_byte_size != Some(file.observation.fingerprint.byte_size)
                        || prepared.job.sample_modified_at_ns
                            != file
                                .observation
                                .fingerprint
                                .modified_at_ns
                                .map(|value| value.to_string())
                })
        }) {
            self.database.fail_scan(scan_id, "stability_drift")?;
            return Err(ApplicationError::InvalidMonitoringRequest);
        }
        let cataloged_paths = output
            .files
            .iter()
            .filter_map(|file| {
                file.absolute_path
                    .strip_prefix(&root.selected_path_native)
                    .ok()
                    .map(normalized_monitoring_path)
            })
            .collect::<HashSet<_>>();
        let persisted = self.persist_reconciliation_output(root, output)?;
        // Persist destinations first so a move keeps its native identity. If
        // destination persistence fails, the source remains current.
        for prepared in jobs {
            let Some(missing) = prepared.missing_path.as_deref() else {
                continue;
            };
            if prepared.job.event_kind == WatchEventKind::Moved {
                let destination_persisted = prepared.current_path.as_ref().is_some_and(|path| {
                    cataloged_paths.contains(&normalized_monitoring_path(path))
                });
                if !destination_persisted {
                    return Err(ApplicationError::InvalidMonitoringRequest);
                }
            }
            self.database
                .mark_current_path_missing(root.root_id, missing, scan_id)?;
        }
        let preserve_overflow =
            self.find_monitored_root(root.root_id)?.status == MonitoringRootStatus::Overflowed;
        self.database.mark_root_reconciled(root.root_id, scan_id)?;
        if preserve_overflow {
            self.database.set_root_monitoring_status(
                root.root_id,
                MonitoringRootStatus::Overflowed,
                Some("bounded_reconciliation"),
                Some("A full bounded reconciliation is still required."),
            )?;
        }
        for prepared in jobs {
            self.database.link_monitoring_job_to_reconciliation_scan(
                &prepared.job.id,
                monitoring_claim_token(&prepared.job)?,
                scan_id,
            )?;
        }

        let outcome = self.finish_incremental_pipeline(
            root,
            jobs,
            &persisted,
            &cataloged_paths,
            is_cancelled,
        )?;
        self.record_monitoring_activity(root, Some(scan_id), &outcome)?;
        Ok(())
    }

    fn finish_incremental_pipeline(
        &self,
        root: &MonitoredRootRecord,
        jobs: &[PreparedJob],
        persisted: &PersistedScan,
        cataloged_paths: &HashSet<PathBuf>,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<BatchOutcome, ApplicationError> {
        let analyzed = u64::try_from(jobs.len()).unwrap_or(u64::MAX);
        if is_cancelled() || persisted.scan.status == "cancelled" {
            self.cancel_monitoring_jobs(jobs)?;
            return Ok(BatchOutcome {
                analyzed,
                ..BatchOutcome::default()
            });
        }

        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Content)?;
        let _content = self.analyze_scan_content(persisted.scan.id, is_cancelled, &mut |_| {})?;
        if is_cancelled() {
            self.cancel_monitoring_jobs(jobs)?;
            return Ok(BatchOutcome {
                analyzed,
                ..BatchOutcome::default()
            });
        }
        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Semantic)?;
        let _semantic =
            self.analyze_scan_semantics(persisted.scan.id, is_cancelled, &mut |_| {})?;
        if is_cancelled() {
            self.cancel_monitoring_jobs(jobs)?;
            return Ok(BatchOutcome {
                analyzed,
                ..BatchOutcome::default()
            });
        }
        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Relationships)?;
        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Proposal)?;
        let dirty_file_ids = persisted
            .files
            .iter()
            .filter_map(|file| file.file_id.parse::<FileId>().ok())
            .collect::<Vec<_>>();
        let deleted_file_ids =
            self.deleted_proposal_file_ids(root.workspace_id, root.root_id, jobs)?;
        let has_current = self
            .database
            .current_organization_proposal_id_for_root(root.workspace_id, root.root_id)?
            .is_some();
        let proposal =
            if has_current && (!dirty_file_ids.is_empty() || !deleted_file_ids.is_empty()) {
                self.update_organization_proposal_incrementally(
                    root.workspace_id,
                    root.root_id,
                    &dirty_file_ids,
                    &deleted_file_ids,
                    is_cancelled,
                    &mut |_| {},
                )?
                .proposal
            } else {
                self.generate_organization_proposal_for_root(
                    root.workspace_id,
                    root.root_id,
                    has_current,
                    is_cancelled,
                    &mut |_| {},
                )?
            };
        if is_cancelled() || proposal.status == OrganizationProposalStatus::Cancelled {
            self.cancel_monitoring_jobs(jobs)?;
            return Ok(BatchOutcome {
                analyzed,
                ..BatchOutcome::default()
            });
        }

        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Search)?;
        let persisted_file_ids = persisted
            .files
            .iter()
            .map(|file| file.file_id.clone())
            .collect::<HashSet<_>>();
        let ready_from_proposal = proposal
            .operations
            .iter()
            .filter(|operation| {
                persisted_file_ids.contains(&operation.file_id.to_string())
                    && !operation.needs_review
                    && operation.confidence_score >= 0.80
            })
            .count();
        let review_from_proposal = proposal
            .operations
            .iter()
            .filter(|operation| {
                persisted_file_ids.contains(&operation.file_id.to_string())
                    && (operation.needs_review || operation.confidence_score < 0.80)
            })
            .count();
        let mut outcome = BatchOutcome {
            analyzed,
            ready: u64::try_from(ready_from_proposal).unwrap_or(u64::MAX),
            review: u64::try_from(review_from_proposal).unwrap_or(u64::MAX),
            failed: 0,
        };
        self.advance_monitoring_job_stage(jobs, MonitoringJobStage::Finalizing)?;
        for prepared in jobs {
            let cataloged = prepared
                .current_path
                .as_ref()
                .is_none_or(|path| cataloged_paths.contains(&normalized_monitoring_path(path)));
            if cataloged {
                self.database.mark_monitoring_job_completed(
                    &prepared.job.id,
                    monitoring_claim_token(&prepared.job)?,
                )?;
            } else {
                self.database.mark_monitoring_job_to_review(
                    &prepared.job.id,
                    monitoring_claim_token(&prepared.job)?,
                    "catalog_issue",
                )?;
                outcome.review = outcome.review.saturating_add(1);
            }
        }
        outcome.ready = outcome.ready.min(outcome.analyzed);
        outcome.review = outcome.review.min(outcome.analyzed);
        Ok(outcome)
    }

    fn deleted_proposal_file_ids(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        jobs: &[PreparedJob],
    ) -> Result<Vec<FileId>, ApplicationError> {
        let missing_paths = jobs
            .iter()
            .filter_map(|prepared| {
                prepared.missing_path.as_ref().map(|path| {
                    path.to_string_lossy()
                        .replace('/', "\\")
                        .trim_start_matches('\\')
                        .to_ascii_lowercase()
                })
            })
            .collect::<HashSet<_>>();
        if missing_paths.is_empty() {
            return Ok(Vec::new());
        }
        let Ok(proposal) = self
            .database
            .latest_organization_proposal_for_root(workspace_id, root_id)
        else {
            return Ok(Vec::new());
        };
        Ok(proposal
            .operations
            .into_iter()
            .filter_map(|operation| {
                let relative = operation
                    .source
                    .relative_path
                    .replace('/', "\\")
                    .trim_start_matches('\\')
                    .to_ascii_lowercase();
                missing_paths
                    .contains(&relative)
                    .then_some(operation.file_id)
            })
            .collect())
    }

    fn cancel_monitoring_jobs(&self, jobs: &[PreparedJob]) -> Result<(), ApplicationError> {
        let retry_at = now_unix_ms()?.saturating_add(STABILITY_RECHECK_MS);
        for prepared in jobs {
            self.database.requeue_monitoring_job_after_cancellation(
                &prepared.job.id,
                monitoring_claim_token(&prepared.job)?,
                retry_at,
            )?;
        }
        Ok(())
    }

    fn advance_monitoring_job_stage(
        &self,
        jobs: &[PreparedJob],
        stage: MonitoringJobStage,
    ) -> Result<(), ApplicationError> {
        let now = now_unix_ms()?;
        for prepared in jobs {
            let Some(claim_token) = prepared.job.claim_token.as_deref() else {
                return Err(ApplicationError::InvalidMonitoringRequest);
            };
            if !self.database.update_monitoring_job_stage(
                &prepared.job.id,
                claim_token,
                stage,
                now,
            )? {
                return Err(ApplicationError::InvalidMonitoringRequest);
            }
        }
        Ok(())
    }

    fn persist_reconciliation_output(
        &self,
        root: &MonitoredRootRecord,
        output: ScanOutput,
    ) -> Result<PersistedScan, ApplicationError> {
        let files = output
            .files
            .iter()
            .map(|file| ScanFileInput {
                observation: file.observation.clone(),
                extension: file.extension.clone(),
                accessed_at_ns: file.accessed_at_ns,
                readability_status: readability_status(file.readability_status).to_owned(),
                scan_status: scan_item_status(file.scan_status).to_owned(),
                hashing_status: hashing_status(file.hashing_status).to_owned(),
                error_code: file.error.map(|kind| kind.code().to_owned()),
            })
            .collect();
        let issues = output
            .issues
            .iter()
            .map(|issue| ScanIssueInput {
                relative_path: issue.relative_path.clone(),
                code: issue.kind.code().to_owned(),
                message: issue.message.clone(),
                is_directory: issue.is_directory,
                is_error: issue.kind.is_error(),
                skipped: issue.skipped,
            })
            .collect();
        let duplicate_groups = output
            .duplicate_groups
            .iter()
            .map(|group| DuplicateGroupInput {
                digest: group.key.clone(),
                byte_size: group.byte_size,
                members: group.members.clone(),
            })
            .collect();
        self.database
            .complete_scan(&ScanCompletionInput {
                scan_id: output.scan_id,
                workspace_id: root.workspace_id,
                root_id: root.root_id,
                status: if output.cancelled {
                    "cancelled".to_owned()
                } else {
                    "completed".to_owned()
                },
                files_discovered: output.progress.files_discovered,
                directories_discovered: output.progress.directories_discovered,
                bytes_discovered: output.progress.bytes_discovered,
                files_hashed: output.progress.files_hashed,
                errors: output.progress.errors,
                skipped_items: output.progress.skipped_items,
                truncated: output.truncated,
                files,
                issues,
                duplicate_groups,
            })
            .map_err(ApplicationError::Persistence)
    }

    fn persist_empty_reconciliation(
        &self,
        root: &MonitoredRootRecord,
    ) -> Result<ScanId, ApplicationError> {
        let scan_id = ScanId::new();
        self.database.begin_scan_with_kind(
            root.workspace_id,
            root.root_id,
            scan_id,
            ScanKind::Reconciliation,
        )?;
        self.database.complete_scan(&ScanCompletionInput {
            scan_id,
            workspace_id: root.workspace_id,
            root_id: root.root_id,
            status: "completed".to_owned(),
            files_discovered: 0,
            directories_discovered: 0,
            bytes_discovered: 0,
            files_hashed: 0,
            errors: 0,
            skipped_items: 0,
            truncated: false,
            files: Vec::new(),
            issues: Vec::new(),
            duplicate_groups: Vec::new(),
        })?;
        Ok(scan_id)
    }

    fn record_monitoring_activity(
        &self,
        root: &MonitoredRootRecord,
        scan_id: Option<ScanId>,
        outcome: &BatchOutcome,
    ) -> Result<(), ApplicationError> {
        let summary = format!(
            "{} files analyzed; {} organization suggestions ready; {} need review",
            outcome.analyzed, outcome.ready, outcome.review
        );
        self.database
            .record_monitoring_activity(&MonitoringActivityInput {
                batch_id: Uuid::now_v7().to_string(),
                workspace_id: root.workspace_id,
                root_id: Some(root.root_id),
                files_analyzed: outcome.analyzed,
                ready_to_organize: outcome.ready.min(outcome.analyzed),
                needs_review: outcome.review.min(outcome.analyzed),
                failed: outcome.failed.min(outcome.analyzed),
                summary,
                reconciliation_scan_id: scan_id,
            })?;
        Ok(())
    }

    fn reschedule_monitoring_job(
        &self,
        job: &MonitoringJobRecord,
        now: i64,
        error_code: &str,
        message: &str,
    ) -> Result<MonitoringJobStatus, ApplicationError> {
        let exponent = job.attempt_count.min(6);
        let multiplier = 1_i64.checked_shl(exponent).unwrap_or(i64::MAX);
        let retry_after = now.saturating_add(RETRY_BASE_MS.saturating_mul(multiplier));
        let job = self.database.reschedule_monitoring_job(
            &job.id,
            monitoring_claim_token(job)?,
            retry_after,
            Some(error_code),
            Some(message),
        )?;
        Ok(job.status)
    }

    fn all_monitoring_exclusions(
        &self,
        workspace_id: WorkspaceId,
        roots: &[MonitoredRootRecord],
    ) -> Result<Vec<MonitoringExclusionRecord>, ApplicationError> {
        let mut exclusions = self
            .database
            .list_monitoring_exclusions(workspace_id, None)?
            .into_iter()
            .map(|exclusion| (exclusion.id.clone(), exclusion))
            .collect::<HashMap<_, _>>();
        for root in roots {
            for exclusion in self
                .database
                .list_monitoring_exclusions(workspace_id, Some(root.root_id))?
            {
                exclusions.insert(exclusion.id.clone(), exclusion);
            }
        }
        let mut exclusions = exclusions.into_values().collect::<Vec<_>>();
        exclusions.sort_by(|left, right| left.value.cmp(&right.value));
        Ok(exclusions)
    }
}

#[derive(Debug)]
enum StabilityDecision {
    Stable,
    Waiting,
    Excluded,
    Review(&'static str),
    Retry(String),
}

fn record_review_screening(outcome: &mut BatchOutcome) {
    outcome.analyzed = outcome.analyzed.saturating_add(1);
    outcome.review = outcome.review.saturating_add(1);
}

fn record_failed_screening(outcome: &mut BatchOutcome) {
    outcome.analyzed = outcome.analyzed.saturating_add(1);
    outcome.review = outcome.review.saturating_add(1);
    outcome.failed = outcome.failed.saturating_add(1);
}

fn native_watch_backend() -> WatchBackend {
    #[cfg(target_os = "macos")]
    {
        WatchBackend::Fsevents
    }
    #[cfg(target_os = "windows")]
    {
        WatchBackend::ReadDirectoryChanges
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        WatchBackend::Inotify
    }
}

const fn watch_event_kind(kind: LocalEventKind) -> WatchEventKind {
    match kind {
        LocalEventKind::Created => WatchEventKind::Created,
        LocalEventKind::Modified => WatchEventKind::Modified,
        LocalEventKind::Moved => WatchEventKind::Moved,
        LocalEventKind::Removed => WatchEventKind::Removed,
        LocalEventKind::Metadata => WatchEventKind::Metadata,
        LocalEventKind::Overflow => WatchEventKind::Overflow,
        LocalEventKind::RescanRequired => WatchEventKind::RescanRequired,
    }
}

const fn watch_event_scope(scope: ChangeScope) -> WatchEventScope {
    match scope {
        ChangeScope::File => WatchEventScope::File,
        ChangeScope::Directory => WatchEventScope::Directory,
        ChangeScope::Unknown => WatchEventScope::Unknown,
    }
}

fn normalized_monitoring_path(path: &Path) -> PathBuf {
    path.to_str().map_or_else(
        || path.to_path_buf(),
        |path| {
            PathBuf::from(
                path.replace('\\', "/")
                    .trim_start_matches("./")
                    .to_lowercase(),
            )
        },
    )
}

fn moved_source_path(job: &MonitoringJobRecord) -> Option<PathBuf> {
    (job.event_kind == WatchEventKind::Moved
        && job.path_before.as_deref() != job.path_after.as_deref())
    .then(|| job.path_before.clone())
    .flatten()
}

fn monitoring_claim_token(job: &MonitoringJobRecord) -> Result<&str, ApplicationError> {
    job.claim_token
        .as_deref()
        .ok_or(ApplicationError::InvalidMonitoringRequest)
}

fn default_excluded(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(filename) = components.last() else {
        return true;
    };
    if filename.starts_with('.')
        || filename.starts_with("~$")
        || filename.ends_with('~')
        || filename.ends_with(".tmp")
        || filename.ends_with(".temp")
        || filename.ends_with(".part")
        || filename.ends_with(".partial")
        || filename.ends_with(".crdownload")
        || filename.ends_with(".swp")
    {
        return true;
    }
    components.iter().any(|component| {
        matches!(
            component.as_str(),
            ".git"
                | ".svn"
                | ".hg"
                | "node_modules"
                | "target"
                | "cache"
                | "caches"
                | "tmp"
                | "temp"
                | "$recycle.bin"
                | "system volume information"
                | ".supremacy-staging"
        ) || component.ends_with(".app")
    })
}

fn user_excluded(path: &Path, exclusions: &[MonitoringExclusionRecord]) -> bool {
    let normalized = normalized_monitoring_path(path);
    exclusions
        .iter()
        .filter(|exclusion| exclusion.enabled)
        .any(|exclusion| match exclusion.kind {
            MonitoringExclusionKind::PathPrefix => {
                let prefix = normalized_monitoring_path(Path::new(&exclusion.value));
                normalized == prefix || normalized.strip_prefix(&prefix).is_ok()
            }
            MonitoringExclusionKind::Extension => normalized.extension().is_some_and(|extension| {
                extension
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&exclusion.value)
            }),
        })
}

fn now_unix_ms() -> Result<i64, ApplicationError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            ApplicationError::Platform(PlatformError::Precondition(format!(
                "system clock precedes Unix epoch: {error}"
            )))
        })?;
    i64::try_from(duration.as_millis()).map_err(|_| ApplicationError::InvalidMonitoringRequest)
}

const fn readability_status(status: ReadabilityStatus) -> &'static str {
    match status {
        ReadabilityStatus::Readable => "readable",
        ReadabilityStatus::Unreadable => "unreadable",
        ReadabilityStatus::NotChecked => "not_checked",
    }
}

const fn scan_item_status(status: ScanItemStatus) -> &'static str {
    match status {
        ScanItemStatus::Indexed => "indexed",
        ScanItemStatus::IndexedWithErrors => "indexed_with_errors",
    }
}

const fn hashing_status(status: HashingStatus) -> &'static str {
    match status {
        HashingStatus::NotCandidate => "not_candidate",
        HashingStatus::Hashed => "hashed",
        HashingStatus::Failed => "failed",
        HashingStatus::Cancelled => "cancelled",
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use persistence::{Database, DatabaseKey, MonitoringJobStatus};
    use platform_macos::MacOsPlatform;
    use std::{fs, thread, time::Duration};
    use tempfile::TempDir;

    struct MonitoringFixture {
        _temporary: TempDir,
        root_path: PathBuf,
        database: Arc<Database>,
        service: ScannerApplicationService,
        workspace_id: WorkspaceId,
        root_id: RootId,
    }

    #[derive(Debug)]
    struct FailingChangeMonitor {
        source_missing: bool,
    }

    impl ChangeMonitor for FailingChangeMonitor {
        fn start(&self, _root: &Path) -> Result<(), PlatformError> {
            Ok(())
        }

        fn drain_hints(&self) -> Result<Vec<ChangeHint>, PlatformError> {
            if self.source_missing {
                Err(PlatformError::SourceMissing)
            } else {
                Err(PlatformError::Unsupported(
                    "synthetic poll failure".to_owned(),
                ))
            }
        }

        fn stop(&self) -> Result<(), PlatformError> {
            Ok(())
        }
    }

    fn fixture(filename: Option<(&str, &[u8])>, seed: u8) -> MonitoringFixture {
        let temporary = TempDir::new()
            .unwrap_or_else(|error| panic!("temporary monitoring root should exist: {error}"));
        let root_path = temporary.path().join("selected");
        fs::create_dir(&root_path)
            .unwrap_or_else(|error| panic!("selected root should exist: {error}"));
        if let Some((filename, bytes)) = filename {
            fs::write(root_path.join(filename), bytes)
                .unwrap_or_else(|error| panic!("fixture file should exist: {error}"));
        }
        let database = Arc::new(
            Database::open_in_memory(&DatabaseKey::from_bytes([seed; 32]))
                .unwrap_or_else(|error| panic!("encrypted database should open: {error}")),
        );
        let service = ScannerApplicationService::new(database.clone(), Arc::new(MacOsPlatform));
        let workspace = service
            .create_workspace("Continuous monitoring")
            .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
        let root = service
            .register_root(workspace.id, &root_path)
            .unwrap_or_else(|error| panic!("root should be monitored: {error}"));
        database
            .mark_startup_reconciliation_completed(workspace.id)
            .unwrap_or_else(|error| panic!("fixture startup should be reconciled: {error}"));
        MonitoringFixture {
            _temporary: temporary,
            root_path,
            database,
            service,
            workspace_id: workspace.id,
            root_id: root.id,
        }
    }

    /// Drop live watcher handles so synthetic ChangeHints are the only events
    /// under test. Under parallel workspace load, drain of native create/remove
    /// noise otherwise leaves extra pending jobs and can break move identity.
    fn silence_live_watchers(service: &ScannerApplicationService) {
        let mut monitors = service.monitoring.monitors.lock();
        for (_, monitor) in monitors.drain() {
            let _ = monitor.stop();
        }
    }

    fn created_hint(path: &str) -> ChangeHint {
        ChangeHint {
            root_token: "test-root".to_owned(),
            native_key: None,
            path_after: Some(PathBuf::from(path)),
            path_before: None,
            kind: LocalEventKind::Created,
            scope: ChangeScope::File,
        }
    }

    #[test]
    fn burst_events_are_durable_but_coalesce_to_one_bounded_job() {
        let fixture = fixture(None, 81);
        let hints = (0..200)
            .map(|_| created_hint("Inbox/invoice.txt"))
            .collect::<Vec<_>>();
        assert_eq!(
            fixture
                .service
                .record_monitoring_hints(fixture.workspace_id, fixture.root_id, &hints)
                .unwrap_or_else(|error| panic!("hints should persist: {error}")),
            200
        );
        let dashboard = fixture
            .service
            .monitoring_dashboard(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("dashboard should load: {error}"));
        assert_eq!(dashboard.counts.pending_jobs, 1);
        assert_eq!(dashboard.roots[0].pending_jobs, 1);

        let registration = fixture
            .database
            .list_watch_registrations(fixture.root_id)
            .unwrap_or_else(|error| panic!("registration should load: {error}"))
            .remove(0);
        let events = fixture
            .database
            .list_watch_events(&registration.id, None, 500)
            .unwrap_or_else(|error| panic!("raw events should remain durable: {error}"));
        assert_eq!(events.len(), 200);
        assert!(events.iter().all(|event| {
            !event
                .payload_json
                .contains(&fixture.root_path.to_string_lossy()[..])
        }));
        let jobs = fixture
            .database
            .list_due_monitoring_jobs(i64::MAX, 10)
            .unwrap_or_else(|error| panic!("coalesced job should load: {error}"));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].event_count, 200);
        assert_eq!(jobs[0].coalesced_event_count, 199);
    }

    #[test]
    fn registering_a_root_while_workspace_is_paused_does_not_start_a_watcher() {
        let fixture = fixture(None, 89);
        fixture
            .service
            .pause_monitoring(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("monitoring should pause: {error}"));
        let second_root_path = fixture._temporary.path().join("registered-while-paused");
        fs::create_dir(&second_root_path)
            .unwrap_or_else(|error| panic!("second root should exist: {error}"));
        let second_root = fixture
            .service
            .register_root(fixture.workspace_id, &second_root_path)
            .unwrap_or_else(|error| panic!("paused root should register: {error}"));

        let monitored = fixture
            .database
            .list_monitored_roots(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("paused roots should load: {error}"))
            .into_iter()
            .find(|root| root.root_id == second_root.id)
            .unwrap_or_else(|| panic!("paused root should load"));
        assert_eq!(monitored.status, MonitoringRootStatus::Paused);
        assert!(
            !fixture
                .service
                .monitoring
                .monitors
                .lock()
                .contains_key(&second_root.id)
        );
    }

    #[test]
    fn watcher_failures_replace_stale_healthy_dashboard_state() {
        let fixture = fixture(None, 93);
        fixture.service.monitoring.monitors.lock().insert(
            fixture.root_id,
            Arc::new(FailingChangeMonitor {
                source_missing: true,
            }),
        );
        let offline = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("offline watcher should fail safely: {error}"));
        assert_eq!(offline.roots[0].status, MonitoringRootStatus::Offline);
        assert!(
            offline.roots[0]
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("watch_drain_failed"))
        );

        fixture.service.monitoring.monitors.lock().insert(
            fixture.root_id,
            Arc::new(FailingChangeMonitor {
                source_missing: false,
            }),
        );
        let overflowed = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| {
                panic!("failed watcher should request reconciliation: {error}")
            });
        assert_eq!(overflowed.roots[0].status, MonitoringRootStatus::Overflowed);
        assert!(overflowed.roots[0].last_error.is_some());
    }

    #[test]
    fn directory_events_schedule_bounded_descendant_reconciliation() {
        let fixture = fixture(None, 90);
        let subtree = fixture.root_path.join("incoming");
        fs::create_dir(&subtree).unwrap_or_else(|error| panic!("subtree should exist: {error}"));
        for index in 0..12 {
            fs::write(
                subtree.join(format!("file-{index}.txt")),
                format!("file {index}"),
            )
            .unwrap_or_else(|error| panic!("descendant should exist: {error}"));
        }
        fixture
            .service
            .record_monitoring_hints(
                fixture.workspace_id,
                fixture.root_id,
                &[ChangeHint {
                    root_token: "test-root".to_owned(),
                    native_key: None,
                    path_after: Some(PathBuf::from("incoming")),
                    path_before: None,
                    kind: LocalEventKind::Created,
                    scope: ChangeScope::Directory,
                }],
            )
            .unwrap_or_else(|error| panic!("directory event should persist: {error}"));
        silence_live_watchers(&fixture.service);
        thread::sleep(Duration::from_millis(EVENT_DEBOUNCE_MS as u64 + 25));
        let dashboard = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("directory reconciliation should run: {error}"));
        assert_eq!(dashboard.counts.pending_jobs, 12);
        let jobs = fixture
            .database
            .list_due_monitoring_jobs(i64::MAX, 20)
            .unwrap_or_else(|error| panic!("descendant jobs should load: {error}"));
        assert_eq!(jobs.len(), 12);
        assert!(jobs.iter().all(|job| {
            job.coalescing_path
                .as_ref()
                .is_some_and(|path| path.starts_with("incoming"))
        }));
    }

    #[test]
    fn stable_file_runs_incremental_pipeline_without_filesystem_mutation() {
        let initial_contents = b"Invoice 2026-0811 for Project Atlas";
        let final_contents = b"Invoice 2026-0811 for Project Atlas, total EUR 42.00";
        let fixture = fixture(Some(("invoice.txt", initial_contents)), 82);
        fixture
            .service
            .record_monitoring_hints(
                fixture.workspace_id,
                fixture.root_id,
                &[created_hint("invoice.txt")],
            )
            .unwrap_or_else(|error| panic!("created hint should persist: {error}"));

        thread::sleep(Duration::from_millis(
            u64::try_from(EVENT_DEBOUNCE_MS + 100).unwrap_or(1_000),
        ));
        fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("first stability sample should run: {error}"));
        fs::write(fixture.root_path.join("invoice.txt"), final_contents)
            .unwrap_or_else(|error| panic!("simulated copy should continue: {error}"));
        thread::sleep(Duration::from_millis(
            u64::try_from(STABILITY_RECHECK_MS + 100).unwrap_or(1_100),
        ));
        let still_waiting = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("changed stability sample should run: {error}"));
        assert_eq!(still_waiting.counts.pending_jobs, 1);
        let mut dashboard = still_waiting;
        for _ in 0..4 {
            if dashboard.counts.pending_jobs == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(
                u64::try_from(STABILITY_RECHECK_MS + 100).unwrap_or(1_100),
            ));
            dashboard = fixture
                .service
                .run_monitoring_cycle(fixture.workspace_id, &|| false)
                .unwrap_or_else(|error| panic!("stable incremental pipeline should run: {error}"));
        }

        assert_eq!(dashboard.counts.pending_jobs, 0);
        assert_eq!(dashboard.counts.files_analyzed, 1);
        assert_eq!(dashboard.activity.len(), 1);
        assert_eq!(
            fs::read(fixture.root_path.join("invoice.txt"))
                .unwrap_or_else(|error| panic!("source should remain readable: {error}")),
            final_contents
        );
        assert_eq!(
            fs::read_dir(&fixture.root_path)
                .unwrap_or_else(|error| panic!("root should list: {error}"))
                .count(),
            1
        );
        assert_eq!(
            fixture
                .database
                .catalog_snapshot_for_root(fixture.root_id, 10)
                .unwrap_or_else(|error| panic!("catalog should update: {error}"))
                .len(),
            1
        );
        let source = fixture
            .database
            .organization_source(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("proposal source should be available: {error}"));
        assert_eq!(source.files.len(), 1);
        let proposal = fixture
            .service
            .latest_organization_proposal(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("proposal should be generated: {error}"));
        assert_eq!(proposal.summary.files_analyzed, 1);
        assert!(proposal.operations.iter().all(|operation| !operation.stale));
        let search = fixture
            .service
            .search_files(
                fixture.workspace_id,
                search::SearchQuery {
                    text: "Atlas".to_owned(),
                    ..search::SearchQuery::default()
                },
            )
            .unwrap_or_else(|error| panic!("incremental search index should update: {error}"));
        assert_eq!(search.total, 1);
        assert_eq!(search.results[0].filename, "invoice.txt");
    }

    #[test]
    fn interrupted_move_preserves_source_identity_until_destination_is_persisted() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let fixture = fixture(Some(("source.txt", b"move identity")), 91);
        fixture
            .service
            .scan_workspace(fixture.workspace_id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("initial catalog should complete: {error}"));
        fixture
            .database
            .mark_startup_reconciliation_completed(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("startup reconciliation should clear: {error}"));
        let original = fixture
            .database
            .catalog_snapshot_for_root(fixture.root_id, 10)
            .unwrap_or_else(|error| panic!("source identity should load: {error}"))
            .remove(0);
        fs::rename(
            fixture.root_path.join("source.txt"),
            fixture.root_path.join("destination.txt"),
        )
        .unwrap_or_else(|error| panic!("fixture move should succeed: {error}"));
        fixture
            .service
            .record_monitoring_hints(
                fixture.workspace_id,
                fixture.root_id,
                &[ChangeHint {
                    root_token: "test-root".to_owned(),
                    native_key: None,
                    path_before: Some(PathBuf::from("source.txt")),
                    path_after: Some(PathBuf::from("destination.txt")),
                    kind: LocalEventKind::Moved,
                    scope: ChangeScope::File,
                }],
            )
            .unwrap_or_else(|error| panic!("move event should persist: {error}"));
        silence_live_watchers(&fixture.service);

        thread::sleep(Duration::from_millis(
            u64::try_from(EVENT_DEBOUNCE_MS + 100).unwrap_or(1_000),
        ));
        fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("first stability sample should run: {error}"));
        silence_live_watchers(&fixture.service);
        thread::sleep(Duration::from_millis(
            u64::try_from(STABILITY_RECHECK_MS + 100).unwrap_or(1_100),
        ));
        let checks = AtomicUsize::new(0);
        fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| {
                checks.fetch_add(1, Ordering::SeqCst) >= 1
            })
            .unwrap_or_else(|error| panic!("interrupted move should remain recoverable: {error}"));
        let after_interruption = fixture
            .database
            .catalog_snapshot_for_root(fixture.root_id, 10)
            .unwrap_or_else(|error| panic!("source should remain cataloged: {error}"));
        assert_eq!(after_interruption.len(), 1);
        assert_eq!(after_interruption[0].file_id, original.file_id);
        assert_eq!(
            after_interruption[0].current_relative_path,
            PathBuf::from("source.txt")
        );

        silence_live_watchers(&fixture.service);
        let mut dashboard = fixture
            .service
            .monitoring_dashboard(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("dashboard should load: {error}"));
        for _ in 0..6 {
            if dashboard.counts.pending_jobs == 0 {
                break;
            }
            silence_live_watchers(&fixture.service);
            thread::sleep(Duration::from_millis(
                u64::try_from(STABILITY_RECHECK_MS + 100).unwrap_or(1_100),
            ));
            dashboard = fixture
                .service
                .run_monitoring_cycle(fixture.workspace_id, &|| false)
                .unwrap_or_else(|error| panic!("move retry should complete: {error}"));
        }
        assert_eq!(dashboard.counts.pending_jobs, 0);
        let after_retry = fixture
            .database
            .catalog_snapshot_for_root(fixture.root_id, 10)
            .unwrap_or_else(|error| panic!("destination should load: {error}"));
        assert_eq!(after_retry.len(), 1);
        assert_eq!(after_retry[0].file_id, original.file_id);
        assert_eq!(
            after_retry[0].current_relative_path,
            PathBuf::from("destination.txt")
        );
    }

    #[test]
    fn confirmed_deletion_removes_current_search_and_catalog_projections() {
        let fixture = fixture(Some(("obsolete-atlas.txt", b"Obsolete Atlas record")), 92);
        let scan = fixture
            .service
            .scan_workspace(fixture.workspace_id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("initial scan should complete: {error}"));
        fixture
            .service
            .analyze_scan_content(scan.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("initial content should complete: {error}"));
        fixture
            .service
            .analyze_scan_semantics(scan.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("initial semantics should complete: {error}"));
        fixture
            .database
            .mark_startup_reconciliation_completed(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("startup reconciliation should clear: {error}"));
        let before = fixture
            .service
            .search_files(
                fixture.workspace_id,
                search::SearchQuery {
                    text: "obsolete-atlas".to_owned(),
                    ..search::SearchQuery::default()
                },
            )
            .unwrap_or_else(|error| panic!("initial search should work: {error}"));
        assert_eq!(before.total, 1);

        fs::remove_file(fixture.root_path.join("obsolete-atlas.txt"))
            .unwrap_or_else(|error| panic!("fixture should be removed: {error}"));
        fixture
            .service
            .record_monitoring_hints(
                fixture.workspace_id,
                fixture.root_id,
                &[ChangeHint {
                    root_token: "test-root".to_owned(),
                    native_key: None,
                    path_before: Some(PathBuf::from("obsolete-atlas.txt")),
                    path_after: None,
                    kind: LocalEventKind::Removed,
                    scope: ChangeScope::File,
                }],
            )
            .unwrap_or_else(|error| panic!("remove event should persist: {error}"));
        silence_live_watchers(&fixture.service);
        thread::sleep(Duration::from_millis(
            u64::try_from(EVENT_DEBOUNCE_MS + 100).unwrap_or(1_000),
        ));
        let dashboard = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("deletion reconciliation should complete: {error}"));
        assert_eq!(dashboard.counts.pending_jobs, 0);
        assert!(
            fixture
                .database
                .catalog_snapshot_for_root(fixture.root_id, 10)
                .unwrap_or_else(|error| panic!("catalog should reload: {error}"))
                .is_empty()
        );
        let after = fixture
            .service
            .search_files(
                fixture.workspace_id,
                search::SearchQuery {
                    text: "obsolete-atlas".to_owned(),
                    ..search::SearchQuery::default()
                },
            )
            .unwrap_or_else(|error| panic!("post-delete search should work: {error}"));
        assert_eq!(after.total, 0);
    }

    #[test]
    fn targeted_refresh_keeps_unchanged_files_in_the_workspace_proposal() {
        let temporary = TempDir::new()
            .unwrap_or_else(|error| panic!("temporary proposal root should exist: {error}"));
        let root_path = temporary.path().join("selected");
        fs::create_dir(&root_path)
            .unwrap_or_else(|error| panic!("proposal root should exist: {error}"));
        fs::write(root_path.join("first.txt"), b"First invoice")
            .unwrap_or_else(|error| panic!("first file should exist: {error}"));
        fs::write(root_path.join("unchanged.txt"), b"First invoice, updated")
            .unwrap_or_else(|error| panic!("unchanged file should exist: {error}"));
        let database = Arc::new(
            Database::open_in_memory(&DatabaseKey::from_bytes([86; 32]))
                .unwrap_or_else(|error| panic!("proposal database should open: {error}")),
        );
        let service = ScannerApplicationService::new(database.clone(), Arc::new(MacOsPlatform));
        let workspace = service
            .create_workspace("Incremental proposal")
            .unwrap_or_else(|error| panic!("proposal workspace should exist: {error}"));
        let root = service
            .register_root(workspace.id, &root_path)
            .unwrap_or_else(|error| panic!("proposal root should register: {error}"));
        service
            .scan_workspace(workspace.id, &|| false, &mut |_| {})
            .unwrap_or_else(|error| panic!("initial inventory should complete: {error}"));
        database
            .mark_startup_reconciliation_completed(workspace.id)
            .unwrap_or_else(|error| panic!("initial reconciliation should clear: {error}"));

        fs::write(root_path.join("first.txt"), b"First invoice, updated")
            .unwrap_or_else(|error| panic!("first file should update: {error}"));
        service
            .record_monitoring_hints(
                workspace.id,
                root.id,
                &[ChangeHint {
                    root_token: "test-root".to_owned(),
                    native_key: None,
                    path_after: Some(PathBuf::from("first.txt")),
                    path_before: None,
                    kind: LocalEventKind::Modified,
                    scope: ChangeScope::File,
                }],
            )
            .unwrap_or_else(|error| panic!("modified hint should persist: {error}"));
        let mut dashboard = service
            .monitoring_dashboard(workspace.id)
            .unwrap_or_else(|error| panic!("proposal dashboard should load: {error}"));
        for _ in 0..6 {
            if dashboard.counts.pending_jobs == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(1_100));
            dashboard = service
                .run_monitoring_cycle(workspace.id, &|| false)
                .unwrap_or_else(|error| panic!("incremental refresh should run: {error}"));
        }
        assert_eq!(dashboard.counts.pending_jobs, 0);
        assert_eq!(
            database
                .catalog_snapshot_for_root(root.id, 10)
                .unwrap_or_else(|error| panic!("current catalog should load: {error}"))
                .len(),
            2
        );
        let proposal = service
            .latest_organization_proposal(workspace.id)
            .unwrap_or_else(|error| panic!("incremental proposal should exist: {error}"));
        assert_eq!(proposal.summary.files_analyzed, 2);
        assert!(
            proposal
                .operations
                .iter()
                .any(|operation| operation.source_name == "unchanged.txt")
        );
        let latest_scan = database
            .latest_scan_for_root(root.id)
            .unwrap_or_else(|error| panic!("latest reconciliation should load: {error}"))
            .unwrap_or_else(|| panic!("latest reconciliation should exist"));
        let duplicate_groups = service
            .scan_duplicate_groups(latest_scan.id)
            .unwrap_or_else(|error| panic!("incremental duplicates should load: {error}"));
        assert_eq!(duplicate_groups.len(), 1);
        assert_eq!(duplicate_groups[0].files.len(), 2);
        assert_eq!(
            fs::read(root_path.join("unchanged.txt"))
                .unwrap_or_else(|error| panic!("unchanged file should remain: {error}")),
            b"First invoice, updated"
        );
    }

    #[test]
    fn size_threshold_routes_file_to_review_and_preserves_it() {
        let contents = b"larger than the deliberately tiny test threshold";
        let fixture = fixture(Some(("large.txt", contents)), 83);
        fixture
            .database
            .configure_root_monitoring(
                fixture.root_id,
                RootMonitoringConfiguration {
                    enabled: true,
                    status: MonitoringRootStatus::Active,
                    size_threshold_bytes: 4,
                    startup_entry_limit: 100,
                },
            )
            .unwrap_or_else(|error| panic!("threshold should update: {error}"));
        fixture
            .service
            .record_monitoring_hints(
                fixture.workspace_id,
                fixture.root_id,
                &[created_hint("large.txt")],
            )
            .unwrap_or_else(|error| panic!("created hint should persist: {error}"));
        thread::sleep(Duration::from_millis(
            u64::try_from(EVENT_DEBOUNCE_MS + 100).unwrap_or(1_000),
        ));
        let dashboard = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("threshold check should run: {error}"));
        assert_eq!(dashboard.counts.pending_jobs, 0);
        assert_eq!(dashboard.counts.needs_review, 1);
        assert_eq!(
            fs::read(fixture.root_path.join("large.txt"))
                .unwrap_or_else(|error| panic!("large file should remain: {error}")),
            contents
        );
        let registration = fixture
            .database
            .list_watch_registrations(fixture.root_id)
            .unwrap_or_else(|error| panic!("registration should load: {error}"))
            .remove(0);
        let event = fixture
            .database
            .list_watch_events(&registration.id, None, 10)
            .unwrap_or_else(|error| panic!("event should load: {error}"))
            .remove(0);
        let job = fixture
            .database
            .claim_monitoring_jobs(&[], 0)
            .unwrap_or_else(|error| panic!("empty claim should be valid: {error}"));
        assert!(job.is_empty());
        assert_eq!(event.kind, WatchEventKind::Created);
    }

    #[test]
    fn temporary_and_user_excluded_files_are_ignored_without_mutation() {
        let fixture = fixture(Some(("editor.tmp", b"temporary")), 85);
        fixture
            .service
            .add_monitoring_exclusion(
                fixture.workspace_id,
                Some(fixture.root_id),
                MonitoringExclusionKind::Extension,
                ".bak",
            )
            .unwrap_or_else(|error| panic!("user exclusion should persist: {error}"));
        fs::write(fixture.root_path.join("private.bak"), b"private")
            .unwrap_or_else(|error| panic!("excluded file should exist: {error}"));
        fixture
            .service
            .record_monitoring_hints(
                fixture.workspace_id,
                fixture.root_id,
                &[created_hint("editor.tmp"), created_hint("private.bak")],
            )
            .unwrap_or_else(|error| panic!("excluded hints should persist: {error}"));
        silence_live_watchers(&fixture.service);
        thread::sleep(Duration::from_millis(
            u64::try_from(EVENT_DEBOUNCE_MS + 100).unwrap_or(1_000),
        ));
        let dashboard = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("exclusion cycle should run: {error}"));
        assert_eq!(dashboard.counts.pending_jobs, 0);
        assert_eq!(dashboard.counts.files_analyzed, 0);
        assert!(
            fixture
                .database
                .catalog_snapshot_for_root(fixture.root_id, 10)
                .unwrap_or_else(|error| panic!("catalog should remain empty: {error}"))
                .is_empty()
        );
        assert_eq!(
            fs::read(fixture.root_path.join("editor.tmp"))
                .unwrap_or_else(|error| panic!("temporary file should remain: {error}")),
            b"temporary"
        );
        assert_eq!(
            fs::read(fixture.root_path.join("private.bak"))
                .unwrap_or_else(|error| panic!("excluded file should remain: {error}")),
            b"private"
        );
    }

    #[test]
    fn pause_resume_and_restart_restore_only_safe_monitoring_state() {
        let temporary = TempDir::new()
            .unwrap_or_else(|error| panic!("temporary restart root should exist: {error}"));
        let root_path = temporary.path().join("selected");
        fs::create_dir(&root_path)
            .unwrap_or_else(|error| panic!("selected restart root should exist: {error}"));
        fs::write(root_path.join("pending.txt"), b"pending")
            .unwrap_or_else(|error| panic!("pending file should exist: {error}"));
        let database_path = temporary.path().join("catalog.db");
        let key_bytes = [84_u8; 32];

        let database = Arc::new(
            Database::open(&database_path, &DatabaseKey::from_bytes(key_bytes))
                .unwrap_or_else(|error| panic!("restart database should open: {error}")),
        );
        let service = ScannerApplicationService::new(database.clone(), Arc::new(MacOsPlatform));
        let workspace = service
            .create_workspace("Restart monitoring")
            .unwrap_or_else(|error| panic!("workspace should exist: {error}"));
        let root = service
            .register_root(workspace.id, &root_path)
            .unwrap_or_else(|error| panic!("root should exist: {error}"));
        database
            .mark_startup_reconciliation_completed(workspace.id)
            .unwrap_or_else(|error| panic!("startup flag should clear: {error}"));
        service
            .record_monitoring_hints(workspace.id, root.id, &[created_hint("pending.txt")])
            .unwrap_or_else(|error| panic!("pending event should persist: {error}"));
        let paused = service
            .pause_monitoring(workspace.id)
            .unwrap_or_else(|error| panic!("monitoring should pause: {error}"));
        assert!(paused.state.paused);
        let resumed = service
            .resume_monitoring(workspace.id)
            .unwrap_or_else(|error| panic!("monitoring should resume: {error}"));
        assert!(!resumed.state.paused);
        drop(service);
        drop(database);
        fs::write(root_path.join("missed-while-closed.txt"), b"missed")
            .unwrap_or_else(|error| panic!("missed file should be created: {error}"));

        let reopened = Arc::new(
            Database::open(&database_path, &DatabaseKey::from_bytes(key_bytes))
                .unwrap_or_else(|error| panic!("database should reopen: {error}")),
        );
        let restored_service = ScannerApplicationService::new(reopened, Arc::new(MacOsPlatform));
        restored_service
            .restore_monitoring_runtime()
            .unwrap_or_else(|error| panic!("monitoring runtime should restore: {error}"));
        let restored = restored_service
            .restore_workspace_session()
            .unwrap_or_else(|error| panic!("workspace session should restore: {error}"))
            .unwrap_or_else(|| panic!("workspace session should be present"));
        assert_eq!(restored.workspace.id, workspace.id);
        assert_eq!(restored.root.map(|value| value.id), Some(root.id));
        assert!(restored.safe_read_only);
        assert!(!restored.filesystem_execution_resumed);
        let dashboard = restored_service
            .monitoring_dashboard(workspace.id)
            .unwrap_or_else(|error| panic!("restored dashboard should load: {error}"));
        assert_eq!(dashboard.state.mode, persistence::MonitoringMode::Prudent);
        assert!(dashboard.state.startup_reconciliation_pending);
        assert_eq!(dashboard.counts.pending_jobs, 1);
        let reconciled = restored_service
            .run_monitoring_cycle(workspace.id, &|| false)
            .unwrap_or_else(|error| panic!("startup gap should be reconciled: {error}"));
        assert!(!reconciled.state.startup_reconciliation_pending);
        assert_eq!(reconciled.counts.pending_jobs, 2);
        let due = restored_service
            .database
            .list_due_monitoring_jobs(i64::MAX, 10)
            .unwrap_or_else(|error| panic!("pending job should remain durable: {error}"));
        assert!(due.iter().all(|job| matches!(
            job.status,
            MonitoringJobStatus::Pending | MonitoringJobStatus::Waiting
        )));
    }

    #[test]
    fn unavailable_startup_root_defers_reconciliation_as_durable_work() {
        let fixture = fixture(None, 87);
        fs::remove_dir_all(&fixture.root_path)
            .unwrap_or_else(|error| panic!("fixture root should become unavailable: {error}"));
        fixture
            .database
            .mark_startup_reconciliation_pending(fixture.workspace_id)
            .unwrap_or_else(|error| panic!("startup reconciliation should be pending: {error}"));

        let dashboard = fixture
            .service
            .run_monitoring_cycle(fixture.workspace_id, &|| false)
            .unwrap_or_else(|error| panic!("unavailable startup root should fail safely: {error}"));

        assert!(!dashboard.state.startup_reconciliation_pending);
        assert!(
            dashboard.counts.pending_jobs >= 1,
            "expected a durable retry job: {dashboard:#?}"
        );
        let root = dashboard
            .roots
            .iter()
            .find(|root| root.root_id == fixture.root_id)
            .unwrap_or_else(|| panic!("monitored root should remain visible"));
        assert!(matches!(
            root.status,
            MonitoringRootStatus::Failed | MonitoringRootStatus::Overflowed
        ));
        assert!(root.last_error.as_deref().is_some_and(|error| {
            error.contains("startup_reconciliation_failed")
                || error.contains("bounded_reconciliation")
        }));

        fs::create_dir(&fixture.root_path)
            .unwrap_or_else(|error| panic!("fixture root should return: {error}"));
        fs::write(fixture.root_path.join("recovered.txt"), b"recovered")
            .unwrap_or_else(|error| panic!("recovered file should exist: {error}"));
        let mut recovered = dashboard;
        for _ in 0..6 {
            thread::sleep(Duration::from_millis(1_100));
            recovered = fixture
                .service
                .run_monitoring_cycle(fixture.workspace_id, &|| false)
                .unwrap_or_else(|error| panic!("deferred reconciliation should retry: {error}"));
            if recovered.counts.pending_jobs == 0 {
                break;
            }
        }
        assert_eq!(recovered.counts.pending_jobs, 0);
        assert_eq!(
            recovered
                .roots
                .iter()
                .find(|root| root.root_id == fixture.root_id)
                .map(|root| root.status),
            Some(MonitoringRootStatus::Active)
        );
        assert_eq!(
            fixture
                .database
                .catalog_snapshot_for_root(fixture.root_id, 10)
                .unwrap_or_else(|error| panic!("recovered catalog should load: {error}"))
                .len(),
            1
        );
    }
}
