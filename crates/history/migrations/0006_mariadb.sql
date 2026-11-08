CREATE TABLE connections_new (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL UNIQUE,
    engine TEXT NOT NULL CHECK (engine IN ('postgres', 'mysql', 'mariadb', 'mongodb', 'sqlite')),
    host TEXT,
    port INTEGER CHECK (port BETWEEN 1 AND 65535),
    database_name TEXT NOT NULL,
    username TEXT,
    secret_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

INSERT INTO connections_new
    (id, display_name, engine, host, port, database_name, username, secret_ref, created_at,
     updated_at)
SELECT id, display_name, engine, host, port, database_name, username, secret_ref, created_at,
       updated_at
FROM connections;

DROP TABLE connections;

ALTER TABLE connections_new RENAME TO connections;

CREATE TABLE restore_executions_new (
    id TEXT PRIMARY KEY NOT NULL,
    artifact_id TEXT REFERENCES backup_artifacts (id) ON DELETE SET NULL,
    source_location TEXT NOT NULL,
    source_sha256 TEXT CHECK (source_sha256 IS NULL OR length(source_sha256) = 64),
    engine TEXT NOT NULL CHECK (engine IN ('postgres', 'mysql', 'mariadb', 'mongodb', 'sqlite')),
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
SELECT id, artifact_id, source_location, source_sha256, engine, target_host, target_port,
       target_database, mode, previous_database, status, started_at, finished_at, duration_ms,
       error_message, checksum_verified
FROM restore_executions;

DROP TABLE restore_executions;

ALTER TABLE restore_executions_new RENAME TO restore_executions;

CREATE INDEX idx_restore_executions_started ON restore_executions (started_at);

CREATE TABLE profiles_new (
    name TEXT PRIMARY KEY NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
    engine TEXT NOT NULL CHECK(engine IN ('postgres', 'mysql', 'mariadb', 'mongodb')),
    host TEXT NOT NULL CHECK(length(host) > 0),
    port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
    database_name TEXT NOT NULL CHECK(length(database_name) > 0),
    username TEXT NOT NULL CHECK(length(username) > 0),
    tls TEXT NOT NULL CHECK(tls IN ('auto', 'disable', 'verify-full')),
    auth_database TEXT CHECK(auth_database IS NULL OR engine = 'mongodb'),
    password_file TEXT
) STRICT;

INSERT INTO profiles_new
    (name, engine, host, port, database_name, username, tls, auth_database, password_file)
SELECT name, engine, host, port, database_name, username, tls, auth_database, password_file
FROM profiles;

DROP TABLE profiles;

ALTER TABLE profiles_new RENAME TO profiles;
