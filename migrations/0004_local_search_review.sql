BEGIN IMMEDIATE;

-- Rebuildable catalog projection for Milestone 4 lexical search. Extracted text
-- remains canonical in content_extraction_results and is exposed to FTS through
-- a view, avoiding a second application-owned full-text copy.
CREATE TABLE local_search_documents (
    id INTEGER PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    extraction_result_id TEXT REFERENCES content_extraction_results(id) ON DELETE SET NULL,
    filename TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    extension TEXT,
    detected_type TEXT,
    type_group TEXT NOT NULL CHECK (
        type_group IN (
            'pdf', 'documents', 'spreadsheets', 'presentations',
            'images', 'archives', 'other'
        )
    ),
    metadata_text TEXT NOT NULL DEFAULT '',
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    modified_at_native TEXT,
    created_at_native TEXT,
    extraction_status TEXT CHECK (
        extraction_status IS NULL OR extraction_status IN (
            'pending', 'running', 'success', 'partial',
            'unsupported', 'skipped', 'failed'
        )
    ),
    ocr_status TEXT CHECK (
        ocr_status IS NULL OR ocr_status IN ('used', 'not_used', 'unavailable')
    ),
    indexed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (instr(filename, char(0)) = 0),
    CHECK (instr(relative_path, char(0)) = 0)
) STRICT;

CREATE INDEX idx_local_search_workspace
    ON local_search_documents(workspace_id, type_group);
CREATE INDEX idx_local_search_workspace_modified
    ON local_search_documents(workspace_id, modified_at_native);
CREATE INDEX idx_local_search_extraction
    ON local_search_documents(workspace_id, extraction_status);
CREATE INDEX idx_local_search_ocr
    ON local_search_documents(workspace_id, ocr_status);
CREATE INDEX idx_local_search_version
    ON local_search_documents(file_version_id);
CREATE INDEX idx_local_search_result
    ON local_search_documents(extraction_result_id)
    WHERE extraction_result_id IS NOT NULL;

CREATE VIEW local_search_content AS
SELECT
    d.id,
    d.filename,
    d.relative_path,
    CASE
        WHEN d.extraction_status IN ('success', 'partial')
        THEN COALESCE(r.extracted_text, '')
        ELSE ''
    END AS extracted_text,
    d.metadata_text
FROM local_search_documents d
LEFT JOIN content_extraction_results r ON r.id = d.extraction_result_id;

CREATE VIRTUAL TABLE local_search_fts USING fts5(
    filename,
    relative_path,
    extracted_text,
    metadata_text,
    content = 'local_search_content',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3 4'
);

CREATE TRIGGER local_search_fts_insert
AFTER INSERT ON local_search_documents
BEGIN
    INSERT INTO local_search_fts(
        rowid, filename, relative_path, extracted_text, metadata_text
    )
    SELECT id, filename, relative_path, extracted_text, metadata_text
    FROM local_search_content
    WHERE id = NEW.id;
END;

CREATE TRIGGER local_search_fts_update_before
BEFORE UPDATE ON local_search_documents
BEGIN
    INSERT INTO local_search_fts(
        local_search_fts, rowid, filename, relative_path, extracted_text, metadata_text
    ) VALUES (
        'delete',
        OLD.id,
        OLD.filename,
        OLD.relative_path,
        CASE
            WHEN OLD.extraction_status IN ('success', 'partial')
            THEN COALESCE(
                (
                    SELECT extracted_text
                    FROM content_extraction_results
                    WHERE id = OLD.extraction_result_id
                ),
                ''
            )
            ELSE ''
        END,
        OLD.metadata_text
    );
END;

CREATE TRIGGER local_search_fts_update_after
AFTER UPDATE ON local_search_documents
BEGIN
    INSERT INTO local_search_fts(
        rowid, filename, relative_path, extracted_text, metadata_text
    )
    SELECT id, filename, relative_path, extracted_text, metadata_text
    FROM local_search_content
    WHERE id = NEW.id;
END;

