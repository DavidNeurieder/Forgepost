//! HTTP routes: setup, auth, documents, public articles, comments, RSS, analytics.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use openpublish_analytics::EventKind;
use openpublish_content::{Document, now_ms, render_html};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::analytics::{block_stats, preview_text};
use crate::auth::{AuthUser, verify_csrf};
use crate::error::ApiError;
use crate::model::{AnalyticsEvent, DocumentSummary, FullDocument, User};
use crate::{AppState, repository::Repository};

/// Anonymous visitor cookie used to de-duplicate unique readers.
pub const VISITOR_COOKIE: &str = "opv";

// ---------------------------------------------------------------------------
// Response DTOs
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct SetupStatus {
    pub setup_complete: bool,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
}

impl From<User> for UserResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            display_name: u.display_name,
            role: u.role,
        }
    }
}

/// Session bootstrap: the caller's identity plus the CSRF token that must be
/// echoed back in the `x-csrf-token` header on mutating requests.
#[derive(Serialize)]
pub struct SessionResponse {
    pub user: UserResponse,
    pub csrf_token: String,
}

#[derive(Serialize)]
pub struct BlockView {
    pub id: Uuid,
    pub kind: String,
    pub content: serde_json::Value,
}

#[derive(Serialize)]
pub struct DocumentView {
    pub id: Uuid,
    pub title: String,
    pub slug: String,
    pub status: String,
    pub published_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub tags: Vec<String>,
    pub blocks: Vec<BlockView>,
}

#[derive(Serialize)]
pub struct ArticleView {
    pub id: Uuid,
    pub title: String,
    pub slug: String,
    pub published_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub tags: Vec<String>,
    pub blocks: Vec<BlockView>,
    pub html: String,
    /// Per-block rendered HTML so the client can attach tracking attributes.
    pub rendered_blocks: Vec<RenderedBlock>,
}

/// A single block rendered to HTML, keyed by its stable block id.
#[derive(Serialize)]
pub struct RenderedBlock {
    pub id: Uuid,
    pub kind: String,
    pub html: String,
}

#[derive(Serialize)]
pub struct CommentView {
    pub id: Uuid,
    pub document_id: Uuid,
    pub author_name: String,
    pub body: String,
    pub status: String,
    pub created_at_ms: i64,
}

/// A single block from a fresh Markdown parse (no stable identity yet).
#[derive(Serialize)]
pub struct ParsedBlockView {
    pub kind: String,
    pub content: serde_json::Value,
}

/// Result of rendering Markdown for live editor preview.
#[derive(Serialize)]
pub struct RenderView {
    pub html: String,
    pub blocks: Vec<ParsedBlockView>,
}

fn block_views(doc: &Document) -> Vec<BlockView> {
    doc.blocks
        .iter()
        .filter_map(|b| {
            doc.current_content(b.id).map(|content| BlockView {
                id: b.id,
                kind: format!("{:?}", b.kind),
                content: content.clone(),
            })
        })
        .collect()
}

fn doc_view(full: &FullDocument, tags: Vec<String>) -> DocumentView {
    DocumentView {
        id: full.document.id,
        title: full.document.title.clone(),
        slug: full.slug.clone(),
        status: full.status.clone(),
        published_at_ms: full.published_at_ms,
        updated_at_ms: full.document.updated_at_ms,
        tags,
        blocks: block_views(&full.document),
    }
}

fn article_view(full: &FullDocument, tags: Vec<String>) -> ArticleView {
    let block_refs: Vec<_> = full
        .document
        .blocks
        .iter()
        .filter_map(|b| full.document.current_content(b.id).map(|c| (b.kind, c)))
        .collect();
    let rendered_blocks = full
        .document
        .blocks
        .iter()
        .filter_map(|b| {
            full.document.current_content(b.id).map(|c| RenderedBlock {
                id: b.id,
                kind: format!("{:?}", b.kind),
                html: render_html([(b.kind, c)]),
            })
        })
        .collect();
    ArticleView {
        id: full.document.id,
        title: full.document.title.clone(),
        slug: full.slug.clone(),
        published_at_ms: full.published_at_ms,
        updated_at_ms: full.document.updated_at_ms,
        tags,
        blocks: block_views(&full.document),
        html: render_html(block_refs),
        rendered_blocks,
    }
}

