//! Server-rendered pages (single binary): home, setup, login, dashboard,
//! editor, stats, article, and comments.
//!
//! Pages use POST-REDIRECT-GET: mutating forms redirect to the page with a
//! `?flash=key` so the "Saved"/"Published"/"comment awaiting moderation"
//! messages survive the reload and a refresh never resubmits the form. All
//! admin forms carry a hidden `csrf_token` field verified against the session.

use askama::Template;
use axum::Json;
use axum::extract::{Form, Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use forgepost_content::{BlockKind, now_ms};
use forgepost_experiments::Recommendation;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{self, AuthUser};
use crate::error::{ApiError, PageError};
use crate::experiments::ExperimentView;
use crate::model::{Media, SiteSettings};
use crate::repository::RepositoryError;

// ---------------------------------------------------------------------------
// Template structs
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    seo: SeoMeta,
    posts: Vec<HomePost>,
}

struct HomePost {
    title: String,
    slug: String,
    date: String,
}

/// SEO metadata rendered into `<head>` (canonical, Open Graph, Twitter,
/// meta description, and JSON-LD).
struct SeoMeta {
    title: String,
    description: String,
    url: String,
    image: String,
    date_published: String,
    date_modified: String,
    author: String,
}

#[derive(Template)]
#[template(path = "setup.html")]
struct SetupTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    error: String,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    error: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    display_name: String,
    csrf_token: String,
    docs: Vec<DashboardDoc>,
    pending: Vec<PendingComment>,
}

struct DashboardDoc {
    id: String,
    title: String,
    status: String,
    updated: String,
}

struct PendingComment {
    id: String,
    author_name: String,
    body: String,
    short_id: String,
}

#[derive(Template)]
#[template(path = "editor.html")]
struct EditorTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    id: String,
    slug: String,
    title: String,
    tags: String,
    markdown: String,
    status: String,
    csrf_token: String,
}

#[derive(Template)]
#[template(path = "stats.html")]
struct StatsTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    doc_id: String,
    csrf_token: String,
    views: i64,
    unique_readers: i64,
    avg_read_time: String,
    completion: String,
    funnel: Vec<FunnelRow>,
    blocks: Vec<BlockStatRow>,
    experiments: Vec<ExperimentRow>,
    blocks_for_test: Vec<ExperimentableBlock>,
    variant_fields: Vec<VariantField>,
}

struct FunnelRow {
    band: i64,
    pageviews: i64,
    pct: i64,
}

struct BlockStatRow {
    kind: String,
    preview: String,
    reach: i64,
    dropoff: i64,
    dropoff_pct: i64,
    impressions: i64,
}

struct ExperimentableBlock {
    id: String,
    label: String,
}

struct VariantField {
    id: String,
    name: String,
    label: String,
}

struct ExperimentRow {
    id: String,
    name: String,
    status: String,
    status_class: String,
    goal: String,
    traffic: String,
    block_label: String,
    draft: bool,
    running: bool,
    variants: Vec<VariantRow>,
    report_line: String,
    decision_line: String,
    decisions: Vec<DecisionRow>,
}

struct VariantRow {
    label: String,
    weight_line: String,
    impressions: String,
    conversions: String,
    conv_rate: String,
    beats: String,
    beats_pct: i64,
}

struct DecisionRow {
    date: String,
    summary: String,
}

#[derive(Template)]
#[template(path = "article.html")]
struct ArticleTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    seo: SeoMeta,
    seo_ld: JsonLd,
    slug: String,
    title: String,
    date: String,
    tags: Vec<String>,
    comments_enabled: bool,
    rendered_blocks: Vec<ArticleBlock>,
    comments: Vec<ArticleComment>,
    comment_error: String,
}

/// Pre-serialized, script-safe JSON strings for the JSON-LD block. The values
/// are JSON-encoded and then additionally escaped so `<`, `>`, `&`, and the
/// line/paragraph separators can never terminate the `<script>` element.
struct JsonLd {
    headline: String,
    description: String,
    author: String,
    publisher: String,
    image: String,
}

struct ArticleBlock {
    id: String,
    experiment_id: String,
    variant_id: String,
    html: String,
}

struct ArticleComment {
    author_name: String,
    date: String,
    body: String,
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    seo: SeoMeta,
    query: String,
    results: Vec<SearchResult>,
}

struct SearchResult {
    title: String,
    slug: String,
    date: String,
    tags: Vec<String>,
    snippet: String,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    status: String,
    message: String,
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    authed: bool,
    flash: String,
    site_name: String,
    theme: String,
    site_url: String,
    tagline: String,
    comments_enabled: bool,
    csrf_token: String,
    themes: Vec<ThemeOption>,
    error: String,
}

struct ThemeOption {
    value: String,
    label: String,
    selected: bool,
}

/// The selectable themes (also used to validate the settings form).
pub(crate) const THEMES: &[(&str, &str)] = &[
    ("system", "System (auto)"),
    ("light", "Light"),
    ("dark", "Dark"),
    ("sepia", "Sepia"),
    ("solarized", "Solarized"),
];

// ---------------------------------------------------------------------------
// Form payloads
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct FlashQuery {
    #[serde(default)]
    flash: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    #[serde(default)]
    q: String,
}