CREATE TRIGGER local_search_fts_delete
BEFORE DELETE ON local_search_documents
BEGIN
    INSERT INTO local_search_fts(
        local_search_fts, rowid, filename, relative_path, extracted_text, metadata_text
    ) VALUES (
        'delete',
        OLD.id,
        OLD.filename,
        OLD.relative_path,
        CASE
            WHEN OLD.extraction_status IN ('success', 'partial')
            THEN COALESCE(
                (
                    SELECT extracted_text
                    FROM content_extraction_results
                    WHERE id = OLD.extraction_result_id
                ),
                ''
            )
            ELSE ''
        END,
        OLD.metadata_text
    );
END;

-- Force projection cleanup while referenced extraction text is still present;
-- this keeps FTS deletes correct regardless of foreign-key cascade order.
CREATE TRIGGER local_search_file_delete
BEFORE DELETE ON files
BEGIN
    DELETE FROM local_search_documents WHERE file_id = OLD.id;
END;

CREATE TRIGGER local_search_version_delete
BEFORE DELETE ON file_versions
BEGIN
    DELETE FROM local_search_documents WHERE file_version_id = OLD.id;
END;

CREATE TRIGGER local_search_extraction_delete
BEFORE DELETE ON content_extraction_results
BEGIN
    UPDATE local_search_documents
    SET extraction_result_id = NULL,
        extraction_status = NULL,
        ocr_status = NULL,
        indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE extraction_result_id = OLD.id;
END;

