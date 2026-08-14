-- Supremacy local-first catalog.
-- Requires SQLite >= 3.37 with FTS5 and JSON functions enabled.
-- SQLCipher keying and connection pragmas are deliberately configured by Rust.

PRAGMA foreign_keys = ON;

BEGIN IMMEDIATE;

CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

-- Identity, workspace, collection, and scan boundaries.

CREATE TABLE principals (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('human', 'service', 'device')),
    display_name TEXT NOT NULL,
    external_reference TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    disabled_at TEXT,
    CHECK (disabled_at IS NULL OR disabled_at >= created_at)
) STRICT;

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    settings_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(settings_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    archived_at TEXT,
    CHECK (archived_at IS NULL OR archived_at >= created_at)
) STRICT;

CREATE INDEX idx_workspaces_owner_principal
    ON workspaces(owner_principal_id);

CREATE TABLE workspace_memberships (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'administrator', 'reviewer', 'operator', 'reader')),
    granted_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    granted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    revoked_at TEXT,
    PRIMARY KEY (workspace_id, principal_id),
    CHECK (revoked_at IS NULL OR revoked_at >= granted_at)
) STRICT;

CREATE INDEX idx_workspace_memberships_principal
    ON workspace_memberships(principal_id);
CREATE INDEX idx_workspace_memberships_granted_by
    ON workspace_memberships(granted_by_principal_id);

CREATE TABLE volumes (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    platform TEXT NOT NULL CHECK (platform IN ('macos', 'windows', 'linux', 'other')),
    stable_identifier TEXT NOT NULL,
    display_name TEXT NOT NULL,
    filesystem_type TEXT,
    case_sensitive INTEGER NOT NULL CHECK (case_sensitive IN (0, 1)),
    removable INTEGER NOT NULL DEFAULT 0 CHECK (removable IN (0, 1)),
    first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id, stable_identifier),
    CHECK (last_seen_at >= first_seen_at)
) STRICT;

CREATE INDEX idx_volumes_workspace
    ON volumes(workspace_id);

CREATE TABLE roots (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL REFERENCES volumes(id) ON DELETE RESTRICT,
    added_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    absolute_path TEXT NOT NULL,
    normalized_path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    recursive INTEGER NOT NULL DEFAULT 1 CHECK (recursive IN (0, 1)),
    follow_symbolic_links INTEGER NOT NULL DEFAULT 0 CHECK (follow_symbolic_links IN (0, 1)),
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'offline', 'paused', 'retired')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    retired_at TEXT,
    UNIQUE (workspace_id, normalized_path),
    CHECK (retired_at IS NULL OR retired_at >= created_at)
) STRICT;

CREATE INDEX idx_roots_workspace
    ON roots(workspace_id);
CREATE INDEX idx_roots_volume
    ON roots(volume_id);
CREATE INDEX idx_roots_added_by
    ON roots(added_by_principal_id);

CREATE TABLE policies (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT REFERENCES roots(id) ON DELETE CASCADE,
    created_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    supersedes_policy_id TEXT REFERENCES policies(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('collection', 'privacy', 'retention', 'processing', 'cloud')),
    version INTEGER NOT NULL CHECK (version > 0),
    rules_json TEXT NOT NULL CHECK (json_valid(rules_json)),
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'active', 'superseded', 'retired')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    activated_at TEXT,
    retired_at TEXT,
    CHECK (activated_at IS NULL OR activated_at >= created_at),
    CHECK (retired_at IS NULL OR retired_at >= created_at)
) STRICT;

CREATE INDEX idx_policies_workspace
    ON policies(workspace_id);
CREATE INDEX idx_policies_root
    ON policies(root_id);
CREATE INDEX idx_policies_created_by
    ON policies(created_by_principal_id);
CREATE INDEX idx_policies_supersedes
    ON policies(supersedes_policy_id);
CREATE UNIQUE INDEX uq_policies_active_workspace
    ON policies(workspace_id, kind, name)
    WHERE root_id IS NULL AND status = 'active';
CREATE UNIQUE INDEX uq_policies_active_root
    ON policies(root_id, kind, name)
    WHERE root_id IS NOT NULL AND status = 'active';

CREATE TABLE scans (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    policy_id TEXT REFERENCES policies(id) ON DELETE RESTRICT,
    requested_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('initial', 'incremental', 'verification', 'reconciliation')),
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    cursor_before TEXT,
    cursor_after TEXT,
    started_at TEXT,
    completed_at TEXT,
    discovered_count INTEGER NOT NULL DEFAULT 0 CHECK (discovered_count >= 0),
    changed_count INTEGER NOT NULL DEFAULT 0 CHECK (changed_count >= 0),
    issue_count INTEGER NOT NULL DEFAULT 0 CHECK (issue_count >= 0),
    error_text TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (completed_at IS NULL OR started_at IS NOT NULL),
    CHECK (completed_at IS NULL OR completed_at >= started_at)
) STRICT;

CREATE INDEX idx_scans_workspace
    ON scans(workspace_id);
CREATE INDEX idx_scans_root
    ON scans(root_id);
CREATE INDEX idx_scans_policy
    ON scans(policy_id);
CREATE INDEX idx_scans_requested_by
    ON scans(requested_by_principal_id);
CREATE INDEX idx_scans_root_created
    ON scans(root_id, created_at DESC);

-- Durable logical files, native identities, observed locations, and byte versions.

CREATE TABLE files (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_by_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('regular', 'directory', 'symbolic_link', 'special')),
    lifecycle_state TEXT NOT NULL DEFAULT 'present' CHECK (lifecycle_state IN ('present', 'missing', 'offline', 'retired')),
    first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    retired_at TEXT,
    CHECK (last_seen_at >= first_seen_at),
    CHECK (retired_at IS NULL OR retired_at >= first_seen_at)
) STRICT;

CREATE INDEX idx_files_workspace
    ON files(workspace_id);
CREATE INDEX idx_files_created_by
    ON files(created_by_principal_id);
CREATE INDEX idx_files_workspace_state
    ON files(workspace_id, lifecycle_state);

CREATE TABLE native_identities (
    id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL REFERENCES volumes(id) ON DELETE RESTRICT,
    valid_from_scan_id TEXT REFERENCES scans(id) ON DELETE RESTRICT,
    valid_to_scan_id TEXT REFERENCES scans(id) ON DELETE RESTRICT,
    identity_kind TEXT NOT NULL CHECK (identity_kind IN ('posix_inode', 'windows_file_id', 'platform_bookmark', 'synthetic')),
    identity_key BLOB NOT NULL CHECK (length(identity_key) > 0),
    birth_marker TEXT,
    first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (last_seen_at >= first_seen_at),
    CHECK (valid_to_scan_id IS NULL OR valid_from_scan_id IS NOT NULL)
) STRICT;

CREATE INDEX idx_native_identities_file
    ON native_identities(file_id);
CREATE INDEX idx_native_identities_volume
    ON native_identities(volume_id);
CREATE INDEX idx_native_identities_valid_from_scan
    ON native_identities(valid_from_scan_id);
CREATE INDEX idx_native_identities_valid_to_scan
    ON native_identities(valid_to_scan_id);
CREATE UNIQUE INDEX uq_native_identities_current_native
    ON native_identities(volume_id, identity_kind, identity_key)
    WHERE valid_to_scan_id IS NULL;
CREATE UNIQUE INDEX uq_native_identities_current_file
    ON native_identities(file_id, volume_id, identity_kind)
    WHERE valid_to_scan_id IS NULL;

CREATE TABLE file_locations (
    id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    valid_from_scan_id TEXT REFERENCES scans(id) ON DELETE RESTRICT,
    valid_to_scan_id TEXT REFERENCES scans(id) ON DELETE RESTRICT,
    relative_path TEXT NOT NULL,
    normalized_relative_path TEXT NOT NULL,
    basename TEXT NOT NULL,
    parent_normalized_path TEXT,
    first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (substr(relative_path, 1, 1) <> '/'),
    CHECK (substr(normalized_relative_path, 1, 1) <> '/'),
    CHECK (instr(relative_path, char(0)) = 0),
    CHECK (last_seen_at >= first_seen_at),
    CHECK (valid_to_scan_id IS NULL OR valid_from_scan_id IS NOT NULL)
) STRICT;

CREATE INDEX idx_file_locations_file
    ON file_locations(file_id);
CREATE INDEX idx_file_locations_root
    ON file_locations(root_id);
CREATE INDEX idx_file_locations_valid_from_scan
    ON file_locations(valid_from_scan_id);
CREATE INDEX idx_file_locations_valid_to_scan
    ON file_locations(valid_to_scan_id);
