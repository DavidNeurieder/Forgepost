-- Recommendations (phase 2): per-visitor interest queries against the event
-- stream need a visitor_id-led index; the existing ones are all document_id-led.
CREATE INDEX IF NOT EXISTS idx_analytics_visitor_time
    ON analytics_events(visitor_id, created_at_ms);
CREATE INDEX IF NOT EXISTS idx_analytics_visitor_type
    ON analytics_events(visitor_id, event_type);
