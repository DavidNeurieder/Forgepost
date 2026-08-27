//! Authentication service: login, logout, setup.

use std::sync::Arc;

use uuid::Uuid;

use crate::ports::{Repository, SessionRepo, UserRepo};
use crate::services::ServiceError;
use forgepost_domain::security::{hash_password, verify_password};

pub struct AuthService {
    repo: Arc<dyn UserRepo>,
    sessions: Arc<dyn SessionRepo>,
}

/// Result of a successful login/setup: the session token, CSRF token, and the
/// user id so the caller can set the cookie and build the response.
pub struct AuthResult {
    pub user_id: Uuid,
    pub token: String,
    pub csrf: String,
}

impl AuthService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        let user_repo: Arc<dyn UserRepo> = repo.clone();
        let session_repo: Arc<dyn SessionRepo> = repo;
        Self {
            repo: user_repo,
            sessions: session_repo,
        }
    }

    /// Whether the very first user has been created.
    pub async fn is_setup_complete(&self) -> Result<bool, ServiceError> {
        Ok(self.repo.is_setup_complete().await?)
    }

    /// Create the first admin user (setup wizard).
    pub async fn setup(
        &self,
        email: &str,
        display_name: &str,
        password: &str,
    ) -> Result<AuthResult, ServiceError> {
        if self.repo.is_setup_complete().await? {
            return Err(ServiceError::Conflict("already set up".into()));
        }
        validate_credentials(email, password)?;
        let hash = hash_password(password)
            .map_err(|_| ServiceError::Internal("could not hash password".into()))?;
        let user = self
            .repo
            .create_first_user(email, display_name, &hash)
            .await?;
        let session = self.sessions.create_session(user.id).await?;
        Ok(AuthResult {
            user_id: user.id,
            token: session.token,
            csrf: session.csrf,
        })
    }

    /// Validate credentials and create a session.
    pub async fn login(&self, email: &str, password: &str) -> Result<AuthResult, ServiceError> {
        let user = self
            .repo
            .find_user_by_email(email)
            .await?
            .ok_or_else(|| ServiceError::Validation("invalid email or password".into()))?;
        if !verify_password(&user.password_hash, password) {
            return Err(ServiceError::Validation("invalid email or password".into()));
        }
        let session = self.sessions.create_session(user.id).await?;
        Ok(AuthResult {
            user_id: user.id,
            token: session.token,
            csrf: session.csrf,
        })
    }

    /// Delete the session for `token` (logout).
    pub async fn logout(&self, token: &str) -> Result<(), ServiceError> {
        self.sessions.delete_session(token).await?;
        Ok(())
    }
}

fn validate_credentials(email: &str, password: &str) -> Result<(), ServiceError> {
    if !email.contains('@') || !email.contains('.') {
        return Err(ServiceError::Validation("invalid email address".into()));
    }
    if password.len() < 8 {
        return Err(ServiceError::Validation(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}
