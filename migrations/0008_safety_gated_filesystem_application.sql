BEGIN IMMEDIATE;

CREATE TABLE local_execution_sessions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL UNIQUE,
    proposal_id TEXT NOT NULL REFERENCES local_organization_proposals(id) ON DELETE RESTRICT,
    proposal_revision_id TEXT NOT NULL REFERENCES local_organization_proposal_revisions(id) ON DELETE RESTRICT,
    proposal_revision INTEGER NOT NULL CHECK (proposal_revision > 0),
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE RESTRICT,
    source_scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (
        status IN (
            'prepared', 'awaiting_confirmation', 'approved', 'running', 'paused',
            'cancelled', 'completed', 'partial', 'failed', 'recovery_required',
            'recovery_available', 'recovery_ambiguous', 'rolling_back',
            'rolled_back', 'rollback_partial'
        )
    ),
    recovery_state TEXT NOT NULL DEFAULT 'recovery_not_required' CHECK (
        recovery_state IN (
            'recovery_not_required', 'recovery_available',
            'recovery_required', 'recovery_ambiguous'
        )
    ),
    plan_digest TEXT NOT NULL CHECK (length(plan_digest) = 64),
    approved_operation_count INTEGER NOT NULL CHECK (approved_operation_count >= 0),
    affected_file_count INTEGER NOT NULL CHECK (affected_file_count >= 0),
    folder_count INTEGER NOT NULL CHECK (folder_count >= 0),
    move_count INTEGER NOT NULL CHECK (move_count >= 0),
    rename_count INTEGER NOT NULL CHECK (rename_count >= 0),
    unchanged_count INTEGER NOT NULL CHECK (unchanged_count >= 0),
    conflict_count INTEGER NOT NULL CHECK (conflict_count >= 0),
    needs_review_count INTEGER NOT NULL CHECK (needs_review_count >= 0),
    preflight_ok_count INTEGER NOT NULL CHECK (preflight_ok_count >= 0),
    blocked_count INTEGER NOT NULL CHECK (blocked_count >= 0),
    skipped_count INTEGER NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    applied_count INTEGER NOT NULL DEFAULT 0 CHECK (applied_count >= 0),
    failed_count INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    rolled_back_count INTEGER NOT NULL DEFAULT 0 CHECK (rolled_back_count >= 0),
    rollback_blocked_count INTEGER NOT NULL DEFAULT 0 CHECK (rollback_blocked_count >= 0),
    rollback_failed_count INTEGER NOT NULL DEFAULT 0 CHECK (rollback_failed_count >= 0),
    confirmation_phrase_required INTEGER NOT NULL CHECK (
        confirmation_phrase_required IN (0, 1)
    ),
    user_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (user_confirmed IN (0, 1)),
    rollback_available INTEGER NOT NULL DEFAULT 0 CHECK (rollback_available IN (0, 1)),
    pause_requested INTEGER NOT NULL DEFAULT 0 CHECK (pause_requested IN (0, 1)),
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
    current_operation TEXT,
    journal_sequence INTEGER NOT NULL DEFAULT -1 CHECK (journal_sequence >= -1),
    journal_chain_head TEXT CHECK (
        journal_chain_head IS NULL OR length(journal_chain_head) = 64
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    approved_at TEXT,
    started_at TEXT,
    completed_at TEXT,
    rolled_back_at TEXT,
    error_message TEXT,
    CHECK (approved_at IS NULL OR user_confirmed = 1),
    CHECK (length(plan_id) BETWEEN 1 AND 64),
    CHECK (current_operation IS NULL OR length(current_operation) <= 4096),
    CHECK (error_message IS NULL OR length(error_message) <= 2048)
) STRICT;

CREATE INDEX idx_local_execution_sessions_history
    ON local_execution_sessions(workspace_id, created_at DESC);
CREATE INDEX idx_local_execution_sessions_recovery
    ON local_execution_sessions(recovery_state, status, created_at);
CREATE UNIQUE INDEX idx_local_execution_one_active
    ON local_execution_sessions((1))
    WHERE status IN (
        'prepared', 'awaiting_confirmation', 'approved', 'running', 'paused',
        'recovery_required', 'recovery_available', 'recovery_ambiguous', 'rolling_back'
    );

