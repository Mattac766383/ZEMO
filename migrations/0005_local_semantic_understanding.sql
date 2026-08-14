BEGIN IMMEDIATE;

CREATE TABLE semantic_analysis_batches (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'completed', 'cancelled', 'failed')
    ),
    files_queued INTEGER NOT NULL DEFAULT 0 CHECK (files_queued >= 0),
    files_completed INTEGER NOT NULL DEFAULT 0 CHECK (files_completed >= 0),
    high_confidence_count INTEGER NOT NULL DEFAULT 0 CHECK (high_confidence_count >= 0),
    needs_review_count INTEGER NOT NULL DEFAULT 0 CHECK (needs_review_count >= 0),
    unknown_count INTEGER NOT NULL DEFAULT 0 CHECK (unknown_count >= 0),
    partial_count INTEGER NOT NULL DEFAULT 0 CHECK (partial_count >= 0),
    failed_count INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    started_at TEXT,
    completed_at TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (files_completed <= files_queued),
    CHECK (completed_at IS NULL OR started_at IS NOT NULL),
    CHECK (completed_at IS NULL OR completed_at >= started_at)
) STRICT;

CREATE INDEX idx_semantic_batches_scan
    ON semantic_analysis_batches(scan_id, created_at DESC);
CREATE INDEX idx_semantic_batches_workspace
    ON semantic_analysis_batches(workspace_id, created_at DESC);