CREATE TABLE file_review_items (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    extraction_result_id TEXT REFERENCES content_extraction_results(id) ON DELETE SET NULL,
    reason TEXT NOT NULL CHECK (
        reason IN (
            'unreadable', 'encrypted', 'unsupported_format', 'corrupt',
            'too_large', 'ocr_failed', 'ocr_provider_unavailable',
            'type_mismatch', 'permission_denied', 'partial_extraction',
            'extraction_failed', 'unknown'
        )
    ),
    source_subsystem TEXT NOT NULL CHECK (
        source_subsystem IN ('scanner', 'extraction')
    ),
    severity TEXT NOT NULL CHECK (
        severity IN ('information', 'warning', 'error')
    ),
    explanation TEXT NOT NULL,
    technical_details TEXT,
    status TEXT NOT NULL DEFAULT 'needs_review' CHECK (
        status IN ('needs_review', 'resolved', 'ignored')
    ),
    retry_available INTEGER NOT NULL DEFAULT 0 CHECK (retry_available IN (0, 1)),
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    last_retried_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    resolved_at TEXT,
    UNIQUE (file_version_id, reason),
    CHECK (resolved_at IS NULL OR status = 'resolved'),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX idx_file_review_workspace_status
    ON file_review_items(workspace_id, status, updated_at DESC);
CREATE INDEX idx_file_review_workspace_reason
    ON file_review_items(workspace_id, reason, status);
CREATE INDEX idx_file_review_file
    ON file_review_items(file_id, updated_at DESC);
CREATE INDEX idx_file_review_result
    ON file_review_items(extraction_result_id)
    WHERE extraction_result_id IS NOT NULL;

-- Bring existing catalogs forward. New scans and extractions use the same
-- transactional upsert path in persistence.
WITH ranked_versions AS (
    SELECT
        fv.*,
        ROW_NUMBER() OVER (
            PARTITION BY fv.file_id
            ORDER BY fv.version_number DESC, fv.observed_at DESC
        ) AS version_rank
    FROM file_versions fv
),
current_versions AS (
    SELECT * FROM ranked_versions WHERE version_rank = 1
),
latest_extractions AS (
    SELECT *
    FROM (
        SELECT
            cer.*,
            ROW_NUMBER() OVER (
                PARTITION BY cer.file_version_id
                ORDER BY COALESCE(cer.extracted_at, cer.created_at) DESC, cer.id DESC
            ) AS extraction_rank
        FROM content_extraction_results cer
        WHERE cer.status NOT IN ('pending', 'running')
    )
    WHERE extraction_rank = 1
)
INSERT INTO local_search_documents(
    workspace_id, file_id, file_version_id, extraction_result_id,
    filename, relative_path, extension, detected_type, type_group,
    metadata_text, byte_size, modified_at_native, created_at_native,
    extraction_status, ocr_status
)
SELECT
    f.workspace_id,
    f.id,
    fv.id,
    cer.id,
    fl.basename,
    fl.relative_path,
    sfs.extension,
    COALESCE(cer.detected_content_type, c.media_type),
    CASE
        WHEN lower(COALESCE(sfs.extension, '')) = 'pdf' THEN 'pdf'
        WHEN lower(COALESCE(sfs.extension, '')) IN (
            'txt', 'md', 'log', 'json', 'xml', 'doc', 'docx', 'rtf', 'odt'
        ) THEN 'documents'
        WHEN lower(COALESCE(sfs.extension, '')) IN ('csv', 'xls', 'xlsx', 'ods')
            THEN 'spreadsheets'
        WHEN lower(COALESCE(sfs.extension, '')) IN ('ppt', 'pptx', 'odp')
            THEN 'presentations'
        WHEN lower(COALESCE(sfs.extension, '')) IN (
            'png', 'jpg', 'jpeg', 'webp', 'tif', 'tiff', 'bmp', 'gif', 'heic'
        ) THEN 'images'
        WHEN lower(COALESCE(sfs.extension, '')) IN (
            'zip', '7z', 'rar', 'tar', 'gz', 'bz2', 'xz'
        ) THEN 'archives'
        ELSE 'other'
    END,
    trim(
        COALESCE(sfs.extension, '') || ' ' ||
        COALESCE(cer.detected_content_type, c.media_type, '')
    ),
    fv.byte_size,
    fv.modified_at,
    fv.created_at_native,
    cer.status,
    CASE
        WHEN cer.ocr_used = 1 THEN 'used'
        WHEN cer.requires_ocr = 1 AND cer.error_category = 'ocr_unavailable'
            THEN 'unavailable'
        WHEN cer.id IS NOT NULL THEN 'not_used'
        ELSE NULL
    END
FROM files f
JOIN current_versions fv ON fv.file_id = f.id
JOIN file_locations fl ON fl.id = fv.location_id
LEFT JOIN scan_file_statuses sfs
    ON sfs.scan_id = fv.observed_by_scan_id
   AND sfs.file_version_id = fv.id
LEFT JOIN contents c ON c.id = fv.content_id
LEFT JOIN latest_extractions cer ON cer.file_version_id = fv.id
WHERE f.kind = 'regular'
  AND f.lifecycle_state = 'present'
  AND fl.valid_to_scan_id IS NULL;

WITH latest_extractions AS (
    SELECT *
    FROM (
        SELECT
            cer.*,
            ROW_NUMBER() OVER (
                PARTITION BY cer.file_version_id
                ORDER BY COALESCE(cer.extracted_at, cer.created_at) DESC, cer.id DESC
            ) AS extraction_rank
        FROM content_extraction_results cer
        WHERE cer.status NOT IN ('pending', 'running')
    )
    WHERE extraction_rank = 1
)
INSERT OR IGNORE INTO file_review_items(
    id, workspace_id, file_id, file_version_id, extraction_result_id,
    reason, source_subsystem, severity, explanation, technical_details,
    retry_available
)
SELECT
    lower(hex(randomblob(16))),
    f.workspace_id,
    cer.file_id,
    cer.file_version_id,
    cer.id,
    CASE
        WHEN cer.type_mismatch = 1 OR cer.error_category = 'type_mismatch'
            THEN 'type_mismatch'
        WHEN cer.error_category = 'unreadable' THEN 'unreadable'
        WHEN cer.error_category = 'encrypted_document' THEN 'encrypted'
        WHEN cer.error_category = 'unsupported' THEN 'unsupported_format'
        WHEN cer.error_category = 'corrupt' THEN 'corrupt'
        WHEN cer.error_category IN (
            'too_large', 'too_many_pages', 'too_many_cells', 'too_many_entries',
            'potential_archive_bomb'
        ) THEN 'too_large'
        WHEN cer.error_category = 'ocr_failed' THEN 'ocr_failed'
        WHEN cer.error_category = 'ocr_unavailable' THEN 'ocr_provider_unavailable'
        WHEN cer.error_category = 'permission_denied' THEN 'permission_denied'
        WHEN cer.status = 'partial' THEN 'partial_extraction'
        WHEN cer.status = 'failed' THEN 'extraction_failed'
        ELSE 'unknown'
    END,
    'extraction',
    CASE WHEN cer.status = 'failed' THEN 'error' ELSE 'warning' END,
    CASE
        WHEN cer.type_mismatch = 1 OR cer.error_category = 'type_mismatch'
            THEN 'Le type réel de ce fichier ne correspond pas à son extension.'
        WHEN cer.error_category = 'unreadable'
            THEN 'Ce fichier ne peut pas être lu de façon sûre.'
        WHEN cer.error_category = 'encrypted_document'
            THEN 'Ce document est chiffré et ne peut pas être analysé sans mot de passe.'
        WHEN cer.error_category = 'unsupported'
            THEN 'Ce format de fichier n’est pas encore pris en charge.'
        WHEN cer.error_category = 'corrupt'
            THEN 'Ce fichier semble endommagé ou incomplet.'
        WHEN cer.error_category IN (
            'too_large', 'too_many_pages', 'too_many_cells', 'too_many_entries',
            'potential_archive_bomb'
        ) THEN 'Ce fichier dépasse une limite de sécurité de l’analyse locale.'
        WHEN cer.error_category = 'ocr_failed'
            THEN 'La reconnaissance locale du texte n’a pas abouti.'
        WHEN cer.error_category = 'ocr_unavailable'
            THEN 'Ce document semble contenir du texte numérisé, mais la reconnaissance locale est indisponible.'
        WHEN cer.error_category = 'permission_denied'
            THEN 'L’application n’a pas l’autorisation de lire ce fichier.'
        WHEN cer.status = 'partial'
            THEN 'Une partie seulement du contenu a pu être extraite.'
        ELSE 'L’extraction locale n’a pas pu traiter complètement ce fichier.'
    END,
    cer.error_message,
    CASE
        WHEN cer.error_category IN (
            'unsupported', 'encrypted_document', 'too_large', 'too_many_pages',
            'too_many_cells', 'too_many_entries', 'potential_archive_bomb'
        ) THEN 0
        ELSE 1
    END
FROM latest_extractions cer
JOIN files f ON f.id = cer.file_id
WHERE cer.status IN ('partial', 'failed', 'unsupported')
   OR cer.type_mismatch = 1
   OR cer.error_category IN (
       'unreadable', 'encrypted_document', 'unsupported', 'corrupt',
       'too_large', 'too_many_pages', 'too_many_cells', 'too_many_entries',
       'ocr_failed', 'ocr_unavailable', 'type_mismatch', 'permission_denied',
       'parser_failure'
   );

INSERT OR IGNORE INTO file_review_items(
    id, workspace_id, file_id, file_version_id, reason,
    source_subsystem, severity, explanation, technical_details,
    retry_available
)
SELECT
    lower(hex(randomblob(16))),
    f.workspace_id,
    fv.file_id,
    sfs.file_version_id,
    CASE
        WHEN lower(COALESCE(sfs.error_code, '')) LIKE '%permission%'
            THEN 'permission_denied'
        ELSE 'unreadable'
    END,
    'scanner',
    'error',
    CASE
        WHEN lower(COALESCE(sfs.error_code, '')) LIKE '%permission%'
            THEN 'L’application n’a pas l’autorisation de lire ce fichier.'
        ELSE 'Ce fichier ne peut pas être lu de façon sûre.'
    END,
    sfs.error_code,
    1
FROM scan_file_statuses sfs
JOIN file_versions fv ON fv.id = sfs.file_version_id
JOIN files f ON f.id = fv.file_id
WHERE sfs.readability_status = 'unreadable';

INSERT INTO schema_migrations(version, name)
VALUES (4, '0004_local_search_review');

PRAGMA user_version = 4;

COMMIT;
