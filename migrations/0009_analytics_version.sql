-- Event provenance + one running experiment per block.
-- `version_id` records the exact immutable block version a validated
-- experiment event relates to (the version its assigned variant points at),
-- so historical events can be reconstructed against the version pool.
ALTER TABLE analytics_events ADD COLUMN version_id TEXT;

-- Assignment is per (block, visitor): a second running experiment on the same
-- block would be silently resolved by array order. The DB now forbids it; the
-- server maps the constraint violation to a Conflict on start.
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_running_exp_per_block
    ON experiments(block_id) WHERE status = 'running';