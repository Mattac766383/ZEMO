BEGIN IMMEDIATE;

CREATE TABLE local_organization_preferences (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    client_first INTEGER NOT NULL DEFAULT 1 CHECK (client_first IN (0, 1)),
    include_year_folders INTEGER NOT NULL DEFAULT 1 CHECK (include_year_folders IN (0, 1)),
    maximum_depth INTEGER NOT NULL DEFAULT 6 CHECK (maximum_depth BETWEEN 2 AND 8),
    minimum_group_size INTEGER NOT NULL DEFAULT 2 CHECK (minimum_group_size BETWEEN 1 AND 20),
    keep_photos_inside_projects INTEGER NOT NULL DEFAULT 1 CHECK (
        keep_photos_inside_projects IN (0, 1)
    ),
    supplier_invoices_inside_projects INTEGER NOT NULL DEFAULT 1 CHECK (
        supplier_invoices_inside_projects IN (0, 1)
    ),
    naming_language TEXT NOT NULL DEFAULT 'en' CHECK (
        naming_language IN ('en', 'fr')
    ),
    preserve_existing_folders INTEGER NOT NULL DEFAULT 1 CHECK (
        preserve_existing_folders IN (0, 1)
    ),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE TABLE local_organization_proposals (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE RESTRICT,
    source_scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE RESTRICT,
    current_revision_id TEXT REFERENCES local_organization_proposal_revisions(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'draft', 'ready_for_review', 'reviewed',
            'approved_for_future_apply', 'superseded', 'cancelled'
        )
    ),
    engine_version TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    superseded_at TEXT,
    CHECK (length(engine_version) BETWEEN 1 AND 64),
    CHECK (length(policy_version) BETWEEN 1 AND 64),
    CHECK (updated_at >= created_at),
    CHECK (superseded_at IS NULL OR status = 'superseded')
) STRICT;

CREATE INDEX idx_local_org_proposals_workspace
    ON local_organization_proposals(workspace_id, status, updated_at DESC);
CREATE UNIQUE INDEX idx_local_org_current_workspace
    ON local_organization_proposals(workspace_id)
    WHERE status IN ('draft', 'ready_for_review', 'reviewed', 'approved_for_future_apply');