CREATE UNIQUE INDEX uq_file_locations_current_path
    ON file_locations(root_id, normalized_relative_path)
    WHERE valid_to_scan_id IS NULL;
CREATE INDEX idx_file_locations_current_file
    ON file_locations(file_id)
    WHERE valid_to_scan_id IS NULL;

CREATE TABLE contents (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    media_type TEXT,
    storage_kind TEXT NOT NULL DEFAULT 'filesystem' CHECK (storage_kind IN ('filesystem', 'database', 'artifact', 'virtual')),
    storage_reference TEXT,
    inline_bytes BLOB,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (storage_kind = 'database' AND inline_bytes IS NOT NULL AND storage_reference IS NULL)
        OR
        (storage_kind <> 'database' AND inline_bytes IS NULL)
    ),
    CHECK (inline_bytes IS NULL OR length(inline_bytes) = byte_size)
) STRICT;

CREATE INDEX idx_contents_workspace
    ON contents(workspace_id);

CREATE TABLE content_digests (
    id TEXT PRIMARY KEY,
    content_id TEXT NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    algorithm TEXT NOT NULL CHECK (algorithm IN ('sha256', 'sha512', 'blake3')),
    digest BLOB NOT NULL,
    computed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (content_id, algorithm),
    UNIQUE (algorithm, digest),
    CHECK (
        (algorithm IN ('sha256', 'blake3') AND length(digest) = 32)
        OR
        (algorithm = 'sha512' AND length(digest) = 64)
    )
) STRICT;

CREATE INDEX idx_content_digests_content
    ON content_digests(content_id);

CREATE TABLE file_versions (
    id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    content_id TEXT REFERENCES contents(id) ON DELETE RESTRICT,
    native_identity_id TEXT REFERENCES native_identities(id) ON DELETE RESTRICT,
    location_id TEXT REFERENCES file_locations(id) ON DELETE RESTRICT,
    observed_by_scan_id TEXT REFERENCES scans(id) ON DELETE RESTRICT,
    version_number INTEGER NOT NULL CHECK (version_number > 0),
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    modified_at TEXT,
    created_at_native TEXT,
    mode_bits INTEGER,
    hidden INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),
    symbolic_link_target TEXT,
    attributes_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes_json)),
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (file_id, version_number),
    CHECK (content_id IS NOT NULL OR byte_size = 0)
) STRICT;

CREATE INDEX idx_file_versions_file
    ON file_versions(file_id);
CREATE INDEX idx_file_versions_content
    ON file_versions(content_id);
CREATE INDEX idx_file_versions_native_identity
    ON file_versions(native_identity_id);
CREATE INDEX idx_file_versions_location
    ON file_versions(location_id);
CREATE INDEX idx_file_versions_scan
    ON file_versions(observed_by_scan_id);

CREATE TABLE scan_observations (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    native_identity_id TEXT REFERENCES native_identities(id) ON DELETE RESTRICT,
    location_id TEXT REFERENCES file_locations(id) ON DELETE RESTRICT,
    file_version_id TEXT REFERENCES file_versions(id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL CHECK (outcome IN ('unchanged', 'created', 'changed', 'moved', 'missing', 'inaccessible')),
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE (scan_id, file_id, location_id)
) STRICT;

CREATE INDEX idx_scan_observations_scan
    ON scan_observations(scan_id);
CREATE INDEX idx_scan_observations_file
    ON scan_observations(file_id);
CREATE INDEX idx_scan_observations_native_identity
    ON scan_observations(native_identity_id);
CREATE INDEX idx_scan_observations_location
    ON scan_observations(location_id);
CREATE INDEX idx_scan_observations_file_version
    ON scan_observations(file_version_id);

CREATE TABLE scan_issues (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    relative_path TEXT,
    code TEXT NOT NULL,
    severity TEXT NOT NULL CHECK (severity IN ('information', 'warning', 'error')),
    message TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json)),
    occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX idx_scan_issues_scan
    ON scan_issues(scan_id);

CREATE TABLE duplicate_groups (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    canonical_content_id TEXT REFERENCES contents(id) ON DELETE RESTRICT,
    method TEXT NOT NULL CHECK (method IN ('exact_digest', 'perceptual', 'semantic')),
    algorithm TEXT NOT NULL,
    group_key BLOB NOT NULL CHECK (length(group_key) > 0),
    confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    generated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id, method, algorithm, group_key)
) STRICT;

CREATE INDEX idx_duplicate_groups_workspace
    ON duplicate_groups(workspace_id);
CREATE INDEX idx_duplicate_groups_canonical_content
    ON duplicate_groups(canonical_content_id);

CREATE TABLE duplicate_group_members (
    duplicate_group_id TEXT NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    content_id TEXT NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    file_version_id TEXT REFERENCES file_versions(id) ON DELETE CASCADE,
    distance REAL NOT NULL DEFAULT 0.0 CHECK (distance >= 0.0),
    is_canonical INTEGER NOT NULL DEFAULT 0 CHECK (is_canonical IN (0, 1)),
    PRIMARY KEY (duplicate_group_id, content_id, file_version_id)
) STRICT;

CREATE INDEX idx_duplicate_group_members_content
    ON duplicate_group_members(content_id);
CREATE INDEX idx_duplicate_group_members_file_version
    ON duplicate_group_members(file_version_id);

-- Processing registry, models, jobs, event streams, artifacts, and lineage.

CREATE TABLE processors (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'ocr', 'chunker', 'embedder', 'classifier', 'entity_extractor', 'organizer')),
    version TEXT NOT NULL,
    entrypoint TEXT NOT NULL,
    deterministic INTEGER NOT NULL DEFAULT 0 CHECK (deterministic IN (0, 1)),
    capabilities_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(capabilities_json)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (name, version)
) STRICT;

CREATE TABLE models (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    name TEXT NOT NULL,
    revision TEXT NOT NULL,
    execution_location TEXT NOT NULL CHECK (execution_location IN ('local', 'cloud', 'hybrid')),
    dimensions INTEGER CHECK (dimensions IS NULL OR dimensions > 0),
    context_window INTEGER CHECK (context_window IS NULL OR context_window > 0),
    capabilities_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(capabilities_json)),
    retired_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (provider, name, revision)
) STRICT;

CREATE TABLE processor_models (
    processor_id TEXT NOT NULL REFERENCES processors(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    priority INTEGER NOT NULL DEFAULT 0,
    configuration_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(configuration_json)),
    PRIMARY KEY (processor_id, model_id)
) STRICT;

CREATE INDEX idx_processor_models_model
    ON processor_models(model_id);

CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    processor_id TEXT NOT NULL REFERENCES processors(id) ON DELETE RESTRICT,
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    file_version_id TEXT REFERENCES file_versions(id) ON DELETE CASCADE,
    input_artifact_id TEXT REFERENCES artifacts(id) ON DELETE CASCADE,
    parent_job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
    requested_by_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'blocked')),
    priority INTEGER NOT NULL DEFAULT 0,
    attempt INTEGER NOT NULL DEFAULT 1 CHECK (attempt > 0),
    idempotency_key TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(input_json)),
    policy_snapshot_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(policy_snapshot_json)),
    queued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    started_at TEXT,
    completed_at TEXT,
    error_code TEXT,
    error_text TEXT,
    UNIQUE (workspace_id, idempotency_key),
    CHECK (completed_at IS NULL OR started_at IS NOT NULL),
    CHECK (completed_at IS NULL OR completed_at >= started_at)
) STRICT;

CREATE INDEX idx_jobs_workspace
    ON jobs(workspace_id);
CREATE INDEX idx_jobs_processor
    ON jobs(processor_id);
CREATE INDEX idx_jobs_model
    ON jobs(model_id);
CREATE INDEX idx_jobs_file_version
    ON jobs(file_version_id);
CREATE INDEX idx_jobs_input_artifact
    ON jobs(input_artifact_id);
CREATE INDEX idx_jobs_parent
    ON jobs(parent_job_id);
CREATE INDEX idx_jobs_requested_by
    ON jobs(requested_by_principal_id);
CREATE INDEX idx_jobs_queue
    ON jobs(status, priority DESC, queued_at)
    WHERE status IN ('queued', 'blocked');

CREATE TABLE job_events (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('queued', 'started', 'progress', 'retrying', 'succeeded', 'failed', 'cancelled', 'heartbeat')),
    payload_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
    occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (job_id, sequence_number)
) STRICT;

