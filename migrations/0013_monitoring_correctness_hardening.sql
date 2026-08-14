PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- Restore only when there is exactly one unarchived workspace. Multiple
-- workspaces require an explicit user choice and are never ordered implicitly.
INSERT OR IGNORE INTO application_restore_state(singleton) VALUES (1);
UPDATE application_restore_state
SET current_workspace_id = CASE
        WHEN (SELECT COUNT(*) FROM workspaces WHERE archived_at IS NULL) = 1
        THEN (SELECT id FROM workspaces WHERE archived_at IS NULL)
        ELSE NULL
    END,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE singleton = 1
  AND (
      current_workspace_id IS NULL
      OR NOT EXISTS (
          SELECT 1
          FROM workspaces
          WHERE id = application_restore_state.current_workspace_id
            AND archived_at IS NULL
      )
  );

ALTER TABLE application_restore_state
    ADD COLUMN current_root_id TEXT REFERENCES roots(id) ON DELETE SET NULL;
UPDATE application_restore_state
SET current_root_id = CASE
        WHEN (
            SELECT COUNT(*)
            FROM roots
            WHERE workspace_id = application_restore_state.current_workspace_id
              AND state <> 'retired'
        ) = 1
        THEN (
            SELECT id
            FROM roots
            WHERE workspace_id = application_restore_state.current_workspace_id
              AND state <> 'retired'
        )
        ELSE NULL
    END,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE singleton = 1;

-- A workspace may have one current proposal per root, never one proposal whose
-- source silently changes to the most recently scanned root.
DROP INDEX idx_local_org_current_workspace;
CREATE UNIQUE INDEX idx_local_org_current_workspace_root
    ON local_organization_proposals(workspace_id, root_id)
    WHERE status IN ('draft', 'ready_for_review', 'reviewed', 'approved_for_future_apply');

-- Rebuild duplicate groups with root ownership in the key. Historical
-- workspace-wide groups are split only where a root has at least two members;
-- cross-root singleton matches intentionally disappear.
ALTER TABLE scan_duplicate_groups RENAME TO scan_duplicate_groups_before_m101;
ALTER TABLE duplicate_group_members RENAME TO duplicate_group_members_before_m101;
ALTER TABLE duplicate_groups RENAME TO duplicate_groups_before_m101;

CREATE TABLE duplicate_groups (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL,
    canonical_content_id TEXT REFERENCES contents(id) ON DELETE RESTRICT,
    method TEXT NOT NULL CHECK (method IN ('exact_digest', 'perceptual', 'semantic')),
    algorithm TEXT NOT NULL,
    group_key BLOB NOT NULL CHECK (length(group_key) > 0),
    confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    generated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (root_id, workspace_id)
        REFERENCES roots(id, workspace_id) ON DELETE CASCADE,
    UNIQUE (workspace_id, root_id, method, algorithm, group_key)
) STRICT;

CREATE TABLE duplicate_group_members (
    duplicate_group_id TEXT NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    content_id TEXT NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    file_version_id TEXT REFERENCES file_versions(id) ON DELETE CASCADE,
    distance REAL NOT NULL DEFAULT 0.0 CHECK (distance >= 0.0),
    is_canonical INTEGER NOT NULL DEFAULT 0 CHECK (is_canonical IN (0, 1)),
    PRIMARY KEY (duplicate_group_id, content_id, file_version_id)
) STRICT;

CREATE TABLE scan_duplicate_groups (
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    duplicate_group_id TEXT NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (scan_id, duplicate_group_id)
) STRICT;

INSERT INTO duplicate_groups(
    id, workspace_id, root_id, canonical_content_id, method, algorithm,
    group_key, confidence, generated_at
)
SELECT
    old_group.id || ':' || location.root_id,
    old_group.workspace_id,
    location.root_id,
    (
        SELECT member2.content_id
        FROM duplicate_group_members_before_m101 AS member2
        JOIN file_versions AS version2 ON version2.id = member2.file_version_id
        JOIN file_locations AS location2 ON location2.id = version2.location_id
        WHERE member2.duplicate_group_id = old_group.id
          AND location2.root_id = location.root_id
        ORDER BY location2.normalized_relative_path, member2.file_version_id
        LIMIT 1
    ),
    old_group.method,
    old_group.algorithm,
    old_group.group_key,
    old_group.confidence,
    old_group.generated_at
