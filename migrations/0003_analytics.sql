-- M2: per-block analytics.
-- Append-only public event stream written through a rate-limited endpoint.
-- Documents are never hard-deleted; events survive via ON DELETE SET NULL.
-- Scroll depth is reported as bands (25/50/75/100) plus one read event per
-- pageview; block impressions come from an IntersectionObserver.

CREATE TABLE IF NOT EXISTS analytics_events (
    id            TEXT PRIMARY KEY NOT NULL,
    document_id   TEXT REFERENCES documents(id) ON DELETE SET NULL,
    event_type    TEXT NOT NULL,   -- article_view | article_scroll | article_read | block_view
    band          INTEGER,         -- 25 | 50 | 75 | 100 (article_scroll)
    block_id      TEXT,            -- block_view only
    pageview_id   TEXT NOT NULL,   -- client-generated, one per page load
    visitor_id    TEXT NOT NULL,   -- anonymous visitor uuid from the `opv` cookie
    referrer      TEXT,
    user_agent    TEXT,
    read_time_ms  INTEGER,         -- article_read only
    created_at_ms INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE INDEX IF NOT EXISTS idx_analytics_doc_time ON analytics_events(document_id, created_at_ms);
CREATE INDEX IF NOT EXISTS idx_analytics_doc_type ON analytics_events(document_id, event_type);
CREATE INDEX IF NOT EXISTS idx_analytics_block ON analytics_events(document_id, block_id);