CREATE TABLE local_organization_proposal_revisions (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES local_organization_proposals(id) ON DELETE CASCADE,
    revision_number INTEGER NOT NULL CHECK (revision_number > 0),
    trigger_kind TEXT NOT NULL CHECK (
        trigger_kind IN (
            'initial', 'manual_recompute', 'semantic_changed',
            'relationships_changed', 'user_override', 'algorithm_changed'
        )
    ),
    status TEXT NOT NULL CHECK (
        status IN ('draft', 'ready_for_review', 'reviewed', 'cancelled')
    ),
    source_semantic_version TEXT,
    source_relationship_version TEXT,
    files_analyzed INTEGER NOT NULL CHECK (files_analyzed >= 0),
    proposed_moves INTEGER NOT NULL CHECK (proposed_moves >= 0),
    proposed_renames INTEGER NOT NULL CHECK (proposed_renames >= 0),
    unchanged_count INTEGER NOT NULL CHECK (unchanged_count >= 0),
    needs_review_count INTEGER NOT NULL CHECK (needs_review_count >= 0),
    unresolved_count INTEGER NOT NULL CHECK (unresolved_count >= 0),
    conflict_count INTEGER NOT NULL CHECK (conflict_count >= 0),
    high_confidence_count INTEGER NOT NULL CHECK (high_confidence_count >= 0),
    medium_confidence_count INTEGER NOT NULL CHECK (medium_confidence_count >= 0),
    low_confidence_count INTEGER NOT NULL CHECK (low_confidence_count >= 0),
    duplicate_no_action_count INTEGER NOT NULL CHECK (duplicate_no_action_count >= 0),
    average_depth REAL NOT NULL CHECK (average_depth >= 0.0 AND average_depth <= 8.0),
    maximum_depth INTEGER NOT NULL CHECK (maximum_depth BETWEEN 0 AND 8),
    destinations_changed INTEGER NOT NULL DEFAULT 0 CHECK (destinations_changed >= 0),
    files_added INTEGER NOT NULL DEFAULT 0 CHECK (files_added >= 0),
    conflicts_resolved INTEGER NOT NULL DEFAULT 0 CHECK (conflicts_resolved >= 0),
    moved_to_review INTEGER NOT NULL DEFAULT 0 CHECK (moved_to_review >= 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (proposal_id, revision_number),
    CHECK (
        source_semantic_version IS NULL
        OR length(source_semantic_version) BETWEEN 1 AND 64
    ),
    CHECK (
        source_relationship_version IS NULL
        OR length(source_relationship_version) BETWEEN 1 AND 64
    )
) STRICT;

CREATE INDEX idx_local_org_revisions_proposal
    ON local_organization_proposal_revisions(proposal_id, revision_number DESC);

CREATE TABLE local_organization_proposal_operations (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES local_organization_proposals(id) ON DELETE CASCADE,
    revision_id TEXT NOT NULL REFERENCES local_organization_proposal_revisions(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE RESTRICT,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN (
            'move_proposal', 'rename_proposal', 'create_folder_proposal',
            'keep_in_place', 'to_review', 'no_action'
        )
    ),
    source_relative_path TEXT NOT NULL,
    source_name TEXT NOT NULL,
    source_hash TEXT,
    source_byte_size INTEGER NOT NULL CHECK (source_byte_size >= 0),
    source_modified_at TEXT,
    machine_destination_json TEXT NOT NULL CHECK (json_valid(machine_destination_json)),
    machine_name TEXT NOT NULL,
    proposed_destination_json TEXT NOT NULL CHECK (json_valid(proposed_destination_json)),
    proposed_name TEXT NOT NULL,
    confidence_score REAL NOT NULL CHECK (
        confidence_score >= 0.0 AND confidence_score <= 1.0
    ),
    confidence_level TEXT NOT NULL CHECK (
        confidence_level IN ('very_high', 'high', 'medium', 'low')
    ),
    conflict_state TEXT NOT NULL CHECK (
        conflict_state IN (
            'none', 'auto_resolved', 'destination_collision',
            'invalid_path', 'path_too_long', 'stale_source'
        )
    ),
    needs_review INTEGER NOT NULL CHECK (needs_review IN (0, 1)),
    stale INTEGER NOT NULL CHECK (stale IN (0, 1)),
    user_override INTEGER NOT NULL CHECK (user_override IN (0, 1)),
    disruption_score REAL NOT NULL CHECK (
        disruption_score >= 0.0 AND disruption_score <= 1.0
    ),
    proposed_path_length INTEGER NOT NULL CHECK (proposed_path_length >= 1),
    proposed_depth INTEGER NOT NULL CHECK (proposed_depth BETWEEN 0 AND 8),
    semantic_context TEXT NOT NULL CHECK (
        semantic_context IN ('personal', 'business', 'mixed', 'unknown')
    ),
    document_type TEXT NOT NULL,
    customer_name TEXT,
    supplier_name TEXT,
    project_name TEXT,
    duplicate_group_id TEXT,
    duplicate_canonical INTEGER NOT NULL CHECK (duplicate_canonical IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (revision_id, file_id),
    CHECK (length(source_relative_path) BETWEEN 1 AND 4096),
    CHECK (length(source_name) BETWEEN 1 AND 512),
    CHECK (source_hash IS NULL OR length(source_hash) BETWEEN 1 AND 256),
    CHECK (length(machine_destination_json) <= 8192),
    CHECK (length(proposed_destination_json) <= 8192),
    CHECK (length(machine_name) BETWEEN 1 AND 512),
    CHECK (length(proposed_name) BETWEEN 1 AND 512),
    CHECK (length(document_type) BETWEEN 1 AND 64),
    CHECK (customer_name IS NULL OR length(customer_name) BETWEEN 1 AND 512),
    CHECK (supplier_name IS NULL OR length(supplier_name) BETWEEN 1 AND 512),
    CHECK (project_name IS NULL OR length(project_name) BETWEEN 1 AND 512)
) STRICT;

CREATE INDEX idx_local_org_operations_revision
    ON local_organization_proposal_operations(revision_id, operation_kind, needs_review, conflict_state);
CREATE INDEX idx_local_org_operations_file
    ON local_organization_proposal_operations(proposal_id, file_id);
CREATE INDEX idx_local_org_operations_relationships
    ON local_organization_proposal_operations(revision_id, customer_name, project_name, supplier_name);

CREATE TABLE local_organization_proposal_reasons (
    id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL REFERENCES local_organization_proposal_operations(id) ON DELETE CASCADE,
    reason_order INTEGER NOT NULL CHECK (reason_order >= 0),
    reason_code TEXT NOT NULL,
    explanation TEXT NOT NULL,
    evidence_references_json TEXT NOT NULL DEFAULT '[]' CHECK (
        json_valid(evidence_references_json)
    ),
    UNIQUE (operation_id, reason_order),
    CHECK (length(reason_code) BETWEEN 1 AND 64),
    CHECK (length(explanation) BETWEEN 1 AND 512),
    CHECK (length(evidence_references_json) <= 8192)
) STRICT;

CREATE INDEX idx_local_org_reasons_operation
    ON local_organization_proposal_reasons(operation_id, reason_order);

CREATE TABLE local_organization_virtual_nodes (
    id TEXT PRIMARY KEY,
    revision_id TEXT NOT NULL REFERENCES local_organization_proposal_revisions(id) ON DELETE CASCADE,
    parent_id TEXT REFERENCES local_organization_virtual_nodes(id) ON DELETE CASCADE,
    node_kind TEXT NOT NULL CHECK (node_kind IN ('root', 'folder', 'file')),
    name TEXT NOT NULL,
    virtual_path TEXT NOT NULL,
    operation_id TEXT REFERENCES local_organization_proposal_operations(id) ON DELETE CASCADE,
    child_count INTEGER NOT NULL CHECK (child_count >= 0),
    needs_review_count INTEGER NOT NULL CHECK (needs_review_count >= 0),
    conflict_count INTEGER NOT NULL CHECK (conflict_count >= 0),
    CHECK (length(name) BETWEEN 1 AND 512),
    CHECK (length(virtual_path) <= 8192),
    CHECK (
        (node_kind = 'file' AND operation_id IS NOT NULL)
        OR (node_kind <> 'file' AND operation_id IS NULL)
    )
) STRICT;

CREATE INDEX idx_local_org_nodes_parent
    ON local_organization_virtual_nodes(revision_id, parent_id, name);
CREATE INDEX idx_local_org_nodes_operation
    ON local_organization_virtual_nodes(operation_id)
    WHERE operation_id IS NOT NULL;

CREATE TABLE local_organization_user_overrides (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES local_organization_proposals(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    action TEXT NOT NULL CHECK (
        action IN (
            'destination', 'rename', 'destination_and_rename',
            'keep_in_place', 'to_review', 'reject'
        )
    ),
    destination_json TEXT CHECK (
        destination_json IS NULL OR json_valid(destination_json)
    ),
    proposed_name TEXT,
    reason TEXT,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    superseded_at TEXT,
    CHECK (destination_json IS NULL OR length(destination_json) <= 8192),
    CHECK (proposed_name IS NULL OR length(proposed_name) BETWEEN 1 AND 512),
    CHECK (reason IS NULL OR length(reason) <= 512),
    CHECK (updated_at >= created_at),
    CHECK (superseded_at IS NULL OR active = 0)
) STRICT;

CREATE UNIQUE INDEX idx_local_org_active_override
    ON local_organization_user_overrides(proposal_id, file_id)
    WHERE active = 1;
CREATE INDEX idx_local_org_override_history
    ON local_organization_user_overrides(proposal_id, file_id, updated_at DESC);

PRAGMA user_version = 7;
COMMIT;
