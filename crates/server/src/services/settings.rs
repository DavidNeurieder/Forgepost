//! Settings service: validated read/write for blog-wide settings.

use std::sync::Arc;

use crate::model::SiteSettings;
use crate::repository::{Repository, SettingsRepo};
use crate::services::ServiceError;

pub struct SettingsService {
    repo: Arc<dyn SettingsRepo>,
}

impl SettingsService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self { repo }
    }

    /// Read the current site settings.
    pub async fn get(&self) -> Result<SiteSettings, ServiceError> {
        Ok(self.repo.site_settings().await?)
    }

    /// Validate and persist all settings from the admin form.
    pub async fn update(
        &self,
        name: &str,
        theme: &str,
        url: &str,
        tagline: &str,
        image: &str,
        comments_enabled: bool,
    ) -> Result<(), ServiceError> {
        let error = validate(name, theme, url, tagline, image);
        if !error.is_empty() {
            return Err(ServiceError::Validation(error));
        }
        self.repo.set_setting("site.name", name.trim()).await?;
        self.repo.set_setting("theme", theme).await?;
        self.repo.set_setting("site.url", url.trim()).await?;
        self.repo
            .set_setting("site.tagline", tagline.trim())
            .await?;
        self.repo.set_setting("site.image", image.trim()).await?;
        self.repo
            .set_setting("comments.enabled", if comments_enabled { "1" } else { "0" })
            .await?;
        Ok(())
    }
}

fn validate(name: &str, _theme: &str, _url: &str, _tagline: &str, _image: &str) -> String {
    if name.trim().is_empty() {
        return "Enter a blog name.".into();
    }
    String::new()
}
