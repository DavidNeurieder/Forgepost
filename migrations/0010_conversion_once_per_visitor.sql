-- P13/P14 (security hardening for 0.2.0): a visitor may convert at most once
-- per experiment.
--
-- A partial unique index on `analytics_events`: only `experiment_conversion`
-- rows that carry an experiment context participate. Duplicate conversions are
-- then idempotent no-ops at the insert site (record_analytics_event uses
-- `ON CONFLICT ... WHERE event_type = 'experiment_conversion' AND
-- experiment_id IS NOT NULL DO NOTHING`), so a conversion can never be double
-- counted by resubmission. Non-conversion rows and conversion rows for other
-- (experiment, visitor) pairs are unaffected.
CREATE UNIQUE INDEX idx_one_conversion_per_visitor
    ON analytics_events(experiment_id, visitor_id)
    WHERE event_type = 'experiment_conversion' AND experiment_id IS NOT NULL;