CREATE INDEX idx_job_events_job
    ON job_events(job_id);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('text', 'structured', 'image', 'thumbnail', 'ocr', 'embedding_input', 'report', 'other')),
    media_type TEXT,
    payload BLOB,
    storage_uri TEXT,
    byte_size INTEGER NOT NULL DEFAULT 0 CHECK (byte_size >= 0),
    digest_algorithm TEXT CHECK (digest_algorithm IS NULL OR digest_algorithm IN ('sha256', 'blake3')),
    digest BLOB,
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (payload IS NULL OR storage_uri IS NULL),
    CHECK (payload IS NULL OR length(payload) = byte_size),
    CHECK ((digest_algorithm IS NULL) = (digest IS NULL)),
    CHECK (digest IS NULL OR length(digest) = 32)
) STRICT;

CREATE INDEX idx_artifacts_job
    ON artifacts(job_id);
CREATE INDEX idx_artifacts_job_kind
    ON artifacts(job_id, kind);

CREATE TABLE provenance (
    id TEXT PRIMARY KEY,
    artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    source_file_version_id TEXT REFERENCES file_versions(id) ON DELETE RESTRICT,
    source_artifact_id TEXT REFERENCES artifacts(id) ON DELETE RESTRICT,
    relation TEXT NOT NULL CHECK (relation IN ('derived_from', 'extracted_from', 'transformed_from', 'aggregated_from')),
    ordinal INTEGER NOT NULL DEFAULT 0 CHECK (ordinal >= 0),
    parameters_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(parameters_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (source_file_version_id IS NOT NULL AND source_artifact_id IS NULL)
        OR
        (source_file_version_id IS NULL AND source_artifact_id IS NOT NULL)
    ),
    CHECK (source_artifact_id IS NULL OR source_artifact_id <> artifact_id),
    UNIQUE (artifact_id, relation, ordinal)
) STRICT;

CREATE INDEX idx_provenance_artifact
    ON provenance(artifact_id);
CREATE INDEX idx_provenance_source_file_version
    ON provenance(source_file_version_id);
CREATE INDEX idx_provenance_source_artifact
    ON provenance(source_artifact_id);

-- Extracted document structure and source-coordinate mapping.

CREATE TABLE extractions (
    id TEXT PRIMARY KEY,
    artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    processor_id TEXT NOT NULL REFERENCES processors(id) ON DELETE RESTRICT,
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    method TEXT NOT NULL CHECK (method IN ('native_text', 'parser', 'ocr', 'hybrid')),
    language TEXT,
    title TEXT,
    full_text TEXT NOT NULL DEFAULT '',
    structure_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(structure_json)),
    character_count INTEGER NOT NULL DEFAULT 0 CHECK (character_count >= 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (artifact_id),
    CHECK (character_count = length(full_text))
) STRICT;

CREATE INDEX idx_extractions_artifact
    ON extractions(artifact_id);
CREATE INDEX idx_extractions_file_version
    ON extractions(file_version_id);
CREATE INDEX idx_extractions_processor
    ON extractions(processor_id);
CREATE INDEX idx_extractions_model
    ON extractions(model_id);

CREATE TABLE pages (
    id TEXT PRIMARY KEY,
    extraction_id TEXT NOT NULL REFERENCES extractions(id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL CHECK (page_number > 0),
    text TEXT NOT NULL DEFAULT '',
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset INTEGER NOT NULL CHECK (end_offset >= start_offset),
    width_points REAL CHECK (width_points IS NULL OR width_points > 0.0),
    height_points REAL CHECK (height_points IS NULL OR height_points > 0.0),
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE (extraction_id, page_number)
) STRICT;

CREATE INDEX idx_pages_extraction
    ON pages(extraction_id);

CREATE TABLE ocr_results (
    id TEXT PRIMARY KEY,
    page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    engine_version TEXT NOT NULL,
    text TEXT NOT NULL,
    mean_confidence REAL CHECK (mean_confidence IS NULL OR (mean_confidence >= 0.0 AND mean_confidence <= 1.0)),
    blocks_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(blocks_json)),
    language TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (page_id, artifact_id)
) STRICT;

CREATE INDEX idx_ocr_results_page
    ON ocr_results(page_id);
CREATE INDEX idx_ocr_results_artifact
    ON ocr_results(artifact_id);
CREATE INDEX idx_ocr_results_model
    ON ocr_results(model_id);

CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    extraction_id TEXT NOT NULL REFERENCES extractions(id) ON DELETE CASCADE,
    page_id TEXT REFERENCES pages(id) ON DELETE CASCADE,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    strategy TEXT NOT NULL CHECK (strategy IN ('document', 'page', 'paragraph', 'sentence', 'semantic', 'fixed_window')),
    text TEXT NOT NULL,
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset INTEGER NOT NULL CHECK (end_offset >= start_offset),
    token_count INTEGER CHECK (token_count IS NULL OR token_count >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE (extraction_id, sequence_number)
) STRICT;

CREATE INDEX idx_chunks_extraction
    ON chunks(extraction_id);
CREATE INDEX idx_chunks_page
    ON chunks(page_id);

CREATE TABLE spans (
    id TEXT PRIMARY KEY,
    chunk_id TEXT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    page_id TEXT REFERENCES pages(id) ON DELETE CASCADE,
    ocr_result_id TEXT REFERENCES ocr_results(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('source', 'structural', 'ocr', 'citation', 'mention')),
    chunk_start INTEGER NOT NULL CHECK (chunk_start >= 0),
    chunk_end INTEGER NOT NULL CHECK (chunk_end >= chunk_start),
    source_start INTEGER CHECK (source_start IS NULL OR source_start >= 0),
    source_end INTEGER CHECK (source_end IS NULL OR source_end >= source_start),
    geometry_json TEXT CHECK (geometry_json IS NULL OR json_valid(geometry_json)),
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    CHECK ((source_start IS NULL) = (source_end IS NULL))
) STRICT;

CREATE INDEX idx_spans_chunk
    ON spans(chunk_id);
CREATE INDEX idx_spans_page
    ON spans(page_id);
CREATE INDEX idx_spans_ocr_result
    ON spans(ocr_result_id);

-- External-content FTS projection. search_documents is derived and rebuildable.

CREATE TABLE search_documents (
    id INTEGER PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    extraction_id TEXT NOT NULL REFERENCES extractions(id) ON DELETE CASCADE,
    title TEXT NOT NULL DEFAULT '',
    path TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    language TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    is_current INTEGER NOT NULL DEFAULT 1 CHECK (is_current IN (0, 1)),
    indexed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (extraction_id)
) STRICT;

CREATE INDEX idx_search_documents_file
    ON search_documents(file_id);
CREATE INDEX idx_search_documents_file_version
    ON search_documents(file_version_id);
CREATE INDEX idx_search_documents_extraction
    ON search_documents(extraction_id);
CREATE UNIQUE INDEX uq_search_documents_current_file
    ON search_documents(file_id)
    WHERE is_current = 1;

CREATE VIRTUAL TABLE search_documents_fts USING fts5(
    title,
    path,
    body,
    content = 'search_documents',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER search_documents_fts_insert
AFTER INSERT ON search_documents
BEGIN
    INSERT INTO search_documents_fts(rowid, title, path, body)
    VALUES (NEW.id, NEW.title, NEW.path, NEW.body);
END;

CREATE TRIGGER search_documents_fts_update
AFTER UPDATE OF title, path, body ON search_documents
BEGIN
    INSERT INTO search_documents_fts(search_documents_fts, rowid, title, path, body)
    VALUES ('delete', OLD.id, OLD.title, OLD.path, OLD.body);
    INSERT INTO search_documents_fts(rowid, title, path, body)
    VALUES (NEW.id, NEW.title, NEW.path, NEW.body);
END;

CREATE TRIGGER search_documents_fts_delete
AFTER DELETE ON search_documents
BEGIN
    INSERT INTO search_documents_fts(search_documents_fts, rowid, title, path, body)
    VALUES ('delete', OLD.id, OLD.title, OLD.path, OLD.body);
END;

-- Embedding generations are versioned so a whole vector index can be replaced atomically.

CREATE TABLE vector_generations (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE RESTRICT,
    created_by_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    purpose TEXT NOT NULL CHECK (purpose IN ('search', 'classification', 'entity_resolution', 'organization')),
    dimensions INTEGER NOT NULL CHECK (dimensions > 0),
    distance_metric TEXT NOT NULL CHECK (distance_metric IN ('cosine', 'dot_product', 'euclidean')),
    status TEXT NOT NULL DEFAULT 'building' CHECK (status IN ('building', 'ready', 'active', 'superseded', 'failed')),
    configuration_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(configuration_json)),
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at TEXT,
    activated_at TEXT,
    CHECK (completed_at IS NULL OR completed_at >= started_at),
    CHECK (activated_at IS NULL OR completed_at IS NOT NULL)
) STRICT;

