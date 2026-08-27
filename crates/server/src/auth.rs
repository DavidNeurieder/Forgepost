//! Auth: argon2 password hashing, server-side sessions, CSRF, cookies, and the
//! `AuthUser` request extractor.

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, HeaderValue, header, request::Parts},
};
use forgepost_content::now_ms;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use forgepost_domain::model::{Session, User};

pub const SESSION_COOKIE: &str = "forgepost_session";
pub const CSRF_HEADER: &str = "x-csrf-token";

// Password-hashing helpers live in the domain crate (shared with the
// application and infrastructure layers); re-exported here so callers keep
// their `crate::auth::` paths.
pub use forgepost_domain::security::{SESSION_TTL_MS, hash_password, sha256_hex, verify_password};

/// Read a cookie from a request's headers.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in value.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=')
            && k.trim() == name
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Set the session cookie. `secure` appends the `Secure` flag so the cookie is
/// only ever sent over TLS (used once HTTPS is active).
pub fn set_session_cookie(token: &str) -> HeaderValue {
    set_session_cookie_secure(token, false)
}

pub fn set_session_cookie_secure(token: &str, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{secure}",
        SESSION_TTL_MS / 1000
    ))
    .expect("cookie value is valid")
}

pub fn clear_session_cookie() -> HeaderValue {
    clear_session_cookie_secure(false)
}

pub fn clear_session_cookie_secure(secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure}"
    ))
    .expect("cookie value is valid")
}

/// Require the CSRF header to match the session's token (mutating requests).
pub fn verify_csrf(headers: &HeaderMap, session_csrf: &str) -> Result<(), ApiError> {
    verify_csrf_form(headers, None, session_csrf)
}

/// Require a CSRF token to match the session's. The token may arrive in the
/// `x-csrf-token` header (API clients) or a hidden form field (server-rendered
/// pages).
pub fn verify_csrf_form(
    headers: &HeaderMap,
    form_token: Option<&str>,
    session_csrf: &str,
) -> Result<(), ApiError> {
    let header = headers
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if header == session_csrf || form_token == Some(session_csrf) {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

/// Authenticated user extracted from the session cookie.
#[derive(Clone)]
pub struct AuthUser {
    pub user: User,
    pub csrf_token: String,
}

/// Resolve the session cookie to an `AuthUser`, or `None` when there is no
/// valid session. Page handlers use this so they can decide to redirect to
/// `/login` instead of returning a 401 JSON error.
pub async fn session_user(state: &AppState, headers: &HeaderMap) -> Option<AuthUser> {
    let token = cookie(headers, SESSION_COOKIE)?;
    let session = state.repo.session_by_token(&token).await.ok()??;
    let user = state.repo.find_user_by_id(session.user_id).await.ok()??;
    Some(AuthUser {
        user,
        csrf_token: session.csrf,
    })
}

impl AuthUser {
    pub fn new_session(user: User) -> Session {
        Session {
            token: Uuid::new_v4().to_string(),
            csrf: Uuid::new_v4().to_string(),
            user_id: user.id,
            expires_at_ms: now_ms() + SESSION_TTL_MS,
        }
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or_else(ApiError::unauthorized)?;
        let session = state
            .repo
            .session_by_token(&token)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(ApiError::unauthorized)?;
        let user = state
            .repo
            .find_user_by_id(session.user_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(ApiError::unauthorized)?;
        Ok(AuthUser {
            user,
            csrf_token: session.csrf,
        })
    }
}
