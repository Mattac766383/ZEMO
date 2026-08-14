BEGIN IMMEDIATE;

-- Local embedding providers are explicit catalog records. A provider can be
-- unavailable or development-only; no row authorizes network access or model
-- download. Packaged production models may be registered by a later release.
CREATE TABLE local_embedding_models (
    provider_id TEXT NOT NULL,
    version TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions BETWEEN 0 AND 4096),
    availability TEXT NOT NULL CHECK (
        availability IN ('available_development', 'available_production', 'unavailable')
    ),
    local_only INTEGER NOT NULL DEFAULT 1 CHECK (local_only = 1),
    production_ready INTEGER NOT NULL DEFAULT 0 CHECK (production_ready IN (0, 1)),
    requires_download INTEGER NOT NULL DEFAULT 0 CHECK (requires_download = 0),
    model_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (model_size_bytes >= 0),
    max_model_size_bytes INTEGER NOT NULL CHECK (
        max_model_size_bytes > 0
        AND model_size_bytes <= max_model_size_bytes
        AND max_model_size_bytes <= 1073741824
    ),
    registered_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (provider_id, version),
    CHECK (length(provider_id) BETWEEN 1 AND 128),
    CHECK (length(version) BETWEEN 1 AND 64),
    CHECK (production_ready = 0 OR availability = 'available_production'),
    CHECK (updated_at >= registered_at)
) STRICT, WITHOUT ROWID;

-- Compact signed-int8 unit vectors. Canonical file, extraction, semantic and
-- identity records remain in their existing SQLite tables. This table is a
-- derived, incrementally replaceable local index with bounded source links.
CREATE TABLE local_search_embeddings (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    semantic_analysis_id TEXT REFERENCES semantic_analyses(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL,
    embedding_version TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions BETWEEN 1 AND 4096),
    source_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('semantic_summary', 'text_chunk')
    ),
    source_start_offset INTEGER CHECK (
        source_start_offset IS NULL OR source_start_offset >= 0
    ),
    source_end_offset INTEGER CHECK (
        source_end_offset IS NULL OR source_end_offset >= source_start_offset
    ),
    vector BLOB NOT NULL CHECK (length(vector) = dimensions),
    input_digest BLOB NOT NULL CHECK (length(input_digest) = 32),
    indexed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (provider_id, embedding_version)
        REFERENCES local_embedding_models(provider_id, version) ON DELETE RESTRICT,
    UNIQUE (file_version_id, provider_id, embedding_version, source_id),
    CHECK (length(source_id) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX idx_local_embeddings_workspace_version
    ON local_search_embeddings(
        workspace_id, provider_id, embedding_version, dimensions, file_id
    );
CREATE INDEX idx_local_embeddings_file
    ON local_search_embeddings(file_id, indexed_at DESC);
CREATE INDEX idx_local_embeddings_analysis
    ON local_search_embeddings(semantic_analysis_id)
    WHERE semantic_analysis_id IS NOT NULL;

CREATE TABLE local_search_embedding_state (
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL,
    embedding_version TEXT NOT NULL,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    semantic_analysis_id TEXT REFERENCES semantic_analyses(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (
        status IN ('indexed', 'unavailable', 'failed', 'stale')
    ),
    source_count INTEGER NOT NULL DEFAULT 0 CHECK (source_count >= 0),
    error_code TEXT,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (file_id, provider_id, embedding_version),
    FOREIGN KEY (provider_id, embedding_version)
        REFERENCES local_embedding_models(provider_id, version) ON DELETE RESTRICT,
    CHECK (error_code IS NULL OR length(error_code) <= 128)
) STRICT, WITHOUT ROWID;

CREATE INDEX idx_local_embedding_state_workspace_lookup
    ON local_search_embedding_state(provider_id, embedding_version, status, file_id);

-- Indexed deterministic facts used by structured candidate retrieval. Values
-- remain canonical in semantic/identity tables and corrections stay separate.
CREATE INDEX idx_semantic_fields_hybrid_text
    ON semantic_fields(field_key, display_value, analysis_id)
    WHERE is_primary = 1 AND display_value IS NOT NULL;
CREATE INDEX idx_semantic_fields_hybrid_amount
    ON semantic_fields(
        field_key,
        CAST(json_extract(normalized_value_json, '$.amount_minor') AS INTEGER),
        analysis_id
    )
    WHERE is_primary = 1 AND field_key IN ('subtotal', 'tax', 'total', 'amount');
CREATE INDEX idx_semantic_fields_hybrid_date
    ON semantic_fields(
        field_key,
        json_extract(normalized_value_json, '$.iso_date'),
        analysis_id
    )
    WHERE is_primary = 1
      AND field_key IN (
          'issue_date', 'due_date', 'expiration_date', 'document_date'
      );
CREATE INDEX idx_semantic_corrections_hybrid_text
    ON semantic_user_corrections(field_key, display_value, file_id)
    WHERE active = 1;
CREATE INDEX idx_identity_relationship_hybrid_type
    ON identity_relationships(
        workspace_id, relationship_type, source_file_id, status, active
    )
    WHERE source_file_id IS NOT NULL;

INSERT INTO schema_migrations(version, name)
VALUES (12, '0012_hybrid_semantic_search');

PRAGMA user_version = 12;

COMMIT;
