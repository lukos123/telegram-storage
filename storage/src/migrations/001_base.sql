CREATE TABLE IF NOT EXISTS folders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id   INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS files (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id   INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    orig_name   TEXT NOT NULL,
    size        INTEGER NOT NULL,
    mime        TEXT,
    hash_full   BLOB NOT NULL,
    salt BLOB NOT NULL DEFAULT x'',
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS file_parts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id     INTEGER REFERENCES files(id) ON DELETE CASCADE,
    part_no     INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    file_id_tg  TEXT NOT NULL,
    sha256      BLOB NOT NULL,
    nonce BLOB NOT NULL DEFAULT x'',
    UNIQUE(file_id, part_no)
);

CREATE TABLE IF NOT EXISTS thumbnails (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id     INTEGER REFERENCES files(id) ON DELETE CASCADE,
    file_id_tg  TEXT NOT NULL,
    width       INTEGER,
    height      INTEGER
);
