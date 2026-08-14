BEGIN IMMEDIATE;

-- A single local restoration pointer. Paths remain in the encrypted catalog.
CREATE TABLE application_restore_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    current_workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

INSERT INTO application_restore_state(singleton) VALUES (1);

CREATE INDEX idx_application_restore_workspace
    ON application_restore_state(current_workspace_id);

CREATE TABLE workspace_monitoring_state (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    operational_mode TEXT NOT NULL DEFAULT 'PRUDENT' CHECK (
        operational_mode IN ('PRUDENT', 'AUTOMATIC', 'RULES')
    ),
    global_paused INTEGER NOT NULL DEFAULT 0 CHECK (global_paused IN (0, 1)),
    startup_reconciliation_pending INTEGER NOT NULL DEFAULT 1 CHECK (
        startup_reconciliation_pending IN (0, 1)
    ),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

-- This composite key lets monitoring rows prove that a root belongs to a workspace.
CREATE UNIQUE INDEX uq_roots_id_workspace_monitoring
    ON roots(id, workspace_id);

CREATE TABLE root_monitoring_settings (
    root_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    status TEXT NOT NULL DEFAULT 'paused' CHECK (
        status IN (
            'starting', 'active', 'paused', 'reconciling', 'overflowed',
            'offline', 'failed', 'stopped'
        )
    ),
    size_threshold_bytes INTEGER NOT NULL DEFAULT 4294967296 CHECK (
        size_threshold_bytes BETWEEN 1 AND 4398046511104
    ),
    startup_entry_limit INTEGER NOT NULL DEFAULT 100000 CHECK (
        startup_entry_limit BETWEEN 1 AND 1000000
    ),
    last_reconciliation_scan_id TEXT REFERENCES scans(id) ON DELETE SET NULL,
    last_reconciled_at TEXT,
    last_checkpoint_sequence INTEGER CHECK (
        last_checkpoint_sequence IS NULL OR last_checkpoint_sequence >= 0
    ),
    last_checkpoint_at TEXT,
    last_error_code TEXT,
    last_error_message TEXT,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (root_id, workspace_id)
        REFERENCES roots(id, workspace_id) ON DELETE CASCADE,
    CHECK (length(last_error_code) <= 256),
    CHECK (length(last_error_message) <= 2048)
) STRICT;

CREATE INDEX idx_root_monitoring_workspace
    ON root_monitoring_settings(workspace_id, enabled, status);
CREATE INDEX idx_root_monitoring_reconciliation_scan
    ON root_monitoring_settings(last_reconciliation_scan_id);

CREATE TABLE monitoring_exclusions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT,
    exclusion_kind TEXT NOT NULL CHECK (
        exclusion_kind IN ('path_prefix', 'extension')
    ),
    exclusion_value TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (root_id, workspace_id)
        REFERENCES roots(id, workspace_id) ON DELETE CASCADE,
    CHECK (
        (exclusion_kind = 'path_prefix'
         AND length(exclusion_value) BETWEEN 1 AND 4096
         AND instr(exclusion_value, char(0)) = 0)
        OR
        (exclusion_kind = 'extension'
         AND length(exclusion_value) BETWEEN 1 AND 64
         AND instr(exclusion_value, '/') = 0
         AND instr(exclusion_value, '\') = 0)
    )
) STRICT;

CREATE INDEX idx_monitoring_exclusions_workspace
    ON monitoring_exclusions(workspace_id, enabled, exclusion_kind);
CREATE INDEX idx_monitoring_exclusions_root
    ON monitoring_exclusions(root_id, enabled, exclusion_kind);
CREATE UNIQUE INDEX uq_monitoring_exclusion_workspace
    ON monitoring_exclusions(workspace_id, exclusion_kind, exclusion_value)
    WHERE root_id IS NULL;
CREATE UNIQUE INDEX uq_monitoring_exclusion_root
    ON monitoring_exclusions(root_id, exclusion_kind, exclusion_value)
    WHERE root_id IS NOT NULL;

CREATE TABLE monitoring_jobs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL,
    watch_registration_id TEXT REFERENCES watch_registrations(id) ON DELETE SET NULL,
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'created', 'modified', 'moved', 'removed', 'metadata',
            'overflow', 'rescan_required'
        )
    ),
    path_before TEXT,
    path_after TEXT,
    coalescing_path TEXT,
    status TEXT NOT NULL CHECK (
        status IN (
            'pending', 'waiting', 'processing', 'completed', 'to_review',
            'failed', 'cancelled', 'excluded'
        )
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 20),
    maximum_attempts INTEGER NOT NULL DEFAULT 5 CHECK (maximum_attempts BETWEEN 1 AND 20),
    sample_byte_size INTEGER CHECK (sample_byte_size IS NULL OR sample_byte_size >= 0),
    sample_modified_at_ns TEXT,
    stable_sample_count INTEGER NOT NULL DEFAULT 0 CHECK (
        stable_sample_count BETWEEN 0 AND 100
    ),
    debounce_ready_at_unix_ms INTEGER NOT NULL,
    retry_after_unix_ms INTEGER,
    last_sampled_at_unix_ms INTEGER,
    event_count INTEGER NOT NULL DEFAULT 1 CHECK (event_count >= 1),
    coalesced_event_count INTEGER NOT NULL DEFAULT 0 CHECK (coalesced_event_count >= 0),
    reconciliation_scan_id TEXT REFERENCES scans(id) ON DELETE SET NULL,
    last_error_code TEXT,
    last_error_message TEXT,
    claimed_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (root_id, workspace_id)
        REFERENCES roots(id, workspace_id) ON DELETE CASCADE,
    CHECK (path_before IS NULL OR (
        length(path_before) BETWEEN 1 AND 4096 AND instr(path_before, char(0)) = 0
    )),
    CHECK (path_after IS NULL OR (
        length(path_after) BETWEEN 1 AND 4096 AND instr(path_after, char(0)) = 0
    )),
    CHECK (coalescing_path IS NULL OR (
        length(coalescing_path) BETWEEN 1 AND 4096
        AND instr(coalescing_path, char(0)) = 0
    )),
    CHECK (
        coalescing_path IS NOT NULL
        OR event_kind IN ('overflow', 'rescan_required')
    ),
    CHECK (sample_modified_at_ns IS NULL OR length(sample_modified_at_ns) <= 64),
    CHECK (attempt_count <= maximum_attempts),
    CHECK (coalesced_event_count = event_count - 1),
    CHECK (last_error_code IS NULL OR length(last_error_code) BETWEEN 1 AND 256),
    CHECK (last_error_message IS NULL OR length(last_error_message) <= 2048),
    CHECK (completed_at IS NULL OR status IN (
        'completed', 'to_review', 'failed', 'cancelled', 'excluded'
    ))
) STRICT;

