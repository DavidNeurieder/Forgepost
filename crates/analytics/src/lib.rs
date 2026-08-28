//! Event collection + per-block metrics + reach math.
//!
//! Analytics writes go through the [`EventSink`] boundary so a bulk store
//! (ClickHouse) can replace the SQLite writer later without touching the
//! domain. Scroll depth arrives as bands (25/50/75/100); block reach is
//! *estimated* by mapping a block's position to the band needed to pass it.
//! These figures are labeled "estimated" in the UI; ad-blockers and no-JS
//! visitors mean the stream undercounts by design.

use async_trait::async_trait;
use forgepost_content::{BlockId, DocumentId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

pub type SessionId = Uuid;
pub type EventId = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    View,
    BandedScroll,
    BlockImpression,
    BlockCompletion,
    ArticleRead,
    ExperimentImpression,
    ExperimentConversion,
    Referral,
    RecommendationImpression,
    RecommendationClick,
    ShareClick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_id: EventId,
    pub session_id: SessionId,
    pub document_id: DocumentId,
    pub block_id: Option<BlockId>,
    pub kind: EventKind,
    /// Unix timestamp in milliseconds.
    pub occurred_at_ms: i64,
    /// Kind-specific payload (band, referral source, experiment id, ...).
    pub payload: serde_json::Value,
}

/// Boundary for analytics writes (§5.4). Storage behind this trait can be
/// swapped without touching domain logic.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn record(&self, event: RawEvent) -> Result<(), AnalyticsError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("invalid event: {0}")]
    InvalidEvent(String),
}

// ---------------------------------------------------------------------------
// Scroll-depth band constants
// ---------------------------------------------------------------------------

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
    inner: Arc<std::sync::Mutex<std::collections::HashMap<String, Window>>>,
    max_per_window: u32,
}

impl RateLimiter {
    pub const WINDOW_MS: i64 = 60_000;
    /// Public analytics writes per window (per client identity).
    pub const DEFAULT_MAX: u32 = 120;
    /// Allowed failed logins per window per client+account key.
    pub const DEFAULT_LOGIN_MAX: u32 = 10;
    /// Allowed anonymous comment submissions per window per client.
    pub const DEFAULT_COMMENT_MAX: u32 = 10;

    pub fn new(max_per_window: u32) -> Self {
        Self {
            inner: Arc::default(),
            max_per_window,
        }
    }

    /// Returns `true` if a request from `key` is allowed, otherwise `false`.
    /// Consumes one slot from the window (used for count-everything limits).
    pub fn allow(&self, key: &str, now_ms: i64) -> bool {
        let mut map = self.inner.lock().expect("rate limiter mutex");
        let window = self.window(&mut map, key, now_ms);
        if window.count >= self.max_per_window {
            return false;
        }
        window.count += 1;
        true
    }

    /// Returns `true` if `key` still has headroom, without consuming a slot.
    /// Use with [`Self::record`] to count only failures (e.g. failed logins).
    pub fn peek(&self, key: &str, now_ms: i64) -> bool {
        let mut map = self.inner.lock().expect("rate limiter mutex");
        let window = self.window(&mut map, key, now_ms);
        window.count < self.max_per_window
    }

    /// Consume a slot for `key` without an allow decision. Used to record a
    /// failure after [`Self::peek`] admitted the attempt. The count never
    /// exceeds `max_per_window`, so repeated record calls cannot grow unbounded.
    pub fn record(&self, key: &str, now_ms: i64) {
        let mut map = self.inner.lock().expect("rate limiter mutex");
        let window = self.window(&mut map, key, now_ms);
        window.count = window.count.saturating_add(1).min(self.max_per_window);
    }

    /// The current (possibly just-reset) window for `key`.
    fn window<'a>(
        &self,
        map: &'a mut std::collections::HashMap<String, Window>,
        key: &str,
        now_ms: i64,
    ) -> &'a mut Window {
        let window = map.entry(key.to_string()).or_insert(Window {
            start_ms: now_ms,
            count: 0,
        });
        // Saturating so adversarial/backwards clock values can never overflow
        // and panic the process.
        if now_ms.saturating_sub(window.start_ms) >= Self::WINDOW_MS {
            *window = Window {
                start_ms: now_ms,
                count: 0,
            };
        }
        window
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
    /// Distinct pageviews that fired a `share_click` event.
    pub shares: i64,
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
// Traffic sources
// ---------------------------------------------------------------------------

/// Where a pageview's referrer points, bucketed for the Stats page
/// ("traffic sources"): search engines, internal/direct navigation, and
/// everything external (social, blogs, RSS readers, aggregators).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrafficSource {
    Search,
    Direct,
    Community,
}

impl TrafficSource {
    pub fn label(self) -> &'static str {
        match self {
            TrafficSource::Search => "Search",
            TrafficSource::Direct => "Direct",
            TrafficSource::Community => "Community",
        }
    }
}

/// A bucketed traffic-source row: `pageviews` distinct view pageviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficSourceCount {
    pub source: TrafficSource,
    pub pageviews: i64,
}

