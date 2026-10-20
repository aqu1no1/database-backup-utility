CREATE TABLE connections (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL UNIQUE,
    engine TEXT NOT NULL CHECK (engine IN ('postgres', 'mysql', 'mongodb', 'sqlite')),
    host TEXT,
    port INTEGER CHECK (port BETWEEN 1 AND 65535),
    database_name TEXT NOT NULL,
    username TEXT,
    secret_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE backup_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    connection_id TEXT NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    backup_type TEXT NOT NULL CHECK (backup_type IN ('full')),
    created_at TEXT NOT NULL,
    UNIQUE (connection_id, name)
) STRICT;

CREATE TABLE backup_executions (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL REFERENCES backup_jobs (id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'canceled')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER CHECK (duration_ms >= 0),
    error_message TEXT
) STRICT;

CREATE INDEX idx_backup_executions_job_started ON backup_executions (job_id, started_at);

CREATE TABLE backup_artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    execution_id TEXT NOT NULL REFERENCES backup_executions (id) ON DELETE CASCADE,
    location TEXT NOT NULL,
    format TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    checksum_sha256 TEXT CHECK (checksum_sha256 IS NULL OR length(checksum_sha256) = 64),
    tool_version TEXT,
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX idx_backup_artifacts_execution ON backup_artifacts (execution_id);
