CREATE TABLE profiles (
    name TEXT PRIMARY KEY NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
    engine TEXT NOT NULL CHECK(engine IN ('postgres', 'mysql', 'mongodb')),
    host TEXT NOT NULL CHECK(length(host) > 0),
    port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
    database_name TEXT NOT NULL CHECK(length(database_name) > 0),
    username TEXT NOT NULL CHECK(length(username) > 0),
    tls TEXT NOT NULL CHECK(tls IN ('auto', 'disable', 'verify-full')),
    auth_database TEXT CHECK(auth_database IS NULL OR engine = 'mongodb'),
    password_file TEXT
) STRICT;