FROM duplicate_groups_before_m101 AS old_group
JOIN duplicate_group_members_before_m101 AS member
  ON member.duplicate_group_id = old_group.id
JOIN file_versions AS version ON version.id = member.file_version_id
JOIN file_locations AS location ON location.id = version.location_id
GROUP BY old_group.id, location.root_id
HAVING COUNT(DISTINCT member.file_version_id) >= 2;

INSERT INTO duplicate_group_members(
    duplicate_group_id, content_id, file_version_id, distance, is_canonical
)
SELECT
    member.duplicate_group_id || ':' || location.root_id,
    member.content_id,
    member.file_version_id,
    member.distance,
    CASE WHEN member.file_version_id = (
        SELECT member2.file_version_id
        FROM duplicate_group_members_before_m101 AS member2
        JOIN file_versions AS version2 ON version2.id = member2.file_version_id
        JOIN file_locations AS location2 ON location2.id = version2.location_id
        WHERE member2.duplicate_group_id = member.duplicate_group_id
          AND location2.root_id = location.root_id
        ORDER BY location2.normalized_relative_path, member2.file_version_id
        LIMIT 1
    ) THEN 1 ELSE 0 END
FROM duplicate_group_members_before_m101 AS member
JOIN file_versions AS version ON version.id = member.file_version_id
JOIN file_locations AS location ON location.id = version.location_id
WHERE EXISTS (
    SELECT 1
    FROM duplicate_groups AS current_group
    WHERE current_group.id = member.duplicate_group_id || ':' || location.root_id
);

INSERT INTO scan_duplicate_groups(scan_id, duplicate_group_id)
SELECT old_scan_group.scan_id, old_scan_group.duplicate_group_id || ':' || scan.root_id
FROM scan_duplicate_groups_before_m101 AS old_scan_group
JOIN scans AS scan ON scan.id = old_scan_group.scan_id
WHERE EXISTS (
    SELECT 1
    FROM duplicate_groups AS current_group
    WHERE current_group.id = old_scan_group.duplicate_group_id || ':' || scan.root_id
);

UPDATE local_organization_proposal_operations
SET duplicate_group_id = duplicate_group_id || ':' || (
        SELECT proposal.root_id
        FROM local_organization_proposals AS proposal
        WHERE proposal.id = local_organization_proposal_operations.proposal_id
    )
WHERE duplicate_group_id IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM local_organization_proposals AS proposal
      JOIN duplicate_groups AS current_group
        ON current_group.id =
           local_organization_proposal_operations.duplicate_group_id || ':' || proposal.root_id
      WHERE proposal.id = local_organization_proposal_operations.proposal_id
  );

DROP TABLE scan_duplicate_groups_before_m101;
DROP TABLE duplicate_group_members_before_m101;
DROP TABLE duplicate_groups_before_m101;

CREATE INDEX idx_duplicate_groups_workspace
    ON duplicate_groups(workspace_id, root_id);
CREATE INDEX idx_duplicate_groups_canonical_content
    ON duplicate_groups(canonical_content_id);
CREATE INDEX idx_duplicate_group_members_content
    ON duplicate_group_members(content_id);
CREATE INDEX idx_duplicate_group_members_file_version
    ON duplicate_group_members(file_version_id);
CREATE INDEX idx_scan_duplicate_groups_scan
    ON scan_duplicate_groups(scan_id);

-- Native paths are stored losslessly. The leading byte identifies legacy
-- UTF-8 (0), Unix bytes (1), or Windows WTF-16LE code units (2).
ALTER TABLE watch_events ADD COLUMN path_before_native BLOB;
ALTER TABLE watch_events ADD COLUMN path_after_native BLOB;
ALTER TABLE watch_events ADD COLUMN event_scope TEXT NOT NULL DEFAULT 'unknown'
    CHECK (event_scope IN ('file', 'directory', 'unknown'));
ALTER TABLE monitoring_jobs ADD COLUMN path_before_native BLOB;
ALTER TABLE monitoring_jobs ADD COLUMN path_after_native BLOB;
ALTER TABLE monitoring_jobs ADD COLUMN coalescing_path_native BLOB;
ALTER TABLE monitoring_jobs ADD COLUMN event_scope TEXT NOT NULL DEFAULT 'unknown'
    CHECK (event_scope IN ('file', 'directory', 'unknown'));