CREATE TABLE local_execution_approval_snapshots (
    execution_id TEXT PRIMARY KEY REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    plan_id TEXT NOT NULL UNIQUE,
    proposal_id TEXT NOT NULL REFERENCES local_organization_proposals(id) ON DELETE RESTRICT,
    proposal_revision_id TEXT NOT NULL REFERENCES local_organization_proposal_revisions(id) ON DELETE RESTRICT,
    proposal_revision INTEGER NOT NULL CHECK (proposal_revision > 0),
    source_snapshot_version TEXT NOT NULL REFERENCES scans(id) ON DELETE RESTRICT,
    approved_operation_ids_json TEXT NOT NULL CHECK (json_valid(approved_operation_ids_json)),
    operation_count INTEGER NOT NULL CHECK (operation_count >= 0),
    user_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (user_confirmed IN (0, 1)),
    frozen_at TEXT NOT NULL,
    approved_at TEXT,
    snapshot_digest TEXT NOT NULL CHECK (length(snapshot_digest) = 64),
    CHECK (length(approved_operation_ids_json) <= 16777216),
    CHECK (approved_at IS NULL OR user_confirmed = 1)
) STRICT;

CREATE TABLE local_execution_operations (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    proposal_operation_id TEXT REFERENCES local_organization_proposal_operations(id) ON DELETE RESTRICT,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN (
            'create_directory', 'move', 'rename', 'move_and_rename', 'internal_stage'
        )
    ),
    source_relative_path TEXT,
    destination_relative_path TEXT NOT NULL,
    original_source_relative_path TEXT,
    expected_source_hash TEXT,
    expected_source_size INTEGER CHECK (expected_source_size IS NULL OR expected_source_size >= 0),
    expected_source_modified_at TEXT,
    live_fingerprint_json TEXT CHECK (
        live_fingerprint_json IS NULL OR json_valid(live_fingerprint_json)
    ),
    post_fingerprint_json TEXT CHECK (
        post_fingerprint_json IS NULL OR json_valid(post_fingerprint_json)
    ),
    preconditions_json TEXT NOT NULL CHECK (json_valid(preconditions_json)),
    dependencies_json TEXT NOT NULL CHECK (json_valid(dependencies_json)),
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    status TEXT NOT NULL CHECK (
        status IN (
            'planned', 'preflight_ok', 'blocked', 'running', 'applied', 'failed',
            'skipped', 'stale', 'recovered', 'rolling_back', 'rolled_back',
            'rollback_blocked', 'rollback_failed'
        )
    ),
    directory_existed_before INTEGER CHECK (
        directory_existed_before IS NULL OR directory_existed_before IN (0, 1)
    ),
    reason TEXT,
    error_code TEXT,
    error_message TEXT,
    started_at TEXT,
    completed_at TEXT,
    rolled_back_at TEXT,
    UNIQUE (execution_id, sequence_number),
    CHECK (source_relative_path IS NULL OR length(source_relative_path) BETWEEN 1 AND 4096),
    CHECK (length(destination_relative_path) BETWEEN 1 AND 4096),
    CHECK (
        original_source_relative_path IS NULL
        OR length(original_source_relative_path) BETWEEN 1 AND 4096
    ),
    CHECK (expected_source_hash IS NULL OR length(expected_source_hash) = 64),
    CHECK (live_fingerprint_json IS NULL OR length(live_fingerprint_json) <= 32768),
    CHECK (post_fingerprint_json IS NULL OR length(post_fingerprint_json) <= 32768),
    CHECK (length(preconditions_json) <= 32768),
    CHECK (length(dependencies_json) <= 32768),
    CHECK (reason IS NULL OR length(reason) <= 1024),
    CHECK (error_code IS NULL OR length(error_code) <= 128),
    CHECK (error_message IS NULL OR length(error_message) <= 2048)
) STRICT;

CREATE INDEX idx_local_execution_operations_session
    ON local_execution_operations(execution_id, sequence_number);
CREATE INDEX idx_local_execution_operations_status
    ON local_execution_operations(execution_id, status, sequence_number);
