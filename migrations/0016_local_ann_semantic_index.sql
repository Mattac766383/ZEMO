-- M9.1 Step 2: durable chunk↔ANN key mapping and index state.
-- Canonical quantized vectors remain in local_search_embeddings.
-- ANN graph snapshots live in application-controlled files; SQLite stores
-- compatibility metadata and chunk↔key mapping only.

PRAGMA user_version = 16;

INSERT INTO schema_migrations(name) VALUES ('0016_local_ann_semantic_index');

CREATE TABLE local_semantic_chunks (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    semantic_analysis_id TEXT REFERENCES semantic_analyses(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL,
    embedding_version TEXT NOT NULL,
    chunking_policy_version TEXT NOT NULL CHECK (length(chunking_policy_version) BETWEEN 1 AND 64),
    dimensions INTEGER NOT NULL CHECK (dimensions BETWEEN 1 AND 4096),
    ann_key INTEGER NOT NULL,
    source_id TEXT NOT NULL CHECK (length(source_id) BETWEEN 1 AND 64),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('semantic_summary', 'text_chunk')
    ),
    sequence_index INTEGER NOT NULL CHECK (sequence_index >= 0),
    start_offset INTEGER CHECK (start_offset IS NULL OR start_offset >= 0),
    end_offset INTEGER CHECK (
        end_offset IS NULL OR end_offset >= COALESCE(start_offset, end_offset)
    ),
    page_number INTEGER CHECK (page_number IS NULL OR page_number >= 1),
    sheet_or_slide TEXT CHECK (sheet_or_slide IS NULL OR length(sheet_or_slide) <= 80),
    text_preview TEXT NOT NULL CHECK (length(text_preview) <= 512),
    text_hash BLOB NOT NULL CHECK (length(text_hash) = 32),
    status TEXT NOT NULL CHECK (
        status IN ('active', 'tombstone', 'partial')
    ),
    truncated_file INTEGER NOT NULL DEFAULT 0 CHECK (truncated_file IN (0, 1)),
    indexed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (provider_id, embedding_version)
        REFERENCES local_embedding_models(provider_id, version) ON DELETE RESTRICT,
    UNIQUE (workspace_id, ann_key),
    UNIQUE (file_version_id, provider_id, embedding_version, source_id)
) STRICT;

CREATE INDEX idx_local_semantic_chunks_file
    ON local_semantic_chunks(file_id, provider_id, embedding_version, status);
CREATE INDEX idx_local_semantic_chunks_ann
    ON local_semantic_chunks(workspace_id, provider_id, embedding_version, ann_key)
    WHERE status = 'active';
CREATE INDEX idx_local_semantic_chunks_hash
    ON local_semantic_chunks(file_id, text_hash, embedding_version);

CREATE TABLE local_ann_index_state (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL,
    embedding_version TEXT NOT NULL,
    chunking_policy_version TEXT NOT NULL CHECK (length(chunking_policy_version) BETWEEN 1 AND 64),
    index_format_version INTEGER NOT NULL CHECK (index_format_version >= 1),
    dimensions INTEGER NOT NULL CHECK (dimensions BETWEEN 1 AND 4096),
    status TEXT NOT NULL CHECK (
        status IN (
            'not_available',
            'building',
            'ready',
            'degraded',
            'rebuild_required',
            'failed'
        )
    ),
    vector_count INTEGER NOT NULL DEFAULT 0 CHECK (vector_count >= 0),
    next_ann_key INTEGER NOT NULL DEFAULT 1 CHECK (next_ann_key >= 1),
    snapshot_sha256 TEXT CHECK (
        snapshot_sha256 IS NULL OR length(snapshot_sha256) = 64
    ),
    last_error TEXT CHECK (last_error IS NULL OR length(last_error) <= 512),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (workspace_id, provider_id, embedding_version),
    FOREIGN KEY (provider_id, embedding_version)
        REFERENCES local_embedding_models(provider_id, version) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

-- Extend embedding state with partial indexing.
-- SQLite cannot alter CHECK constraints portably; new status values are stored
-- in local_semantic_chunks.truncated_file / status instead.
