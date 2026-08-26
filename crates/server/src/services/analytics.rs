//! Analytics service: event recording, stats aggregation, dashboard.

use std::sync::Arc;

use forgepost_content::now_ms;
use uuid::Uuid;

use crate::analytics::{RateLimiter, block_stats, preview_text};
use crate::model::{AnalyticsEvent, DashboardMetric};
use crate::repository::Repository;
use crate::services::ServiceError;

pub struct AnalyticsService {
    repo: Arc<dyn Repository>,
    rate_limiter: RateLimiter,
}

/// Parsed and validated event fields (what the handler hands to the service).
pub struct ParsedEvent {
    pub event_type: &'static str,
    pub band: Option<i64>,
    pub block_id: Option<Uuid>,
    pub read_time_ms: Option<i64>,
    pub experiment_id: Option<Uuid>,
    pub variant_id: Option<Uuid>,
    pub recommended_slug: Option<String>,
}

impl AnalyticsService {
    pub fn new(repo: Arc<dyn Repository>, rate_limiter: RateLimiter) -> Self {
        Self { repo, rate_limiter }
    }

    /// Check rate limit. Returns `Ok(())` if allowed, `Err(RateLimited)` otherwise.
    pub fn check_rate_limit(&self, client_ip: &str) -> Result<(), ServiceError> {
        if !self.rate_limiter.allow(client_ip, now_ms()) {
            return Err(ServiceError::RateLimited);
        }
        Ok(())
    }

