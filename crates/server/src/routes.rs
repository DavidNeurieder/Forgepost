//! HTTP routes: setup, auth, documents, public articles, comments, RSS, analytics.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use forgepost_analytics::EventKind;
use forgepost_content::{Document, now_ms, render_html};
use forgepost_experiments::{ExperimentId, VariantId, assign_variant};
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

/// A single block rendered to HTML, keyed by its stable block id. When the
/// block is the subject of a running experiment the served markup is the
/// assigned variant's, and the experiment/variant ids are attached so the
/// tracker can report impressions and conversions (§5.1 overlays).
#[derive(Serialize)]
pub struct RenderedBlock {
    pub id: Uuid,
    pub kind: String,
    pub html: String,
    pub experiment_id: Option<Uuid>,
    pub variant_id: Option<Uuid>,
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

pub(crate) fn article_view(full: &FullDocument, tags: Vec<String>) -> ArticleView {
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
                experiment_id: None,
                variant_id: None,
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

/// Resolve which variant each block in the document shows for `visitor`.
/// Assignment is deterministic per (experiment, visitor), so a reader sees the
/// same headline/CTA across reloads. Only the first running experiment per
/// block participates (one experiment per block at a time).
pub(crate) fn assigned_variants(
    experiments: &[crate::model::ExperimentRecord],
    visitor_id: Uuid,
) -> std::collections::HashMap<Uuid, (ExperimentId, VariantId)> {
    let mut map = std::collections::HashMap::new();
    for exp in experiments {
        let control = exp
            .variants
            .iter()
            .find(|v| v.is_control)
            .expect("experiment always has a control variant");
        let others: Vec<(VariantId, f64)> = exp
            .variants
            .iter()
            .filter(|v| !v.is_control)
            .map(|v| (v.id, v.weight))
            .collect();
        let control_share = 1.0 - (exp.traffic_weight / 100.0).clamp(0.0, 1.0);
        let chosen = assign_variant(&exp.id, &visitor_id, control.id, control_share, &others);
        map.entry(exp.block_id).or_insert((exp.id, chosen));
    }
    map
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
    /// Required for `experiment_impression` / `experiment_conversion`.
    #[serde(default)]
    experiment_id: Option<Uuid>,
    #[serde(default)]
    variant_id: Option<Uuid>,
}

/// A new experiment: an overlay on one block with one or more content variants.
#[derive(Deserialize)]
pub struct CreateExperimentRequest {
    document_id: Uuid,
    block_id: Uuid,
    #[serde(default)]
    name: String,
    /// `completion` is the only goal in the MVP.
    #[serde(default = "default_goal")]
    goal: String,
    /// Percentage of visitors who see a variant (the rest see control).
    #[serde(default = "default_traffic_weight")]
    traffic_weight: f64,
    #[serde(default = "default_confidence")]
    confidence_threshold: f64,
    #[serde(default = "default_min_sample")]
    min_sample_per_variant: u64,
    #[serde(default = "default_no_winner")]
    no_winner_prob: f64,
    #[serde(default = "default_max_duration")]
    max_duration_ms: i64,
    variants: Vec<CreateExperimentVariantRequest>,
}

#[derive(Deserialize)]
pub struct CreateExperimentVariantRequest {
    /// Markdown-free structured content for this variant's block kind.
    content: serde_json::Value,
    #[serde(default = "default_weight")]
    weight: f64,
}

fn default_goal() -> String {
    "completion".into()
}
fn default_traffic_weight() -> f64 {
    50.0
}
fn default_confidence() -> f64 {
    0.95
}
fn default_min_sample() -> u64 {
    100
}
fn default_no_winner() -> f64 {
    0.05
}
fn default_max_duration() -> i64 {
    30 * 24 * 60 * 60 * 1000
}
fn default_weight() -> f64 {
    50.0
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
    let cookie = crate::auth::set_session_cookie_secure(&session.token, state.secure_cookies);
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
    let cookie = crate::auth::set_session_cookie_secure(&session.token, state.secure_cookies);
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
        [(
            header::SET_COOKIE,
            crate::auth::clear_session_cookie_secure(state.secure_cookies),
        )],
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
    let parsed = forgepost_content::parse_markdown(&body.markdown);
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

/// Overlay running experiments onto a rendered article view: swap each block
/// that is the subject of a running experiment to its assigned variant's markup
/// and attach the experiment/variant ids so the tracker can report impressions
/// and conversions (§5.1 overlays).
pub(crate) fn apply_assignments(
    full: &FullDocument,
    experiments: &[crate::model::ExperimentRecord],
    visitor_id: Uuid,
    view: &mut ArticleView,
) {
    let assigned = assigned_variants(experiments, visitor_id);
    for block in view.rendered_blocks.iter_mut() {
        let Some((exp_id, variant_id)) = assigned.get(&block.id) else {
            continue;
        };
        let exp = experiments
            .iter()
            .find(|e| e.id == *exp_id)
            .expect("assignment references a running experiment");
        let Some(variant) = exp.variants.iter().find(|v| v.id == *variant_id) else {
            continue;
        };
        // Render the variant's immutable version content, not the canonical one.
        if let (Some(content), Some(b)) = (
            full.document.version(variant.version_id),
            full.document.block(block.id),
        ) {
            block.html = render_html([(b.kind, &content.content)]);
            block.experiment_id = Some(*exp_id);
            block.variant_id = Some(*variant_id);
        }
    }
}

pub async fn article(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let full = state
        .repo
        .get_published_by_slug(&slug)
        .await?
        .ok_or_else(|| ApiError::bad_request("article not found"))?;
    let tags = state.repo.document_tags(full.document.id).await?;

    // Stable per-visitor assignment needs the anonymous visitor id, so mint the
    // cookie here if the reader does not have one yet.
    let (visitor_id, cookie) = visitor_identity_with_secure(&headers, state.secure_cookies);
    let experiments = state
        .repo
        .running_experiments_for_document(full.document.id)
        .await?;

    let mut view = article_view(&full, tags);
    apply_assignments(&full, &experiments, visitor_id, &mut view);

    let mut resp_headers = HeaderMap::new();
    if let Some(c) = cookie {
        resp_headers.insert(header::SET_COOKIE, c);
    }
    Ok((resp_headers, Json(view)).into_response())
}

pub async fn list_comments(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<CommentView>>, ApiError> {
    if !state.repo.site_settings().await?.comments_enabled {
        return Ok(Json(Vec::new()));
    }
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
    if !state.repo.site_settings().await?.comments_enabled {
        return Err(ApiError::bad_request("comments are disabled"));
    }
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

pub async fn rss(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let site = state.repo.site_settings().await?;
    let base = crate::pages::canonical_base(&state, &site, &headers);
    let published = state.repo.list_published().await?;
    let mut items = String::new();
    for summary in published {
        if let Some(full) = state.repo.get_document(summary.id).await? {
            let html = article_html(&full.document);
            let text: String = html.chars().filter(|c| !c.is_control()).take(500).collect();
            let url = format!("{base}/articles/{}", xml_escape(&summary.slug));
            let pub_date = summary
                .published_at_ms
                .map(crate::pages::format_rfc822)
                .unwrap_or_default();
            items.push_str(&format!(
                "<item><title>{}</title><link>{}</link><guid isPermaLink=\"true\">{}</guid><description>{}</description><pubDate>{}</pubDate></item>",
                xml_escape(&full.document.title),
                url,
                url,
                xml_escape(&text),
                pub_date,
            ));
        }
    }
    let feed = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\"><channel><title>{}</title><link>{}</link><description>{}</description><atom:link href=\"{}/rss\" rel=\"self\" type=\"application/rss+xml\"/>{items}</channel></rss>",
        xml_escape(&site.name),
        base,
        xml_escape(&site.tagline),
        base,
    );
    Ok(Html(feed))
}

pub async fn robots_txt(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let site = state.repo.site_settings().await?;
    let base = crate::pages::canonical_base(&state, &site, &headers);
    Ok(Html(format!(
        "User-agent: *\nAllow: /\nSitemap: {base}/sitemap.xml\n"
    )))
}

pub async fn sitemap_xml(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let site = state.repo.site_settings().await?;
    let base = crate::pages::canonical_base(&state, &site, &headers);
    let published = state.repo.list_published().await?;
    let mut urls = format!("<url><loc>{}/</loc></url>", base);
    for summary in published {
        urls.push_str(&format!(
            "<url><loc>{base}/articles/{}</loc><lastmod>{}</lastmod></url>",
            xml_escape(&summary.slug),
            crate::pages::format_iso_utc(summary.published_at_ms.unwrap_or(0)),
        ));
    }
    Ok(Html(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{urls}</urlset>"
    )))
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

    let parsed = parse_event(&body)?;
    if let Some(bid) = parsed.block_id
        && full.document.block(bid).is_none()
    {
        return Err(ApiError::bad_request("unknown block"));
    }
    // Experiment events must reference a running experiment that owns the
    // variant and belongs to this article.
    if let (Some(exp_id), Some(variant_id)) = (parsed.experiment_id, parsed.variant_id) {
        let exp = state
            .repo
            .experiment(exp_id)
            .await?
            .ok_or_else(|| ApiError::bad_request("unknown experiment"))?;
        if exp.status != "running" {
            return Err(ApiError::bad_request("experiment is not running"));
        }
        if exp.document_id != document_id {
            return Err(ApiError::bad_request(
                "experiment belongs to another article",
            ));
        }
        if !state
            .repo
            .experiment_variant_belongs(exp_id, variant_id)
            .await?
        {
            return Err(ApiError::bad_request(
                "variant does not belong to experiment",
            ));
        }
    }

    let (visitor_id, cookie) = visitor_identity_with_secure(&headers, state.secure_cookies);
    let event = AnalyticsEvent {
        id: Uuid::new_v4(),
        document_id,
        event_type: parsed.event_type.into(),
        band: parsed.band,
        block_id: parsed.block_id,
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
        read_time_ms: parsed.read_time_ms,
        experiment_id: parsed.experiment_id,
        variant_id: parsed.variant_id,
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

    let mut blocks_sorted: Vec<&forgepost_content::Block> = full.document.blocks.iter().collect();
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

// ---------------------------------------------------------------------------
// Experiments (M3)
// ---------------------------------------------------------------------------

/// Admin: all experiments for a document with live reports.
pub async fn list_experiments(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::experiments::ExperimentView>>, ApiError> {
    let id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }
    let experiments = state.repo.experiments_for_document(id).await?;
    let mut views = Vec::new();
    for exp in experiments {
        views.push(crate::experiments::experiment_view(&*state.repo, &exp).await?);
    }
    Ok(Json(views))
}

/// Admin: create an experiment overlay on a block with one or more variants.
pub async fn create_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(body): Json<CreateExperimentRequest>,
) -> Result<Json<crate::experiments::ExperimentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let full = state
        .repo
        .get_document(body.document_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }
    let block = full
        .document
        .block(body.block_id)
        .ok_or_else(|| ApiError::bad_request("block not found in document"))?;
    if !block.kind.is_experimentable() {
        return Err(ApiError::bad_request(
            "this block kind cannot be tested (use a heading, paragraph, image, or CTA)",
        ));
    }
    if body.variants.is_empty() {
        return Err(ApiError::bad_request("at least one variant is required"));
    }
    if body.variants.iter().any(|v| v.weight <= 0.0) {
        return Err(ApiError::bad_request("variant weights must be positive"));
    }
    if !(0.0..=100.0).contains(&body.traffic_weight) {
        return Err(ApiError::bad_request("traffic weight must be 0–100"));
    }

    let inputs: Vec<crate::model::ExperimentVariantInput> = body
        .variants
        .into_iter()
        .map(|v| crate::model::ExperimentVariantInput {
            content: v.content,
            weight: v.weight,
        })
        .collect();
    let exp = state
        .repo
        .create_experiment(
            body.document_id,
            body.block_id,
            &crate::model::NewExperiment {
                name: body.name.trim().to_string(),
                goal: body.goal,
                traffic_weight: body.traffic_weight,
                confidence_threshold: body.confidence_threshold,
                min_sample_per_variant: body.min_sample_per_variant,
                no_winner_prob: body.no_winner_prob,
                max_duration_ms: body.max_duration_ms,
                variants: inputs,
            },
        )
        .await?;
    Ok(Json(
        crate::experiments::experiment_view(&*state.repo, &exp).await?,
    ))
}

/// Load an experiment and verify the caller owns its document.
async fn owned_experiment(
    state: &AppState,
    auth: &AuthUser,
    id: Uuid,
) -> Result<crate::model::ExperimentRecord, ApiError> {
    let exp = state
        .repo
        .experiment(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("experiment not found"))?;
    let full = state
        .repo
        .get_document(exp.document_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden());
    }
    Ok(exp)
}

pub async fn start_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    owned_experiment(&state, &auth, id).await?;
    state.repo.start_experiment(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn stop_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    owned_experiment(&state, &auth, id).await?;
    state.repo.stop_experiment(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Run the sequential-test rules now (normally the background auto-decider
/// does this). Applies a decision if the rules fire.
pub async fn decide_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Option<crate::experiments::DecisionOutcome>>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    owned_experiment(&state, &auth, id).await?;
    let outcome = crate::experiments::decide_experiment(&*state.repo, id).await?;
    Ok(Json(outcome))
}

/// Manual override: promote the current best variant immediately.
pub async fn promote_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<crate::experiments::DecisionOutcome>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    owned_experiment(&state, &auth, id).await?;
    let outcome = crate::experiments::promote_experiment(&*state.repo, id).await?;
    Ok(Json(outcome))
}

/// Manual override: conclude "no improvement" without promoting.
pub async fn conclude_no_winner(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<crate::experiments::DecisionOutcome>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    owned_experiment(&state, &auth, id).await?;
    let outcome = crate::experiments::conclude_no_winner(&*state.repo, id).await?;
    Ok(Json(outcome))
}

/// Normalized event fields extracted from a validated request.
struct ParsedEvent {
    event_type: &'static str,
    band: Option<i64>,
    block_id: Option<Uuid>,
    read_time_ms: Option<i64>,
    experiment_id: Option<Uuid>,
    variant_id: Option<Uuid>,
}

fn parse_event(body: &EventRequest) -> Result<ParsedEvent, ApiError> {
    match body.kind {
        EventKind::View => Ok(ParsedEvent {
            event_type: "view",
            band: None,
            block_id: None,
            read_time_ms: None,
            experiment_id: None,
            variant_id: None,
        }),
        EventKind::BandedScroll => {
            let band = body
                .payload
                .get("band")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ApiError::bad_request("missing band"))?;
            if !crate::analytics::SCROLL_BANDS.contains(&band) {
                return Err(ApiError::bad_request("invalid scroll band"));
            }
            Ok(ParsedEvent {
                event_type: "banded_scroll",
                band: Some(band),
                block_id: None,
                read_time_ms: None,
                experiment_id: None,
                variant_id: None,
            })
        }
        EventKind::ArticleRead => {
            let read_time_ms = body
                .payload
                .get("read_time_ms")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ApiError::bad_request("missing read_time_ms"))?;
            Ok(ParsedEvent {
                event_type: "article_read",
                band: None,
                block_id: None,
                read_time_ms: Some(read_time_ms),
                experiment_id: None,
                variant_id: None,
            })
        }
        EventKind::BlockImpression => {
            let block_id = body
                .block_id
                .ok_or_else(|| ApiError::bad_request("missing block_id"))?;
            Ok(ParsedEvent {
                event_type: "block_impression",
                band: None,
                block_id: Some(block_id),
                read_time_ms: None,
                experiment_id: None,
                variant_id: None,
            })
        }
        EventKind::ExperimentImpression | EventKind::ExperimentConversion => {
            let experiment_id = body
                .experiment_id
                .ok_or_else(|| ApiError::bad_request("missing experiment_id"))?;
            let variant_id = body
                .variant_id
                .ok_or_else(|| ApiError::bad_request("missing variant_id"))?;
            Ok(ParsedEvent {
                event_type: match body.kind {
                    EventKind::ExperimentImpression => "experiment_impression",
                    _ => "experiment_conversion",
                },
                band: None,
                block_id: None,
                read_time_ms: None,
                experiment_id: Some(experiment_id),
                variant_id: Some(variant_id),
            })
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

/// `visitor_identity`, with the option to set the `Secure` flag on the minted
/// cookie (used once HTTPS is active).
pub(crate) fn visitor_identity_with_secure(
    headers: &HeaderMap,
    secure: bool,
) -> (Uuid, Option<axum::http::HeaderValue>) {
    if let Some(v) = headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .and_then(|c| cookie_value(c, VISITOR_COOKIE))
        .and_then(|v| Uuid::parse_str(v).ok())
    {
        return (v, None);
    }
    let v = Uuid::new_v4();
    let secure = if secure { "; Secure" } else { "" };
    let cookie = axum::http::HeaderValue::from_str(&format!(
        "{VISITOR_COOKIE}={v}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{secure}",
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

pub(crate) async fn apply_markdown(
    repo: &dyn Repository,
    doc: &mut Document,
    markdown: &str,
) -> Result<(), ApiError> {
    let parsed = forgepost_content::parse_markdown(markdown);
    let merged = forgepost_content::merge_blocks(
        &doc.blocks,
        &doc.versions,
        parsed,
        forgepost_content::now_ms(),
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