// ---------------------------------------------------------------------------
// Request DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SetupRequest {
    email: String,
    password: String,
    display_name: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct CreateDocumentRequest {
    title: String,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct UpdateDocumentRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct CommentRequest {
    author_name: String,
    body: String,
}

#[derive(Deserialize)]
pub struct RenderRequest {
    markdown: String,
}

/// One analytics event from the browser tracker (public, unauthenticated).
#[derive(Deserialize)]
pub struct EventRequest {
    /// Slug of the published article the event concerns.
    slug: String,
    /// One per page load; ties scroll/read/impression events together.
    session_id: Uuid,
    kind: EventKind,
    #[serde(default)]
    block_id: Option<Uuid>,
    /// Kind-specific payload: `{"band": 75}` or `{"read_time_ms": 24000}`.
    #[serde(default)]
    payload: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

pub async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let setup_complete = state.repo.is_setup_complete().await?;
    Ok(Json(SetupStatus { setup_complete }))
}

pub async fn setup(
    State(state): State<AppState>,
    Json(body): Json<SetupRequest>,
) -> Result<Response, ApiError> {
    if state.repo.is_setup_complete().await? {
        return Err(ApiError::conflict("already set up"));
    }
    validate_credentials(&body.email, &body.password)?;
    let hash = crate::auth::hash_password(&body.password)?;
    let user = state
        .repo
        .create_first_user(&body.email, &body.display_name, &hash)
        .await?;
    let session = state.repo.create_session(user.id).await?;
    let cookie = crate::auth::set_session_cookie(&session.token);
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(SessionResponse {
            user: user.into(),
            csrf_token: session.csrf,
        }),
    )
        .into_response())
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let user = state
        .repo
        .find_user_by_email(&body.email)
        .await?
        .ok_or_else(|| ApiError::bad_request("invalid email or password"))?;
    if !crate::auth::verify_password(&user.password_hash, &body.password) {
        return Err(ApiError::bad_request("invalid email or password"));
    }
    let session = state.repo.create_session(user.id).await?;
    let cookie = crate::auth::set_session_cookie(&session.token);
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(SessionResponse {
            user: user.into(),
            csrf_token: session.csrf,
        }),
    )
        .into_response())
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let token = crate::auth::cookie(&headers, crate::auth::SESSION_COOKIE);
    if let Some(token) = token {
        state.repo.delete_session(&token).await?;
    }
    Ok((
        [(header::SET_COOKIE, crate::auth::clear_session_cookie())],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

pub async fn me(auth: AuthUser) -> Json<SessionResponse> {
    Json(SessionResponse {
        user: auth.user.into(),
        csrf_token: auth.csrf_token,
    })
}

pub async fn list_documents(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<DocumentSummary>>, ApiError> {
    let docs = state.repo.list_documents(auth.user.id).await?;
    Ok(Json(docs))
}

/// Public: published articles for the blog home page.
pub async fn list_articles(
    State(state): State<AppState>,
) -> Result<Json<Vec<DocumentSummary>>, ApiError> {
    let docs = state.repo.list_published().await?;
    Ok(Json(docs))
}

/// Authenticated: render Markdown for the editor's live preview. Read-only, so
/// no CSRF is required — the server parser stays the single source of truth
/// for what the published article will look like.
pub async fn render_markdown(
    _state: State<AppState>,
    _auth: AuthUser,
    Json(body): Json<RenderRequest>,
) -> Result<Json<RenderView>, ApiError> {
    let parsed = openpublish_content::parse_markdown(&body.markdown);
    let html = render_html(parsed.iter().map(|b| (b.kind, &b.content)));
    let blocks = parsed
        .into_iter()
        .map(|b| ParsedBlockView {
            kind: format!("{:?}", b.kind),
            content: b.content,
        })
        .collect();
    Ok(Json(RenderView { html, blocks }))
}

/// Authenticated: comments awaiting moderation across all documents.
pub async fn pending_comments(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<CommentView>>, ApiError> {
    let comments = state.repo.pending_comments().await?;
    Ok(Json(comments.into_iter().map(comment_view).collect()))
}

pub async fn create_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(body): Json<CreateDocumentRequest>,
) -> Result<Json<DocumentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::bad_request("title is required"));
    }
    let mut full = state.repo.create_document(auth.user.id, &title).await?;
    if let Some(markdown) = body.markdown {
        apply_markdown(&*state.repo, &mut full.document, &markdown).await?;
    }
    if let Some(tags) = body.tags {
        state
            .repo
            .set_document_tags(full.document.id, &tags)
            .await?;
    }
    let tags = state.repo.document_tags(full.document.id).await?;
    Ok(Json(doc_view(&full, tags)))
}

pub async fn get_document(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentView>, ApiError> {
    let id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }
    let tags = state.repo.document_tags(id).await?;
    Ok(Json(doc_view(&full, tags)))
}

