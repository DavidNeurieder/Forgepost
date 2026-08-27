//! HTTP routes: setup, auth, documents, public articles, comments, RSS, analytics.

use axum::{
    Json,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Response},
};
use forgepost_analytics::{DocumentStatsView, EventKind, SCROLL_BANDS};
use forgepost_application::experiments::{DecisionOutcome, ExperimentView, experiment_view};
use forgepost_application::ports::DocumentRepo;
use forgepost_content::{Document, now_ms, render_html};
use forgepost_experiments::{ExperimentId, VariantId, assign_variant};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{AuthUser, verify_csrf};
use crate::error::ApiError;
use crate::model::{AnalyticsEvent, DocumentSummary, FullDocument, PostId, User, VisitorId};

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
    /// Slug of the article shown/clicked in "Keep reading" (only for
    /// `recommendation_impression` / `recommendation_click`).
    #[serde(default)]
    recommended_slug: Option<String>,
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

#[tracing::instrument(skip(state, body))]
pub async fn setup(
    State(state): State<AppState>,
    Json(body): Json<SetupRequest>,
) -> Result<Response, ApiError> {
    let result = state
        .auth_service
        .setup(&body.email, &body.display_name, &body.password)
        .await?;
    let cookie = crate::auth::set_session_cookie_secure(&result.token, state.secure_cookies);
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(SessionResponse {
            user: state
                .repo
                .find_user_by_id(result.user_id)
                .await?
                .ok_or_else(|| ApiError::bad_request("user not found"))?
                .into(),
            csrf_token: result.csrf,
        }),
    )
        .into_response())
}

#[tracing::instrument(skip(state, headers, body))]
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let client = client_ip(&headers);
    let result = try_login(&state, &client, &body.email, &body.password).await?;
    let cookie = crate::auth::set_session_cookie_secure(&result.token, state.secure_cookies);
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(SessionResponse {
            user: state
                .repo
                .find_user_by_id(result.user_id)
                .await?
                .ok_or_else(|| ApiError::bad_request("user not found"))?
                .into(),
            csrf_token: result.csrf,
        }),
    )
        .into_response())
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let token = crate::auth::cookie(&headers, crate::auth::SESSION_COOKIE);
    if let Some(token) = token {
        state.auth_service.logout(&token).await?;
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

#[tracing::instrument(skip(state, headers, auth, body))]
pub async fn create_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(body): Json<CreateDocumentRequest>,
) -> Result<Json<DocumentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    tracing::info!(title = %body.title, "creating document");
    let tags = body.tags.as_deref();
    let result = state
        .document_service
        .create(auth.user.id, &body.title, body.markdown.as_deref(), tags)
        .await?;
    Ok(Json(doc_view(&result.full, result.tags)))
}

#[tracing::instrument(skip(state, auth))]
pub async fn get_document(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentView>, ApiError> {
    let id = parse_uuid(&id)?;
    let full = state.document_service.get_owned(id, auth.user.id).await?;
    let tags = state.repo.document_tags(id).await?;
    Ok(Json(doc_view(&full, tags)))
}

#[tracing::instrument(skip(state, headers, auth, body))]
pub async fn update_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateDocumentRequest>,
) -> Result<Json<DocumentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let title = body.title.as_deref();
    let tags = body.tags.as_deref().unwrap_or(&[]);
    let result = state
        .document_service
        .save(id, auth.user.id, title, body.markdown.as_deref(), tags)
        .await?;
    Ok(Json(doc_view(&result.full, result.tags)))
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn publish_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    tracing::info!(document_id = %id, "publishing document");
    let result = state.document_service.publish(id, auth.user.id).await?;
    Ok(Json(doc_view(&result.full, result.tags)))
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

#[tracing::instrument(skip(state, headers))]
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