CREATE INDEX idx_vector_generations_workspace
    ON vector_generations(workspace_id);
CREATE INDEX idx_vector_generations_model
    ON vector_generations(model_id);
CREATE INDEX idx_vector_generations_created_by
    ON vector_generations(created_by_principal_id);
CREATE UNIQUE INDEX uq_vector_generations_active
    ON vector_generations(workspace_id, purpose)
    WHERE status = 'active';

-- Taxonomies, classifications, entities, facts, relationships, and evidence.

CREATE TABLE taxonomies (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_by_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    description TEXT,
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'active', 'superseded', 'retired')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id, name, version)
) STRICT;

CREATE INDEX idx_taxonomies_workspace
    ON taxonomies(workspace_id);
CREATE INDEX idx_taxonomies_created_by
    ON taxonomies(created_by_principal_id);

CREATE TABLE taxonomy_nodes (
    id TEXT PRIMARY KEY,
    taxonomy_id TEXT NOT NULL REFERENCES taxonomies(id) ON DELETE CASCADE,
    parent_node_id TEXT REFERENCES taxonomy_nodes(id) ON DELETE CASCADE,
    stable_key TEXT NOT NULL,
    label TEXT NOT NULL,
    description TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE (taxonomy_id, stable_key),
    CHECK (parent_node_id IS NULL OR parent_node_id <> id)
) STRICT;

CREATE INDEX idx_taxonomy_nodes_taxonomy
    ON taxonomy_nodes(taxonomy_id);
CREATE INDEX idx_taxonomy_nodes_parent
    ON taxonomy_nodes(parent_node_id);

CREATE TABLE taxonomy_edges (
    id TEXT PRIMARY KEY,
    taxonomy_id TEXT NOT NULL REFERENCES taxonomies(id) ON DELETE CASCADE,
    from_node_id TEXT NOT NULL REFERENCES taxonomy_nodes(id) ON DELETE CASCADE,
    to_node_id TEXT NOT NULL REFERENCES taxonomy_nodes(id) ON DELETE CASCADE,
    relation TEXT NOT NULL CHECK (relation IN ('broader', 'narrower', 'related', 'excludes', 'requires')),
    weight REAL NOT NULL DEFAULT 1.0 CHECK (weight >= 0.0),
    UNIQUE (taxonomy_id, from_node_id, to_node_id, relation),
    CHECK (from_node_id <> to_node_id)
) STRICT;

CREATE INDEX idx_taxonomy_edges_taxonomy
    ON taxonomy_edges(taxonomy_id);
CREATE INDEX idx_taxonomy_edges_from_node
    ON taxonomy_edges(from_node_id);
CREATE INDEX idx_taxonomy_edges_to_node
    ON taxonomy_edges(to_node_id);

CREATE TABLE classifications (
    id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    file_version_id TEXT NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    taxonomy_id TEXT NOT NULL REFERENCES taxonomies(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL REFERENCES taxonomy_nodes(id) ON DELETE CASCADE,
    job_id TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    status TEXT NOT NULL DEFAULT 'suggested' CHECK (status IN ('suggested', 'accepted', 'rejected', 'superseded')),
    rationale TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (file_version_id, node_id, model_id)
) STRICT;

CREATE INDEX idx_classifications_file
    ON classifications(file_id);
CREATE INDEX idx_classifications_file_version
    ON classifications(file_version_id);
CREATE INDEX idx_classifications_taxonomy
    ON classifications(taxonomy_id);
CREATE INDEX idx_classifications_node
    ON classifications(node_id);
CREATE INDEX idx_classifications_job
    ON classifications(job_id);
CREATE INDEX idx_classifications_model
    ON classifications(model_id);

CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    type_node_id TEXT REFERENCES taxonomy_nodes(id) ON DELETE SET NULL,
    created_by_job_id TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    canonical_name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'merged', 'retired')),
    attributes_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX idx_entities_workspace
    ON entities(workspace_id);
CREATE INDEX idx_entities_type_node
    ON entities(type_node_id);
CREATE INDEX idx_entities_created_by_job
    ON entities(created_by_job_id);
CREATE INDEX idx_entities_normalized_name
    ON entities(workspace_id, normalized_name);

CREATE TABLE entity_names (
    id TEXT PRIMARY KEY,
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    language TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('canonical', 'alias', 'acronym', 'former', 'translated')),
    confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    UNIQUE (entity_id, normalized_name, kind)
) STRICT;

CREATE INDEX idx_entity_names_entity
    ON entity_names(entity_id);
CREATE INDEX idx_entity_names_normalized
    ON entity_names(normalized_name);

CREATE TABLE entity_mentions (
    id TEXT PRIMARY KEY,
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    extraction_id TEXT NOT NULL REFERENCES extractions(id) ON DELETE CASCADE,
    chunk_id TEXT REFERENCES chunks(id) ON DELETE CASCADE,
    span_id TEXT REFERENCES spans(id) ON DELETE CASCADE,
    job_id TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    surface_text TEXT NOT NULL,
    normalized_text TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    context_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(context_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX idx_entity_mentions_entity
    ON entity_mentions(entity_id);
CREATE INDEX idx_entity_mentions_extraction
    ON entity_mentions(extraction_id);
CREATE INDEX idx_entity_mentions_chunk
    ON entity_mentions(chunk_id);
CREATE INDEX idx_entity_mentions_span
    ON entity_mentions(span_id);
CREATE INDEX idx_entity_mentions_job
    ON entity_mentions(job_id);

CREATE TABLE facts (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    subject_entity_id TEXT REFERENCES entities(id) ON DELETE CASCADE,
    object_entity_id TEXT REFERENCES entities(id) ON DELETE CASCADE,
    created_by_job_id TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    predicate TEXT NOT NULL,
    object_text TEXT,
    object_json TEXT CHECK (object_json IS NULL OR json_valid(object_json)),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    status TEXT NOT NULL DEFAULT 'asserted' CHECK (status IN ('asserted', 'confirmed', 'disputed', 'superseded')),
    valid_from TEXT,
    valid_to TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (object_entity_id IS NOT NULL AND object_text IS NULL AND object_json IS NULL)
        OR
        (object_entity_id IS NULL AND object_text IS NOT NULL AND object_json IS NULL)
        OR
        (object_entity_id IS NULL AND object_text IS NULL AND object_json IS NOT NULL)
    ),
    CHECK (valid_to IS NULL OR valid_from IS NOT NULL),
    CHECK (valid_to IS NULL OR valid_to >= valid_from)
) STRICT;

CREATE INDEX idx_facts_workspace
    ON facts(workspace_id);
CREATE INDEX idx_facts_subject_entity
    ON facts(subject_entity_id);
CREATE INDEX idx_facts_object_entity
    ON facts(object_entity_id);
CREATE INDEX idx_facts_created_by_job
    ON facts(created_by_job_id);
CREATE INDEX idx_facts_predicate
    ON facts(workspace_id, predicate);

CREATE TABLE relationships (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    source_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    created_by_job_id TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    relation_type TEXT NOT NULL,
    directed INTEGER NOT NULL DEFAULT 1 CHECK (directed IN (0, 1)),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    status TEXT NOT NULL DEFAULT 'asserted' CHECK (status IN ('asserted', 'confirmed', 'disputed', 'superseded')),
    attributes_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (source_entity_id <> target_entity_id)
) STRICT;

CREATE INDEX idx_relationships_workspace
    ON relationships(workspace_id);
CREATE INDEX idx_relationships_source_entity
    ON relationships(source_entity_id);
CREATE INDEX idx_relationships_target_entity
    ON relationships(target_entity_id);
CREATE INDEX idx_relationships_created_by_job
    ON relationships(created_by_job_id);
CREATE INDEX idx_relationships_type
    ON relationships(workspace_id, relation_type);

CREATE TABLE evidence (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_version_id TEXT REFERENCES file_versions(id) ON DELETE CASCADE,
    artifact_id TEXT REFERENCES artifacts(id) ON DELETE CASCADE,
    chunk_id TEXT REFERENCES chunks(id) ON DELETE CASCADE,
    span_id TEXT REFERENCES spans(id) ON DELETE CASCADE,
    job_id TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    evidence_kind TEXT NOT NULL CHECK (evidence_kind IN ('source_text', 'metadata', 'classification', 'model_output', 'user_statement')),
    quote_text TEXT,
    locator_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(locator_json)),
    confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        file_version_id IS NOT NULL
        OR artifact_id IS NOT NULL
        OR chunk_id IS NOT NULL
        OR span_id IS NOT NULL
    )
) STRICT;