pub async fn update_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateDocumentRequest>,
) -> Result<Json<DocumentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let mut full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }
    if let Some(title) = body.title {
        let title = title.trim().to_string();
        if !title.is_empty() {
            full.document.title = title.clone();
            state.repo.update_document_title(id, &title).await?;
        }
    }
    if let Some(markdown) = body.markdown {
        apply_markdown(&*state.repo, &mut full.document, &markdown).await?;
    }
    if let Some(tags) = body.tags {
        state.repo.set_document_tags(id, &tags).await?;
    }
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    let tags = state.repo.document_tags(id).await?;
    Ok(Json(doc_view(&full, tags)))
}

pub async fn publish_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }
    state.repo.publish_document(id).await?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    let tags = state.repo.document_tags(id).await?;
    Ok(Json(doc_view(&full, tags)))
}

pub async fn article(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<ArticleView>, ApiError> {
    let full = state
        .repo
        .get_published_by_slug(&slug)
        .await?
        .ok_or_else(|| ApiError::bad_request("article not found"))?;
    let tags = state.repo.document_tags(full.document.id).await?;
    Ok(Json(article_view(&full, tags)))
}

pub async fn list_comments(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<CommentView>>, ApiError> {
    let full = state
        .repo
        .get_published_by_slug(&slug)
        .await?
        .ok_or_else(|| ApiError::bad_request("article not found"))?;
    let comments = state
        .repo
        .comments_for_document(full.document.id, Some("approved"))
        .await?;
    Ok(Json(comments.into_iter().map(comment_view).collect()))
}

pub async fn create_comment(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<CommentRequest>,
) -> Result<(StatusCode, Json<CommentView>), ApiError> {
    let full = state
        .repo
        .get_published_by_slug(&slug)
        .await?
        .ok_or_else(|| ApiError::bad_request("article not found"))?;
    let author = body.author_name.trim().to_string();
    let comment_body = body.body.trim().to_string();
    if author.is_empty() || comment_body.is_empty() {
        return Err(ApiError::bad_request("name and comment are required"));
    }
    if comment_body.len() > 2000 {
        return Err(ApiError::bad_request("comment too long"));
    }
    let comment = state
        .repo
        .create_comment(full.document.id, &author, &comment_body)
        .await?;
    Ok((StatusCode::CREATED, Json(comment_view(comment))))
}

pub async fn approve_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    state.repo.set_comment_status(id, "approved").await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rss(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let published = state.repo.list_published().await?;
    let mut items = String::new();
    for summary in published {
        if let Some(full) = state.repo.get_document(summary.id).await? {
            let html = article_html(&full.document);
            let text: String = html.chars().filter(|c| !c.is_control()).take(500).collect();
            items.push_str(&format!(
                "<item><title>{}</title><link>https://example.invalid/{}</link><description>{}</description><pubDate>{}</pubDate></item>",
                xml_escape(&full.document.title),
                xml_escape(&summary.slug),
                xml_escape(&text),
                summary.published_at_ms.unwrap_or(0),
            ));
        }
    }
    let feed = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<rss version=\"2.0\"><channel><title>OpenPublish</title>{items}</channel></rss>"
    );
    Ok(Html(feed))
}

// ---------------------------------------------------------------------------
// Analytics (M2)
// ---------------------------------------------------------------------------

/// Collect a tracking event. Public write endpoint: rate-limited per client,
/// payload-validated, and identity comes from the anonymous `opv` cookie.
pub async fn record_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EventRequest>,
) -> Result<(StatusCode, HeaderMap), ApiError> {
    let client = client_ip(&headers);
    if !state.rate_limiter.allow(&client, now_ms()) {
        return Err(ApiError::rate_limited());
    }

    let full = state
        .repo
        .get_published_by_slug(&body.slug)
        .await?
        .ok_or_else(|| ApiError::bad_request("article not found"))?;
    let document_id = full.document.id;

    let (event_type, band, block_id, read_time_ms) = parse_event(&body)?;
    if let Some(bid) = block_id
        && full.document.block(bid).is_none()
    {
        return Err(ApiError::bad_request("unknown block"));
    }

    let (visitor_id, cookie) = visitor_identity(&headers);
    let event = AnalyticsEvent {
        id: Uuid::new_v4(),
        document_id,
        event_type,
        band,
        block_id,
        pageview_id: body.session_id,
        visitor_id,
        referrer: headers
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        user_agent: headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        read_time_ms,
        created_at_ms: now_ms(),
    };
    state.repo.record_analytics_event(&event).await?;

    let mut resp_headers = HeaderMap::new();
    if let Some(c) = cookie {
        resp_headers.insert(header::SET_COOKIE, c);
    }
    Ok((StatusCode::NO_CONTENT, resp_headers))
}

