//! Document service: CRUD, markdown application, owner checks.

use std::sync::Arc;

use forgepost_content::{Document, now_ms};
use uuid::Uuid;

use crate::model::{DocumentSummary, FullDocument};
use crate::repository::{DocumentRepo, Repository};
use crate::services::ServiceError;

pub struct DocumentService {
    repo: Arc<dyn DocumentRepo>,
}

/// Result of saving a document: the full document (for re-rendering the view)
/// and its current tags.
pub struct SaveResult {
    pub full: FullDocument,
    pub tags: Vec<String>,
}

impl DocumentService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self {
            repo: repo as Arc<dyn DocumentRepo>,
        }
    }

    /// List all documents owned by `owner_id`.
    pub async fn list(&self, owner_id: Uuid) -> Result<Vec<DocumentSummary>, ServiceError> {
        Ok(self.repo.list_documents(owner_id).await?)
    }

    /// Create a new draft with an optional initial markdown body.
    pub async fn create(
        &self,
        owner_id: Uuid,
        title: &str,
        markdown: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<SaveResult, ServiceError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(ServiceError::Validation("title is required".into()));
        }
        let mut full = self.repo.create_document(owner_id, title).await?;
        if let Some(md) = markdown {
            apply_markdown(&*self.repo, &mut full.document, md).await?;
        }
        if let Some(tags) = tags {
            self.repo.set_document_tags(full.document.id, tags).await?;
        }
        let tags = self.repo.document_tags(full.document.id).await?;
        Ok(SaveResult { full, tags })
    }

    /// Fetch a document and verify ownership.
    pub async fn get_owned(&self, id: Uuid, owner_id: Uuid) -> Result<FullDocument, ServiceError> {
        let full = self
            .repo
            .get_document(id)
            .await?
            .ok_or_else(|| ServiceError::Validation("document not found".into()))?;
        if full.owner_id != owner_id {
            return Err(ServiceError::Forbidden);
        }
        Ok(full)
    }

    /// Save title + markdown + tags for a document.
    pub async fn save(
        &self,
        id: Uuid,
        owner_id: Uuid,
        title: Option<&str>,
        markdown: Option<&str>,
        tags: &[String],
    ) -> Result<SaveResult, ServiceError> {
        let mut full = self.get_owned(id, owner_id).await?;
        if let Some(title) = title {
            let title = title.trim().to_string();
            if !title.is_empty() {
                full.document.title = title.clone();
                self.repo.update_document_title(id, &title).await?;
            }
        }
        if let Some(md) = markdown {
            apply_markdown(&*self.repo, &mut full.document, md).await?;
        }
        self.repo.set_document_tags(id, tags).await?;
        let tags = self.repo.document_tags(id).await?;
        let full = self
            .repo
            .get_document(id)
            .await?
            .ok_or_else(|| ServiceError::Validation("document not found".into()))?;
        Ok(SaveResult { full, tags })
    }

    /// Save from the editor form: title, markdown, comma-separated tags.
    /// Also regenerates the draft slug when the document is still a draft.
    pub async fn editor_save(
        &self,
        id: Uuid,
        owner_id: Uuid,
        title: &str,
        markdown: &str,
        tags_str: &str,
    ) -> Result<(), ServiceError> {
        let mut full = self.get_owned(id, owner_id).await?;
        let title = title.trim().to_string();
        if !title.is_empty() {
            self.repo.update_document_title(id, &title).await?;
            if full.status == "draft" {
                self.repo.regenerate_draft_slug(id, &title).await?;
            }
        }
        apply_markdown(&*self.repo, &mut full.document, markdown).await?;
        let tags: Vec<String> = tags_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.repo.set_document_tags(id, &tags).await?;
        Ok(())
    }

    /// Publish a draft.
    pub async fn publish(&self, id: Uuid, owner_id: Uuid) -> Result<SaveResult, ServiceError> {
        let _full = self.get_owned(id, owner_id).await?;
        self.repo.publish_document(id).await?;
        let full = self
            .repo
            .get_document(id)
            .await?
            .ok_or_else(|| ServiceError::Validation("document not found".into()))?;
        let tags = self.repo.document_tags(id).await?;
        Ok(SaveResult { full, tags })
    }

    /// Permanently delete a document.
    pub async fn delete(&self, id: Uuid, owner_id: Uuid) -> Result<(), ServiceError> {
        let _full = self.get_owned(id, owner_id).await?;
        self.repo.delete_document(id).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Markdown application (shared by DocumentService and ArticleService)
// ---------------------------------------------------------------------------

pub(crate) async fn apply_markdown(
    repo: &dyn DocumentRepo,
    doc: &mut Document,
    markdown: &str,
) -> Result<(), ServiceError> {
    let mut parsed = forgepost_content::parse_markdown(markdown);
    crate::oembed::enrich_video_metadata(&mut parsed).await;
    let merged = forgepost_content::merge_blocks(&doc.blocks, &doc.versions, parsed, now_ms());
    let versions = merged.versions.clone();
    repo.save_document_blocks(doc.id, &merged.blocks, &versions)
        .await?;
    doc.blocks = merged.blocks;
    doc.versions.extend(versions);
    Ok(())
}
