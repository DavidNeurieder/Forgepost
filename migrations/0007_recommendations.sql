-- Recommendations: let analytics events name the article a visitor was shown
-- or clicked in the "Keep reading" section, so the future personalized
-- recommendation engine can measure click-through and build interest signals.
ALTER TABLE analytics_events ADD COLUMN recommended_slug TEXT;