/// Host portion of a URL, lowercased, without scheme, port, or `www.` prefix.
/// Returns `None` for relative URLs (internal links) and unparseable strings.
pub fn host_of(url: &str) -> Option<String> {
    if url.starts_with('/') {
        return None;
    }
    let rest = url.split("://").nth(1)?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.split(':').next()?.trim();
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Classify a `Referer` header value. Missing/empty referrers and links that
/// point at this site's own origin count as `Direct`; known search-engine
/// hosts as `Search`; everything else as `Community`.
pub fn classify_referrer(referrer: Option<&str>, site_host: Option<&str>) -> TrafficSource {
    let Some(raw) = referrer.map(str::trim).filter(|r| !r.is_empty()) else {
        return TrafficSource::Direct;
    };
    if raw.starts_with('/') {
        return TrafficSource::Direct;
    }
    let Some(host) = host_of(raw) else {
        return TrafficSource::Direct;
    };
    let host = host.trim_start_matches("www.");
    if let Some(site) = site_host {
        let site = site.trim_start_matches("www.").to_ascii_lowercase();
        if host == site {
            return TrafficSource::Direct;
        }
    }
    if is_search_host(host) {
        TrafficSource::Search
    } else {
        TrafficSource::Community
    }
}

fn is_search_host(host: &str) -> bool {
    host.starts_with("google.")
        || host.starts_with("yandex.")
        || host == "bing.com"
        || host == "duckduckgo.com"
        || host == "ecosia.org"
        || host == "qwant.com"
        || host == "startpage.com"
        || host == "search.brave.com"
        || host == "baidu.com"
}

/// Aggregate raw per-referrer pageview counts into `TrafficSource` buckets,
/// sorted by pageviews descending (then label for determinism).
pub fn bucket_traffic_sources(
    rows: &[(Option<String>, i64)],
    site_host: Option<&str>,
) -> Vec<TrafficSourceCount> {
    let mut buckets: std::collections::HashMap<TrafficSource, i64> =
        std::collections::HashMap::new();
    for (referrer, pageviews) in rows {
        let source = classify_referrer(referrer.as_deref(), site_host);
        *buckets.entry(source).or_insert(0) += pageviews;
    }
    let mut out: Vec<TrafficSourceCount> = buckets
        .into_iter()
        .map(|(source, pageviews)| TrafficSourceCount { source, pageviews })
        .collect();
    out.sort_by(|a, b| {
        b.pageviews
            .cmp(&a.pageviews)
            .then(a.source.label().cmp(b.source.label()))
    });
    out
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
    impressions: &std::collections::HashMap<Uuid, i64>,
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
            &std::collections::HashMap::new(),
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
    fn classify_referrer_buckets_search_direct_community() {
        let site = Some("blog.example.com");
        assert_eq!(
            classify_referrer(Some("https://www.google.com/search?q=rust"), site),
            TrafficSource::Search
        );
        assert_eq!(
            classify_referrer(Some("https://duckduckgo.com/?q=blog"), site),
            TrafficSource::Search
        );
        assert_eq!(classify_referrer(None, site), TrafficSource::Direct);
        assert_eq!(classify_referrer(Some(""), site), TrafficSource::Direct);
        assert_eq!(
            classify_referrer(Some("/posts/foo"), site),
            TrafficSource::Direct
        );
        assert_eq!(
            classify_referrer(Some("https://blog.example.com/posts/foo"), site),
            TrafficSource::Direct
        );
        assert_eq!(
            classify_referrer(Some("https://news.ycombinator.com/item?id=1"), site),
            TrafficSource::Community
        );
        assert_eq!(
            classify_referrer(Some("https://hacker-news.example.org/"), site),
            TrafficSource::Community
        );
    }

    #[test]
    fn classify_referrer_site_host_ignores_www_and_case() {
        let site = Some("www.Blog.example.com");
        assert_eq!(
            classify_referrer(Some("https://blog.example.com/foo"), site),
            TrafficSource::Direct
        );
    }

    #[test]
    fn bucket_traffic_sources_aggregates_and_sorts() {
        let site = Some("blog.example.com");
        let rows = vec![
            (Some("https://www.google.com/".into()), 5),
            (None, 3),
            (Some("https://news.ycombinator.com/".into()), 9),
            (Some("/internal/link".into()), 1),
        ];
        let buckets = bucket_traffic_sources(&rows, site);
        assert_eq!(
            buckets,
            vec![
                TrafficSourceCount {
                    source: TrafficSource::Community,
                    pageviews: 9
                },
                TrafficSourceCount {
                    source: TrafficSource::Search,
                    pageviews: 5
                },
                TrafficSourceCount {
                    source: TrafficSource::Direct,
                    pageviews: 4
                },
            ]
        );
    }

    #[test]
    fn host_of_strips_scheme_port_path_and_www() {
        assert_eq!(
            host_of("https://www.example.com:8080/path?q=1"),
            Some("www.example.com".into())
        );
        assert_eq!(host_of("/relative"), None);
        assert_eq!(host_of("not a url"), None);
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

    #[test]
    fn rate_limiter_peek_and_record_count_failures_only() {
        let limiter = RateLimiter::new(3);
        let now = 1_000;

        // peek does not consume slots.
        assert!(limiter.peek("user", now));
        assert!(limiter.peek("user", now));
        assert!(limiter.peek("user", now));

        // Record only failures: two failed attempts exhaust the window...
        limiter.record("user", now);
        limiter.record("user", now);
        assert!(limiter.peek("user", now));
        limiter.record("user", now);
        assert!(!limiter.peek("user", now));

        // ...but successful bursts are never blocked because they don't record.
        assert!(!limiter.peek("user", now));

        // record cannot grow past the cap.
        limiter.record("user", now);
        limiter.record("user", now);
        assert!(!limiter.peek("user", now));

        // The window resets normally.
        assert!(limiter.peek("user", now + RateLimiter::WINDOW_MS));
    }

    // -------------------------------------------------------------------------
    // Rate-limiter invariants over generated input spaces (proptest).
    // -------------------------------------------------------------------------
    use proptest::prop_assert;

    // Property A: for any limit N ≥ 1, the first N requests from a key are
    // allowed and request N+1 is denied, regardless of ordering of interleaved
    // requests from other keys.
    proptest::proptest! {
        #[test]
        fn rate_limiter_first_n_allowed_then_denied(n in 1u32..200, now in 0i64..1_000_000) {
            let limiter = RateLimiter::new(n);
            for _ in 0..n {
                prop_assert!(limiter.allow("a", now), "request within the limit must pass");
            }
            prop_assert!(!limiter.allow("a", now), "request past the limit must fail");
            // Interleaving other keys never changes key A's budget.
            for _ in 0..n * 3 {
                let _ = limiter.allow("other", now);
            }
            prop_assert!(!limiter.allow("a", now), "other keys must not interfere");
            prop_assert!(limiter.allow("untouched", now), "fresh key unaffected");
        }
    }

    // Property B: keys are fully isolated — exhausting one never touches
    // another, even when the same window time is shared.
    proptest::proptest! {
        #[test]
        fn rate_limiter_keys_are_isolated(now in 0i64..1_000_000) {
            let limiter = RateLimiter::new(4);
            for c in 'a'..='z' {
                for _ in 0..4 {
                    prop_assert!(limiter.allow(&c.to_string(), now));
                }
                prop_assert!(!limiter.allow(&c.to_string(), now));
            }
            // Every key is independently exhausted; no cross-talk at all.
            for c in 'a'..='z' {
                prop_assert!(!limiter.allow(&c.to_string(), now));
            }
        }
    }

    // Property C: expired windows become usable again exactly once the window
    // boundary has passed, and only for the key that expired.
    proptest::proptest! {
        #[test]
        fn rate_limiter_window_expires(now in 0i64..1_000_000, drift in 1i64..RateLimiter::WINDOW_MS) {
            let limiter = RateLimiter::new(2);
            // Exhaust both keys.
            limiter.allow("a", now);
            limiter.allow("a", now);
            limiter.allow("b", now);
            limiter.allow("b", now);
            // A step smaller than the window keeps both blocked...
            prop_assert!(!limiter.allow("a", now + drift));
            prop_assert!(!limiter.allow("b", now + drift));
            // ...exactly at the boundary both reset.
            let later = now + RateLimiter::WINDOW_MS;
            prop_assert!(limiter.allow("a", later));
            prop_assert!(limiter.allow("b", later));
        }
    }

    // Property D: no clock value — forward, backward, or at the i64 extremes —
    // may panic the limiter, even when the stored window start is far away.
    proptest::proptest! {
        #[test]
        fn rate_limiter_never_panics_on_any_clock(now in i64::MIN..=i64::MAX, key in "[a-z]{0,16}") {
            let limiter = RateLimiter::new(3);
            // Exercise the subtraction across the full range, including the
            // value farthest from `now` (the mirror of i64::MIN).
            let far = now.wrapping_add(i64::MAX);
            let _ = limiter.allow(&key, now);
            let _ = limiter.peek(&key, now);
            limiter.record(&key, now);
            let _ = limiter.allow(&key, far);
            let _ = limiter.allow(&key, far.wrapping_neg());
            let _ = limiter.allow(&key, i64::MIN);
            let _ = limiter.allow(&key, i64::MAX);
        }
    }

    // Property E: per-key record counts saturate at the cap (never grow
    // unbounded), and huge keys exercise the map without panics. Bounded size
    // so the property stays fast under the default case count.
    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]
        #[test]
        fn rate_limiter_record_saturates_and_huge_keys_are_safe(
            huge in proptest::collection::vec(proptest::char::any(), 0..1024),
            now in 0i64..1_000_000,
        ) {
            let limiter = RateLimiter::new(7);
            let key: String = huge.iter().collect();
            for _ in 0..1_000 {
                limiter.record(&key, now);
            }
            // After the cap the key reports exhausted, and record never
            // inflated a per-key counter past the cap.
            prop_assert!(!limiter.peek(&key, now));
            // A second huge key coexists with the first without interfering.
            limiter.record(&key, now);
            prop_assert!(!limiter.peek(&key, now));
        }
    }
}