#[derive(Deserialize)]
pub(crate) struct CsrfForm {
    #[serde(default)]
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SetupForm {
    email: String,
    display: String,
    password: String,
    confirm: String,
}

#[derive(Deserialize)]
pub(crate) struct LoginForm {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct EditorForm {
    title: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    markdown: String,
    #[serde(default)]
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SettingsForm {
    name: String,
    #[serde(default)]
    theme: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    tagline: String,
    #[serde(default)]
    comments_enabled: bool,
    #[serde(default)]
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ExperimentForm {
    block_id: String,
    #[serde(default)]
    name: String,
    #[serde(default = "default_traffic")]
    traffic_weight: f64,
    #[serde(default)]
    variant_1_content: String,
    #[serde(default = "default_weight")]
    variant_1_weight: f64,
    #[serde(default)]
    variant_2_content: String,
    #[serde(default = "default_weight")]
    variant_2_weight: f64,
    #[serde(default)]
    variant_3_content: String,
    #[serde(default = "default_weight")]
    variant_3_weight: f64,
    #[serde(default)]
    csrf_token: Option<String>,
}

fn default_traffic() -> f64 {
    100.0
}

fn default_weight() -> f64 {
    50.0
}

#[derive(Deserialize)]
pub(crate) struct CommentForm {
    author: String,
    body: String,
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

fn render(tpl: &impl Template) -> Result<Html<String>, ApiError> {
    tpl.render()
        .map(Html)
        .map_err(|e| ApiError::bad_request(format!("template error: {e}")))
}

fn page(tpl: &impl Template) -> Result<Response, PageError> {
    Ok(render(tpl)?.into_response())
}

/// Current blog-wide settings (name + theme).
async fn site(state: &AppState) -> Result<SiteSettings, ApiError> {
    Ok(state.repo.site_settings().await?)
}

fn error_page(status: StatusCode, message: String) -> Response {
    let tpl = ErrorTemplate {
        authed: false,
        flash: String::new(),
        site_name: "Forgepost".into(),
        theme: "system".into(),
        status: status.as_u16().to_string(),
        message,
    };
    (
        status,
        render(&tpl).unwrap_or_else(|_| Html("<h1>Error</h1>".into())),
    )
        .into_response()
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        let (status, message) = self.0.status_and_message();
        error_page(status, message)
    }
}

fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError(RepositoryError::NotFound(msg.into()))
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::bad_request("invalid id"))
}

fn flash_message(key: Option<&str>) -> String {
    match key {
        Some("saved") => "Saved".into(),
        Some("published") => "Published".into(),
        Some("comment_pending") => "Thanks! Your comment is awaiting moderation.".into(),
        Some("comment_approved") => "Comment approved.".into(),
        Some("comments_disabled") => "Comments are closed on this site.".into(),
        Some("settings_saved") => "Settings saved.".into(),
        Some("logged_out") => "You have been logged out.".into(),
        Some("not_authorized") => "You need to be signed in to do that.".into(),
        Some("experiment_created") => "Experiment created.".into(),
        Some("experiment_started") => "Experiment started.".into(),
        Some("experiment_stopped") => "Experiment stopped.".into(),
        Some("experiment_removed") => "Experiment deleted.".into(),
        Some("experiment_decided") => "Decision applied.".into(),
        Some("experiment_failed") => "Could not create experiment.".into(),
        Some("variant_required") => "At least one variant with content is required.".into(),
        _ => String::new(),
    }
}

/// Seconds since the Unix epoch → civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn format_date(ms: Option<i64>) -> String {
    match ms {
        None => String::new(),
        Some(ms) => {
            let days = ms.div_euclid(86_400_000);
            let (y, m, d) = civil_from_days(days);
            format!("{y:04}-{m:02}-{d:02}")
        }
    }
}

fn format_datetime(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn format_pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// Absolute base URL for canonical/OG/sitemap/RSS links: the configured
/// `site.url` when set, otherwise derived from the request Host + scheme.
pub(crate) fn canonical_base(state: &AppState, site: &SiteSettings, headers: &HeaderMap) -> String {
    if !site.url.is_empty() {
        return site.url.trim_end_matches('/').to_string();
    }
    let scheme = if state.secure_cookies {
        "https"
    } else {
        "http"
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

/// Full UTC timestamp as ISO 8601 (`YYYY-MM-DDTHH:MM:SSZ`) for JSON-LD.
pub(crate) fn format_iso_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// RFC-822 date for RSS `<pubDate>` (e.g. `Thu, 06 Aug 2026 09:00:00 GMT`).
pub(crate) fn format_rfc822(ms: i64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = ms.div_euclid(86_400_000);
    let secs = ms.div_euclid(1000);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let dow = (days + 4).rem_euclid(7) as usize; // 1970-01-01 was a Thursday
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[dow],
        d,
        MONTHS[(m as usize) - 1],
        y,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Meta description for an article: the first body-text block, whitespace
/// collapsed and truncated to ~155 characters.
fn page_meta_description(full: &crate::model::FullDocument) -> String {
    let text = full
        .document
        .blocks
        .iter()
        .find_map(|b| {
            let c = full.document.current_content(b.id)?;
            match b.kind {
                BlockKind::Paragraph | BlockKind::Quote | BlockKind::CallToAction => {
                    let t = text_of(c);
                    if t.is_empty() { None } else { Some(t) }
                }
                BlockKind::List { .. } => {
                    let items = c
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|it| it.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    if items.is_empty() { None } else { Some(items) }
                }
                _ => None,
            }
        })
        .unwrap_or_default();
    let clean: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= 155 {
        clean
    } else {
        clean.chars().take(152).collect::<String>() + "..."
    }
}

/// Absolute URL of the article's first image block (relative paths get the
/// site base prepended); empty when the article has no image.
fn article_image(full: &crate::model::FullDocument, base: &str) -> String {
    full.document
        .blocks
        .iter()
        .find_map(|b| {
            if b.kind != BlockKind::Image {
                return None;
            }
            let c = full.document.current_content(b.id)?;
            let src = c.get("src").and_then(|v| v.as_str())?;
            if src.starts_with('/') {
                Some(format!("{base}{src}"))
            } else {
                Some(src.to_string())
            }
        })
        .unwrap_or_default()
}

/// Serialize a string as JSON, then additionally escape the characters that
/// are unsafe inside an HTML `<script>` element (`<`, `>`, `&`, and U+2028 /
/// U+2029) so the value can never terminate the script block.
fn script_json(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn format_duration(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        let s = (ms / 1000).max(1);
        format!("{}m {}s", s / 60, s % 60)
    }
}

fn short(id: &Uuid) -> String {
    id.to_string()[..8].to_string()
}

// ---------------------------------------------------------------------------
// Public pages
// ---------------------------------------------------------------------------

pub(crate) async fn home_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, PageError> {
    if !state.repo.is_setup_complete().await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    let site = site(&state).await?;
    let base = canonical_base(&state, &site, &headers);
    let description = if site.tagline.is_empty() {
        format!("Latest posts from {}.", site.name)
    } else {
        site.tagline.clone()
    };
    let published = state.repo.list_published().await?;
    let posts = published
        .iter()
        .map(|p| HomePost {
            title: p.title.clone(),
            slug: p.slug.clone(),
            date: format_date(p.published_at_ms),
        })
        .collect();
    page(&HomeTemplate {
        authed: false,
        flash: String::new(),
        site_name: site.name.clone(),
        theme: site.theme,
        seo: SeoMeta {
            title: site.name,
            description,
            url: base,
            image: String::new(),
            date_published: String::new(),
            date_modified: String::new(),
            author: String::new(),
        },
        posts,
    })
}

pub(crate) async fn search_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Response, PageError> {
    if !state.repo.is_setup_complete().await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    let site = site(&state).await?;
    let base = canonical_base(&state, &site, &headers);
    let q = query.q.trim().to_string();
    let hits = if q.is_empty() {
        Vec::new()
    } else {
        state.repo.search_documents(&q, 50).await?
    };
    let results = hits
        .iter()
        .map(|h| SearchResult {
            title: h.title.clone(),
            slug: h.slug.clone(),
            date: format_date(h.published_at_ms),
            tags: h.tags.clone(),
            snippet: h.snippet.clone(),
        })
        .collect();
    let seo_title = if q.is_empty() {
        format!("Search · {}", site.name)
    } else {
        format!("Search for \"{q}\" · {}", site.name)
    };
    page(&SearchTemplate {
        authed: false,
        flash: String::new(),
        site_name: site.name.clone(),
        theme: site.theme,
        seo: SeoMeta {
            title: seo_title,
            description: format!("Search results for \"{q}\" on {}.", site.name),
            url: base,
            image: String::new(),
            date_published: String::new(),
            date_modified: String::new(),
            author: String::new(),
        },
        query: q,
        results,
    })
}

pub(crate) async fn setup_page(State(state): State<AppState>) -> Result<Response, PageError> {
    if state.repo.is_setup_complete().await? {
        return Ok(Redirect::to("/login").into_response());
    }
    let site = site(&state).await?;
    page(&SetupTemplate {
        authed: false,
        flash: String::new(),
        site_name: site.name,
        theme: site.theme,
        error: String::new(),
    })
}

pub(crate) async fn setup_form(
    State(state): State<AppState>,
    Form(body): Form<SetupForm>,
) -> Result<Response, PageError> {
    let error = validate_setup(&body);
    if !error.is_empty() {
        let site = site(&state).await?;
        return page(&SetupTemplate {
            authed: false,
            flash: String::new(),
            site_name: site.name,
            theme: site.theme,
            error,
        });
    }
    if state.repo.is_setup_complete().await? {
        return Ok(Redirect::to("/login").into_response());
    }
    let hash = auth::hash_password(&body.password)?;
    let user = state
        .repo
        .create_first_user(&body.email, &body.display, &hash)
        .await?;
    let session = state.repo.create_session(user.id).await?;
    let cookie = auth::set_session_cookie_secure(&session.token, state.secure_cookies);
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to("/admin")).into_response())
}

fn validate_setup(body: &SetupForm) -> String {
    if !body.email.contains('@') || !body.email.contains('.') {
        return "Enter a valid email address.".into();
    }
    if body.password.len() < 8 {
        return "Password must be at least 8 characters.".into();
    }
    if body.password != body.confirm {
        return "Passwords do not match.".into();
    }
    if body.display.trim().is_empty() {
        return "Enter a display name.".into();
    }
    String::new()
}

pub(crate) async fn login_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    if !state.repo.is_setup_complete().await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    if auth::session_user(&state, &headers).await.is_some() {
        return Ok(Redirect::to("/admin").into_response());
    }
    let site = site(&state).await?;
    page(&LoginTemplate {
        authed: false,
        flash: flash_message(flash.flash.as_deref()),
        site_name: site.name,
        theme: site.theme,
        error: String::new(),
    })
}

pub(crate) async fn login_form(
    State(state): State<AppState>,
    Form(body): Form<LoginForm>,
) -> Result<Response, PageError> {
    let error = match state.repo.find_user_by_email(&body.email).await? {
        Some(user) if auth::verify_password(&user.password_hash, &body.password) => {
            let session = state.repo.create_session(user.id).await?;
            let cookie = auth::set_session_cookie_secure(&session.token, state.secure_cookies);
            return Ok(([(header::SET_COOKIE, cookie)], Redirect::to("/admin")).into_response());
        }
        _ => "invalid email or password".to_string(),
    };
    let site = site(&state).await?;
    page(&LoginTemplate {
        authed: false,
        flash: String::new(),
        site_name: site.name,
        theme: site.theme,
        error,
    })
}

pub(crate) async fn logout_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(body): Form<CsrfForm>,
) -> Result<Response, PageError> {
    let Some(auth) = auth::session_user(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    if let Some(token) = auth::cookie(&headers, auth::SESSION_COOKIE) {
        state.repo.delete_session(&token).await?;
    }
    Ok((
        [(
            header::SET_COOKIE,
            auth::clear_session_cookie_secure(state.secure_cookies),
        )],
        Redirect::to("/login"),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Admin pages
// ---------------------------------------------------------------------------

async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<AuthUser>, ApiError> {
    let Some(auth) = auth::session_user(state, headers).await else {
        return Ok(None);
    };
    Ok(Some(auth))
}

fn login_redirect() -> Response {
    Redirect::to("/login?flash=not_authorized").into_response()
}

pub(crate) async fn dashboard_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    let docs = state.repo.list_documents(auth.user.id).await?;
    let pending = state.repo.pending_comments().await?;
    let site = site(&state).await?;
    page(&DashboardTemplate {
        authed: true,
        flash: flash_message(flash.flash.as_deref()),
        site_name: site.name,
        theme: site.theme,
        display_name: auth.user.display_name.clone(),
        csrf_token: auth.csrf_token.clone(),
        docs: docs
            .iter()
            .map(|d| DashboardDoc {
                id: d.id.to_string(),
                title: d.title.clone(),
                status: d.status.clone(),
                updated: format_date(Some(d.updated_at_ms)),
            })
            .collect(),
        pending: pending
            .iter()
            .map(|c| PendingComment {
                id: c.id.to_string(),
                author_name: c.author_name.clone(),
                body: c.body.clone(),
                short_id: c.document_id.to_string()[..8].to_string(),
            })
            .collect(),
    })
}

pub(crate) async fn new_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(body): Form<CsrfForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    let doc = state.repo.create_document(auth.user.id, "Untitled").await?;
    let uri = format!("/admin/editor/{}", doc.document.id);
    Ok(Redirect::to(&uri).into_response())
}

pub(crate) async fn editor_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    let id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden().into());
    }
    let tags = state.repo.document_tags(id).await?;
    let site = site(&state).await?;
    page(&EditorTemplate {
        authed: true,
        flash: flash_message(flash.flash.as_deref()),
        site_name: site.name,
        theme: site.theme,
        id: id.to_string(),
        slug: full.slug.clone(),
        title: full.document.title.clone(),
        tags: tags.join(", "),
        markdown: blocks_to_markdown(&full.document),
        status: full.status.clone(),
        csrf_token: auth.csrf_token.clone(),
    })
}