#[tracing::instrument(skip(state))]
pub async fn list_comments(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<CommentView>>, ApiError> {
    let comments = state.comment_service.list_approved(&slug).await?;
    Ok(Json(comments.into_iter().map(comment_view).collect()))
}

#[tracing::instrument(skip(state, headers, body))]
pub async fn create_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(body): Json<CommentRequest>,
) -> Result<(StatusCode, Json<CommentView>), ApiError> {
    if !state
        .comment_rate_limiter
        .allow(&client_ip(&headers), now_ms())
    {
        return Err(ApiError::rate_limited());
    }
    let comment = state
        .comment_service
        .create(&slug, &body.author_name, &body.body)
        .await?;
    Ok((StatusCode::CREATED, Json(comment_view(comment))))
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn approve_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    state.comment_service.approve(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rss(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let site = state.repo.site_settings().await?;
    let base = crate::pages::canonical_base(&state, &site, &headers);
    let published = state.repo.list_published().await?;
    let mut items = String::new();
    for summary in published {
        if let Some(full) = state.repo.get_document(summary.id.0).await? {
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
                xml_escape(&url),
                xml_escape(&url),
                xml_escape(&text),
                pub_date,
            ));
        }
    }
    let base = xml_escape(&base);
    let feed = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\"><channel><title>{}</title><link>{}</link><description>{}</description><atom:link href=\"{}/rss\" rel=\"self\" type=\"application/rss+xml\"/>{items}</channel></rss>",
        xml_escape(&site.name),
        base,
        xml_escape(&site.tagline),
        base,
    );
    Ok((
        [(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")],
        feed,
    )
        .into_response())
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
#[tracing::instrument(skip(state, headers, body))]
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
        if exp.document_id.0 != document_id {
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

    // Recommendation events must name a published article other than the one
    // the reader is currently on (parsed above).
    if let Some(target) = parsed.recommended_slug.as_deref()
        && state.repo.get_published_by_slug(target).await?.is_none()
    {
        return Err(ApiError::bad_request("recommended article not found"));
    }

    let (visitor_id, cookie) = visitor_identity_with_secure(&headers, state.secure_cookies);
    let event = AnalyticsEvent {
        id: Uuid::new_v4(),
        document_id: PostId(document_id),
        event_type: parsed.event_type.into(),
        band: parsed.band,
        block_id: parsed.block_id,
        pageview_id: body.session_id,
        visitor_id: VisitorId(visitor_id),
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
        recommended_slug: parsed.recommended_slug,
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
#[tracing::instrument(skip(state, auth))]
pub async fn document_stats(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DocumentStatsView>, ApiError> {
    let id = parse_uuid(&id)?;
    let stats = state
        .analytics_service
        .document_stats(id, auth.user.id)
        .await?;
    Ok(Json(stats))
}

// ---------------------------------------------------------------------------
// Experiments (M3)
// ---------------------------------------------------------------------------

/// Admin: all experiments for a document with live reports.
pub async fn list_experiments(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ExperimentView>>, ApiError> {
    let id = parse_uuid(&id)?;
    let experiments = state
        .experiment_service
        .list_for_document(id, auth.user.id)
        .await?;
    let mut views = Vec::new();
    for exp in experiments {
        views.push(experiment_view(&*state.repo, &exp).await?);
    }
    Ok(Json(views))
}

/// Admin: create an experiment overlay on a block with one or more variants.
#[tracing::instrument(skip(state, headers, auth, body))]
pub async fn create_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(body): Json<CreateExperimentRequest>,
) -> Result<Json<ExperimentView>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let inputs: Vec<crate::model::ExperimentVariantInput> = body
        .variants
        .into_iter()
        .map(|v| crate::model::ExperimentVariantInput {
            content: v.content,
            weight: v.weight,
        })
        .collect();
    let exp = state
        .experiment_service
        .create(
            body.document_id,
            body.block_id,
            auth.user.id,
            &body.name,
            &body.goal,
            body.traffic_weight,
            body.confidence_threshold,
            body.min_sample_per_variant,
            body.no_winner_prob,
            body.max_duration_ms,
            inputs,
        )
        .await?;
    Ok(Json(experiment_view(&*state.repo, &exp).await?))
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn start_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    state.experiment_service.start(id, auth.user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn stop_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    state.experiment_service.stop(id, auth.user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn decide_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Option<DecisionOutcome>>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let outcome = state.experiment_service.decide(id, auth.user.id).await?;
    Ok(Json(outcome))
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn promote_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DecisionOutcome>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let outcome = state.experiment_service.promote(id, auth.user.id).await?;
    Ok(Json(outcome))
}

#[tracing::instrument(skip(state, headers, auth))]
pub async fn conclude_no_winner(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DecisionOutcome>, ApiError> {
    verify_csrf(&headers, &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let outcome = state
        .experiment_service
        .conclude_no_winner(id, auth.user.id)
        .await?;
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
    recommended_slug: Option<String>,
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
            recommended_slug: None,
        }),
        EventKind::BandedScroll => {
            let band = body
                .payload
                .get("band")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ApiError::bad_request("missing band"))?;
            if !SCROLL_BANDS.contains(&band) {
                return Err(ApiError::bad_request("invalid scroll band"));
            }
            Ok(ParsedEvent {
                event_type: "banded_scroll",
                band: Some(band),
                block_id: None,
                read_time_ms: None,
                experiment_id: None,
                variant_id: None,
                recommended_slug: None,
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
                recommended_slug: None,
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
                recommended_slug: None,
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
                recommended_slug: None,
            })
        }
        EventKind::RecommendationImpression | EventKind::RecommendationClick => {
            let recommended_slug = body
                .recommended_slug
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ApiError::bad_request("missing recommended_slug"))?;
            if recommended_slug == body.slug {
                return Err(ApiError::bad_request(
                    "recommended_slug cannot be the current article",
                ));
            }
            Ok(ParsedEvent {
                event_type: match body.kind {
                    EventKind::RecommendationImpression => "recommendation_impression",
                    _ => "recommendation_click",
                },
                band: None,
                block_id: None,
                read_time_ms: None,
                experiment_id: None,
                variant_id: None,
                recommended_slug: Some(recommended_slug.to_string()),
            })
        }
        EventKind::ShareClick => Ok(ParsedEvent {
            event_type: "share_click",
            band: None,
            block_id: None,
            read_time_ms: None,
            experiment_id: None,
            variant_id: None,
            recommended_slug: None,
        }),
        _ => Err(ApiError::bad_request("event kind not supported yet")),
    }
}

/// Internal header stamped by [`client_ip_mw`] with the effective client
/// identity. Handlers read this and never trust `x-forwarded-for` directly.
const REAL_IP_HEADER: &str = "x-real-ip";

/// Which reverse proxies may supply the `x-forwarded-for` value. The peer
/// address wins unless it is one of these, closing the "fresh bucket per
/// forged header" bypass.
#[derive(Debug, Clone, Default)]
pub struct ClientIpConfig {
    pub trusted_proxies: Vec<IpNet>,
}

impl ClientIpConfig {
    pub fn is_trusted_proxy(&self, ip: &std::net::IpAddr) -> bool {
        self.trusted_proxies.iter().any(|net| net.contains(ip))
    }
}

/// Resolve the effective client identity for rate limiting. A configured
/// trusted proxy's `x-forwarded-for` (first entry, the one the proxy set) is
/// honored; anything else falls back to the raw peer, and without a peer to
/// "unknown".
pub(crate) fn resolve_client_ip(
    cfg: &ClientIpConfig,
    headers: &HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> String {
    let peer_ip = peer.map(|s| s.ip());
    match peer_ip {
        Some(ip) if cfg.is_trusted_proxy(&ip) => headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| ip.to_string()),
        Some(ip) => ip.to_string(),
        None => "unknown".into(),
    }
}

/// Stamp [`REAL_IP_HEADER`] from the socket peer (or a trusted proxy's
/// `x-forwarded-for`) before handlers run. The header is always overwritten,
/// so a client-supplied value carries no weight.
pub(crate) async fn client_ip_mw(
    State(cfg): State<std::sync::Arc<ClientIpConfig>>,
    mut req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0);
    let ip = resolve_client_ip(&cfg, req.headers(), peer);
    let value = HeaderValue::from_str(&ip).unwrap_or_else(|_| HeaderValue::from_static("unknown"));
    req.headers_mut().insert(REAL_IP_HEADER, value);
    next.run(req).await
}

/// Client identity for rate limiting: the `x-real-ip` header set by
/// [`client_ip_mw`], falling back to a shared bucket.
pub(crate) fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get(REAL_IP_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Combined rate-limit key for a login attempt: client identity + normalized
/// account, so a distributed attack can't spread failures across IP buckets.
fn login_rate_key(client: &str, email: &str) -> String {
    format!("{client}|{}", email.trim().to_ascii_lowercase())
}

/// Rate-limited login shared by the JSON API and the page form. Failed attempts
/// consume a slot; successful logins don't, so a correct password is never
/// self-locked out of a window.
pub(crate) async fn try_login(
    state: &AppState,
    client: &str,
    email: &str,
    password: &str,
) -> Result<
    forgepost_application::services::auth::AuthResult,
    forgepost_application::services::ServiceError,
> {
    let key = login_rate_key(client, email);
    if !state.login_rate_limiter.peek(&key, now_ms()) {
        return Err(forgepost_application::services::ServiceError::RateLimited);
    }
    match state.auth_service.login(email, password).await {
        Ok(result) => Ok(result),
        Err(err) => {
            state.login_rate_limiter.record(&key, now_ms());
            Err(err)
        }
    }
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
        document_id: c.document_id.0,
        author_name: c.author_name,
        body: c.body,
        status: c.status,
        created_at_ms: c.created_at_ms,
    }
}

pub(crate) async fn apply_markdown(
    repo: &dyn DocumentRepo,
    doc: &mut Document,
    markdown: &str,
) -> Result<(), ApiError> {
    let mut parsed = forgepost_content::parse_markdown(markdown);
    forgepost_infrastructure::oembed::enrich_video_metadata(&mut parsed).await;
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

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
