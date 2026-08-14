BEGIN IMMEDIATE;

ALTER TABLE file_versions ADD COLUMN accessed_at_native TEXT;

CREATE TABLE scan_metrics (
    scan_id TEXT PRIMARY KEY REFERENCES scans(id) ON DELETE CASCADE,
    files_indexed INTEGER NOT NULL DEFAULT 0 CHECK (files_indexed >= 0),
    directories_discovered INTEGER NOT NULL DEFAULT 0 CHECK (directories_discovered >= 0),
    bytes_discovered INTEGER NOT NULL DEFAULT 0 CHECK (bytes_discovered >= 0),
    files_hashed INTEGER NOT NULL DEFAULT 0 CHECK (files_hashed >= 0),
    error_count INTEGER NOT NULL DEFAULT 0 CHECK (error_count >= 0),
    skipped_count INTEGER NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    duplicate_group_count INTEGER NOT NULL DEFAULT 0 CHECK (duplicate_group_count >= 0),
    truncated INTEGER NOT NULL DEFAULT 0 CHECK (truncated IN (0, 1))
) STRICT;

CREATE TABLE scan_file_statuses (
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    extension TEXT,
    readability_status TEXT NOT NULL CHECK (
        readability_status IN ('readable', 'unreadable', 'not_checked')
    ),
    scan_status TEXT NOT NULL CHECK (
        scan_status IN ('indexed', 'indexed_with_errors')
    ),
    hashing_status TEXT NOT NULL CHECK (
        hashing_status IN ('not_candidate', 'hashed', 'failed', 'cancelled')
    ),
    error_code TEXT,
    PRIMARY KEY (scan_id, file_version_id)
) STRICT;

CREATE INDEX idx_scan_file_statuses_scan
    ON scan_file_statuses(scan_id);

CREATE TABLE scan_duplicate_groups (
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    duplicate_group_id TEXT NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (scan_id, duplicate_group_id)
) STRICT;

CREATE INDEX idx_scan_duplicate_groups_scan
    ON scan_duplicate_groups(scan_id);

INSERT INTO schema_migrations(version, name)
VALUES (2, '0002_safe_scanner');

PRAGMA user_version = 2;

COMMIT;
