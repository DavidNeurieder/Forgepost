//! Analytics support: rate limiting, aggregation models, and reach math.
//!
//! Scroll depth arrives as bands (25/50/75/100). Block reach is *estimated*
//! by mapping a block's position to the band needed to pass it (§5.2: "map
//! scroll depth to block boundaries ... approximate in MVP"). These figures
//! are labeled "estimated" in the UI; ad-blockers and no-JS visitors mean the
//! stream undercounts by design.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uuid::Uuid;

/// Scroll-depth bands reported by the tracking client (§5.2).
pub const SCROLL_BANDS: [i64; 4] = [25, 50, 75, 100];

// ---------------------------------------------------------------------------
// Rate limiting (public, unauthenticated write endpoint)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Window {
    start_ms: i64,
    count: u32,
}

/// In-memory fixed-window limiter keyed by client IP. Good enough for solo
/// mode; a real store replaces this with a proper edge rate limit.
#[derive(Clone, Default)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Window>>>,
    max_per_window: u32,
}

impl RateLimiter {
    pub const WINDOW_MS: i64 = 60_000;
    pub const DEFAULT_MAX: u32 = 120;

    pub fn new(max_per_window: u32) -> Self {
        Self {
            inner: Arc::default(),
            max_per_window,
        }
    }

    /// Returns `true` if a request from `key` is allowed, otherwise `false`.
    pub fn allow(&self, key: &str, now_ms: i64) -> bool {
        let mut map = self.inner.lock().expect("rate limiter mutex");
        let window = map.entry(key.to_string()).or_insert(Window {
            start_ms: now_ms,
            count: 0,
        });
        if now_ms - window.start_ms >= Self::WINDOW_MS {
            *window = Window {
                start_ms: now_ms,
                count: 0,
            };
        }
        if window.count >= self.max_per_window {
            return false;
        }
        window.count += 1;
        true
    }
}

// ---------------------------------------------------------------------------
// Aggregation models
// ---------------------------------------------------------------------------