CREATE INDEX idx_evidence_workspace
    ON evidence(workspace_id);
CREATE INDEX idx_evidence_file_version
    ON evidence(file_version_id);
CREATE INDEX idx_evidence_artifact
    ON evidence(artifact_id);
CREATE INDEX idx_evidence_chunk
    ON evidence(chunk_id);
CREATE INDEX idx_evidence_span
    ON evidence(span_id);
CREATE INDEX idx_evidence_job
    ON evidence(job_id);

CREATE TABLE fact_evidence (
    fact_id TEXT NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    evidence_id TEXT NOT NULL REFERENCES evidence(id) ON DELETE CASCADE,
    support TEXT NOT NULL CHECK (support IN ('supports', 'contradicts', 'context')),
    PRIMARY KEY (fact_id, evidence_id)
) STRICT;

CREATE INDEX idx_fact_evidence_evidence
    ON fact_evidence(evidence_id);

CREATE TABLE relationship_evidence (
    relationship_id TEXT NOT NULL REFERENCES relationships(id) ON DELETE CASCADE,
    evidence_id TEXT NOT NULL REFERENCES evidence(id) ON DELETE CASCADE,
    support TEXT NOT NULL CHECK (support IN ('supports', 'contradicts', 'context')),
    PRIMARY KEY (relationship_id, evidence_id)
) STRICT;

CREATE INDEX idx_relationship_evidence_evidence
    ON relationship_evidence(evidence_id);

CREATE TABLE classification_evidence (
    classification_id TEXT NOT NULL REFERENCES classifications(id) ON DELETE CASCADE,
    evidence_id TEXT NOT NULL REFERENCES evidence(id) ON DELETE CASCADE,
    PRIMARY KEY (classification_id, evidence_id)
) STRICT;

CREATE INDEX idx_classification_evidence_evidence
    ON classification_evidence(evidence_id);

CREATE TABLE embeddings (
    id TEXT PRIMARY KEY,
    generation_id TEXT NOT NULL REFERENCES vector_generations(id) ON DELETE CASCADE,
    search_document_id INTEGER REFERENCES search_documents(id) ON DELETE CASCADE,
    chunk_id TEXT REFERENCES chunks(id) ON DELETE CASCADE,
    entity_id TEXT REFERENCES entities(id) ON DELETE CASCADE,
    fact_id TEXT REFERENCES facts(id) ON DELETE CASCADE,
    vector BLOB NOT NULL CHECK (length(vector) > 0),
    dimensions INTEGER NOT NULL CHECK (dimensions > 0),
    vector_format TEXT NOT NULL DEFAULT 'f32le' CHECK (vector_format IN ('f32le', 'f16le', 'int8')),
    norm REAL CHECK (norm IS NULL OR norm >= 0.0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (search_document_id IS NOT NULL AND chunk_id IS NULL AND entity_id IS NULL AND fact_id IS NULL)
        OR
        (search_document_id IS NULL AND chunk_id IS NOT NULL AND entity_id IS NULL AND fact_id IS NULL)
        OR
        (search_document_id IS NULL AND chunk_id IS NULL AND entity_id IS NOT NULL AND fact_id IS NULL)
        OR
        (search_document_id IS NULL AND chunk_id IS NULL AND entity_id IS NULL AND fact_id IS NOT NULL)
    ),
    CHECK (
        (vector_format = 'f32le' AND length(vector) = dimensions * 4)
        OR
        (vector_format = 'f16le' AND length(vector) = dimensions * 2)
        OR
        (vector_format = 'int8' AND length(vector) = dimensions)
    )
) STRICT;

CREATE INDEX idx_embeddings_generation
    ON embeddings(generation_id);
CREATE INDEX idx_embeddings_search_document
    ON embeddings(search_document_id);
CREATE INDEX idx_embeddings_chunk
    ON embeddings(chunk_id);
CREATE INDEX idx_embeddings_entity
    ON embeddings(entity_id);
CREATE INDEX idx_embeddings_fact
    ON embeddings(fact_id);
CREATE UNIQUE INDEX uq_embeddings_generation_search_document
    ON embeddings(generation_id, search_document_id)
    WHERE search_document_id IS NOT NULL;
CREATE UNIQUE INDEX uq_embeddings_generation_chunk
    ON embeddings(generation_id, chunk_id)
    WHERE chunk_id IS NOT NULL;
CREATE UNIQUE INDEX uq_embeddings_generation_entity
    ON embeddings(generation_id, entity_id)
    WHERE entity_id IS NOT NULL;
CREATE UNIQUE INDEX uq_embeddings_generation_fact
    ON embeddings(generation_id, fact_id)
    WHERE fact_id IS NOT NULL;

-- Organization policy, versioned proposals, evidence, alternatives, and review.

CREATE TABLE organization_policies (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    supersedes_policy_id TEXT REFERENCES organization_policies(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    objective_json TEXT NOT NULL CHECK (json_valid(objective_json)),
    constraints_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(constraints_json)),
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'active', 'superseded', 'retired')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id, name, version)
) STRICT;

CREATE INDEX idx_organization_policies_workspace
    ON organization_policies(workspace_id);
CREATE INDEX idx_organization_policies_created_by
    ON organization_policies(created_by_principal_id);
CREATE INDEX idx_organization_policies_supersedes
    ON organization_policies(supersedes_policy_id);
CREATE UNIQUE INDEX uq_organization_policies_active
    ON organization_policies(workspace_id, name)
    WHERE status = 'active';

CREATE TABLE organization_rules (
    id TEXT PRIMARY KEY,
    policy_id TEXT NOT NULL REFERENCES organization_policies(id) ON DELETE CASCADE,
    stable_key TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    rule_kind TEXT NOT NULL CHECK (rule_kind IN ('routing', 'naming', 'grouping', 'exclusion', 'retention', 'conflict')),
    condition_json TEXT NOT NULL CHECK (json_valid(condition_json)),
    action_json TEXT NOT NULL CHECK (json_valid(action_json)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    UNIQUE (policy_id, stable_key)
) STRICT;

CREATE INDEX idx_organization_rules_policy
    ON organization_rules(policy_id);

CREATE TABLE organization_proposals (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    policy_id TEXT NOT NULL REFERENCES organization_policies(id) ON DELETE RESTRICT,
    based_on_scan_id TEXT REFERENCES scans(id) ON DELETE RESTRICT,
    created_by_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'in_review', 'accepted', 'rejected', 'superseded')),
    summary_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(summary_json)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    closed_at TEXT
) STRICT;

CREATE INDEX idx_organization_proposals_workspace
    ON organization_proposals(workspace_id);
CREATE INDEX idx_organization_proposals_policy
    ON organization_proposals(policy_id);
CREATE INDEX idx_organization_proposals_scan
    ON organization_proposals(based_on_scan_id);
CREATE INDEX idx_organization_proposals_created_by
    ON organization_proposals(created_by_principal_id);

CREATE TABLE organization_revisions (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES organization_proposals(id) ON DELETE CASCADE,
    parent_revision_id TEXT REFERENCES organization_revisions(id) ON DELETE RESTRICT,
    created_by_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    revision_number INTEGER NOT NULL CHECK (revision_number > 0),
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'submitted', 'superseded')),
    rationale TEXT,
    snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json)),
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (proposal_id, revision_number),
    UNIQUE (proposal_id, content_hash),
    CHECK (parent_revision_id IS NULL OR parent_revision_id <> id)
) STRICT;

CREATE INDEX idx_organization_revisions_proposal
    ON organization_revisions(proposal_id);
CREATE INDEX idx_organization_revisions_parent
    ON organization_revisions(parent_revision_id);
CREATE INDEX idx_organization_revisions_created_by
    ON organization_revisions(created_by_principal_id);

