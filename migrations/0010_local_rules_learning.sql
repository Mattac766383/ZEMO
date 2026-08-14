BEGIN IMMEDIATE;

ALTER TABLE local_organization_preferences
    ADD COLUMN personal_root_name TEXT NOT NULL DEFAULT 'Personal'
        CHECK (length(personal_root_name) BETWEEN 1 AND 80);
ALTER TABLE local_organization_preferences
    ADD COLUMN business_root_name TEXT NOT NULL DEFAULT 'Business'
        CHECK (length(business_root_name) BETWEEN 1 AND 80);
ALTER TABLE local_organization_preferences
    ADD COLUMN rename_template TEXT NOT NULL
        DEFAULT '{date}_{party}_{document_type}_{identifier}'
        CHECK (length(rename_template) BETWEEN 1 AND 256);
ALTER TABLE local_organization_preferences
    ADD COLUMN review_threshold REAL NOT NULL DEFAULT 0.65
        CHECK (review_threshold >= 0.5 AND review_threshold <= 0.99);

CREATE TABLE local_user_rules (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    explanation TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    conditions_json TEXT NOT NULL CHECK (json_valid(conditions_json)),
    action_kind TEXT NOT NULL CHECK (
        action_kind IN (
            'set_semantic_field', 'classify_party', 'prefer_project_location',
            'set_destination', 'preserve_subtree', 'use_year_folders'
        )
    ),
    action_json TEXT NOT NULL CHECK (json_valid(action_json)),
    origin TEXT NOT NULL CHECK (origin IN ('user_created', 'accepted_suggestion')),
    source_suggestion_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (length(name) BETWEEN 1 AND 120),
    CHECK (length(explanation) BETWEEN 1 AND 512),
    CHECK (length(conditions_json) BETWEEN 2 AND 16384),
    CHECK (length(action_json) BETWEEN 2 AND 8192),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX idx_local_user_rules_workspace
    ON local_user_rules(workspace_id, enabled, position, id);
CREATE INDEX idx_local_user_rules_suggestion
    ON local_user_rules(source_suggestion_id)
    WHERE source_suggestion_id IS NOT NULL;

CREATE TABLE local_learning_observations (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('semantic_correction', 'organization_override')
    ),
    source_ref TEXT NOT NULL,
    pattern_kind TEXT NOT NULL CHECK (
        pattern_kind IN ('semantic_field', 'project_supplier_invoice', 'destination')
    ),
    pattern_key TEXT NOT NULL,
    evidence_json TEXT NOT NULL CHECK (json_valid(evidence_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source_kind, source_ref),
    CHECK (length(source_ref) BETWEEN 1 AND 128),
    CHECK (length(pattern_key) BETWEEN 1 AND 1024),
    CHECK (length(evidence_json) <= 16384)
) STRICT;

CREATE INDEX idx_local_learning_pattern
    ON local_learning_observations(workspace_id, pattern_kind, pattern_key, created_at);

CREATE TABLE local_rule_suggestions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    signature TEXT NOT NULL,
    title TEXT NOT NULL,
    explanation TEXT NOT NULL,
    evidence_count INTEGER NOT NULL CHECK (evidence_count >= 1),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'accepted', 'dismissed')
    ),
    proposed_rule_json TEXT NOT NULL CHECK (json_valid(proposed_rule_json)),
    accepted_rule_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id, signature),
    CHECK (length(signature) BETWEEN 1 AND 128),
    CHECK (length(title) BETWEEN 1 AND 200),
    CHECK (length(explanation) BETWEEN 1 AND 1024),
    CHECK (length(proposed_rule_json) BETWEEN 2 AND 16384),
    CHECK (
        (status = 'accepted' AND accepted_rule_id IS NOT NULL)
        OR (status <> 'accepted' AND accepted_rule_id IS NULL)
    ),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX idx_local_rule_suggestions_workspace
    ON local_rule_suggestions(workspace_id, status, updated_at DESC);

CREATE TABLE local_rule_file_matches (
    rule_id TEXT NOT NULL REFERENCES local_user_rules(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    boost REAL NOT NULL DEFAULT 0.15 CHECK (boost >= 0.0 AND boost <= 0.25),
    explanation TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (rule_id, file_id),
    CHECK (length(explanation) BETWEEN 1 AND 512)
) STRICT;

CREATE INDEX idx_local_rule_matches_search
    ON local_rule_file_matches(workspace_id, file_id, boost DESC);

INSERT INTO schema_migrations(version, name)
VALUES (10, '0010_local_rules_learning');

PRAGMA user_version = 10;
COMMIT;
