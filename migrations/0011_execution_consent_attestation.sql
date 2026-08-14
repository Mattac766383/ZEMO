BEGIN IMMEDIATE;

-- Consent is a separate state machine. Legacy M8 approvals are retained but
-- invalidated because they were not bound to a native root, policy digest, or
-- authenticated native confirmation challenge.
CREATE TABLE local_execution_consents (
    execution_id TEXT PRIMARY KEY
        REFERENCES local_execution_sessions(id) ON DELETE RESTRICT,
    material_version INTEGER NOT NULL CHECK (material_version > 0),
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'attested', 'consumed', 'expired', 'invalidated')
    ),
    issued_at_unix_ms INTEGER,
    expires_at_unix_ms INTEGER,
    attested_at_unix_ms INTEGER,
    consumed_at_unix_ms INTEGER,
    invalidated_at_unix_ms INTEGER,
    invalidation_reason TEXT,
    nonce BLOB CHECK (nonce IS NULL OR length(nonce) = 32),
    safety_policy_version TEXT NOT NULL,
    safety_policy_digest TEXT NOT NULL CHECK (length(safety_policy_digest) = 64),
    destination_root_path_encoding TEXT NOT NULL CHECK (
        destination_root_path_encoding IN (
            'unix_bytes', 'windows_utf16_le', 'legacy_utf8'
        )
    ),
    destination_root_canonical BLOB NOT NULL,
    destination_root_display TEXT NOT NULL,
    destination_volume_json TEXT NOT NULL CHECK (json_valid(destination_volume_json)),
    attestation_mac BLOB CHECK (
        attestation_mac IS NULL OR length(attestation_mac) = 32
    ),
    state_changed_at_unix_ms INTEGER NOT NULL,
    CHECK (length(safety_policy_version) BETWEEN 1 AND 64),
    CHECK (length(destination_root_canonical) BETWEEN 1 AND 32768),
    CHECK (length(destination_root_display) BETWEEN 1 AND 4096),
    CHECK (length(destination_volume_json) <= 4096),
    CHECK (invalidation_reason IS NULL OR length(invalidation_reason) <= 256),
    CHECK (
        (issued_at_unix_ms IS NULL AND expires_at_unix_ms IS NULL AND nonce IS NULL)
        OR (
            issued_at_unix_ms IS NOT NULL
            AND expires_at_unix_ms IS NOT NULL
            AND expires_at_unix_ms > issued_at_unix_ms
            AND nonce IS NOT NULL
        )
    ),
    CHECK (
        (attested_at_unix_ms IS NULL AND attestation_mac IS NULL)
        OR (
            attested_at_unix_ms IS NOT NULL
            AND attestation_mac IS NOT NULL
            AND issued_at_unix_ms IS NOT NULL
            AND attested_at_unix_ms >= issued_at_unix_ms
            AND attested_at_unix_ms <= expires_at_unix_ms
        )
    ),
    CHECK (
        (state = 'pending'
            AND attested_at_unix_ms IS NULL
            AND consumed_at_unix_ms IS NULL
            AND invalidated_at_unix_ms IS NULL)
        OR
        (state = 'attested'
            AND issued_at_unix_ms IS NOT NULL
            AND attested_at_unix_ms IS NOT NULL
            AND consumed_at_unix_ms IS NULL
            AND invalidated_at_unix_ms IS NULL)
        OR
        (state = 'consumed'
            AND issued_at_unix_ms IS NOT NULL
            AND attested_at_unix_ms IS NOT NULL
            AND consumed_at_unix_ms IS NOT NULL
            AND consumed_at_unix_ms >= attested_at_unix_ms
            AND invalidated_at_unix_ms IS NULL)
        OR
        (state = 'expired'
            AND issued_at_unix_ms IS NOT NULL
            AND consumed_at_unix_ms IS NULL
            AND invalidated_at_unix_ms IS NULL)
        OR
        (state = 'invalidated'
            AND consumed_at_unix_ms IS NULL
            AND invalidated_at_unix_ms IS NOT NULL
            AND invalidation_reason IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_local_execution_consents_state_expiry
    ON local_execution_consents(state, expires_at_unix_ms);

INSERT INTO local_execution_consents(
    execution_id,
    material_version,
    state,
    invalidated_at_unix_ms,
    invalidation_reason,
    safety_policy_version,
    safety_policy_digest,
    destination_root_path_encoding,
    destination_root_canonical,
    destination_root_display,
    destination_volume_json,
    state_changed_at_unix_ms
)
SELECT
    execution.id,
    1,
    'invalidated',
    0,
    'legacy_m8_confirmation_not_authenticated',
    'legacy-m8-unbound',
    lower(hex(zeroblob(32))),
    'legacy_utf8',
    CAST(root.absolute_path AS BLOB),
    root.absolute_path,
    json_object(
        'platform',
        CASE volume.platform
            WHEN 'macos' THEN 'mac_os'
            WHEN 'windows' THEN 'windows'
            WHEN 'linux' THEN 'linux'
            ELSE 'other'
        END,
        'stable_identifier', volume.stable_identifier,
        'filesystem_type', volume.filesystem_type,
        'case_sensitive', json(CASE volume.case_sensitive WHEN 1 THEN 'true' ELSE 'false' END),
        'removable', json(CASE volume.removable WHEN 1 THEN 'true' ELSE 'false' END),
        'local', json('true')
    ),
    0
FROM local_execution_sessions AS execution
JOIN roots AS root ON root.id = execution.root_id
JOIN volumes AS volume ON volume.id = root.volume_id;

INSERT INTO schema_migrations(version, name)
VALUES (11, '0011_execution_consent_attestation');

PRAGMA user_version = 11;

COMMIT;