CREATE UNIQUE INDEX idx_local_execution_proposal_operation
    ON local_execution_operations(execution_id, proposal_operation_id)
    WHERE proposal_operation_id IS NOT NULL AND operation_kind <> 'internal_stage';

CREATE TABLE local_execution_journal (
    execution_id TEXT NOT NULL REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    operation_id TEXT REFERENCES local_execution_operations(id) ON DELETE RESTRICT,
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'approved_durable', 'intent_durable', 'preconditions_validated',
            'applied_observed', 'step_failed', 'execution_finished',
            'rollback_intent', 'rolled_back_observed', 'conflict'
        )
    ),
    canonical_data_json TEXT NOT NULL CHECK (json_valid(canonical_data_json)),
    payload_digest TEXT NOT NULL CHECK (length(payload_digest) = 64),
    previous_entry_digest TEXT CHECK (
        previous_entry_digest IS NULL OR length(previous_entry_digest) = 64
    ),
    entry_digest TEXT NOT NULL CHECK (length(entry_digest) = 64),
    occurred_at_unix_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (execution_id, sequence_number),
    CHECK (length(canonical_data_json) <= 16777216)
) STRICT, WITHOUT ROWID;

CREATE INDEX idx_local_execution_journal_operation
    ON local_execution_journal(operation_id, sequence_number);

CREATE TABLE local_execution_rollback_records (
    execution_id TEXT NOT NULL REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL REFERENCES local_execution_operations(id) ON DELETE RESTRICT,
    rollback_order INTEGER NOT NULL CHECK (rollback_order >= 0),
    source_relative_path TEXT NOT NULL,
    destination_relative_path TEXT NOT NULL,
    expected_fingerprint_json TEXT CHECK (
        expected_fingerprint_json IS NULL OR json_valid(expected_fingerprint_json)
    ),
    status TEXT NOT NULL CHECK (
        status IN ('available', 'running', 'rolled_back', 'blocked', 'failed')
    ),
    prepared_at TEXT NOT NULL,
    completed_at TEXT,
    error_message TEXT,
    PRIMARY KEY (execution_id, operation_id),
    UNIQUE (execution_id, rollback_order),
    CHECK (length(source_relative_path) BETWEEN 1 AND 4096),
    CHECK (length(destination_relative_path) BETWEEN 1 AND 4096),
    CHECK (
        expected_fingerprint_json IS NULL
        OR length(expected_fingerprint_json) <= 32768
    ),
    CHECK (error_message IS NULL OR length(error_message) <= 2048)
) STRICT, WITHOUT ROWID;

CREATE TABLE local_execution_recovery (
    execution_id TEXT PRIMARY KEY REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    recovery_state TEXT NOT NULL CHECK (
        recovery_state IN (
            'recovery_not_required', 'recovery_available',
            'recovery_required', 'recovery_ambiguous'
        )
    ),
    not_started_count INTEGER NOT NULL DEFAULT 0 CHECK (not_started_count >= 0),
    applied_count INTEGER NOT NULL DEFAULT 0 CHECK (applied_count >= 0),
    ambiguous_count INTEGER NOT NULL DEFAULT 0 CHECK (ambiguous_count >= 0),
    details_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json)),
    assessed_at TEXT NOT NULL,
    resolved_at TEXT,
    CHECK (length(details_json) <= 1048576)
) STRICT;

CREATE TABLE local_execution_errors (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    operation_id TEXT REFERENCES local_execution_operations(id) ON DELETE RESTRICT,
    category TEXT NOT NULL CHECK (
        category IN (
            'isolated_failure', 'dependency_failure', 'critical_execution_failure'
        )
    ),
    error_code TEXT NOT NULL,
    user_message TEXT NOT NULL,
    technical_details TEXT,
    created_at TEXT NOT NULL,
    CHECK (length(error_code) BETWEEN 1 AND 128),
    CHECK (length(user_message) BETWEEN 1 AND 1024),
    CHECK (technical_details IS NULL OR length(technical_details) <= 8192)
) STRICT;

CREATE INDEX idx_local_execution_errors_session
    ON local_execution_errors(execution_id, created_at);

PRAGMA user_version = 8;
COMMIT;