CREATE TABLE organization_folders (
    id TEXT PRIMARY KEY,
    revision_id TEXT NOT NULL REFERENCES organization_revisions(id) ON DELETE CASCADE,
    parent_folder_id TEXT REFERENCES organization_folders(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    normalized_relative_path TEXT NOT NULL,
    rationale TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    UNIQUE (revision_id, normalized_relative_path),
    CHECK (parent_folder_id IS NULL OR parent_folder_id <> id)
) STRICT;

CREATE INDEX idx_organization_folders_revision
    ON organization_folders(revision_id);
CREATE INDEX idx_organization_folders_parent
    ON organization_folders(parent_folder_id);

CREATE TABLE organization_items (
    id TEXT PRIMARY KEY,
    revision_id TEXT NOT NULL REFERENCES organization_revisions(id) ON DELETE CASCADE,
    folder_id TEXT REFERENCES organization_folders(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    source_location_id TEXT REFERENCES file_locations(id) ON DELETE RESTRICT,
    classification_id TEXT REFERENCES classifications(id) ON DELETE SET NULL,
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('keep', 'move', 'rename', 'move_and_rename', 'tag', 'set_metadata')),
    proposed_name TEXT,
    proposed_relative_path TEXT,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    rationale TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    UNIQUE (revision_id, file_id)
) STRICT;

CREATE INDEX idx_organization_items_revision
    ON organization_items(revision_id);
CREATE INDEX idx_organization_items_folder
    ON organization_items(folder_id);
CREATE INDEX idx_organization_items_file
    ON organization_items(file_id);
CREATE INDEX idx_organization_items_source_location
    ON organization_items(source_location_id);
CREATE INDEX idx_organization_items_classification
    ON organization_items(classification_id);

CREATE TABLE organization_evidence (
    id TEXT PRIMARY KEY,
    revision_id TEXT NOT NULL REFERENCES organization_revisions(id) ON DELETE CASCADE,
    item_id TEXT REFERENCES organization_items(id) ON DELETE CASCADE,
    evidence_id TEXT REFERENCES evidence(id) ON DELETE CASCADE,
    rule_id TEXT REFERENCES organization_rules(id) ON DELETE SET NULL,
    signal_kind TEXT NOT NULL CHECK (signal_kind IN ('rule', 'classification', 'similarity', 'path', 'metadata', 'user')),
    weight REAL NOT NULL DEFAULT 1.0,
    explanation TEXT,
    details_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json)),
    CHECK (item_id IS NOT NULL OR evidence_id IS NOT NULL OR rule_id IS NOT NULL)
) STRICT;

CREATE INDEX idx_organization_evidence_revision
    ON organization_evidence(revision_id);
CREATE INDEX idx_organization_evidence_item
    ON organization_evidence(item_id);
CREATE INDEX idx_organization_evidence_evidence
    ON organization_evidence(evidence_id);
CREATE INDEX idx_organization_evidence_rule
    ON organization_evidence(rule_id);

CREATE TABLE organization_alternatives (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES organization_items(id) ON DELETE CASCADE,
    folder_id TEXT REFERENCES organization_folders(id) ON DELETE CASCADE,
    rank INTEGER NOT NULL CHECK (rank > 0),
    proposed_name TEXT,
    proposed_relative_path TEXT,
    score REAL NOT NULL CHECK (score >= 0.0 AND score <= 1.0),
    rationale TEXT,
    UNIQUE (item_id, rank)
) STRICT;

CREATE INDEX idx_organization_alternatives_item
    ON organization_alternatives(item_id);
CREATE INDEX idx_organization_alternatives_folder
    ON organization_alternatives(folder_id);

CREATE TABLE organization_conflicts (
    id TEXT PRIMARY KEY,
    revision_id TEXT NOT NULL REFERENCES organization_revisions(id) ON DELETE CASCADE,
    item_id TEXT REFERENCES organization_items(id) ON DELETE CASCADE,
    conflict_kind TEXT NOT NULL CHECK (conflict_kind IN ('path_collision', 'rule_disagreement', 'ambiguous_destination', 'missing_source', 'permission', 'cycle')),
    severity TEXT NOT NULL CHECK (severity IN ('warning', 'blocking')),
    conflict_key TEXT NOT NULL,
    details_json TEXT NOT NULL CHECK (json_valid(details_json)),
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'waived')),
    resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
    UNIQUE (revision_id, conflict_key)
) STRICT;

CREATE INDEX idx_organization_conflicts_revision
    ON organization_conflicts(revision_id);
CREATE INDEX idx_organization_conflicts_item
    ON organization_conflicts(item_id);

CREATE TABLE organization_review_decisions (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES organization_proposals(id) ON DELETE CASCADE,
    revision_id TEXT NOT NULL REFERENCES organization_revisions(id) ON DELETE CASCADE,
    item_id TEXT REFERENCES organization_items(id) ON DELETE CASCADE,
    conflict_id TEXT REFERENCES organization_conflicts(id) ON DELETE CASCADE,
    reviewer_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('approved', 'rejected', 'changes_requested', 'deferred')),
    comment TEXT,
    decided_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (item_id IS NULL OR conflict_id IS NULL)
) STRICT;

CREATE INDEX idx_organization_review_decisions_proposal
    ON organization_review_decisions(proposal_id);
CREATE INDEX idx_organization_review_decisions_revision
    ON organization_review_decisions(revision_id);
CREATE INDEX idx_organization_review_decisions_item
    ON organization_review_decisions(item_id);
CREATE INDEX idx_organization_review_decisions_conflict
    ON organization_review_decisions(conflict_id);
CREATE INDEX idx_organization_review_decisions_reviewer
    ON organization_review_decisions(reviewer_principal_id);

-- Sealed operation plans, approvals, execution journal, filesystem state, and rollback.

CREATE TABLE operation_plans (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    proposal_revision_id TEXT REFERENCES organization_revisions(id) ON DELETE RESTRICT,
    created_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'sealed', 'cancelled')),
    title TEXT NOT NULL,
    plan_hash BLOB,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    sealed_at TEXT,
    CHECK ((sealed_at IS NULL AND plan_hash IS NULL) OR (sealed_at IS NOT NULL AND plan_hash IS NOT NULL)),
    CHECK (plan_hash IS NULL OR length(plan_hash) = 32),
    CHECK ((status = 'sealed') = (sealed_at IS NOT NULL))
) STRICT;

CREATE INDEX idx_operation_plans_workspace
    ON operation_plans(workspace_id);
CREATE INDEX idx_operation_plans_proposal_revision
    ON operation_plans(proposal_revision_id);
CREATE INDEX idx_operation_plans_created_by
    ON operation_plans(created_by_principal_id);

CREATE TABLE operation_steps (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL REFERENCES operation_plans(id) ON DELETE CASCADE,
    sequence_number INTEGER NOT NULL CHECK (sequence_number > 0),
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('create_directory', 'remove_directory_if_empty', 'move', 'no_op')),
    source_location_id TEXT REFERENCES file_locations(id) ON DELETE RESTRICT,
    destination_root_id TEXT REFERENCES roots(id) ON DELETE RESTRICT,
    destination_relative_path TEXT,
    collision_strategy TEXT NOT NULL DEFAULT 'fail' CHECK (collision_strategy = 'fail'),
    options_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(options_json)),
    expected_effect_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(expected_effect_json)),
    UNIQUE (plan_id, sequence_number),
    UNIQUE (id, plan_id),
    CHECK (
        (operation_kind = 'create_directory' AND source_location_id IS NULL AND destination_root_id IS NOT NULL AND destination_relative_path IS NOT NULL)
        OR
        (operation_kind = 'remove_directory_if_empty' AND source_location_id IS NULL AND destination_root_id IS NOT NULL AND destination_relative_path IS NOT NULL)
        OR
        (operation_kind = 'move' AND source_location_id IS NOT NULL AND destination_root_id IS NOT NULL AND destination_relative_path IS NOT NULL)
        OR
        (operation_kind = 'no_op' AND source_location_id IS NULL AND destination_root_id IS NULL AND destination_relative_path IS NULL)
    )
) STRICT;

CREATE INDEX idx_operation_steps_plan
    ON operation_steps(plan_id);
CREATE INDEX idx_operation_steps_source_location
    ON operation_steps(source_location_id);
CREATE INDEX idx_operation_steps_destination_root
    ON operation_steps(destination_root_id);