pub(crate) async fn editor_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(body): Form<EditorForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let mut full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden().into());
    }
    let title = body.title.trim().to_string();
    if !title.is_empty() {
        state.repo.update_document_title(id, &title).await?;
        if full.status == "draft" {
            state.repo.regenerate_draft_slug(id, &title).await?;
        }
    }
    crate::routes::apply_markdown(&*state.repo, &mut full.document, &body.markdown).await?;
    let tags: Vec<String> = body
        .tags
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    state.repo.set_document_tags(id, &tags).await?;
    let uri = format!("/admin/editor/{id}?flash=saved");
    Ok(Redirect::to(&uri).into_response())
}

pub(crate) async fn editor_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(body): Form<CsrfForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden().into());
    }
    state.repo.publish_document(id).await?;
    let uri = format!("/admin/editor/{id}?flash=published");
    Ok(Redirect::to(&uri).into_response())
}

pub(crate) async fn approve_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(body): Form<CsrfForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    state.repo.set_comment_status(id, "approved").await?;
    Ok(Redirect::to("/admin?flash=comment_approved").into_response())
}

pub(crate) async fn settings_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    let site = site(&state).await?;
    page(&settings_template(
        site,
        auth,
        flash_message(flash.flash.as_deref()),
        String::new(),
    ))
}