/// Distinct pageviews whose deepest scroll reached at least `band`.
#[derive(Debug, Clone, Serialize)]
pub struct BandReach {
    pub band: i64,
    pub pageviews: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ArticleStats {
    /// Distinct pageviews that fired a `view` event.
    pub views: i64,
    /// Distinct anonymous visitors.
    pub unique_readers: i64,
    pub avg_read_time_ms: Option<i64>,
    /// Pageviews that fired an `article_read` event.
    pub read_events: i64,
    /// Fraction (0..=1) of pageviews that scrolled to 100%.
    pub completion: Option<f64>,
    /// Cumulative scroll-depth distribution (sorted ascending by band).
    pub band_reach: Vec<BandReach>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockStat {
    pub block_id: Uuid,
    pub position: i64,
    pub kind: String,
    /// Short text snippet for the dashboard.
    pub preview: String,
    /// Distinct pageviews that actually rendered this block (IntersectionObserver).
    pub impressions: i64,
    /// Distinct pageviews *estimated* to have reached this block (scroll bands).
    pub estimated_reach: i64,
    /// Estimated readers lost between this block and the previous one.
    pub estimated_dropoff: i64,
    /// Scroll-derived figures are estimates (§5.2), so this is always true in MVP.
    pub is_estimate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentStatsView {
    pub article: ArticleStats,
    pub blocks: Vec<BlockStat>,
}

// ---------------------------------------------------------------------------
// Reach math (pure, unit-testable)
// ---------------------------------------------------------------------------

fn reach_for_band(band: i64, band_reach: &[BandReach]) -> i64 {
    band_reach
        .iter()
        .find(|b| b.band == band)
        .map(|b| b.pageviews)
        .unwrap_or(0)
}

/// Distinct pageviews *estimated* to have reached `index` of `total` blocks.
///
/// Blocks are approximated as evenly spaced: block `index` is passed once the
/// reader scrolled beyond `(index + 1) / total` of the page, snapped to the
/// nearest scroll band. The last block maps to the 100% band.
pub fn estimated_reach_for_index(index: usize, total: usize, band_reach: &[BandReach]) -> i64 {
    if total == 0 {
        return 0;
    }
    let pct = (index + 1) as f64 / total as f64 * 100.0;
    let mut chosen = 0;
    for band in SCROLL_BANDS {
        if pct <= band as f64 {
            chosen = band;
            break;
        }
    }
    if chosen == 0 {
        // Last block at exactly 100% is covered by the loop; anything past the
        // page (cannot happen for index < total) returns 0.
        chosen = 100;
    }
    reach_for_band(chosen, band_reach)
}

/// Compute per-block stats from aggregations plus the document's block layout.
pub fn block_stats(
    blocks: &[(Uuid, i64, String, String)],
    impressions: &HashMap<Uuid, i64>,
    band_reach: &[BandReach],
    views: i64,
) -> Vec<BlockStat> {
    let total = blocks.len();
    let reaches: Vec<i64> = blocks
        .iter()
        .enumerate()
        .map(|(i, (id, _, _, _))| {
            estimated_reach_for_index(i, total, band_reach).max(*impressions.get(id).unwrap_or(&0))
        })
        .collect();

    blocks
        .iter()
        .enumerate()
        .map(|(i, (id, position, kind, preview))| {
            let prev = if i == 0 { views } else { reaches[i - 1] };
            BlockStat {
                block_id: *id,
                position: *position,
                kind: kind.clone(),
                preview: preview.clone(),
                impressions: *impressions.get(id).unwrap_or(&0),
                estimated_reach: reaches[i],
                estimated_dropoff: (prev - reaches[i]).max(0),
                is_estimate: true,
            }
        })
        .collect()
}

/// First ~80 chars of a block's text content, for dashboard previews.
pub fn preview_text(kind: &str, content: &serde_json::Value) -> String {
    let text = content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            content
                .get("alt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
        });
    let text = text.trim();
    if text.is_empty() {
        match kind {
            "Image" => "Image".into(),
            "Code" => "Code block".into(),
            "Divider" => "Horizontal rule".into(),
            _ => "—".into(),
        }
    } else {
        let mut t = text.to_string();
        t.truncate(80);
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reach(bands: &[(i64, i64)]) -> Vec<BandReach> {
        bands
            .iter()
            .map(|(b, p)| BandReach {
                band: *b,
                pageviews: *p,
            })
            .collect()
    }

    #[test]
    fn first_block_maps_to_lowest_band() {
        let bands = reach(&[(25, 100), (50, 80), (75, 50), (100, 30)]);
        assert_eq!(estimated_reach_for_index(0, 4, &bands), 100);
    }

    #[test]
    fn middle_block_maps_to_half_band() {
        let bands = reach(&[(25, 100), (50, 80), (75, 50), (100, 30)]);
        // index 1 of 4 → 50% → band 50.
        assert_eq!(estimated_reach_for_index(1, 4, &bands), 80);
    }

    #[test]
    fn last_block_maps_to_full_band() {
        let bands = reach(&[(25, 100), (50, 80), (75, 50), (100, 30)]);
        assert_eq!(estimated_reach_for_index(3, 4, &bands), 30);
    }

    #[test]
    fn block_stats_compute_dropoffs() {
        let bands = reach(&[(25, 100), (50, 80), (75, 50), (100, 30)]);
        let id0 = Uuid::new_v4();
        let id1 = Uuid::new_v4();
        let stats = block_stats(
            &[
                (id0, 0, "Paragraph".into(), "first".into()),
                (id1, 1, "Paragraph".into(), "second".into()),
            ],
            &HashMap::new(),
            &bands,
            100,
        );
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].estimated_reach, 80);
        assert_eq!(stats[1].estimated_reach, 30);
        assert_eq!(stats[0].estimated_dropoff, 20);
        assert_eq!(stats[1].estimated_dropoff, 50);
    }

    #[test]
    fn rate_limiter_blocks_excess() {
        let limiter = RateLimiter::new(3);
        let now = 1_000;
        assert!(limiter.allow("ip", now));
        assert!(limiter.allow("ip", now));
        assert!(limiter.allow("ip", now));
        assert!(!limiter.allow("ip", now));
        assert!(limiter.allow("other", now));
    }
}
