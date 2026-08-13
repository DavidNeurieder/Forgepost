-- M6: media uploads.
-- A media table that tracks every uploaded file served from /media.
-- The bytes live on disk in the configured media directory; this table maps a
-- disk name (which is always the generated UUID + canonical extension, never
-- the client's filename) to its metadata and content type. The sha256 lets the
-- backup script and future dedup checks verify the on-disk copy.

CREATE TABLE IF NOT EXISTS media (
    id            TEXT PRIMARY KEY NOT NULL,
    disk_name     TEXT NOT NULL UNIQUE,
    content_type  TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL,
    sha256        TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