CREATE TABLE semantic_analyses (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES semantic_analysis_batches(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    extraction_result_id TEXT REFERENCES content_extraction_results(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'pending', 'running', 'success', 'partial',
            'unknown', 'failed', 'cancelled'
        )
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_version TEXT NOT NULL,
    processing_location TEXT NOT NULL DEFAULT 'local' CHECK (
        processing_location = 'local'
    ),
    input_digest BLOB NOT NULL CHECK (length(input_digest) = 32),
    input_character_count INTEGER NOT NULL CHECK (input_character_count >= 0),
    analyzed_character_count INTEGER NOT NULL CHECK (
        analyzed_character_count >= 0
        AND analyzed_character_count <= input_character_count
    ),
    input_quality REAL NOT NULL CHECK (input_quality >= 0.0 AND input_quality <= 1.0),
    input_quality_status TEXT NOT NULL CHECK (
        input_quality_status IN ('good', 'degraded', 'poor', 'unusable')
    ),
    input_quality_reasons_json TEXT NOT NULL DEFAULT '[]' CHECK (
        json_valid(input_quality_reasons_json)
        AND json_type(input_quality_reasons_json) = 'array'
    ),
    language TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0 CHECK (duration_ms >= 0),
    is_current INTEGER NOT NULL DEFAULT 0 CHECK (is_current IN (0, 1)),
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at TEXT,
    superseded_at TEXT,
    error_message TEXT,
    CHECK (length(analyzer_id) BETWEEN 1 AND 128),
    CHECK (length(analyzer_version) BETWEEN 1 AND 64),
    CHECK (length(provider_id) BETWEEN 1 AND 128),
    CHECK (length(provider_version) BETWEEN 1 AND 64),
    CHECK (completed_at IS NULL OR completed_at >= started_at),
    CHECK (superseded_at IS NULL OR is_current = 0)
) STRICT;

CREATE UNIQUE INDEX idx_semantic_current_file_version
    ON semantic_analyses(file_version_id)
    WHERE is_current = 1;
CREATE INDEX idx_semantic_analysis_file
    ON semantic_analyses(file_id, completed_at DESC);
CREATE INDEX idx_semantic_analysis_batch
    ON semantic_analyses(batch_id, status);
CREATE INDEX idx_semantic_analysis_extraction
    ON semantic_analyses(extraction_result_id)
    WHERE extraction_result_id IS NOT NULL;
CREATE INDEX idx_semantic_analysis_version
    ON semantic_analyses(analyzer_id, analyzer_version, schema_version);

CREATE TABLE semantic_fields (
    id TEXT PRIMARY KEY,
    analysis_id TEXT NOT NULL REFERENCES semantic_analyses(id) ON DELETE CASCADE,
    field_key TEXT NOT NULL CHECK (
        field_key IN (
            'document_type', 'context', 'supplier_candidate',
            'customer_candidate', 'issuer', 'invoice_number',
            'quote_number', 'document_number', 'issue_date', 'due_date',
            'expiration_date', 'document_date', 'subtotal', 'tax', 'total',
            'amount', 'currency', 'purchase_order_reference',
            'project_reference_candidate', 'contract_parties',
            'contract_title', 'contract_type', 'company_identifier'
        )
    ),
    candidate_rank INTEGER NOT NULL DEFAULT 0 CHECK (candidate_rank >= 0),
    is_primary INTEGER NOT NULL CHECK (is_primary IN (0, 1)),
    value_kind TEXT CHECK (
        value_kind IS NULL OR value_kind IN (
            'text', 'date', 'money', 'document_type', 'context', 'text_list'
        )
    ),
    display_value TEXT,
    normalized_value_json TEXT NOT NULL DEFAULT 'null' CHECK (
        json_valid(normalized_value_json)
    ),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    field_status TEXT NOT NULL CHECK (
        field_status IN (
            'confirmed', 'inferred', 'ambiguous', 'unknown', 'conflicting'
        )
    ),
    source_method TEXT NOT NULL CHECK (
        source_method IN (
            'deterministic_rule', 'regex_parser', 'structured_parser',
            'filename_hint', 'metadata', 'local_semantic_provider'
        )
    ),
    analyzer_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (analysis_id, field_key, candidate_rank),
    CHECK (length(display_value) <= 512),
    CHECK (is_primary = 0 OR candidate_rank = 0)
) STRICT;

CREATE UNIQUE INDEX idx_semantic_field_primary
    ON semantic_fields(analysis_id, field_key)
    WHERE is_primary = 1;
CREATE INDEX idx_semantic_field_lookup
    ON semantic_fields(analysis_id, field_key, candidate_rank);

CREATE TABLE semantic_entities (
    id TEXT PRIMARY KEY,
    analysis_id TEXT NOT NULL REFERENCES semantic_analyses(id) ON DELETE CASCADE,
    candidate_key TEXT NOT NULL,
    entity_type TEXT NOT NULL CHECK (
        entity_type IN (
            'person', 'organization', 'customer_candidate',
            'supplier_candidate', 'project_candidate', 'address', 'email',
            'phone', 'date', 'amount', 'currency', 'document_number',
            'invoice_number', 'siret_or_company_id', 'other_identifier'
        )
    ),
    original_value TEXT NOT NULL,
    normalized_value TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    field_status TEXT NOT NULL CHECK (
        field_status IN (
            'confirmed', 'inferred', 'ambiguous', 'unknown', 'conflicting'
        )
    ),
    source_method TEXT NOT NULL CHECK (
        source_method IN (
            'deterministic_rule', 'regex_parser', 'structured_parser',
            'filename_hint', 'metadata', 'local_semantic_provider'
        )
    ),
    analyzer_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (analysis_id, candidate_key),
    CHECK (length(candidate_key) BETWEEN 1 AND 384),
    CHECK (length(original_value) BETWEEN 1 AND 512),
    CHECK (length(normalized_value) BETWEEN 1 AND 512)
) STRICT;

CREATE INDEX idx_semantic_entity_analysis_type
    ON semantic_entities(analysis_id, entity_type);

CREATE TABLE semantic_evidence (
    id TEXT PRIMARY KEY,
    analysis_id TEXT NOT NULL REFERENCES semantic_analyses(id) ON DELETE CASCADE,
    field_id TEXT REFERENCES semantic_fields(id) ON DELETE CASCADE,
    entity_id TEXT REFERENCES semantic_entities(id) ON DELETE CASCADE,
    evidence_type TEXT NOT NULL CHECK (
        evidence_type IN (
            'text_span', 'filename', 'metadata',
            'structural_indicator', 'parser_match', 'ocr_text'
        )
    ),
    exact_text TEXT NOT NULL,
    start_offset INTEGER CHECK (start_offset IS NULL OR start_offset >= 0),
    end_offset INTEGER CHECK (end_offset IS NULL OR end_offset >= 0),
    page_number INTEGER CHECK (page_number IS NULL OR page_number > 0),
    sheet_name TEXT,
    slide_number INTEGER CHECK (slide_number IS NULL OR slide_number > 0),
    source_label TEXT NOT NULL,
    explanation TEXT NOT NULL,
    extraction_method TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK ((field_id IS NOT NULL) != (entity_id IS NOT NULL)),
    CHECK (
        (start_offset IS NULL AND end_offset IS NULL)
        OR (start_offset IS NOT NULL AND end_offset IS NOT NULL AND end_offset >= start_offset)
    ),
    CHECK (length(exact_text) <= 2000),
    CHECK (length(source_label) BETWEEN 1 AND 256),
    CHECK (length(explanation) BETWEEN 1 AND 512),
    CHECK (length(extraction_method) BETWEEN 1 AND 128)
) STRICT;

CREATE INDEX idx_semantic_evidence_field
    ON semantic_evidence(field_id)
    WHERE field_id IS NOT NULL;
CREATE INDEX idx_semantic_evidence_entity
    ON semantic_evidence(entity_id)
    WHERE entity_id IS NOT NULL;

CREATE TABLE semantic_user_corrections (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    field_key TEXT NOT NULL CHECK (
        field_key IN (
            'document_type', 'context', 'supplier_candidate',
            'customer_candidate', 'issuer', 'invoice_number',
            'quote_number', 'document_number', 'issue_date', 'due_date',
            'expiration_date', 'document_date', 'subtotal', 'tax', 'total',
            'amount', 'currency', 'purchase_order_reference',
            'project_reference_candidate', 'contract_parties',
            'contract_title', 'contract_type', 'company_identifier'
        )
    ),
    correction_state TEXT NOT NULL CHECK (
        correction_state IN ('user_confirmed', 'user_corrected')
    ),
    source_analysis_id TEXT REFERENCES semantic_analyses(id) ON DELETE SET NULL,
    source_field_id TEXT REFERENCES semantic_fields(id) ON DELETE SET NULL,
    value_kind TEXT NOT NULL CHECK (
        value_kind IN (
            'text', 'date', 'money', 'document_type', 'context', 'text_list'
        )
    ),
    display_value TEXT NOT NULL,
    normalized_value_json TEXT NOT NULL CHECK (json_valid(normalized_value_json)),
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    superseded_at TEXT,
    CHECK (length(display_value) BETWEEN 1 AND 512),
    CHECK (updated_at >= created_at),
    CHECK (superseded_at IS NULL OR active = 0)
) STRICT;

CREATE UNIQUE INDEX idx_semantic_active_correction
    ON semantic_user_corrections(file_id, field_key)
    WHERE active = 1;
CREATE INDEX idx_semantic_correction_workspace
    ON semantic_user_corrections(workspace_id, updated_at DESC);

ALTER TABLE local_search_documents
    ADD COLUMN semantic_document_type TEXT CHECK (
        semantic_document_type IS NULL OR semantic_document_type IN (
            'invoice', 'quote', 'contract', 'purchase_order', 'delivery_note',
            'bank_statement', 'tax_document', 'payslip',
            'employment_contract', 'insurance_document', 'legal_document',
            'administrative_document', 'receipt', 'report', 'letter', 'cv',
            'photo', 'video', 'spreadsheet', 'presentation', 'archive',
            'other', 'unknown'
        )
    );
ALTER TABLE local_search_documents
    ADD COLUMN semantic_context TEXT CHECK (
        semantic_context IS NULL OR semantic_context IN (
            'personal', 'business', 'mixed', 'unknown'
        )
    );
ALTER TABLE local_search_documents
    ADD COLUMN semantic_status TEXT CHECK (
        semantic_status IS NULL OR semantic_status IN (
            'pending', 'running', 'success', 'partial',
            'unknown', 'failed', 'cancelled'
        )
    );
ALTER TABLE local_search_documents
    ADD COLUMN semantic_confidence REAL CHECK (
        semantic_confidence IS NULL
        OR (semantic_confidence >= 0.0 AND semantic_confidence <= 1.0)
    );

CREATE INDEX idx_local_search_semantic_type
    ON local_search_documents(workspace_id, semantic_document_type);
CREATE INDEX idx_local_search_semantic_context
    ON local_search_documents(workspace_id, semantic_context);

-- Milestone 4 used a strict review-reason/source check. Recreate the table to
-- add a small, bounded set of actionable semantic reasons.
DROP INDEX idx_file_review_workspace_status;
DROP INDEX idx_file_review_workspace_reason;
DROP INDEX idx_file_review_file;
DROP INDEX idx_file_review_result;
ALTER TABLE file_review_items RENAME TO file_review_items_m4;

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
            'extraction_failed', 'unknown', 'semantic_ambiguity',
            'conflicting_fields', 'low_confidence_document_type',
            'low_confidence_context', 'missing_critical_fields'
        )
    ),
    source_subsystem TEXT NOT NULL CHECK (
        source_subsystem IN ('scanner', 'extraction', 'semantic')
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

INSERT INTO file_review_items(
    id, workspace_id, file_id, file_version_id, extraction_result_id,
    reason, source_subsystem, severity, explanation, technical_details,
    status, retry_available, retry_count, last_retried_at,
    created_at, updated_at, resolved_at
)
SELECT
    id, workspace_id, file_id, file_version_id, extraction_result_id,
    reason, source_subsystem, severity, explanation, technical_details,
    status, retry_available, retry_count, last_retried_at,
    created_at, updated_at, resolved_at
FROM file_review_items_m4;

DROP TABLE file_review_items_m4;

CREATE INDEX idx_file_review_workspace_status
    ON file_review_items(workspace_id, status, updated_at DESC);
CREATE INDEX idx_file_review_workspace_reason
    ON file_review_items(workspace_id, reason, status);
CREATE INDEX idx_file_review_file
    ON file_review_items(file_id, updated_at DESC);
CREATE INDEX idx_file_review_result
    ON file_review_items(extraction_result_id)
    WHERE extraction_result_id IS NOT NULL;

INSERT INTO schema_migrations(version, name)
VALUES (5, '0005_local_semantic_understanding');

PRAGMA user_version = 5;

COMMIT;