CREATE TABLE operation_preconditions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL REFERENCES operation_plans(id) ON DELETE CASCADE,
    step_id TEXT NOT NULL,
    precondition_kind TEXT NOT NULL CHECK (precondition_kind IN ('identity_matches', 'digest_matches', 'path_present', 'path_absent', 'permission', 'free_space', 'volume_online', 'custom')),
    expected_json TEXT NOT NULL CHECK (json_valid(expected_json)),
    failure_mode TEXT NOT NULL DEFAULT 'abort_plan' CHECK (failure_mode IN ('abort_plan', 'skip_step', 'require_review')),
    FOREIGN KEY (step_id, plan_id) REFERENCES operation_steps(id, plan_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX idx_operation_preconditions_plan
    ON operation_preconditions(plan_id);
CREATE INDEX idx_operation_preconditions_step_plan
    ON operation_preconditions(step_id, plan_id);

CREATE TABLE operation_dependencies (
    plan_id TEXT NOT NULL REFERENCES operation_plans(id) ON DELETE CASCADE,
    step_id TEXT NOT NULL,
    depends_on_step_id TEXT NOT NULL,
    dependency_kind TEXT NOT NULL DEFAULT 'completion' CHECK (dependency_kind IN ('completion', 'success', 'state')),
    PRIMARY KEY (plan_id, step_id, depends_on_step_id),
    FOREIGN KEY (step_id, plan_id) REFERENCES operation_steps(id, plan_id) ON DELETE CASCADE,
    FOREIGN KEY (depends_on_step_id, plan_id) REFERENCES operation_steps(id, plan_id) ON DELETE CASCADE,
    CHECK (step_id <> depends_on_step_id)
) STRICT;

CREATE INDEX idx_operation_dependencies_step_plan
    ON operation_dependencies(step_id, plan_id);
CREATE INDEX idx_operation_dependencies_depends_on_plan
    ON operation_dependencies(depends_on_step_id, plan_id);

CREATE TABLE operation_approvals (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL REFERENCES operation_plans(id) ON DELETE CASCADE,
    principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('approved', 'rejected', 'revoked')),
    scope TEXT NOT NULL DEFAULT 'entire_plan' CHECK (scope IN ('entire_plan', 'selected_steps')),
    step_ids_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(step_ids_json)),
    plan_hash BLOB NOT NULL CHECK (length(plan_hash) = 32),
    comment TEXT,
    decided_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (plan_id, principal_id, decided_at)
) STRICT;

CREATE INDEX idx_operation_approvals_plan
    ON operation_approvals(plan_id);
CREATE INDEX idx_operation_approvals_principal
    ON operation_approvals(principal_id);

CREATE TABLE operation_executions (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL REFERENCES operation_plans(id) ON DELETE RESTRICT,
    approval_id TEXT NOT NULL REFERENCES operation_approvals(id) ON DELETE RESTRICT,
    started_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'rollback_required', 'rolled_back')),
    executor_version TEXT NOT NULL,
    plan_hash BLOB NOT NULL CHECK (length(plan_hash) = 32),
    started_at TEXT,
    completed_at TEXT,
    error_text TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (completed_at IS NULL OR started_at IS NOT NULL),
    CHECK (completed_at IS NULL OR completed_at >= started_at)
) STRICT;

CREATE INDEX idx_operation_executions_plan
    ON operation_executions(plan_id);
CREATE INDEX idx_operation_executions_approval
    ON operation_executions(approval_id);
CREATE INDEX idx_operation_executions_started_by
    ON operation_executions(started_by_principal_id);

CREATE TABLE operation_journal (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL REFERENCES operation_executions(id) ON DELETE RESTRICT,
    step_id TEXT REFERENCES operation_steps(id) ON DELETE RESTRICT,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('execution_started', 'step_started', 'precondition_checked', 'state_captured', 'step_succeeded', 'step_failed', 'execution_finished', 'rollback_started', 'rollback_step', 'rollback_finished')),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    previous_event_hash BLOB,
    event_hash BLOB NOT NULL CHECK (length(event_hash) = 32),
    occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (execution_id, sequence_number),
    CHECK (previous_event_hash IS NULL OR length(previous_event_hash) = 32)
) STRICT;

CREATE INDEX idx_operation_journal_execution
    ON operation_journal(execution_id);
CREATE INDEX idx_operation_journal_step
    ON operation_journal(step_id);

CREATE TABLE filesystem_states (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL REFERENCES operation_executions(id) ON DELETE RESTRICT,
    step_id TEXT NOT NULL REFERENCES operation_steps(id) ON DELETE RESTRICT,
    native_identity_id TEXT REFERENCES native_identities(id) ON DELETE RESTRICT,
    location_id TEXT REFERENCES file_locations(id) ON DELETE RESTRICT,
    phase TEXT NOT NULL CHECK (phase IN ('before', 'after', 'rollback_before', 'rollback_after')),
    state_json TEXT NOT NULL CHECK (json_valid(state_json)),
    state_hash BLOB NOT NULL CHECK (length(state_hash) = 32),
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (execution_id, step_id, phase)
) STRICT;

CREATE INDEX idx_filesystem_states_execution
    ON filesystem_states(execution_id);
CREATE INDEX idx_filesystem_states_step
    ON filesystem_states(step_id);
CREATE INDEX idx_filesystem_states_native_identity
    ON filesystem_states(native_identity_id);
CREATE INDEX idx_filesystem_states_location
    ON filesystem_states(location_id);

CREATE TABLE rollback_plans (
    id TEXT PRIMARY KEY,
    execution_id TEXT NOT NULL UNIQUE REFERENCES operation_executions(id) ON DELETE RESTRICT,
    created_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'ready', 'running', 'succeeded', 'failed', 'manual_intervention')),
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at TEXT
) STRICT;

CREATE INDEX idx_rollback_plans_execution
    ON rollback_plans(execution_id);
CREATE INDEX idx_rollback_plans_created_by
    ON rollback_plans(created_by_principal_id);

CREATE TABLE rollback_steps (
    id TEXT PRIMARY KEY,
    rollback_plan_id TEXT NOT NULL REFERENCES rollback_plans(id) ON DELETE CASCADE,
    original_step_id TEXT NOT NULL REFERENCES operation_steps(id) ON DELETE RESTRICT,
    sequence_number INTEGER NOT NULL CHECK (sequence_number > 0),
    action_kind TEXT NOT NULL CHECK (action_kind IN ('restore_location', 'restore_name', 'restore_metadata', 'revert_copy', 'revert_directory_creation')),
    expected_state_id TEXT REFERENCES filesystem_states(id) ON DELETE RESTRICT,
    action_json TEXT NOT NULL CHECK (json_valid(action_json)),
    UNIQUE (rollback_plan_id, sequence_number),
    UNIQUE (rollback_plan_id, original_step_id)
) STRICT;

CREATE INDEX idx_rollback_steps_plan
    ON rollback_steps(rollback_plan_id);
CREATE INDEX idx_rollback_steps_original_step
    ON rollback_steps(original_step_id);
CREATE INDEX idx_rollback_steps_expected_state
    ON rollback_steps(expected_state_id);

CREATE TABLE rollback_executions (
    id TEXT PRIMARY KEY,
    rollback_plan_id TEXT NOT NULL REFERENCES rollback_plans(id) ON DELETE RESTRICT,
    started_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'manual_intervention')),
    started_at TEXT,
    completed_at TEXT,
    error_text TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (completed_at IS NULL OR started_at IS NOT NULL),
    CHECK (completed_at IS NULL OR completed_at >= started_at)
) STRICT;

CREATE INDEX idx_rollback_executions_plan
    ON rollback_executions(rollback_plan_id);
CREATE INDEX idx_rollback_executions_started_by
    ON rollback_executions(started_by_principal_id);

-- Cloud consent, disclosures, tamper-evident audit, secret references, and watches.

CREATE TABLE cloud_providers (
    id TEXT PRIMARY KEY,
    stable_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    privacy_policy_url TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(capabilities_json)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE TABLE cloud_consents (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    provider_id TEXT NOT NULL REFERENCES cloud_providers(id) ON DELETE RESTRICT,
    previous_consent_id TEXT REFERENCES cloud_consents(id) ON DELETE RESTRICT,
    purpose TEXT NOT NULL CHECK (purpose IN ('processing', 'ocr', 'embedding', 'classification', 'organization')),
    decision TEXT NOT NULL CHECK (decision IN ('granted', 'denied', 'revoked')),
    data_classes_json TEXT NOT NULL CHECK (json_valid(data_classes_json)),
    constraints_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(constraints_json)),
    policy_version TEXT NOT NULL,
    decided_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    expires_at TEXT,
    CHECK (expires_at IS NULL OR expires_at >= decided_at),
    CHECK (previous_consent_id IS NULL OR previous_consent_id <> id)
) STRICT;

CREATE INDEX idx_cloud_consents_workspace
    ON cloud_consents(workspace_id);
CREATE INDEX idx_cloud_consents_principal
    ON cloud_consents(principal_id);
CREATE INDEX idx_cloud_consents_provider
    ON cloud_consents(provider_id);
CREATE INDEX idx_cloud_consents_previous
    ON cloud_consents(previous_consent_id);
CREATE INDEX idx_cloud_consents_lookup
    ON cloud_consents(workspace_id, principal_id, provider_id, purpose, decided_at DESC);

