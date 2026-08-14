//! Article recommendation engine.
//!
//! Phase 1 (content-based, live): published posts are ranked by how many tags
//! they share with the article being read, newest-first as a tiebreak, with the
//! most recent posts backfilling when few share tags.
//!
//! Phase 2 (personalized, designed but not built): `recommend` already accepts
//! the anonymous `visitor_id` so the call site and this signature stay stable.
//! A future interest engine will score candidates against a per-visitor
//! tag-affinity vector derived from the visitor's own `analytics_events` rows
//! (view / article_read events are already keyed by the anonymous `opv`
//! cookie), decay affinity by recency, exclude posts the visitor has already
//! read, and blend affinity with recency and popularity. No new data
//! collection is needed for that phase.

use uuid::Uuid;

use crate::AppState;
use crate::model::PublishedPost;
use crate::repository::RepositoryError;

/// Rank the articles to suggest after `current` (up to `limit`).
///
/// `visitor_id` is reserved for the future personalized engine and unused in
/// the current content-based implementation.
pub(crate) async fn recommend(
    state: &AppState,
    _visitor_id: Option<Uuid>,
    current: &crate::model::FullDocument,
    current_tags: &[String],
    limit: usize,
) -> Result<Vec<PublishedPost>, RepositoryError> {
    let published = state.repo.list_published_with_tags().await?;
    Ok(rank_related(
        &published,
        current.document.id,
        current_tags,
        limit,
    ))
}

/// Content-based ranking: shared-tag count descending, then newest first,
/// capped at `limit` and excluding the article currently being read.
fn rank_related(
    published: &[PublishedPost],
    current_id: Uuid,
    current_tags: &[String],
    limit: usize,
) -> Vec<PublishedPost> {
    let mut ranked: Vec<(usize, &PublishedPost)> = published
        .iter()
        .filter(|p| p.id != current_id)
        .map(|p| (shared_tag_count(&p.tags, current_tags), p))
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.published_at_ms.cmp(&a.1.published_at_ms))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, p)| p.clone())
        .collect()
}

fn shared_tag_count(candidate: &[String], current: &[String]) -> usize {
    candidate.iter().filter(|t| current.contains(t)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(id: u32, tags: &[&str], published_at_ms: Option<i64>) -> PublishedPost {
        PublishedPost {
            id: Uuid::from_u128(id as u128),
            title: format!("Post {id}"),
            slug: format!("post-{id}"),
            published_at_ms,
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn excludes_current_and_ranks_by_shared_tags() {
        let current = post(1, &["tech"], Some(500));
        let published = vec![
            post(1, &["tech"], Some(500)),
            post(2, &["tech", "dev"], Some(400)),
            post(3, &["food"], Some(600)),
            post(4, &["tech"], Some(300)),
        ];
        let ranked = rank_related(&published, current.id, &current.tags, 10);
        let slugs: Vec<&str> = ranked.iter().map(|p| p.slug.as_str()).collect();
        // Posts 2 and 4 share one tag each (newest first: 2 then 4), post 3
        // shares none, and the current post never appears.
        assert_eq!(slugs, ["post-2", "post-4", "post-3"]);
    }

    #[test]
    fn backfills_with_most_recent_and_caps_at_limit() {
        let current = post(1, &["tech"], Some(500));
        let published = vec![
            post(2, &["food"], Some(900)),
            post(3, &["food"], Some(700)),
            post(4, &["tech"], Some(100)),
        ];
        // Tag match first (post 4), then the newer backfill (post 2), then
        // post 3; limited to the first two.
        let ranked = rank_related(&published, current.id, &current.tags, 2);
        let slugs: Vec<&str> = ranked.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["post-4", "post-2"]);
    }
}
