-- Allow the fail-closed executor to record safe cleanup of directories that
-- became empty after their files were moved. Existing execution history is
-- copied byte-for-byte; only the operation_kind CHECK is widened.

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE local_execution_operations_v18 (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    proposal_operation_id TEXT REFERENCES local_organization_proposal_operations(id) ON DELETE RESTRICT,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN (
            'create_directory', 'remove_directory_if_empty',
            'move', 'rename', 'move_and_rename', 'internal_stage'
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

INSERT INTO local_execution_operations_v18(
    id, execution_id, proposal_operation_id, operation_kind,
    source_relative_path, destination_relative_path, original_source_relative_path,
    expected_source_hash, expected_source_size, expected_source_modified_at,
    live_fingerprint_json, post_fingerprint_json, preconditions_json, dependencies_json,
    sequence_number, status, directory_existed_before, reason, error_code, error_message,
    started_at, completed_at, rolled_back_at
)
SELECT
    id, execution_id, proposal_operation_id, operation_kind,
    source_relative_path, destination_relative_path, original_source_relative_path,
    expected_source_hash, expected_source_size, expected_source_modified_at,
    live_fingerprint_json, post_fingerprint_json, preconditions_json, dependencies_json,
    sequence_number, status, directory_existed_before, reason, error_code, error_message,
    started_at, completed_at, rolled_back_at
FROM local_execution_operations;

DROP TABLE local_execution_operations;
ALTER TABLE local_execution_operations_v18 RENAME TO local_execution_operations;

CREATE INDEX idx_local_execution_operations_session
    ON local_execution_operations(execution_id, sequence_number);
CREATE INDEX idx_local_execution_operations_status
    ON local_execution_operations(execution_id, status, sequence_number);
CREATE UNIQUE INDEX idx_local_execution_proposal_operation
    ON local_execution_operations(execution_id, proposal_operation_id)
    WHERE proposal_operation_id IS NOT NULL AND operation_kind <> 'internal_stage';
CREATE UNIQUE INDEX idx_local_execution_operations_execution_id
    ON local_execution_operations(execution_id, id);

INSERT INTO schema_migrations(version, name)
VALUES (18, '0018_execution_empty_directory_cleanup');

PRAGMA user_version = 18;
COMMIT;
PRAGMA foreign_keys = ON;
