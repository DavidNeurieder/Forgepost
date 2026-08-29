//! Analytics service: event recording, stats aggregation, dashboard.

use std::sync::Arc;

use forgepost_analytics::{DocumentStatsView, RateLimiter, block_stats, preview_text};
use forgepost_content::now_ms;
use forgepost_domain::model::{Comment, DashboardMetric, DocumentSummary};
use uuid::Uuid;

use crate::ports::{AnalyticsRepo, CommentRepo, DocumentRepo, Repository};
use crate::services::ServiceError;

pub struct AnalyticsService {
    analytics_repo: Arc<dyn AnalyticsRepo>,
    doc_repo: Arc<dyn DocumentRepo>,
    comment_repo: Arc<dyn CommentRepo>,
    rate_limiter: RateLimiter,
}

impl AnalyticsService {
    pub fn new(repo: Arc<dyn Repository>, rate_limiter: RateLimiter) -> Self {
        Self {
            analytics_repo: repo.clone(),
            doc_repo: repo.clone(),
            comment_repo: repo,
            rate_limiter,
        }
    }

    /// Check rate limit. Returns `Ok(())` if allowed, `Err(RateLimited)` otherwise.
    pub fn check_rate_limit(&self, client_ip: &str) -> Result<(), ServiceError> {
        if !self.rate_limiter.allow(client_ip, now_ms()) {
            return Err(ServiceError::RateLimited);
        }
        Ok(())
    }

    /// Per-document stats for the admin dashboard.
    pub async fn document_stats(
        &self,
        document_id: Uuid,
        owner_id: Uuid,
    ) -> Result<DocumentStatsView, ServiceError> {
        let full = self
            .doc_repo
            .get_document(document_id)
            .await?
            .ok_or_else(|| ServiceError::Validation("document not found".into()))?;
        if full.owner_id != owner_id {
            return Err(ServiceError::Forbidden);
        }

        let mut article = self.analytics_repo.article_stats(document_id).await?;
        let band_reach = self.analytics_repo.band_reach(document_id).await?;
        let impressions = self.analytics_repo.block_impressions(document_id).await?;
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
        Ok(DocumentStatsView { article, blocks })
    }

    /// Dashboard metrics: best post, nudge, and per-document metrics.
    pub async fn dashboard(&self, owner_id: Uuid) -> Result<DashboardResult, ServiceError> {
        let docs = self.doc_repo.list_documents(owner_id).await?;
        let metrics = self.analytics_repo.dashboard_metrics(now_ms()).await?;
        let by_id: std::collections::HashMap<Uuid, DashboardMetric> =
            metrics.into_iter().map(|m| (m.document_id.0, m)).collect();

        let published: Vec<(&DocumentSummary, &DashboardMetric)> = docs
            .iter()
            .filter(|d| d.status == "published")
            .filter_map(|d| by_id.get(&d.id.0).map(|m| (d, m)))
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
                let m = by_id.get(&d.id.0);
                let views_7d = m.map(|m| m.views_7d).unwrap_or(0);
                let views_prev_7d = m.map(|m| m.views_prev_7d).unwrap_or(0);
                let completed = m.map(|m| m.completed).unwrap_or(0);
                let views_total = m.map(|m| m.views_total).unwrap_or(0);
                (
                    d.id.0,
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
            pending: self.comment_repo.pending_comments().await?,
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
    pub docs: Vec<DocumentSummary>,
    pub pending: Vec<Comment>,
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