ALTER TABLE file_locations ADD COLUMN relative_path_native BLOB;
ALTER TABLE file_locations ADD COLUMN normalized_relative_path_native BLOB;
ALTER TABLE roots ADD COLUMN absolute_path_native BLOB;
ALTER TABLE roots ADD COLUMN normalized_path_native BLOB;

UPDATE watch_events
SET path_before_native = CASE
        WHEN path_before IS NULL THEN NULL
        ELSE CAST(zeroblob(1) || CAST(path_before AS BLOB) AS BLOB)
    END,
    path_after_native = CASE
        WHEN path_after IS NULL THEN NULL
        ELSE CAST(zeroblob(1) || CAST(path_after AS BLOB) AS BLOB)
    END;
UPDATE monitoring_jobs
SET path_before_native = CASE
        WHEN path_before IS NULL THEN NULL
        ELSE CAST(zeroblob(1) || CAST(path_before AS BLOB) AS BLOB)
    END,
    path_after_native = CASE
        WHEN path_after IS NULL THEN NULL
        ELSE CAST(zeroblob(1) || CAST(path_after AS BLOB) AS BLOB)
    END,
    coalescing_path_native = CASE
        WHEN coalescing_path IS NULL THEN NULL
        ELSE CAST(zeroblob(1) || CAST(coalescing_path AS BLOB) AS BLOB)
    END;
UPDATE roots
SET absolute_path_native =
        CAST(zeroblob(1) || CAST(absolute_path AS BLOB) AS BLOB),
    normalized_path_native =
        CAST(zeroblob(1) || CAST(normalized_path AS BLOB) AS BLOB);
UPDATE file_locations
SET relative_path_native =
        CAST(zeroblob(1) || CAST(relative_path AS BLOB) AS BLOB),
    normalized_relative_path_native =
        CAST(zeroblob(1) || CAST(normalized_relative_path AS BLOB) AS BLOB);

-- Earlier releases stored roots only after a lossy UTF conversion. The
-- original native bytes cannot be reconstructed when the replacement
-- character is present, so fail closed until the user re-registers the root.
UPDATE roots
SET state = 'offline'
WHERE state <> 'retired'
  AND instr(absolute_path, char(65533)) > 0;

UPDATE root_monitoring_settings
SET enabled = 0,
    status = 'offline',
    last_error_code = 'legacy_lossy_path_requires_reregistration',
    last_error_message =
        'This legacy root path cannot be reconstructed losslessly; select the folder again.',
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE root_id IN (
    SELECT id
    FROM roots
    WHERE state = 'offline'
      AND instr(absolute_path, char(65533)) > 0
);

DROP INDEX uq_file_locations_current_path;
CREATE UNIQUE INDEX uq_file_locations_current_native_path
    ON file_locations(root_id, normalized_relative_path_native)
    WHERE valid_to_scan_id IS NULL;

DROP INDEX uq_monitoring_active_path_job;
DROP INDEX uq_monitoring_active_root_job;
CREATE UNIQUE INDEX uq_monitoring_active_native_path_job
    ON monitoring_jobs(root_id, coalescing_path_native)
    WHERE coalescing_path_native IS NOT NULL
      AND status IN ('pending', 'waiting');
CREATE UNIQUE INDEX uq_monitoring_active_root_job
    ON monitoring_jobs(root_id)
    WHERE coalescing_path_native IS NULL
      AND status IN ('pending', 'waiting');
CREATE UNIQUE INDEX uq_roots_workspace_native_path
    ON roots(workspace_id, normalized_path_native)
    WHERE state <> 'retired';

CREATE TRIGGER watch_events_validate_native_paths_insert
BEFORE INSERT ON watch_events
WHEN length(COALESCE(NEW.path_before_native, X'')) > 16385
  OR length(COALESCE(NEW.path_after_native, X'')) > 16385
BEGIN
    SELECT RAISE(ABORT, 'native watch event path exceeds monitoring bounds');
END;

CREATE TRIGGER watch_events_validate_native_paths_update
BEFORE UPDATE ON watch_events
WHEN length(COALESCE(NEW.path_before_native, X'')) > 16385
  OR length(COALESCE(NEW.path_after_native, X'')) > 16385