CREATE TABLE cloud_disclosures (
    id TEXT PRIMARY KEY,
    consent_id TEXT NOT NULL REFERENCES cloud_consents(id) ON DELETE RESTRICT,
    provider_id TEXT NOT NULL REFERENCES cloud_providers(id) ON DELETE RESTRICT,
    job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
    artifact_id TEXT REFERENCES artifacts(id) ON DELETE RESTRICT,
    request_identifier TEXT NOT NULL,
    purpose TEXT NOT NULL,
    data_manifest_json TEXT NOT NULL CHECK (json_valid(data_manifest_json)),
    payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
    redaction_summary_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(redaction_summary_json)),
    disclosed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    response_received_at TEXT,
    UNIQUE (provider_id, request_identifier),
    CHECK (job_id IS NOT NULL OR artifact_id IS NOT NULL),
    CHECK (response_received_at IS NULL OR response_received_at >= disclosed_at)
) STRICT;

CREATE INDEX idx_cloud_disclosures_consent
    ON cloud_disclosures(consent_id);
CREATE INDEX idx_cloud_disclosures_provider
    ON cloud_disclosures(provider_id);
CREATE INDEX idx_cloud_disclosures_job
    ON cloud_disclosures(job_id);
CREATE INDEX idx_cloud_disclosures_artifact
    ON cloud_disclosures(artifact_id);

CREATE TABLE audit_events (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    previous_event_id TEXT REFERENCES audit_events(id) ON DELETE RESTRICT,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    event_kind TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id TEXT,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    previous_event_hash BLOB,
    event_hash BLOB NOT NULL CHECK (length(event_hash) = 32),
    occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (workspace_id, sequence_number),
    UNIQUE (workspace_id, event_hash),
    CHECK (previous_event_hash IS NULL OR length(previous_event_hash) = 32),
    CHECK (previous_event_id IS NULL OR previous_event_id <> id)
) STRICT;

CREATE INDEX idx_audit_events_workspace
    ON audit_events(workspace_id);
CREATE INDEX idx_audit_events_principal
    ON audit_events(principal_id);
CREATE INDEX idx_audit_events_previous
    ON audit_events(previous_event_id);
CREATE INDEX idx_audit_events_subject
    ON audit_events(workspace_id, subject_kind, subject_id);

CREATE TABLE secret_references (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    owner_principal_id TEXT REFERENCES principals(id) ON DELETE RESTRICT,
    provider_id TEXT REFERENCES cloud_providers(id) ON DELETE RESTRICT,
    secret_kind TEXT NOT NULL CHECK (secret_kind IN ('api_key', 'oauth_token', 'credential', 'encryption_key', 'other')),
    keychain_service TEXT NOT NULL,
    keychain_account TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'rotating', 'revoked', 'missing')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    rotated_at TEXT,
    UNIQUE (workspace_id, keychain_service, keychain_account)
) STRICT;

CREATE INDEX idx_secret_references_workspace
    ON secret_references(workspace_id);
CREATE INDEX idx_secret_references_owner
    ON secret_references(owner_principal_id);
CREATE INDEX idx_secret_references_provider
    ON secret_references(provider_id);

CREATE TABLE watch_registrations (
    id TEXT PRIMARY KEY,
    root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    requested_by_principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE RESTRICT,
    backend TEXT NOT NULL CHECK (backend IN ('fsevents', 'read_directory_changes', 'inotify', 'polling')),
    recursive INTEGER NOT NULL DEFAULT 1 CHECK (recursive IN (0, 1)),
    status TEXT NOT NULL DEFAULT 'starting' CHECK (status IN ('starting', 'active', 'paused', 'overflowed', 'failed', 'stopped')),
    backend_cursor TEXT,
    configuration_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(configuration_json)),
    started_at TEXT,
    stopped_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (stopped_at IS NULL OR started_at IS NOT NULL),
    CHECK (stopped_at IS NULL OR stopped_at >= started_at)
) STRICT;

CREATE INDEX idx_watch_registrations_root
    ON watch_registrations(root_id);
CREATE INDEX idx_watch_registrations_requested_by
    ON watch_registrations(requested_by_principal_id);
CREATE UNIQUE INDEX uq_watch_registrations_active_root
    ON watch_registrations(root_id)
    WHERE status IN ('starting', 'active', 'paused', 'overflowed');

CREATE TABLE watch_events (
    id TEXT PRIMARY KEY,
    watch_registration_id TEXT NOT NULL REFERENCES watch_registrations(id) ON DELETE CASCADE,
    resulting_scan_id TEXT REFERENCES scans(id) ON DELETE SET NULL,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('created', 'modified', 'moved', 'removed', 'metadata', 'overflow', 'rescan_required')),
    path_before TEXT,
    path_after TEXT,
    native_identity_key BLOB,
    payload_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (watch_registration_id, sequence_number),
    CHECK (path_before IS NOT NULL OR path_after IS NOT NULL OR event_kind = 'overflow')
) STRICT;

CREATE INDEX idx_watch_events_registration
    ON watch_events(watch_registration_id);
CREATE INDEX idx_watch_events_resulting_scan
    ON watch_events(resulting_scan_id);

CREATE TABLE watch_checkpoints (
    id TEXT PRIMARY KEY,
    watch_registration_id TEXT NOT NULL REFERENCES watch_registrations(id) ON DELETE CASCADE,
    sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
    backend_cursor TEXT NOT NULL,
    state_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(state_json)),
    checkpointed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (watch_registration_id, sequence_number)
) STRICT;

CREATE INDEX idx_watch_checkpoints_registration
    ON watch_checkpoints(watch_registration_id);

-- A sealed plan freezes its definition. Approvals and execution records remain appendable.

CREATE TRIGGER operation_plans_block_sealed_update
BEFORE UPDATE ON operation_plans
WHEN OLD.sealed_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan is immutable');
END;

CREATE TRIGGER operation_plans_block_sealed_delete
BEFORE DELETE ON operation_plans
WHEN OLD.sealed_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan is immutable');
END;

CREATE TRIGGER operation_steps_block_sealed_insert
BEFORE INSERT ON operation_steps
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = NEW.plan_id AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_steps_block_sealed_update
BEFORE UPDATE ON operation_steps
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id IN (OLD.plan_id, NEW.plan_id) AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_steps_block_sealed_delete
BEFORE DELETE ON operation_steps
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = OLD.plan_id AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_preconditions_block_sealed_insert
BEFORE INSERT ON operation_preconditions
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = NEW.plan_id AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_preconditions_block_sealed_update
BEFORE UPDATE ON operation_preconditions
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id IN (OLD.plan_id, NEW.plan_id) AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_preconditions_block_sealed_delete
BEFORE DELETE ON operation_preconditions
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = OLD.plan_id AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_dependencies_block_sealed_insert
BEFORE INSERT ON operation_dependencies
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = NEW.plan_id AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_dependencies_block_sealed_update
BEFORE UPDATE ON operation_dependencies
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id IN (OLD.plan_id, NEW.plan_id) AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_dependencies_block_sealed_delete
BEFORE DELETE ON operation_dependencies
WHEN EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = OLD.plan_id AND sealed_at IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'sealed operation plan definition is immutable');
END;

CREATE TRIGGER operation_approvals_require_sealed_plan
BEFORE INSERT ON operation_approvals
WHEN NOT EXISTS (
    SELECT 1 FROM operation_plans
    WHERE id = NEW.plan_id
      AND sealed_at IS NOT NULL
      AND plan_hash = NEW.plan_hash
)
BEGIN
    SELECT RAISE(ABORT, 'approval requires the matching sealed plan');
END;

CREATE TRIGGER operation_executions_require_matching_plan
BEFORE INSERT ON operation_executions
WHEN NOT EXISTS (
    SELECT 1
    FROM operation_plans AS plan
    JOIN operation_approvals AS approval
      ON approval.plan_id = plan.id
    WHERE plan.id = NEW.plan_id
      AND approval.id = NEW.approval_id
      AND approval.decision = 'approved'
      AND plan.plan_hash = NEW.plan_hash
      AND approval.plan_hash = NEW.plan_hash
)
BEGIN
    SELECT RAISE(ABORT, 'execution requires a matching approval and plan hash');
END;

-- Journal and audit rows are append-only.

CREATE TRIGGER operation_journal_block_update
BEFORE UPDATE ON operation_journal
BEGIN
    SELECT RAISE(ABORT, 'operation journal is immutable');
END;

CREATE TRIGGER operation_journal_block_delete
BEFORE DELETE ON operation_journal
BEGIN
    SELECT RAISE(ABORT, 'operation journal is immutable');
END;

CREATE TRIGGER audit_events_block_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit event is immutable');
END;

CREATE TRIGGER audit_events_block_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit event is immutable');
END;

INSERT INTO schema_migrations(version, name)
VALUES (1, '0001_initial');

PRAGMA user_version = 1;

COMMIT;
