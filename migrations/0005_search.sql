-- M5: full-text search.
-- SQLite FTS5 (compiled into the bundled libsqlite3-sys) indexes the current
-- published content of every document: title, body (current block versions),
-- and tags. `search_rows` maps a document UUID to its FTS rowid so a doc can
-- be re-indexed idempotently (FTS5 rowid deletes are the only reliable way
-- to replace a row).

CREATE VIRTUAL TABLE IF NOT EXISTS document_search USING fts5(
    document_id UNINDEXED,
    title,
    body,
    tags,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS search_rows (
    document_id TEXT PRIMARY KEY NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    fts_rowid   INTEGER NOT NULL UNIQUE
);