pub(crate) async fn settings_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(body): Form<SettingsForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;

    let error = validate_settings(&body);
    if !error.is_empty() {
        let site = site(&state).await?;
        return page(&settings_template(site, auth, String::new(), error));
    }
    state
        .repo
        .set_setting("site.name", body.name.trim())
        .await?;
    state.repo.set_setting("theme", &body.theme).await?;
    state.repo.set_setting("site.url", body.url.trim()).await?;
    state
        .repo
        .set_setting("site.tagline", body.tagline.trim())
        .await?;
    state
        .repo
        .set_setting(
            "comments.enabled",
            if body.comments_enabled { "1" } else { "0" },
        )
        .await?;
    Ok(Redirect::to("/admin/settings?flash=settings_saved").into_response())
}

fn settings_template(
    site: SiteSettings,
    auth: AuthUser,
    flash: String,
    error: String,
) -> SettingsTemplate {
    let theme = site.theme.clone();
    SettingsTemplate {
        authed: true,
        flash,
        site_name: site.name,
        theme: site.theme,
        site_url: site.url,
        tagline: site.tagline,
        comments_enabled: site.comments_enabled,
        csrf_token: auth.csrf_token,
        themes: THEMES
            .iter()
            .map(|(value, label)| ThemeOption {
                value: value.to_string(),
                label: label.to_string(),
                selected: *value == theme,
            })
            .collect(),
        error,
    }
}

