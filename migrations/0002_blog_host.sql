-- M1: thin blog host + activation.
-- users/sessions (argon2, server-side sessions, CSRF), documents with immutable
-- block versions, tags, and moderated comments.

CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY NOT NULL,
    email         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'owner',
    created_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    updated_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash    TEXT PRIMARY KEY NOT NULL,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token    TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    expires_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);

CREATE TABLE IF NOT EXISTS documents (
    id             TEXT PRIMARY KEY NOT NULL,
    owner_id       TEXT NOT NULL REFERENCES users(id),
    title          TEXT NOT NULL,
    slug           TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'draft',
    published_at_ms INTEGER,
    deleted_at_ms  INTEGER,
    created_at_ms  INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    updated_at_ms  INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    UNIQUE (owner_id, slug)
);

CREATE TABLE IF NOT EXISTS blocks (
    id                 TEXT PRIMARY KEY NOT NULL,
    document_id        TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    kind               TEXT NOT NULL,
    position           INTEGER NOT NULL,
    current_version_id TEXT NOT NULL,
    created_at_ms      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    updated_at_ms      INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    UNIQUE (document_id, position)
);

CREATE INDEX IF NOT EXISTS idx_blocks_document ON blocks(document_id, position);

CREATE TABLE IF NOT EXISTS block_versions (
    id            TEXT PRIMARY KEY NOT NULL,
    block_id      TEXT NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    content_json  TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_block_versions_block ON block_versions(block_id, created_at_ms);

CREATE TABLE IF NOT EXISTS tags (
    id   TEXT PRIMARY KEY NOT NULL,
    slug TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS document_tags (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    tag_id      TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (document_id, tag_id)
);

CREATE TABLE IF NOT EXISTS comments (
    id            TEXT PRIMARY KEY NOT NULL,
    document_id   TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    author_name   TEXT NOT NULL,
    body          TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'pending',
    created_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_comments_document ON comments(document_id);
