BEGIN IMMEDIATE;

CREATE TABLE content_extraction_batches (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'completed', 'cancelled', 'failed')
    ),
    files_queued INTEGER NOT NULL DEFAULT 0 CHECK (files_queued >= 0),
    files_completed INTEGER NOT NULL DEFAULT 0 CHECK (files_completed >= 0),
    successful_count INTEGER NOT NULL DEFAULT 0 CHECK (successful_count >= 0),
    partial_count INTEGER NOT NULL DEFAULT 0 CHECK (partial_count >= 0),
    unsupported_count INTEGER NOT NULL DEFAULT 0 CHECK (unsupported_count >= 0),
    skipped_count INTEGER NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    failed_count INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    ocr_processed_count INTEGER NOT NULL DEFAULT 0 CHECK (ocr_processed_count >= 0),
    started_at TEXT,
    completed_at TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (completed_at IS NULL OR started_at IS NOT NULL),
    CHECK (completed_at IS NULL OR completed_at >= started_at),
    CHECK (files_completed <= files_queued)
) STRICT;

CREATE INDEX idx_content_extraction_batches_scan
    ON content_extraction_batches(scan_id, created_at DESC);
CREATE INDEX idx_content_extraction_batches_workspace
    ON content_extraction_batches(workspace_id, created_at DESC);

CREATE TABLE content_extraction_results (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES content_extraction_batches(id) ON DELETE CASCADE,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'success', 'partial', 'unsupported', 'skipped', 'failed')
    ),
    extractor_type TEXT,
    extractor_version TEXT,
    extension TEXT,
    detected_content_type TEXT,
    type_mismatch INTEGER NOT NULL DEFAULT 0 CHECK (type_mismatch IN (0, 1)),
    extracted_text TEXT NOT NULL DEFAULT '',
    character_count INTEGER NOT NULL DEFAULT 0 CHECK (character_count >= 0),
    page_count INTEGER CHECK (page_count IS NULL OR page_count >= 0),
    sheet_count INTEGER CHECK (sheet_count IS NULL OR sheet_count >= 0),
    slide_count INTEGER CHECK (slide_count IS NULL OR slide_count >= 0),
    image_width INTEGER CHECK (image_width IS NULL OR image_width > 0),
    image_height INTEGER CHECK (image_height IS NULL OR image_height > 0),
    requires_ocr INTEGER NOT NULL DEFAULT 0 CHECK (requires_ocr IN (0, 1)),
    ocr_used INTEGER NOT NULL DEFAULT 0 CHECK (ocr_used IN (0, 1)),
    ocr_confidence REAL CHECK (
        ocr_confidence IS NULL OR (ocr_confidence >= 0.0 AND ocr_confidence <= 1.0)
    ),
    language_hint TEXT,
    extraction_duration_ms INTEGER NOT NULL DEFAULT 0 CHECK (extraction_duration_ms >= 0),
    truncated INTEGER NOT NULL DEFAULT 0 CHECK (truncated IN (0, 1)),
    structured_metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(structured_metadata_json)),
    error_category TEXT,
    error_message TEXT,
    started_at TEXT,
    extracted_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (batch_id, file_version_id),
    CHECK (character_count = length(extracted_text)),
    CHECK (extracted_at IS NULL OR started_at IS NOT NULL),
    CHECK (extracted_at IS NULL OR extracted_at >= started_at)
) STRICT;

CREATE INDEX idx_content_extraction_results_batch
    ON content_extraction_results(batch_id, status);
CREATE INDEX idx_content_extraction_results_scan
    ON content_extraction_results(scan_id);
CREATE INDEX idx_content_extraction_results_file
    ON content_extraction_results(file_id, extracted_at DESC);
CREATE INDEX idx_content_extraction_results_file_version
    ON content_extraction_results(file_version_id);
CREATE INDEX idx_content_extraction_results_review
    ON content_extraction_results(error_category)
    WHERE error_category IS NOT NULL;

INSERT INTO schema_migrations(version, name)
VALUES (3, '0003_safe_content_extraction');

PRAGMA user_version = 3;

COMMIT;
