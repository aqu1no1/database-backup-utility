CREATE TABLE restore_executions_new (
    id TEXT PRIMARY KEY NOT NULL,
    artifact_id TEXT REFERENCES backup_artifacts (id) ON DELETE SET NULL,
    source_location TEXT NOT NULL,
    source_sha256 TEXT CHECK (source_sha256 IS NULL OR length(source_sha256) = 64),
    engine TEXT NOT NULL CHECK (engine IN ('postgres', 'mysql', 'mongodb', 'sqlite')),
    target_host TEXT,
    target_port INTEGER CHECK (target_port BETWEEN 1 AND 65535),
    target_database TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('create', 'overwrite')),
    previous_database TEXT,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'canceled')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER CHECK (duration_ms >= 0),
    error_message TEXT,
    checksum_verified INTEGER NOT NULL DEFAULT 1 CHECK (checksum_verified IN (0, 1)),
    CHECK ((target_host IS NULL) = (target_port IS NULL))
) STRICT;

INSERT INTO restore_executions_new
    (id, artifact_id, source_location, source_sha256, engine, target_host, target_port,
     target_database, mode, previous_database, status, started_at, finished_at, duration_ms,
     error_message, checksum_verified)
SELECT id, artifact_id, source_location, source_sha256, 'postgres', target_host, target_port,
       target_database, mode, previous_database, status, started_at, finished_at, duration_ms,
       error_message, checksum_verified
FROM restore_executions;

DROP TABLE restore_executions;

ALTER TABLE restore_executions_new RENAME TO restore_executions;

CREATE INDEX idx_restore_executions_started ON restore_executions (started_at);
