CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL CHECK (key IN ('runner', 'backup_directory')),
    value TEXT NOT NULL CHECK (length(value) BETWEEN 1 AND 4096)
) STRICT;
