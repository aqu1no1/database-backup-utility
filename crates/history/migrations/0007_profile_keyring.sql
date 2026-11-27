ALTER TABLE profiles ADD COLUMN keyring INTEGER NOT NULL DEFAULT 0
    CHECK (keyring IN (0, 1) AND (keyring = 0 OR password_file IS NULL));
