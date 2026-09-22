CREATE TABLE IF NOT EXISTS review_sessions (
    token       TEXT PRIMARY KEY,
    branch      TEXT NOT NULL,
    label       TEXT NOT NULL,
    opened_seq  INTEGER NOT NULL,
    created_unix INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS session_ops (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token TEXT NOT NULL,
    branch TEXT NOT NULL,
    op_seq INTEGER NOT NULL
);
