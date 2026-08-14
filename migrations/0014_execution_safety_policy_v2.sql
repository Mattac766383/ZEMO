BEGIN IMMEDIATE;

ALTER TABLE local_execution_consents
ADD COLUMN maximum_rehash_bytes INTEGER NOT NULL DEFAULT 68719476736
    CHECK (maximum_rehash_bytes > 0 AND maximum_rehash_bytes <= 68719476736);

ALTER TABLE local_execution_consents
ADD COLUMN allow_qualified_case_only_rename INTEGER NOT NULL DEFAULT 0
    CHECK (allow_qualified_case_only_rename IN (0, 1));

INSERT INTO schema_migrations(version, name)
VALUES (14, '0014_execution_safety_policy_v2');

PRAGMA user_version = 14;

COMMIT;