fn validate_settings(body: &SettingsForm) -> String {
    if body.name.trim().is_empty() {
        return "Enter a blog name.".into();
    }
    if body.name.trim().len() > 80 {
        return "Blog name is too long.".into();
    }
    if !THEMES.iter().any(|(value, _)| *value == body.theme) {
        return "Unknown theme.".into();
    }
    let url = body.url.trim();
    if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
        return "Site URL must start with http:// or https://.".into();
    }
    if body.tagline.trim().len() > 200 {
        return "Tagline is too long (200 characters max).".into();
    }
    String::new()
}

pub(crate) async fn stats_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    let doc_id = parse_uuid(&id)?;

    let stats = crate::routes::document_stats(State(state.clone()), auth.clone(), Path(id.clone()))
        .await?
        .0;
    let experiments =
        crate::routes::list_experiments(State(state.clone()), auth.clone(), Path(id.clone()))
            .await?
            .0;

    let views = stats.article.views;
    let funnel = stats
        .article
        .band_reach
        .iter()
        .map(|b| FunnelRow {
            band: b.band,
            pageviews: b.pageviews,
            pct: if views > 0 {
                (b.pageviews as f64 / views as f64 * 100.0).round() as i64
            } else {
                0
            },
        })
        .collect();

    let max_dropoff = stats
        .blocks
        .iter()
        .map(|b| b.estimated_dropoff)
        .max()
        .unwrap_or(0)
        .max(1);
    let blocks = stats
        .blocks
        .iter()
        .map(|b| BlockStatRow {
            kind: b.kind.clone(),
            preview: b.preview.clone(),
            reach: b.estimated_reach,
            dropoff: b.estimated_dropoff,
            dropoff_pct: (b.estimated_dropoff as f64 / max_dropoff as f64 * 100.0).round() as i64,
            impressions: b.impressions,
        })
        .collect();

    let blocks_for_test = stats
        .blocks
        .iter()
        .filter(|b| {
            b.kind.starts_with("Paragraph")
                || b.kind.starts_with("Heading")
                || b.kind.starts_with("CallToAction")
        })
        .map(|b| ExperimentableBlock {
            id: b.block_id.to_string(),
            label: format!("{} — {}", b.kind, b.preview),
        })
        .collect();

    let experiment_rows = experiments
        .iter()
        .map(|exp| experiment_row(exp, &stats.blocks))
        .collect();

    let site = site(&state).await?;
    page(&StatsTemplate {
        authed: true,
        flash: flash_message(flash.flash.as_deref()),
        site_name: site.name,
        theme: site.theme,
        doc_id: doc_id.to_string(),
        csrf_token: auth.csrf_token.clone(),
        views,
        unique_readers: stats.article.unique_readers,
        avg_read_time: stats
            .article
            .avg_read_time_ms
            .map(format_duration)
            .unwrap_or_else(|| "—".into()),
        completion: stats
            .article
            .completion
            .map(format_pct)
            .unwrap_or_else(|| "—".into()),
        funnel,
        blocks,
        experiments: experiment_rows,
        blocks_for_test,
        variant_fields: vec![
            VariantField {
                id: "variant_1_content".into(),
                name: "variant_1_content".into(),
                label: "Variant 1 content".into(),
            },
            VariantField {
                id: "variant_2_content".into(),
                name: "variant_2_content".into(),
                label: "Variant 2 content".into(),
            },
            VariantField {
                id: "variant_3_content".into(),
                name: "variant_3_content".into(),
                label: "Variant 3 content".into(),
            },
        ],
    })
}

