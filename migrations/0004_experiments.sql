-- M3: block experiments.
-- Experiments are overlays on the immutable block-version pool: each variant
-- points at an existing block_version (its content never mutates). Promoting a
-- winner simply repoints the block to that version. Decisions are append-only.

ALTER TABLE analytics_events
    ADD COLUMN experiment_id TEXT;

ALTER TABLE analytics_events
    ADD COLUMN variant_id TEXT;

CREATE TABLE IF NOT EXISTS experiments (
    id                      TEXT PRIMARY KEY NOT NULL,
    document_id             TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    block_id                TEXT NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    name                    TEXT NOT NULL DEFAULT '',
    status                  TEXT NOT NULL DEFAULT 'draft', -- draft|running|decided|stopped
    control_version_id      TEXT NOT NULL,                  -- block's version at creation
    goal                    TEXT NOT NULL DEFAULT 'completion',
    traffic_weight          REAL NOT NULL DEFAULT 50.0,     -- % of visitors who see variants
    confidence_threshold    REAL NOT NULL DEFAULT 0.95,
    min_sample_per_variant  INTEGER NOT NULL DEFAULT 100,
    no_winner_prob          REAL NOT NULL DEFAULT 0.05,
    max_duration_ms         INTEGER NOT NULL DEFAULT 2592000000, -- 30 days
    started_at_ms           INTEGER,
    decided_at_ms           INTEGER,
    decision                TEXT,                           -- winner|no_improvement|stopped
    winning_variant_id      TEXT,
    created_at_ms           INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000)
);

CREATE TABLE IF NOT EXISTS experiment_variants (
    id            TEXT PRIMARY KEY NOT NULL,
    experiment_id TEXT NOT NULL REFERENCES experiments(id) ON DELETE CASCADE,
    block_id      TEXT NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    version_id    TEXT NOT NULL REFERENCES block_versions(id),
    weight        REAL NOT NULL DEFAULT 50.0,
    is_control    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_experiments_document ON experiments(document_id, status);
CREATE INDEX IF NOT EXISTS idx_experiment_variants_exp ON experiment_variants(experiment_id);

CREATE TABLE IF NOT EXISTS experiment_decisions (
    id                     TEXT PRIMARY KEY NOT NULL,
    experiment_id          TEXT NOT NULL REFERENCES experiments(id) ON DELETE CASCADE,
    decided_at_ms          INTEGER NOT NULL DEFAULT (strftime('%s', 'now') * 1000),
    decision               TEXT NOT NULL,       -- winner|no_improvement|stopped
    winner_variant_id      TEXT,
    promoted_version_id    TEXT,
    effect_size            REAL,                -- winner conversion rate - control rate
    confidence             REAL,                -- P(winner beats control) at conclusion
    control_impressions    INTEGER,
    control_conversions    INTEGER,
    variant_impressions    INTEGER,
    variant_conversions    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_experiment_decisions_exp ON experiment_decisions(experiment_id);