BEGIN
    SELECT RAISE(ABORT, 'native watch event path exceeds monitoring bounds');
END;

CREATE TRIGGER monitoring_jobs_validate_native_paths_insert
BEFORE INSERT ON monitoring_jobs
WHEN length(COALESCE(NEW.path_before_native, X'')) > 16385
  OR length(COALESCE(NEW.path_after_native, X'')) > 16385
  OR length(COALESCE(NEW.coalescing_path_native, X'')) > 16385
BEGIN
    SELECT RAISE(ABORT, 'native monitoring job path exceeds bounds');
END;

CREATE TRIGGER monitoring_jobs_validate_native_paths_update
BEFORE UPDATE ON monitoring_jobs
WHEN length(COALESCE(NEW.path_before_native, X'')) > 16385
  OR length(COALESCE(NEW.path_after_native, X'')) > 16385
  OR length(COALESCE(NEW.coalescing_path_native, X'')) > 16385
BEGIN
    SELECT RAISE(ABORT, 'native monitoring job path exceeds bounds');
END;

CREATE TRIGGER file_locations_require_native_path_insert
BEFORE INSERT ON file_locations
WHEN NEW.relative_path_native IS NULL
  OR NEW.normalized_relative_path_native IS NULL
  OR length(NEW.relative_path_native) > 16385
  OR length(NEW.normalized_relative_path_native) > 16385
BEGIN
    SELECT RAISE(ABORT, 'file location requires a bounded native path');
END;

CREATE TRIGGER file_locations_require_native_path_update
BEFORE UPDATE OF relative_path_native, normalized_relative_path_native ON file_locations
WHEN NEW.relative_path_native IS NULL
  OR NEW.normalized_relative_path_native IS NULL
  OR length(NEW.relative_path_native) > 16385
  OR length(NEW.normalized_relative_path_native) > 16385
BEGIN
    SELECT RAISE(ABORT, 'file location requires a bounded native path');
END;

CREATE TRIGGER roots_require_native_path_insert
BEFORE INSERT ON roots
WHEN NEW.absolute_path_native IS NULL
  OR NEW.normalized_path_native IS NULL
  OR length(NEW.absolute_path_native) > 16385
  OR length(NEW.normalized_path_native) > 16385
BEGIN
    SELECT RAISE(ABORT, 'root requires a bounded native path');
END;

CREATE TRIGGER roots_require_native_path_update
BEFORE UPDATE OF absolute_path_native, normalized_path_native ON roots
WHEN NEW.absolute_path_native IS NULL
  OR NEW.normalized_path_native IS NULL
  OR length(NEW.absolute_path_native) > 16385
  OR length(NEW.normalized_path_native) > 16385
BEGIN
    SELECT RAISE(ABORT, 'root requires a bounded native path');
END;

-- Processing claims are leases. Expired work is eligible for bounded recovery
-- without waiting for a process restart.
ALTER TABLE monitoring_jobs ADD COLUMN claim_token TEXT;
ALTER TABLE monitoring_jobs ADD COLUMN lease_expires_at_unix_ms INTEGER;
ALTER TABLE monitoring_jobs ADD COLUMN processing_stage TEXT NOT NULL DEFAULT 'queued'
    CHECK (processing_stage IN (
        'queued', 'stability', 'catalog', 'content', 'semantic',
        'relationships', 'proposal', 'search', 'finalizing'
    ));

UPDATE monitoring_jobs
SET status = 'pending',
    retry_after_unix_ms = NULL,
    completed_at = NULL,
    last_error_code = 'migrated_retryable_cancellation',
    last_error_message = NULL,
    claimed_at = NULL,
    processing_stage = 'queued',
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE status = 'cancelled';

DROP INDEX idx_monitoring_jobs_due;
CREATE INDEX idx_monitoring_jobs_due
    ON monitoring_jobs(
        status, debounce_ready_at_unix_ms, retry_after_unix_ms,
        lease_expires_at_unix_ms, created_at
    )
    WHERE status IN ('pending', 'waiting', 'processing');

INSERT OR IGNORE INTO schema_migrations(version, name)
VALUES (13, '0013_monitoring_correctness_hardening');

PRAGMA user_version = 13;
COMMIT;
PRAGMA foreign_keys = ON;
