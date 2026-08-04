-- M0: minimal storage bootstrap.
-- The full schema (users, documents, blocks, block_versions, analytics_events,
-- experiments, ...) lands with M1/M2/M3. This proves the migration + repository
-- path and drives the /setup status endpoint.

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

INSERT INTO settings (key, value)
VALUES ('schema.version', '0')
ON CONFLICT(key) DO NOTHING;
