//! Comment service: creation, approval, feature-flag gating.

use std::sync::Arc;

use uuid::Uuid;

use crate::model::Comment;
use crate::repository::Repository;
use crate::services::ServiceError;

pub struct CommentService {
    repo: Arc<dyn Repository>,
}

impl CommentService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    /// List approved comments for a published article (by slug).
    pub async fn list_approved(&self, slug: &str) -> Result<Vec<Comment>, ServiceError> {
        if !self.repo.site_settings().await?.comments_enabled {
            return Ok(Vec::new());
        }
        let full = self
            .repo
            .get_published_by_slug(slug)
            .await?
            .ok_or_else(|| ServiceError::Validation("article not found".into()))?;
        Ok(self
            .repo
            .comments_for_document(full.document.id, Some("approved"))
            .await?)
    }

    /// Create a new comment (pending moderation).
    pub async fn create(
        &self,
        slug: &str,
        author_name: &str,
        body: &str,
    ) -> Result<Comment, ServiceError> {
        if !self.repo.site_settings().await?.comments_enabled {
            return Err(ServiceError::Validation("comments are disabled".into()));
        }
        let author = author_name.trim().to_string();
        let comment_body = body.trim().to_string();
        if author.is_empty() || comment_body.is_empty() {
            return Err(ServiceError::Validation(
                "name and comment are required".into(),
            ));
        }
        if comment_body.len() > 2000 {
            return Err(ServiceError::Validation("comment too long".into()));
        }
        let full = self
            .repo
            .get_published_by_slug(slug)
            .await?
            .ok_or_else(|| ServiceError::Validation("article not found".into()))?;
        Ok(self
            .repo
            .create_comment(full.document.id, &author, &comment_body)
            .await?)
    }

    /// Approve a pending comment.
    pub async fn approve(&self, comment_id: Uuid) -> Result<(), ServiceError> {
        self.repo.set_comment_status(comment_id, "approved").await?;
        Ok(())
    }

    /// List all pending comments (admin dashboard).
    pub async fn pending(&self) -> Result<Vec<Comment>, ServiceError> {
        Ok(self.repo.pending_comments().await?)
    }
}