    /// Record an analytics event after full validation.
    pub async fn record_event(
        &self,
        slug: &str,
        parsed: &ParsedEvent,
        visitor_id: Uuid,
        pageview_id: Uuid,
        referrer: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ServiceError> {
        let full = self
            .repo
            .get_published_by_slug(slug)
            .await?
            .ok_or_else(|| ServiceError::Validation("article not found".into()))?;
        let document_id = full.document.id;

        // Validate block_id exists in document.
        if let Some(bid) = parsed.block_id
            && full.document.block(bid).is_none()
        {
            return Err(ServiceError::Validation("unknown block".into()));
        }

        // Validate experiment references.
        if let (Some(exp_id), Some(variant_id)) = (parsed.experiment_id, parsed.variant_id) {
            let exp = self
                .repo
                .experiment(exp_id)
                .await?
                .ok_or_else(|| ServiceError::Validation("unknown experiment".into()))?;
            if exp.status != "running" {
                return Err(ServiceError::Validation("experiment is not running".into()));
            }
            if exp.document_id != document_id {
                return Err(ServiceError::Validation(
                    "experiment belongs to another article".into(),
                ));
            }
            if !self
                .repo
                .experiment_variant_belongs(exp_id, variant_id)
                .await?
            {
                return Err(ServiceError::Validation(
                    "variant does not belong to experiment".into(),
                ));
            }
        }

        // Validate recommendation slug.
        if let Some(target) = parsed.recommended_slug.as_deref()
            && self.repo.get_published_by_slug(target).await?.is_none()
        {
            return Err(ServiceError::Validation(
                "recommended article not found".into(),
            ));
        }

        let event = AnalyticsEvent {
            id: Uuid::new_v4(),
            document_id,
            event_type: parsed.event_type.into(),
            band: parsed.band,
            block_id: parsed.block_id,
            pageview_id,
            visitor_id,
            referrer,
            user_agent,
            read_time_ms: parsed.read_time_ms,
            experiment_id: parsed.experiment_id,
            variant_id: parsed.variant_id,
            recommended_slug: parsed.recommended_slug.clone(),
            created_at_ms: now_ms(),
        };
        self.repo.record_analytics_event(&event).await?;
        Ok(())
    }

    /// Per-document stats for the admin dashboard.
    pub async fn document_stats(
        &self,
        document_id: Uuid,
        owner_id: Uuid,
    ) -> Result<crate::analytics::DocumentStatsView, ServiceError> {
        let full = self
            .repo
            .get_document(document_id)
            .await?
            .ok_or_else(|| ServiceError::Validation("document not found".into()))?;
        if full.owner_id != owner_id {
            return Err(ServiceError::Forbidden);
        }

        let mut article = self.repo.article_stats(document_id).await?;
        let band_reach = self.repo.band_reach(document_id).await?;
        let impressions = self.repo.block_impressions(document_id).await?;
        article.band_reach = band_reach.clone();
        article.completion = band_reach
            .iter()
            .find(|b| b.band == 100)
            .map(|b| b.pageviews)
            .filter(|&completed| completed > 0)
            .map(|completed| completed as f64 / article.views.max(1) as f64);

        let mut blocks_sorted: Vec<&forgepost_content::Block> =
            full.document.blocks.iter().collect();
        blocks_sorted.sort_by_key(|b| b.position);
        let layout: Vec<(Uuid, i64, String, String)> = blocks_sorted
            .iter()
            .filter_map(|b| {
                let kind = format!("{:?}", b.kind);
                full.document
                    .current_content(b.id)
                    .map(|c| (b.id, b.position, kind.clone(), preview_text(&kind, c)))
            })
            .collect();
        let blocks = block_stats(&layout, &impressions, &band_reach, article.views);
        Ok(crate::analytics::DocumentStatsView { article, blocks })
    }

    /// Dashboard metrics: best post, nudge, and per-document metrics.
    pub async fn dashboard(&self, owner_id: Uuid) -> Result<DashboardResult, ServiceError> {
        let docs = self.repo.list_documents(owner_id).await?;
        let metrics = self.repo.dashboard_metrics(now_ms()).await?;
        let by_id: std::collections::HashMap<Uuid, DashboardMetric> =
            metrics.into_iter().map(|m| (m.document_id, m)).collect();

        let published: Vec<(&crate::model::DocumentSummary, &DashboardMetric)> = docs
            .iter()
            .filter(|d| d.status == "published")
            .filter_map(|d| by_id.get(&d.id).map(|m| (d, m)))
            .collect();

        let best_post = published
            .iter()
            .max_by(|a, b| (a.1.views_7d, a.1.views_total).cmp(&(b.1.views_7d, b.1.views_total)))
            .filter(|(_, m)| m.views_7d > 0)
            .map(|(d, m)| BestPost {
                title: d.title.clone(),
                id: d.id.to_string(),
                views: m.views_7d,
            });

        let nudge = published
            .iter()
            .filter(|(_, m)| m.views_total >= 5)
            .min_by(|a, b| completion_rate(a.1).total_cmp(&completion_rate(b.1)))
            .filter(|(_, m)| completion_rate(m) < 1.0)
            .map(|(d, m)| {
                format!(
                    "Only {}% of readers reach the end of \u{201c}{}\u{201d}. Try a variant of the opening to keep them reading.",
                    (completion_rate(m) * 100.0).round() as i64,
                    d.title,
                )
            })
            .unwrap_or_default();

        let doc_metrics: Vec<(Uuid, i64, i64, f64)> = docs
            .iter()
            .map(|d| {
                let m = by_id.get(&d.id);
                let views_7d = m.map(|m| m.views_7d).unwrap_or(0);
                let views_prev_7d = m.map(|m| m.views_prev_7d).unwrap_or(0);
                let completed = m.map(|m| m.completed).unwrap_or(0);
                let views_total = m.map(|m| m.views_total).unwrap_or(0);
                (
                    d.id,
                    views_7d,
                    views_prev_7d,
                    if views_total > 0 {
                        completed as f64 / views_total as f64
                    } else {
                        0.0
                    },
                )
            })
            .collect();

        Ok(DashboardResult {
            docs,
            pending: self.repo.pending_comments().await?,
            best_post,
            nudge,
            doc_metrics,
        })
    }
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

pub struct BestPost {
    pub title: String,
    pub id: String,
    pub views: i64,
}

pub struct DashboardResult {
    pub docs: Vec<crate::model::DocumentSummary>,
    pub pending: Vec<crate::model::Comment>,
    pub best_post: Option<BestPost>,
    pub nudge: String,
    /// Per-document metrics: (document_id, views_7d, views_prev_7d, completion_rate).
    pub doc_metrics: Vec<(Uuid, i64, i64, f64)>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn completion_rate(m: &DashboardMetric) -> f64 {
    if m.views_total == 0 {
        return 1.0;
    }
    m.completed as f64 / m.views_total as f64
}