CREATE INDEX idx_monitoring_jobs_workspace
    ON monitoring_jobs(workspace_id, status, updated_at DESC);
CREATE INDEX idx_monitoring_jobs_root
    ON monitoring_jobs(root_id, status, updated_at DESC);
CREATE INDEX idx_monitoring_jobs_registration
    ON monitoring_jobs(watch_registration_id);
CREATE INDEX idx_monitoring_jobs_scan
    ON monitoring_jobs(reconciliation_scan_id);
CREATE INDEX idx_monitoring_jobs_due
    ON monitoring_jobs(status, debounce_ready_at_unix_ms, retry_after_unix_ms, created_at)
    WHERE status IN ('pending', 'waiting');
CREATE UNIQUE INDEX uq_monitoring_active_path_job
    ON monitoring_jobs(root_id, coalescing_path)
    WHERE coalescing_path IS NOT NULL
      AND status IN ('pending', 'waiting', 'processing');
CREATE UNIQUE INDEX uq_monitoring_active_root_job
    ON monitoring_jobs(root_id)
    WHERE coalescing_path IS NULL
      AND status IN ('pending', 'waiting', 'processing');

-- The raw event remains durable and sequenced in watch_events; this table records
-- which coalesced unit of work currently represents it.
CREATE TABLE monitoring_job_events (
    watch_event_id TEXT PRIMARY KEY REFERENCES watch_events(id) ON DELETE CASCADE,
    monitoring_job_id TEXT NOT NULL REFERENCES monitoring_jobs(id) ON DELETE CASCADE,
    linked_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX idx_monitoring_job_events_job
    ON monitoring_job_events(monitoring_job_id);

CREATE TABLE monitoring_activity_batches (
    batch_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT,
    files_analyzed INTEGER NOT NULL CHECK (files_analyzed >= 0),
    ready_to_organize INTEGER NOT NULL CHECK (ready_to_organize >= 0),
    needs_review INTEGER NOT NULL CHECK (needs_review >= 0),
    failed INTEGER NOT NULL CHECK (failed >= 0),
    summary TEXT NOT NULL,
    reconciliation_scan_id TEXT REFERENCES scans(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (root_id, workspace_id)
        REFERENCES roots(id, workspace_id) ON DELETE CASCADE,
    CHECK (length(batch_id) BETWEEN 1 AND 128),
    CHECK (length(summary) BETWEEN 1 AND 2048),
    CHECK (ready_to_organize <= files_analyzed),
    CHECK (needs_review <= files_analyzed),
    CHECK (failed <= files_analyzed)
) STRICT;

CREATE INDEX idx_monitoring_activity_workspace
    ON monitoring_activity_batches(workspace_id, created_at DESC);
CREATE INDEX idx_monitoring_activity_root
    ON monitoring_activity_batches(root_id, created_at DESC);
CREATE INDEX idx_monitoring_activity_scan
    ON monitoring_activity_batches(reconciliation_scan_id);

-- Existing raw watcher tables predate bounded M10 inputs. Enforce the same
-- encrypted-local path and payload limits for new writes without rebuilding them.
CREATE TRIGGER watch_registrations_validate_m10_insert
BEFORE INSERT ON watch_registrations
WHEN length(NEW.configuration_json) > 32768
  OR length(COALESCE(NEW.backend_cursor, '')) > 4096
BEGIN
    SELECT RAISE(ABORT, 'watch registration exceeds monitoring bounds');
END;

CREATE TRIGGER watch_registrations_validate_m10_update
BEFORE UPDATE ON watch_registrations
WHEN length(NEW.configuration_json) > 32768
  OR length(COALESCE(NEW.backend_cursor, '')) > 4096
BEGIN
    SELECT RAISE(ABORT, 'watch registration exceeds monitoring bounds');
END;

CREATE TRIGGER watch_events_validate_m10_insert
BEFORE INSERT ON watch_events
WHEN length(COALESCE(NEW.path_before, '')) > 4096
  OR length(COALESCE(NEW.path_after, '')) > 4096
  OR instr(COALESCE(NEW.path_before, ''), char(0)) > 0
  OR instr(COALESCE(NEW.path_after, ''), char(0)) > 0
  OR length(NEW.payload_json) > 32768
  OR length(COALESCE(NEW.native_identity_key, X'')) > 4096
BEGIN
    SELECT RAISE(ABORT, 'watch event exceeds monitoring bounds');
END;

CREATE TRIGGER watch_events_validate_m10_update
BEFORE UPDATE ON watch_events
WHEN length(COALESCE(NEW.path_before, '')) > 4096
  OR length(COALESCE(NEW.path_after, '')) > 4096
  OR instr(COALESCE(NEW.path_before, ''), char(0)) > 0
  OR instr(COALESCE(NEW.path_after, ''), char(0)) > 0
  OR length(NEW.payload_json) > 32768
  OR length(COALESCE(NEW.native_identity_key, X'')) > 4096
BEGIN
    SELECT RAISE(ABORT, 'watch event exceeds monitoring bounds');
END;

INSERT OR IGNORE INTO schema_migrations(version, name) VALUES
    (7, '0007_local_organization_proposals'),
    (8, '0008_safety_gated_filesystem_application'),
    (9, '0009_continuous_monitoring');

PRAGMA user_version = 9;
COMMIT;