/// Per-document analytics for the dashboard: article-level aggregates plus
/// per-block reach/drop-off ("estimated" labeling; §5.2).
pub async fn document_stats(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<crate::analytics::DocumentStatsView>, ApiError> {
    let id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }

    let mut article = state.repo.article_stats(id).await?;
    let band_reach = state.repo.band_reach(id).await?;
    let impressions = state.repo.block_impressions(id).await?;
    article.band_reach = band_reach.clone();
    article.completion = band_reach
        .iter()
        .find(|b| b.band == 100)
        .map(|b| b.pageviews)
        .filter(|&completed| completed > 0)
        .map(|completed| completed as f64 / article.views.max(1) as f64);

    let mut blocks_sorted: Vec<&openpublish_content::Block> = full.document.blocks.iter().collect();
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
    Ok(Json(crate::analytics::DocumentStatsView {
        article,
        blocks,
    }))
}

/// Normalized event fields extracted from a validated request.
type ParsedEvent = (String, Option<i64>, Option<Uuid>, Option<i64>);

fn parse_event(body: &EventRequest) -> Result<ParsedEvent, ApiError> {
    match body.kind {
        EventKind::View => Ok(("view".into(), None, None, None)),
        EventKind::BandedScroll => {
            let band = body
                .payload
                .get("band")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ApiError::bad_request("missing band"))?;
            if !crate::analytics::SCROLL_BANDS.contains(&band) {
                return Err(ApiError::bad_request("invalid scroll band"));
            }
            Ok(("banded_scroll".into(), Some(band), None, None))
        }
        EventKind::ArticleRead => {
            let read_time_ms = body
                .payload
                .get("read_time_ms")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ApiError::bad_request("missing read_time_ms"))?;
            Ok(("article_read".into(), None, None, Some(read_time_ms)))
        }
        EventKind::BlockImpression => {
            let block_id = body
                .block_id
                .ok_or_else(|| ApiError::bad_request("missing block_id"))?;
            Ok(("block_impression".into(), None, Some(block_id), None))
        }
        _ => Err(ApiError::bad_request("event kind not supported yet")),
    }
}

/// Best-effort client identity: first `x-forwarded-for` entry (Vite/dev + proxy
/// deployments) falling back to a placeholder key.
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Read or mint the anonymous visitor id (`opv` cookie). Returns the visitor id
/// and, when a new cookie was minted, its `Set-Cookie` header.
fn visitor_identity(headers: &HeaderMap) -> (Uuid, Option<axum::http::HeaderValue>) {
    if let Some(v) = headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .and_then(|c| cookie_value(c, VISITOR_COOKIE))
        .and_then(|v| Uuid::parse_str(v).ok())
    {
        return (v, None);
    }
    let v = Uuid::new_v4();
    let cookie = axum::http::HeaderValue::from_str(&format!(
        "{VISITOR_COOKIE}={v}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        365 * 24 * 60 * 60
    ))
    .expect("visitor cookie is valid");
    (v, Some(cookie))
}

fn cookie_value<'a>(cookies: &'a str, name: &str) -> Option<&'a str> {
    cookies.split(';').find_map(|part| {
        let part = part.trim();
        let (k, v) = part.split_once('=')?;
        if k.trim() == name {
            Some(v.trim())
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn comment_view(c: crate::model::Comment) -> CommentView {
    CommentView {
        id: c.id,
        document_id: c.document_id,
        author_name: c.author_name,
        body: c.body,
        status: c.status,
        created_at_ms: c.created_at_ms,
    }
}

async fn apply_markdown(
    repo: &dyn Repository,
    doc: &mut Document,
    markdown: &str,
) -> Result<(), ApiError> {
    let parsed = openpublish_content::parse_markdown(markdown);
    let merged = openpublish_content::merge_blocks(
        &doc.blocks,
        &doc.versions,
        parsed,
        openpublish_content::now_ms(),
    );
    let versions = merged.versions.clone();
    repo.save_document_blocks(doc.id, &merged.blocks, &versions)
        .await?;
    doc.blocks = merged.blocks;
    doc.versions.extend(versions);
    Ok(())
}

fn article_html(doc: &Document) -> String {
    let block_refs: Vec<_> = doc
        .blocks
        .iter()
        .filter_map(|b| doc.current_content(b.id).map(|c| (b.kind, c)))
        .collect();
    render_html(block_refs)
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("invalid id"))
}

fn validate_credentials(email: &str, password: &str) -> Result<(), ApiError> {
    if !email.contains('@') || !email.contains('.') {
        return Err(ApiError::bad_request("invalid email address"));
    }
    if password.len() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ));
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
