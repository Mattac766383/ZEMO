-- M14: incremental organization proposal metadata.
-- Correctness remains authoritative; rebuild_mode records full vs incremental.

BEGIN IMMEDIATE;

INSERT INTO schema_migrations(name) VALUES ('0017_incremental_organization_proposals');

ALTER TABLE local_organization_proposal_revisions
    ADD COLUMN rebuild_mode TEXT NOT NULL DEFAULT 'full';

ALTER TABLE local_organization_proposal_revisions
    ADD COLUMN rebuild_reason TEXT;

ALTER TABLE local_organization_proposal_revisions
    ADD COLUMN dirty_file_count INTEGER NOT NULL DEFAULT 0;

-- Lightweight dependency projection for incremental invalidation.
CREATE TABLE local_organization_proposal_dependencies (
    revision_id TEXT NOT NULL REFERENCES local_organization_proposal_revisions(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    dependency_kind TEXT NOT NULL CHECK (
        dependency_kind IN (
            'identity_customer', 'identity_supplier', 'identity_project',
            'destination_prefix', 'collision_key'
        )
    ),
    dependency_key TEXT NOT NULL CHECK (length(dependency_key) BETWEEN 1 AND 1024),
    PRIMARY KEY (revision_id, file_id, dependency_kind, dependency_key)
) STRICT;

CREATE INDEX idx_local_org_proposal_deps_key
    ON local_organization_proposal_dependencies(revision_id, dependency_kind, dependency_key);
CREATE INDEX idx_local_org_proposal_deps_file
    ON local_organization_proposal_dependencies(revision_id, file_id);

PRAGMA user_version = 17;

COMMIT;
