BEGIN IMMEDIATE;

CREATE UNIQUE INDEX idx_local_execution_operations_execution_id
    ON local_execution_operations(execution_id, id);

CREATE TABLE local_executor_sessions (
    session_id TEXT PRIMARY KEY CHECK (length(session_id) = 64),
    execution_id TEXT NOT NULL
        REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    plan_id TEXT NOT NULL,
    plan_digest TEXT NOT NULL CHECK (length(plan_digest) = 64),
    purpose TEXT NOT NULL CHECK (purpose IN ('forward', 'rollback')),
    coordinator_pid INTEGER NOT NULL CHECK (coordinator_pid > 0),
    child_pid INTEGER CHECK (child_pid IS NULL OR child_pid > 0),
    worker_nonce_hash TEXT NOT NULL CHECK (length(worker_nonce_hash) = 64),
    coordinator_nonce_hash TEXT NOT NULL CHECK (length(coordinator_nonce_hash) = 64),
    response_nonce_hash TEXT CHECK (
        response_nonce_hash IS NULL OR length(response_nonce_hash) = 64
    ),
    opened_at_unix_ms INTEGER NOT NULL CHECK (opened_at_unix_ms >= 0),
    closed_at_unix_ms INTEGER CHECK (
        closed_at_unix_ms IS NULL OR closed_at_unix_ms >= opened_at_unix_ms
    ),
    UNIQUE (session_id, execution_id),
    CHECK (length(plan_id) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX idx_local_executor_sessions_execution
    ON local_executor_sessions(execution_id, opened_at_unix_ms, session_id);

CREATE TABLE local_executor_requests (
    request_id TEXT PRIMARY KEY CHECK (length(request_id) = 64),
    session_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('forward', 'rollback')),
    request_sequence INTEGER NOT NULL CHECK (request_sequence > 0),
    request_nonce BLOB NOT NULL CHECK (length(request_nonce) = 32),
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
    intent_event_sequence INTEGER NOT NULL CHECK (intent_event_sequence >= 0),
    intent_event_digest TEXT NOT NULL CHECK (length(intent_event_digest) = 64),
    state TEXT NOT NULL CHECK (
        state IN (
            'intent_durable', 'acknowledged_success', 'proven_not_applied',
            'recovery_required', 'proven_not_started', 'proven_applied', 'ambiguous'
        )
    ),
    response_digest TEXT CHECK (
        response_digest IS NULL OR length(response_digest) = 64
    ),
    outcome_class TEXT CHECK (
        outcome_class IS NULL OR outcome_class IN (
            'success', 'proven_not_applied', 'recovery_required', 'protocol_refusal',
            'transport_ambiguous'
        )
    ),
    attempt_count INTEGER CHECK (
        attempt_count IS NULL OR attempt_count BETWEEN 1 AND 3
    ),
    error_class TEXT CHECK (
        error_class IS NULL OR length(error_class) BETWEEN 1 AND 128
    ),
    prepared_at TEXT NOT NULL,
    response_recorded_at TEXT,
    proof_recorded_at TEXT,
    FOREIGN KEY (session_id, execution_id)
        REFERENCES local_executor_sessions(session_id, execution_id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id, operation_id)
        REFERENCES local_execution_operations(execution_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id, intent_event_sequence)
        REFERENCES local_execution_journal(execution_id, sequence_number) ON DELETE RESTRICT,
    UNIQUE (session_id, request_sequence),
    UNIQUE (session_id, request_nonce),
    CHECK (
        (state = 'intent_durable'
            AND response_digest IS NULL
            AND response_recorded_at IS NULL)
        OR state <> 'intent_durable'
    ),
    CHECK (
        (response_digest IS NULL AND response_recorded_at IS NULL)
        OR (response_digest IS NOT NULL AND response_recorded_at IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_local_executor_requests_recovery
    ON local_executor_requests(execution_id, state, direction, request_sequence);

CREATE INDEX idx_local_executor_requests_operation
    ON local_executor_requests(execution_id, operation_id, direction, prepared_at);

CREATE TRIGGER local_executor_request_state_one_way
BEFORE UPDATE OF state ON local_executor_requests
FOR EACH ROW
WHEN NEW.state <> OLD.state
BEGIN
    SELECT CASE
        WHEN OLD.state = 'intent_durable'
         AND NEW.state IN (
            'acknowledged_success', 'proven_not_applied', 'recovery_required',
            'proven_not_started', 'proven_applied', 'ambiguous'
         ) THEN NULL
        WHEN OLD.state = 'acknowledged_success'
         AND NEW.state IN ('proven_applied', 'recovery_required', 'ambiguous') THEN NULL
        WHEN OLD.state = 'recovery_required'
         AND NEW.state IN ('proven_not_started', 'proven_applied', 'ambiguous') THEN NULL
        WHEN OLD.state = 'proven_not_applied'
         AND NEW.state IN ('recovery_required', 'proven_not_started') THEN NULL
        WHEN OLD.state IN ('proven_not_applied', 'proven_not_started', 'proven_applied')
         AND NEW.state = 'ambiguous' THEN NULL
        ELSE RAISE(ABORT, 'executor request state transition is not one-way')
    END;
END;

CREATE TABLE local_execution_retention (
    execution_id TEXT PRIMARY KEY
        REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    finalized_at TEXT,
    journal_retention_reason TEXT NOT NULL,
    rollback_retention_reason TEXT NOT NULL,
    minimum_retain_until TEXT,
    active_recovery INTEGER NOT NULL CHECK (active_recovery IN (0, 1)),
    rollback_eligible INTEGER NOT NULL CHECK (rollback_eligible IN (0, 1)),
    cleanup_eligible_at TEXT,
    cleanup_eligibility_reason TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (length(journal_retention_reason) BETWEEN 1 AND 256),
    CHECK (length(rollback_retention_reason) BETWEEN 1 AND 256),
    CHECK (length(cleanup_eligibility_reason) BETWEEN 1 AND 256),
    CHECK (
        active_recovery = 0
        OR (cleanup_eligible_at IS NULL AND rollback_eligible = 0)
    )
) STRICT;

INSERT INTO local_execution_retention(
    execution_id,
    finalized_at,
    journal_retention_reason,
    rollback_retention_reason,
    minimum_retain_until,
    active_recovery,
    rollback_eligible,
    cleanup_eligible_at,
    cleanup_eligibility_reason,
    updated_at
)
SELECT
    id,
    COALESCE(rolled_back_at, completed_at),
    CASE
        WHEN recovery_state <> 'recovery_not_required'
            THEN 'recovery_evidence_required'
        WHEN rollback_available = 1
            THEN 'rollback_evidence_required'
        ELSE 'execution_audit_history'
    END,
    CASE
        WHEN rollback_available = 1 THEN 'verified_applied_operations'
        ELSE 'no_rollback_eligible_operations'
    END,
    CASE
        WHEN COALESCE(rolled_back_at, completed_at) IS NULL THEN NULL
        ELSE strftime(
            '%Y-%m-%dT%H:%M:%fZ',
            julianday(COALESCE(rolled_back_at, completed_at)) + 30
        )
    END,
    CASE WHEN recovery_state <> 'recovery_not_required' THEN 1 ELSE 0 END,
    CASE
        WHEN recovery_state = 'recovery_not_required' AND rollback_available = 1 THEN 1
        ELSE 0
    END,
    CASE
        WHEN recovery_state = 'recovery_not_required'
         AND rollback_available = 0
         AND COALESCE(rolled_back_at, completed_at) IS NOT NULL
        THEN strftime(
            '%Y-%m-%dT%H:%M:%fZ',
            julianday(COALESCE(rolled_back_at, completed_at)) + 30
        )
        ELSE NULL
    END,
    CASE
        WHEN recovery_state <> 'recovery_not_required' THEN 'active_recovery_blocks_cleanup'
        WHEN rollback_available = 1 THEN 'rollback_eligibility_blocks_cleanup'
        WHEN COALESCE(rolled_back_at, completed_at) IS NULL THEN 'execution_not_finalized'
        ELSE 'eligible_after_minimum_retention'
    END,
    COALESCE(rolled_back_at, completed_at, created_at)
FROM local_execution_sessions;

INSERT INTO schema_migrations(version, name)
VALUES (15, '0015_cross_process_recovery_hardening');

PRAGMA user_version = 15;

COMMIT;
