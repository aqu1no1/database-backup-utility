ALTER TABLE restore_executions
    ADD COLUMN checksum_verified INTEGER NOT NULL DEFAULT 1 CHECK (checksum_verified IN (0, 1));
