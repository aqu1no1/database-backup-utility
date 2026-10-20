CREATE TABLE restore_executions (
    id TEXT PRIMARY KEY NOT NULL,
    artifact_id TEXT REFERENCES backup_artifacts (id) ON DELETE SET NULL,
    source_location TEXT NOT NULL,
    source_sha256 TEXT CHECK (source_sha256 IS NULL OR length(source_sha256) = 64),
    target_host TEXT NOT NULL,
    target_port INTEGER NOT NULL CHECK (target_port BETWEEN 1 AND 65535),
    target_database TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('create', 'overwrite')),
    previous_database TEXT,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'canceled')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER CHECK (duration_ms >= 0),
    error_message TEXT
) STRICT;

CREATE INDEX idx_restore_executions_started ON restore_executions (started_at);
