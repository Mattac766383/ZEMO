BEGIN IMMEDIATE;

CREATE TABLE identity_resolver_runs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    trigger_kind TEXT NOT NULL CHECK (
        trigger_kind IN (
            'semantic_analysis', 'semantic_correction', 'new_file',
            'resolver_upgrade', 'manual'
        )
    ),
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'completed', 'cancelled', 'failed')
    ),
    resolver_id TEXT NOT NULL,
    resolver_version TEXT NOT NULL,
    processing_location TEXT NOT NULL DEFAULT 'local' CHECK (
        processing_location = 'local'
    ),
    files_considered INTEGER NOT NULL DEFAULT 0 CHECK (files_considered >= 0),
    occurrences_processed INTEGER NOT NULL DEFAULT 0 CHECK (occurrences_processed >= 0),
    blocking_memberships INTEGER NOT NULL DEFAULT 0 CHECK (blocking_memberships >= 0),
    comparisons INTEGER NOT NULL DEFAULT 0 CHECK (comparisons >= 0),
    candidates_created INTEGER NOT NULL DEFAULT 0 CHECK (candidates_created >= 0),
    auto_links_created INTEGER NOT NULL DEFAULT 0 CHECK (auto_links_created >= 0),
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at TEXT,
    error_message TEXT,
    CHECK (length(resolver_id) BETWEEN 1 AND 128),
    CHECK (length(resolver_version) BETWEEN 1 AND 64),
    CHECK (error_message IS NULL OR length(error_message) <= 512),
    CHECK (completed_at IS NULL OR completed_at >= started_at)
) STRICT;

CREATE INDEX idx_identity_runs_workspace
    ON identity_resolver_runs(workspace_id, started_at DESC);
CREATE INDEX idx_identity_runs_status
    ON identity_resolver_runs(workspace_id, status);
CREATE UNIQUE INDEX idx_identity_runs_single_active
    ON identity_resolver_runs(workspace_id)
    WHERE status = 'running';