fn experiment_row(exp: &ExperimentView, blocks: &[crate::analytics::BlockStat]) -> ExperimentRow {
    let name = if exp.name.is_empty() {
        "Untitled experiment".to_string()
    } else {
        exp.name.clone()
    };
    let block_label = blocks
        .iter()
        .find(|b| b.block_id == exp.block_id)
        .map(|b| format!("{} — {}", b.kind, b.preview))
        .unwrap_or_else(|| short(&exp.block_id));

    let non_control_ids: Vec<&Uuid> = exp
        .variants
        .iter()
        .filter(|v| !v.is_control)
        .map(|v| &v.id)
        .collect();
    let variants = exp
        .variants
        .iter()
        .map(|v| {
            let report = exp
                .report
                .as_ref()
                .and_then(|r| r.variants.iter().find(|rv| rv.variant_id == v.id));
            let beats = report.and_then(|r| r.prob_beats_control);
            VariantRow {
                label: if v.is_control {
                    "Control (current)".into()
                } else {
                    let idx = non_control_ids
                        .iter()
                        .position(|id| *id == &v.id)
                        .unwrap_or(0)
                        + 1;
                    format!("Variant {idx}")
                },
                weight_line: if v.is_control {
                    format!("{:.0}% traffic", v.weight)
                } else {
                    format!("{:.0}% of tested traffic", v.weight)
                },
                impressions: report
                    .map(|r| r.impressions.to_string())
                    .unwrap_or_else(|| "—".into()),
                conversions: report
                    .map(|r| r.conversions.to_string())
                    .unwrap_or_else(|| "—".into()),
                conv_rate: report
                    .map(|r| format_pct(r.conversion_rate))
                    .unwrap_or_else(|| "—".into()),
                beats: beats.map(format_pct).unwrap_or_else(|| "—".into()),
                beats_pct: beats.map(|b| (b * 100.0).round() as i64).unwrap_or(0),
            }
        })
        .collect();

    let running = exp.status == "running";
    let report_line = if running {
        exp.report.as_ref().map(|r| {
            format!(
                "Running for {} · {}",
                format_duration(r.elapsed_ms),
                recommendation_text(exp)
            )
        })
    } else {
        None
    };
    let decision_line = exp.decision.as_ref().map(|d| format!("Decision: {d}"));

    ExperimentRow {
        id: exp.id.to_string(),
        name,
        status: exp.status.clone(),
        status_class: match exp.status.as_str() {
            "running" => "badge ok".into(),
            "decided" => "badge info".into(),
            _ => "badge".into(),
        },
        goal: exp.goal.clone(),
        traffic: format!("{:.0}% of visitors", exp.traffic_weight),
        block_label,
        draft: exp.status == "draft",
        running,
        variants,
        report_line: report_line.unwrap_or_default(),
        decision_line: decision_line.unwrap_or_default(),
        decisions: exp
            .decisions
            .iter()
            .map(|d| DecisionRow {
                date: format_datetime(d.decided_at_ms),
                summary: d.decision.clone(),
            })
            .collect(),
    }
}

fn recommendation_text(exp: &ExperimentView) -> String {
    let Some(report) = exp.report.as_ref() else {
        return String::new();
    };
    match &report.recommendation {
        Recommendation::Continue => format!(
            "Collecting data… {} look(s) so far, threshold {}.",
            report.n_looks,
            format_pct(report.adjusted_confidence_threshold)
        ),
        Recommendation::NoWinner => "Variant is (near-)certain not to beat control.".into(),
        Recommendation::Promote {
            variant_id,
            confidence,
        } => format!(
            "Variant {} likely better ({}).",
            short(variant_id),
            format_pct(*confidence)
        ),
    }
}

pub(crate) async fn create_experiment_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(body): Form<ExperimentForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    let doc_id = parse_uuid(&id)?;
    let full = state
        .repo
        .get_document(doc_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden().into());
    }
    let block_id = parse_uuid(&body.block_id)?;

    let mut variants = Vec::new();
    for (content, weight) in [
        (&body.variant_1_content, body.variant_1_weight),
        (&body.variant_2_content, body.variant_2_weight),
        (&body.variant_3_content, body.variant_3_weight),
    ] {
        if !content.trim().is_empty() {
            variants.push(crate::model::ExperimentVariantInput {
                content: serde_json::json!({ "text": content.trim() }),
                weight,
            });
        }
    }
    let flash = if variants.is_empty() {
        "variant_required".to_string()
    } else {
        let new_exp = crate::model::NewExperiment {
            name: body.name.trim().to_string(),
            goal: "completion".into(),
            traffic_weight: body.traffic_weight,
            confidence_threshold: 0.95,
            min_sample_per_variant: 100,
            no_winner_prob: 0.05,
            max_duration_ms: 30 * 24 * 60 * 60 * 1000,
            variants,
        };
        match state
            .repo
            .create_experiment(doc_id, block_id, &new_exp)
            .await
        {
            Ok(_) => "experiment_created".to_string(),
            Err(_) => "experiment_failed".to_string(),
        }
    };
    let uri = format!("/admin/stats/{doc_id}?flash={flash}");
    Ok(Redirect::to(&uri).into_response())
}

