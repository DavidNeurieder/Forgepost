//! Article service: public article rendering, experiment overlays,
//! recommendations, comments, SEO data.

use std::sync::Arc;

use uuid::Uuid;

use crate::repository::{DocumentRepo, ExperimentRepo, Repository, UserRepo};
use crate::services::ServiceError;

pub struct ArticleService {
    doc_repo: Arc<dyn DocumentRepo>,
    exp_repo: Arc<dyn ExperimentRepo>,
    user_repo: Arc<dyn UserRepo>,
}

impl ArticleService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self {
            doc_repo: repo.clone(),
            exp_repo: repo.clone(),
            user_repo: repo,
        }
    }

    /// Load a published article by slug.
    pub async fn get_by_slug(
        &self,
        slug: &str,
    ) -> Result<crate::model::FullDocument, ServiceError> {
        self.doc_repo
            .get_published_by_slug(slug)
            .await?
            .ok_or_else(|| ServiceError::NotFound("article not found".into()))
    }

    /// Load a document by id (any status).
    pub async fn get_by_id(&self, id: Uuid) -> Result<crate::model::FullDocument, ServiceError> {
        self.doc_repo
            .get_document(id)
            .await?
            .ok_or_else(|| ServiceError::NotFound("document not found".into()))
    }

    /// Tags for a document.
    pub async fn tags(&self, document_id: Uuid) -> Result<Vec<String>, ServiceError> {
        Ok(self.doc_repo.document_tags(document_id).await?)
    }

    /// Running experiments for a document.
    pub async fn running_experiments(
        &self,
        document_id: Uuid,
    ) -> Result<Vec<crate::model::ExperimentRecord>, ServiceError> {
        Ok(self
            .exp_repo
            .running_experiments_for_document(document_id)
            .await?)
    }

    /// User by id (for author name fallback).
    pub async fn user_by_id(&self, id: Uuid) -> Result<Option<crate::model::User>, ServiceError> {
        Ok(self.user_repo.find_user_by_id(id).await?)
    }

    /// All published posts (RSS, sitemap).
    pub async fn list_published(&self) -> Result<Vec<crate::model::DocumentSummary>, ServiceError> {
        Ok(self.doc_repo.list_published().await?)
    }

    /// All published posts with tags (blog home page).
    pub async fn list_published_with_tags(
        &self,
    ) -> Result<Vec<crate::model::PublishedPost>, ServiceError> {
        Ok(self.doc_repo.list_published_with_tags().await?)
    }

    /// All published posts tagged `tag`.
    pub async fn list_published_with_tag(
        &self,
        tag: &str,
    ) -> Result<Vec<crate::model::PublishedPost>, ServiceError> {
        Ok(self.doc_repo.list_published_with_tag(tag).await?)
    }

    /// Render article blocks as HTML.
    pub fn render_html(doc: &forgepost_content::Document) -> String {
        let block_refs: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| doc.current_content(b.id).map(|c| (b.kind, c)))
            .collect();
        forgepost_content::render_html(block_refs)
    }
}