CREATE TABLE resolved_identities (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    identity_type TEXT NOT NULL CHECK (
        identity_type IN ('organization', 'person', 'project')
    ),
    display_name TEXT NOT NULL,
    normalized_display_name TEXT NOT NULL,
    resolution_status TEXT NOT NULL DEFAULT 'unresolved' CHECK (
        resolution_status IN (
            'unresolved', 'candidate', 'auto_linked', 'user_confirmed'
        )
    ),
    lifecycle_status TEXT NOT NULL DEFAULT 'active' CHECK (
        lifecycle_status IN ('active', 'merged', 'split')
    ),
    merged_into_identity_id TEXT
        REFERENCES resolved_identities(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    user_locked INTEGER NOT NULL DEFAULT 0 CHECK (user_locked IN (0, 1)),
    confidence REAL NOT NULL DEFAULT 0.0 CHECK (
        confidence >= 0.0 AND confidence <= 1.0
    ),
    creation_source TEXT NOT NULL CHECK (
        creation_source IN ('resolver', 'auto_link', 'user', 'merge', 'split')
    ),
    resolver_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (length(display_name) BETWEEN 1 AND 512),
    CHECK (length(normalized_display_name) BETWEEN 1 AND 512),
    CHECK (length(resolver_version) BETWEEN 1 AND 64),
    CHECK (
        (lifecycle_status = 'merged' AND merged_into_identity_id IS NOT NULL)
        OR (lifecycle_status <> 'merged' AND merged_into_identity_id IS NULL)
    ),
    CHECK (merged_into_identity_id IS NULL OR merged_into_identity_id <> id),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX idx_identities_workspace_type
    ON resolved_identities(workspace_id, identity_type, lifecycle_status);
CREATE INDEX idx_identities_normalized_name
    ON resolved_identities(
        workspace_id, identity_type, normalized_display_name, lifecycle_status
    );
CREATE INDEX idx_identities_merge_target
    ON resolved_identities(merged_into_identity_id)
    WHERE merged_into_identity_id IS NOT NULL;

CREATE TABLE identity_occurrences (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    source_key TEXT NOT NULL,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    semantic_analysis_id TEXT REFERENCES semantic_analyses(id) ON DELETE SET NULL,
    semantic_entity_id TEXT REFERENCES semantic_entities(id) ON DELETE SET NULL,
    semantic_field_id TEXT REFERENCES semantic_fields(id) ON DELETE SET NULL,
    occurrence_type TEXT NOT NULL CHECK (
        occurrence_type IN ('organization', 'person', 'project')
    ),
    original_value TEXT NOT NULL,
    normalized_value TEXT NOT NULL,
    normalized_core TEXT NOT NULL,
    legal_suffix TEXT,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    source_method TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    resolver_version TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    first_observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    superseded_at TEXT,
    UNIQUE (workspace_id, source_key),
    CHECK ((semantic_entity_id IS NOT NULL) != (semantic_field_id IS NOT NULL)),
    CHECK (length(source_key) BETWEEN 1 AND 768),
    CHECK (length(original_value) BETWEEN 1 AND 512),
    CHECK (length(normalized_value) BETWEEN 1 AND 512),
    CHECK (length(normalized_core) BETWEEN 1 AND 512),
    CHECK (legal_suffix IS NULL OR length(legal_suffix) BETWEEN 1 AND 32),
    CHECK (length(source_method) BETWEEN 1 AND 128),
    CHECK (length(analyzer_version) BETWEEN 1 AND 64),
    CHECK (length(resolver_version) BETWEEN 1 AND 64),
    CHECK (last_observed_at >= first_observed_at),
    CHECK (superseded_at IS NULL OR active = 0)
) STRICT;

CREATE INDEX idx_identity_occurrences_identity
    ON identity_occurrences(identity_id, active, last_observed_at DESC);
CREATE INDEX idx_identity_occurrences_file
    ON identity_occurrences(file_id, active);
CREATE INDEX idx_identity_occurrences_name
    ON identity_occurrences(
        workspace_id, occurrence_type, normalized_value, active
    );
CREATE INDEX idx_identity_occurrences_core
    ON identity_occurrences(
        workspace_id, occurrence_type, normalized_core, active
    );
CREATE UNIQUE INDEX idx_identity_occurrence_semantic_entity
    ON identity_occurrences(semantic_entity_id)
    WHERE semantic_entity_id IS NOT NULL;
CREATE UNIQUE INDEX idx_identity_occurrence_semantic_field
    ON identity_occurrences(semantic_field_id)
    WHERE semantic_field_id IS NOT NULL;

CREATE TABLE identity_occurrence_signals (
    id TEXT PRIMARY KEY,
    occurrence_id TEXT NOT NULL REFERENCES identity_occurrences(id) ON DELETE CASCADE,
    signal_kind TEXT NOT NULL CHECK (
        signal_kind IN (
            'name', 'company_identifier', 'vat_identifier', 'email', 'domain',
            'phone', 'address', 'account_identifier', 'project_reference',
            'customer_identity', 'date', 'path_context'
        )
    ),
    original_value TEXT NOT NULL,
    normalized_value TEXT NOT NULL,
    semantic_entity_id TEXT REFERENCES semantic_entities(id) ON DELETE SET NULL,
    semantic_field_id TEXT REFERENCES semantic_fields(id) ON DELETE SET NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    source_method TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (occurrence_id, signal_kind, normalized_value),
    CHECK (length(original_value) BETWEEN 1 AND 512),
    CHECK (length(normalized_value) BETWEEN 1 AND 512),
    CHECK (length(source_method) BETWEEN 1 AND 128),
    CHECK (length(analyzer_version) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX idx_identity_signals_lookup
    ON identity_occurrence_signals(signal_kind, normalized_value, occurrence_id);
CREATE INDEX idx_identity_signals_occurrence
    ON identity_occurrence_signals(occurrence_id, signal_kind);

CREATE TABLE identity_aliases (
    id TEXT PRIMARY KEY,
    identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    occurrence_id TEXT REFERENCES identity_occurrences(id) ON DELETE SET NULL,
    original_value TEXT NOT NULL,
    normalized_value TEXT NOT NULL,
    legal_suffix TEXT,
    source TEXT NOT NULL CHECK (source IN ('occurrence', 'user', 'merge', 'split')),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (identity_id, normalized_value, original_value),
    CHECK (length(original_value) BETWEEN 1 AND 512),
    CHECK (length(normalized_value) BETWEEN 1 AND 512),
    CHECK (legal_suffix IS NULL OR length(legal_suffix) BETWEEN 1 AND 32)
) STRICT;

CREATE INDEX idx_identity_aliases_identity
    ON identity_aliases(identity_id, active);
CREATE INDEX idx_identity_aliases_normalized
    ON identity_aliases(normalized_value, active);

CREATE TABLE identity_roles (
    id TEXT PRIMARY KEY,
    identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (role IN ('customer', 'supplier')),
    occurrence_id TEXT REFERENCES identity_occurrences(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('observed', 'user_confirmed')),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (identity_id, role, occurrence_id),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX idx_identity_roles_identity ON identity_roles(identity_id, active);
CREATE INDEX idx_identity_roles_role ON identity_roles(role, identity_id, active);

CREATE TABLE identity_candidates (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    left_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    right_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    pair_key TEXT NOT NULL,
    review_group_key TEXT NOT NULL,
    score REAL NOT NULL CHECK (score >= 0.0 AND score <= 1.0),
    policy_decision TEXT NOT NULL CHECK (
        policy_decision IN ('auto_link', 'review', 'keep_separate', 'unknown')
    ),
    status TEXT NOT NULL CHECK (
        status IN (
            'candidate', 'auto_linked', 'user_confirmed', 'user_rejected',
            'conflicting', 'superseded'
        )
    ),
    creation_source TEXT NOT NULL CHECK (
        creation_source IN ('resolver', 'incremental', 'user')
    ),
    resolver_version TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    decided_at TEXT,
    UNIQUE (workspace_id, pair_key, resolver_version),
    CHECK (left_identity_id < right_identity_id),
    CHECK (length(pair_key) BETWEEN 3 AND 300),
    CHECK (length(review_group_key) BETWEEN 1 AND 768),
    CHECK (length(resolver_version) BETWEEN 1 AND 64),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX idx_identity_candidates_review
    ON identity_candidates(workspace_id, status, score DESC, updated_at DESC);
CREATE INDEX idx_identity_candidates_left
    ON identity_candidates(left_identity_id, active);
CREATE INDEX idx_identity_candidates_right
    ON identity_candidates(right_identity_id, active);
CREATE INDEX idx_identity_candidates_group
    ON identity_candidates(workspace_id, review_group_key, active);

CREATE TABLE identity_candidate_evidence (
    id TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL REFERENCES identity_candidates(id) ON DELETE CASCADE,
    evidence_type TEXT NOT NULL,
    strength TEXT NOT NULL CHECK (
        strength IN ('very_strong', 'strong', 'medium', 'weak', 'conflicting')
    ),
    polarity TEXT NOT NULL CHECK (polarity IN ('supports', 'conflicts')),
    left_value TEXT NOT NULL,
    right_value TEXT NOT NULL,
    weight REAL NOT NULL CHECK (weight >= 0.0 AND weight <= 1.0),
    explanation TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (length(evidence_type) BETWEEN 1 AND 64),
    CHECK (length(left_value) <= 512),
    CHECK (length(right_value) <= 512),
    CHECK (length(explanation) BETWEEN 1 AND 512)
) STRICT;

CREATE INDEX idx_identity_candidate_evidence_candidate
    ON identity_candidate_evidence(candidate_id, strength);

CREATE TABLE identity_decisions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    decision_type TEXT NOT NULL CHECK (
        decision_type IN (
            'candidate_created', 'auto_linked', 'confirm_match', 'reject_match',
            'keep_separate', 'user_merge', 'unlink_occurrence', 'split_identity'
        )
    ),
    decision_source TEXT NOT NULL CHECK (decision_source IN ('resolver', 'user')),
    candidate_id TEXT REFERENCES identity_candidates(id) ON DELETE SET NULL,
    primary_identity_id TEXT REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    secondary_identity_id TEXT REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    occurrence_id TEXT REFERENCES identity_occurrences(id) ON DELETE SET NULL,
    reason TEXT,
    evidence_json TEXT NOT NULL DEFAULT '[]' CHECK (
        json_valid(evidence_json) AND json_type(evidence_json) = 'array'
    ),
    resolver_version TEXT NOT NULL,
    reversed_by_decision_id TEXT REFERENCES identity_decisions(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (reason IS NULL OR length(reason) <= 512),
    CHECK (length(resolver_version) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX idx_identity_decisions_workspace
    ON identity_decisions(workspace_id, created_at DESC);
CREATE INDEX idx_identity_decisions_candidate
    ON identity_decisions(candidate_id)
    WHERE candidate_id IS NOT NULL;
CREATE INDEX idx_identity_decisions_primary
    ON identity_decisions(primary_identity_id, created_at DESC)
    WHERE primary_identity_id IS NOT NULL;

CREATE TABLE identity_rejection_constraints (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    left_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    right_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    pair_key TEXT NOT NULL,
    decision_id TEXT NOT NULL REFERENCES identity_decisions(id) ON DELETE RESTRICT,
    reason TEXT,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    revoked_at TEXT,
    CHECK (left_identity_id < right_identity_id),
    CHECK (length(pair_key) BETWEEN 3 AND 300),
    CHECK (reason IS NULL OR length(reason) <= 512),
    CHECK (revoked_at IS NULL OR active = 0)
) STRICT;

CREATE UNIQUE INDEX idx_identity_rejections_active_pair
    ON identity_rejection_constraints(workspace_id, pair_key)
    WHERE active = 1;
CREATE INDEX idx_identity_rejections_left
    ON identity_rejection_constraints(left_identity_id, active);
CREATE INDEX idx_identity_rejections_right
    ON identity_rejection_constraints(right_identity_id, active);

CREATE TABLE identity_merge_history (
    id TEXT PRIMARY KEY,
    decision_id TEXT NOT NULL REFERENCES identity_decisions(id) ON DELETE RESTRICT,
    occurrence_id TEXT NOT NULL REFERENCES identity_occurrences(id) ON DELETE RESTRICT,
    from_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    to_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    restored_at TEXT,
    UNIQUE (decision_id, occurrence_id),
    CHECK (from_identity_id <> to_identity_id)
) STRICT;

CREATE INDEX idx_identity_merge_history_decision
    ON identity_merge_history(decision_id, restored_at);
CREATE INDEX idx_identity_merge_history_occurrence
    ON identity_merge_history(occurrence_id, restored_at);

CREATE TABLE identity_audit_events (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL CHECK (
        event_type IN (
            'candidate_created', 'auto_linked', 'user_confirmed', 'user_rejected',
            'user_merged', 'occurrence_unlinked', 'identity_split',
            'resolver_started', 'resolver_completed', 'resolver_cancelled',
            'resolver_failed'
        )
    ),
    decision_source TEXT NOT NULL CHECK (decision_source IN ('resolver', 'user')),
    identity_id TEXT REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    related_identity_id TEXT REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    candidate_id TEXT REFERENCES identity_candidates(id) ON DELETE SET NULL,
    occurrence_id TEXT REFERENCES identity_occurrences(id) ON DELETE SET NULL,
    reason TEXT,
    resolver_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (reason IS NULL OR length(reason) <= 512),
    CHECK (length(resolver_version) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX idx_identity_audit_workspace
    ON identity_audit_events(workspace_id, created_at DESC);
CREATE INDEX idx_identity_audit_identity
    ON identity_audit_events(identity_id, created_at DESC)
    WHERE identity_id IS NOT NULL;

CREATE TABLE identity_relationships (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('file', 'identity')),
    source_file_id TEXT REFERENCES files(id) ON DELETE CASCADE,
    source_identity_id TEXT REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    target_identity_id TEXT NOT NULL REFERENCES resolved_identities(id) ON DELETE RESTRICT,
    relationship_type TEXT NOT NULL CHECK (
        relationship_type IN (
            'file_customer', 'file_supplier', 'file_project',
            'project_customer', 'document_project'
        )
    ),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    status TEXT NOT NULL CHECK (
        status IN (
            'candidate', 'auto_linked', 'user_confirmed',
            'user_rejected', 'conflicting'
        )
    ),
    creation_source TEXT NOT NULL CHECK (
        creation_source IN ('resolver', 'user', 'semantic_occurrence')
    ),
    resolver_version TEXT NOT NULL,
    user_confirmation_state TEXT CHECK (
        user_confirmation_state IS NULL
        OR user_confirmation_state IN ('confirmed', 'rejected')
    ),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (source_kind = 'file' AND source_file_id IS NOT NULL AND source_identity_id IS NULL)
        OR
        (source_kind = 'identity' AND source_file_id IS NULL AND source_identity_id IS NOT NULL)
    ),
    CHECK (source_identity_id IS NULL OR source_identity_id <> target_identity_id),
    CHECK (length(resolver_version) BETWEEN 1 AND 64),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE UNIQUE INDEX idx_identity_relationship_unique_file
    ON identity_relationships(
        workspace_id, source_file_id, target_identity_id, relationship_type
    )
    WHERE source_kind = 'file' AND active = 1;
CREATE UNIQUE INDEX idx_identity_relationship_unique_identity
    ON identity_relationships(
        workspace_id, source_identity_id, target_identity_id, relationship_type
    )
    WHERE source_kind = 'identity' AND active = 1;
CREATE INDEX idx_identity_relationship_target
    ON identity_relationships(target_identity_id, status, active);
CREATE INDEX idx_identity_relationship_file
    ON identity_relationships(source_file_id, active)
    WHERE source_file_id IS NOT NULL;

CREATE TABLE identity_relationship_evidence (
    id TEXT PRIMARY KEY,
    relationship_id TEXT NOT NULL REFERENCES identity_relationships(id) ON DELETE CASCADE,
    occurrence_id TEXT REFERENCES identity_occurrences(id) ON DELETE SET NULL,
    candidate_evidence_id TEXT REFERENCES identity_candidate_evidence(id) ON DELETE SET NULL,
    evidence_type TEXT NOT NULL,
    explanation TEXT NOT NULL,
    exact_text TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (length(evidence_type) BETWEEN 1 AND 64),
    CHECK (length(explanation) BETWEEN 1 AND 512),
    CHECK (exact_text IS NULL OR length(exact_text) <= 2000)
) STRICT;

CREATE INDEX idx_identity_relationship_evidence
    ON identity_relationship_evidence(relationship_id);
CREATE UNIQUE INDEX idx_identity_relationship_evidence_occurrence
    ON identity_relationship_evidence(
        relationship_id, occurrence_id, evidence_type
    )
    WHERE occurrence_id IS NOT NULL;

CREATE TABLE identity_review_groups (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    review_reason TEXT NOT NULL CHECK (
        review_reason IN (
            'possible_duplicate_identity', 'conflicting_identity_evidence',
            'ambiguous_project_match', 'ambiguous_person_match'
        )
    ),
    group_key TEXT NOT NULL,
    title TEXT NOT NULL,
    explanation TEXT NOT NULL,
    max_score REAL NOT NULL CHECK (max_score >= 0.0 AND max_score <= 1.0),
    candidate_count INTEGER NOT NULL CHECK (candidate_count >= 1),
    occurrence_count INTEGER NOT NULL CHECK (occurrence_count >= 2),
    file_count INTEGER NOT NULL CHECK (file_count >= 1),
    status TEXT NOT NULL DEFAULT 'needs_review' CHECK (
        status IN ('needs_review', 'resolved', 'ignored')
    ),
    resolver_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    resolved_at TEXT,
    UNIQUE (workspace_id, group_key),
    CHECK (length(group_key) BETWEEN 1 AND 768),
    CHECK (length(title) BETWEEN 1 AND 512),
    CHECK (length(explanation) BETWEEN 1 AND 512),
    CHECK (length(resolver_version) BETWEEN 1 AND 64),
    CHECK (updated_at >= created_at),
    CHECK (resolved_at IS NULL OR status = 'resolved')
) STRICT;

CREATE INDEX idx_identity_review_workspace
    ON identity_review_groups(workspace_id, status, updated_at DESC);
CREATE INDEX idx_identity_review_reason
    ON identity_review_groups(workspace_id, review_reason, status);

CREATE TABLE identity_review_group_candidates (
    review_group_id TEXT NOT NULL REFERENCES identity_review_groups(id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL REFERENCES identity_candidates(id) ON DELETE CASCADE,
    PRIMARY KEY (review_group_id, candidate_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX idx_identity_review_candidate
    ON identity_review_group_candidates(candidate_id);

CREATE TABLE identity_resolution_state (
    file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    semantic_analysis_id TEXT REFERENCES semantic_analyses(id) ON DELETE SET NULL,
    resolver_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'completed', 'cancelled', 'failed')
    ),
    source_digest BLOB CHECK (source_digest IS NULL OR length(source_digest) = 32),
    last_run_id TEXT REFERENCES identity_resolver_runs(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (length(resolver_version) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX idx_identity_resolution_state_pending
    ON identity_resolution_state(workspace_id, status, updated_at);

INSERT INTO schema_migrations(version, name)
VALUES (6, '0006_local_cross_file_relationships');

PRAGMA user_version = 6;

COMMIT;