pub(crate) async fn experiment_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, action)): Path<(String, String)>,
    Form(body): Form<CsrfForm>,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok(login_redirect());
    };
    auth::verify_csrf_form(&headers, body.csrf_token.as_deref(), &auth.csrf_token)?;
    let id = parse_uuid(&id)?;
    let exp = state
        .repo
        .experiment(id)
        .await?
        .ok_or_else(|| ApiError::bad_request("experiment not found"))?;
    let doc_id = exp.document_id;
    let full = state
        .repo
        .get_document(doc_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("document not found"))?;
    if full.owner_id != auth.user.id {
        return Err(ApiError::forbidden().into());
    }
    let flash = match action.as_str() {
        "start" => {
            state.repo.start_experiment(id).await?;
            "experiment_started"
        }
        "stop" => {
            state.repo.stop_experiment(id).await?;
            "experiment_stopped"
        }
        "decide" => {
            crate::experiments::decide_experiment(&*state.repo, id).await?;
            "experiment_decided"
        }
        "promote" => {
            crate::experiments::promote_experiment(&*state.repo, id).await?;
            "experiment_decided"
        }
        "no-winner" => {
            crate::experiments::conclude_no_winner(&*state.repo, id).await?;
            "experiment_decided"
        }
        _ => return Err(ApiError::bad_request("unknown action").into()),
    };
    let uri = format!("/admin/stats/{doc_id}?flash={flash}");
    Ok(Redirect::to(&uri).into_response())
}

// ---------------------------------------------------------------------------
// Public article page
// ---------------------------------------------------------------------------

pub(crate) async fn article_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    build_article_page(&state, &headers, &slug, flash.flash.as_deref(), "").await
}

pub(crate) async fn comment_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Form(body): Form<CommentForm>,
) -> Result<Response, PageError> {
    if !state.repo.site_settings().await?.comments_enabled {
        let uri = format!("/articles/{slug}?flash=comments_disabled");
        return Ok(Redirect::to(&uri).into_response());
    }
    let author = body.author.trim().to_string();
    let comment_body = body.body.trim().to_string();
    if author.is_empty() || comment_body.is_empty() {
        return build_article_page(
            &state,
            &headers,
            &slug,
            None,
            "Name and comment are required.",
        )
        .await;
    }
    if comment_body.len() > 2000 {
        return build_article_page(&state, &headers, &slug, None, "Comment too long.").await;
    }
    let full = state
        .repo
        .get_published_by_slug(&slug)
        .await?
        .ok_or_else(|| not_found("Article not found"))?;
    state
        .repo
        .create_comment(full.document.id, &author, &comment_body)
        .await?;
    let uri = format!("/articles/{slug}?flash=comment_pending");
    Ok(Redirect::to(&uri).into_response())
}

async fn build_article_page(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
    flash: Option<&str>,
    comment_error: &str,
) -> Result<Response, PageError> {
    let full = state
        .repo
        .get_published_by_slug(slug)
        .await?
        .ok_or_else(|| not_found("Article not found"))?;
    let tags = state.repo.document_tags(full.document.id).await?;

    // Stable per-visitor assignment needs the anonymous visitor id, so mint the
    // cookie here if the reader does not have one yet.
    let (visitor_id, cookie) =
        crate::routes::visitor_identity_with_secure(headers, state.secure_cookies);
    let experiments = state
        .repo
        .running_experiments_for_document(full.document.id)
        .await?;
    let mut view = crate::routes::article_view(&full, tags.clone());
    crate::routes::apply_assignments(&full, &experiments, visitor_id, &mut view);

    let site = site(state).await?;
    let comments = if site.comments_enabled {
        state
            .repo
            .comments_for_document(full.document.id, Some("approved"))
            .await?
    } else {
        Vec::new()
    };
    let base = canonical_base(state, &site, headers);
    let description = {
        let d = page_meta_description(&full);
        if d.is_empty() { view.title.clone() } else { d }
    };
    let author = match state.repo.find_user_by_id(full.owner_id).await? {
        Some(u) => u.display_name.clone(),
        None => site.name.clone(),
    };
    let seo = SeoMeta {
        title: view.title.clone(),
        description,
        url: format!("{base}/articles/{}", view.slug),
        image: article_image(&full, &base),
        date_published: full.published_at_ms.map(format_iso_utc).unwrap_or_default(),
        date_modified: format_iso_utc(full.document.updated_at_ms),
        author,
    };
    let seo_ld = JsonLd {
        headline: script_json(&view.title),
        description: script_json(&seo.description),
        author: script_json(&seo.author),
        publisher: script_json(&site.name),
        image: script_json(&seo.image),
    };
    let tpl = ArticleTemplate {
        authed: false,
        flash: flash_message(flash),
        site_name: site.name.clone(),
        theme: site.theme,
        seo,
        seo_ld,
        slug: view.slug.clone(),
        title: view.title.clone(),
        date: format_date(view.published_at_ms),
        tags: view.tags.clone(),
        comments_enabled: site.comments_enabled,
        rendered_blocks: view
            .rendered_blocks
            .iter()
            .map(|b| ArticleBlock {
                id: b.id.to_string(),
                experiment_id: b.experiment_id.map(|i| i.to_string()).unwrap_or_default(),
                variant_id: b.variant_id.map(|i| i.to_string()).unwrap_or_default(),
                html: b.html.clone(),
            })
            .collect(),
        comments: comments
            .iter()
            .map(|c| ArticleComment {
                author_name: c.author_name.clone(),
                date: format_date(Some(c.created_at_ms)),
                body: c.body.clone(),
            })
            .collect(),
        comment_error: comment_error.to_string(),
    };
    let mut resp = render(&tpl)?.into_response();
    if let Some(c) = cookie {
        resp.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Static assets
// ---------------------------------------------------------------------------

pub(crate) async fn static_file(Path(name): Path<String>) -> Result<Response, PageError> {
    let (body, content_type) = match name.as_str() {
        "app.css" => (include_str!("../static/app.css"), "text/css"),
        "favicon.svg" => (include_str!("../static/favicon.svg"), "image/svg+xml"),
        "tracker.js" => (
            include_str!("../static/tracker.js"),
            "application/javascript",
        ),
        _ => return Err(not_found("static file not found").into()),
    };
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
}

/// Maximum accepted upload size (10 MiB), enough for photos; the MVP stores
/// original bytes only, so huge files would otherwise bloat disk usage.
const MAX_MEDIA_BYTES: usize = 10 * 1024 * 1024;

/// Upload an image to the media directory. Owner-only; the CSRF token travels
/// in the `x-csrf-token` header. The client's filename is never used: files
/// are stored under `<uuid>.<ext>` where `<ext>` comes from a magic-byte sniff
/// (PNG/JPEG/GIF/WebP only — SVG and anything else is rejected).
pub(crate) async fn media_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, PageError> {
    let Some(auth) = require_admin(&state, &headers).await? else {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response());
    };
    auth::verify_csrf(&headers, &auth.csrf_token)?;

    let mut data: Vec<u8> = Vec::new();
    let mut saw_data = false;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("multipart error: {e}")))?
    {
        if field.name() != Some("data") {
            continue;
        }
        saw_data = true;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::bad_request(format!("upload error: {e}")))?
        {
            data.extend_from_slice(&chunk);
            if data.len() > MAX_MEDIA_BYTES {
                return Err(ApiError::bad_request("image too large (max 10 MiB)").into());
            }
        }
    }
    if !saw_data {
        return Err(ApiError::bad_request("missing `data` field").into());
    }
    let Some((content_type, ext)) = sniff_image(&data) else {
        return Err(ApiError::bad_request(
            "unsupported image type (only PNG, JPEG, GIF, and WebP are accepted)",
        )
        .into());
    };
    let sha256 = hex::encode(Sha256::digest(&data));
    let disk_name = format!("{}.{ext}", Uuid::new_v4());
    tokio::fs::create_dir_all(&state.media_dir).await?;
    tokio::fs::write(state.media_dir.join(&disk_name), &data).await?;
    state
        .repo
        .insert_media(&Media {
            id: Uuid::new_v4(),
            disk_name: disk_name.clone(),
            content_type: content_type.to_string(),
            size_bytes: data.len() as i64,
            sha256,
            created_at_ms: now_ms(),
        })
        .await?;
    Ok(Json(serde_json::json!({ "url": format!("/media/{disk_name}") })).into_response())
}

/// Serve an uploaded file. Content type comes from the media table (the sniffed
/// value stored at upload), so browsers render images rather than downloading
/// them; `nosniff` prevents the browser from sniffing a different type. Names
/// are plain `<uuid>.<ext>` so path traversal is impossible by construction,
/// but the name is still validated before touching the filesystem.
pub(crate) async fn media_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, PageError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(not_found("media not found").into());
    }
    let Some(media) = state.repo.media_by_disk_name(&name).await? else {
        return Err(not_found("media not found").into());
    };
    let bytes = tokio::fs::read(state.media_dir.join(&media.disk_name)).await?;
    let mut response =
        ([(header::CONTENT_TYPE, media.content_type.as_str())], bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&media.size_bytes.to_string()).expect("valid header value"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(response)
}

/// Sniff the real image type from leading magic bytes. Returns the canonical
/// MIME type and file extension. Deliberately does NOT accept SVG: serving an
/// attacker-controlled SVG would let them ship script-bearing documents.
fn sniff_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(("image/png", "png"))
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else if data.starts_with(b"GIF8") {
        Some(("image/gif", "gif"))
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a document's blocks back into Markdown for the editor textarea.
fn blocks_to_markdown(doc: &forgepost_content::Document) -> String {
    let mut parts = Vec::new();
    for b in &doc.blocks {
        let Some(c) = doc.current_content(b.id) else {
            continue;
        };
        let part = match b.kind {
            BlockKind::Heading { level } => {
                format!("{} {}", "#".repeat(level as usize), text_of(c))
            }
            BlockKind::Quote => text_of(c)
                .split('\n')
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            BlockKind::Code => {
                let lang = c
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let code = c.get("code").and_then(|v| v.as_str()).unwrap_or_default();
                format!("```{lang}\n{code}\n```")
            }
            BlockKind::Image => {
                let alt = c.get("alt").and_then(|v| v.as_str()).unwrap_or_default();
                let src = c.get("src").and_then(|v| v.as_str()).unwrap_or_default();
                format!("![{alt}]({src})")
            }
            BlockKind::List { ordered } => {
                let items = c
                    .get("items")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|it| it.as_str()).collect::<Vec<_>>())
                    .unwrap_or_default();
                let marker = if ordered { "1." } else { "-" };
                items
                    .iter()
                    .map(|item| format!("{marker} {item}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            BlockKind::Divider => "---".into(),
            BlockKind::Paragraph | BlockKind::CallToAction => text_of(c),
        };
        parts.push(part);
    }
    parts.join("\n\n")
}

fn text_of(c: &serde_json::Value) -> String {
    c.get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}
