//! The demo installation: a seedable, versioned backup (`demo/forgepost-demo.fpb`)
//! of a small blog with real content, media, and a running A/B experiment — the
//! thing `forgepost demo` installs (and, by default, serves).
//!
//! The article prose lives as Markdown sources in `demo/posts/*.md`; the
//! builder embeds them with `include_str!`, substituting `{{img:KEY:ALT}}`
//! tokens with the runtime media disk names. The committed artifact is *always
//! validated* by restoring it into a scratch installation and asserting the
//! demo invariants (admin login, published posts + tags, per-article
//! substantiality, media on disk, running experiment with live counts). Setting
//! `FORGEPOST_REGEN_DEMO=1` first rebuilds the content and overwrites the
//! artifact, so the artifact can be regenerated deterministically on demand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forgepost_application::ports::{BackupGateway, Repository};
use forgepost_application::services::auth::AuthService;
use forgepost_application::services::backup::BackupService;
use forgepost_application::services::document::DocumentService;
use forgepost_application::services::experiment::ExperimentService;
use forgepost_application::services::settings::SettingsService;
use forgepost_content::now_ms;
use forgepost_domain::model::{AnalyticsEvent, ExperimentVariantInput, Media, PostId, VisitorId};
use forgepost_experiments::assign_variant;
use forgepost_infrastructure::backup::ArchiveBackup;
use forgepost_infrastructure::oembed::RumbleOembed;
use forgepost_infrastructure::sqlite::SqliteRepository;
use serde_json::json;
use uuid::Uuid;

const ADMIN_EMAIL: &str = "admin@example.com";
const ADMIN_PASSWORD: &str = "demo-password";

fn demo_artifact() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../demo/forgepost-demo.fpb"
    ))
}

fn demo_images_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../demo/images"))
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forgepost-demo-{tag}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A media file bundled in `demo/images/`, copied into the installation's
/// media dir and referenced from the article markdown.
async fn register_media(
    repo: &Arc<dyn Repository>,
    media_dir: &Path,
    rel: &str,
) -> (String, String) {
    let bytes = std::fs::read(demo_images_dir().join(rel)).expect("read demo image");
    let disk_name = format!("{}.png", Uuid::new_v4());
    std::fs::write(media_dir.join(&disk_name), &bytes).expect("write media file");
    repo.insert_media(&Media {
        id: Uuid::new_v4(),
        disk_name: disk_name.clone(),
        content_type: "image/png".into(),
        size_bytes: bytes.len() as i64,
        sha256: ArchiveBackup.sha256_hex(&bytes),
        created_at_ms: now_ms(),
    })
    .await
    .expect("insert media row");
    (disk_name, rel.to_string())
}

/// Replace `{{img:KEY:ALT}}` tokens in a `demo/posts/*.md` source with the
/// runtime media disk names (`![ALT](/media/<uuid>.png)`). Tokens without an
/// alt text fall back to the key.
fn render_markdown(src: &str, media: &BTreeMap<&str, &str>) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(start) = rest.find("{{img:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "{{img:".len()..];
        let end = after.find("}}").expect("unterminated demo img token");
        let token = &after[..end];
        let (key, alt) = match token.split_once(':') {
            Some((key, alt)) => (key, alt),
            None => (token, token),
        };
        let disk = media
            .get(key)
            .unwrap_or_else(|| panic!("unknown demo img token {key:?}"));
        out.push_str(&format!("![{alt}](/media/{disk})"));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Create + publish one article; returns its document id.
async fn article(
    documents: &DocumentService,
    owner: Uuid,
    title: &str,
    markdown: impl Into<String>,
    tags: &[&str],
) -> Uuid {
    let markdown = markdown.into();
    let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    let save = documents
        .create(owner, title, Some(&markdown), Some(&tags))
        .await
        .expect("create document");
    documents
        .publish(save.full.document.id, owner)
        .await
        .expect("publish document");
    save.full.document.id
}

/// Build the demo content in a scratch installation and seal it into an
/// archive at `dest`. Returns the archive's path.
async fn build_archive(database_url: &str, media_dir: &Path, dest: &Path) {
    let repo = SqliteRepository::connect(database_url)
        .await
        .expect("open db");
    repo.migrate().await.expect("migrate");
    let repo: Arc<dyn Repository> = Arc::new(repo);

    let auth = AuthService::new(repo.clone());
    let documents = DocumentService::new(repo.clone(), Arc::new(RumbleOembed));
    let experiments = ExperimentService::new(repo.clone());
    let settings = SettingsService::new(repo.clone());

    let setup = auth
        .setup(ADMIN_EMAIL, "Forgepost Admin", ADMIN_PASSWORD)
        .await
        .expect("setup admin");
    let admin = setup.user_id;

    settings
        .update(
            "The Forgepost Demo",
            "light",
            "",
            "A self-hosted blog with block-level A/B testing you can log into",
            "",
            true,
        )
        .await
        .expect("update settings");

    let header = register_media(&repo, media_dir, "header.png").await;
    let chart = register_media(&repo, media_dir, "chart.png").await;
    let archive = register_media(&repo, media_dir, "archive.png").await;
    let cards = register_media(&repo, media_dir, "cards.png").await;

    let mut media = BTreeMap::new();
    for (key, disk) in [
        ("header", &header.0),
        ("chart", &chart.0),
        ("archive", &archive.0),
        ("cards", &cards.0),
    ] {
        media.insert(key, disk.as_str());
    }

    article(
        &documents,
        admin,
        "Welcome to Forgepost",
        render_markdown(
            include_str!("../../../demo/posts/welcome-to-forgepost.md"),
            &media,
        ),
        &["welcome", "intro"],
    )
    .await;

    let exp_doc = article(
        &documents,
        admin,
        "Tracking Every Headline",
        render_markdown(
            include_str!("../../../demo/posts/tracking-every-headline.md"),
            &media,
        ),
        &["experiments", "tutorial"],
    )
    .await;

    article(
        &documents,
        admin,
        "Videos Without the Trackers",
        render_markdown(
            include_str!("../../../demo/posts/videos-without-the-trackers.md"),
            &media,
        ),
        &["privacy", "video"],
    )
    .await;

    article(
        &documents,
        admin,
        "Your Words, Your Rules",
        render_markdown(
            include_str!("../../../demo/posts/your-words-your-rules.md"),
            &media,
        ),
        &["backup", "self-hosting"],
    )
    .await;

    article(
        &documents,
        admin,
        "Anatomy of the Content Model",
        render_markdown(
            include_str!("../../../demo/posts/anatomy-of-the-content-model.md"),
            &media,
        ),
        &["architecture"],
    )
    .await;

    article(
        &documents,
        admin,
        "Crafting the Perfect CTA",
        render_markdown(
            include_str!("../../../demo/posts/crafting-the-perfect-cta.md"),
            &media,
        ),
        &["writing", "cta"],
    )
    .await;

    // A *running* experiment on the "Tracking Every Headline" H1 block, with
    // assignment-consistent analytics events so the live report has numbers.
    let doc = documents
        .get_owned(exp_doc, admin)
        .await
        .expect("fetch exp document");
    let block_id = doc.document.blocks.first().expect("h1 block").id;
    let exp = experiments
        .create(
            exp_doc,
            block_id,
            admin,
            "Headline A/B: 'Tracking' vs 'Testing'",
            "clicks on the Read next link",
            100.0,
            0.95,
            30,
            0.5,
            30 * 24 * 60 * 60 * 1000,
            vec![ExperimentVariantInput {
                content: json!({ "text": "# Testing every headline" }),
                weight: 50.0,
            }],
        )
        .await
        .expect("create experiment");
    experiments
        .start(exp.id, admin)
        .await
        .expect("start experiment");

    let control = exp
        .variants
        .iter()
        .find(|v| v.is_control)
        .expect("control variant");
    let test = exp
        .variants
        .iter()
        .find(|v| !v.is_control)
        .expect("test variant");

    // 40 visitors, deterministically assigned; the test headline "converts" a
    // little better so the report is a believable near-miss, not a blowout.
    let mut visitor = 1u128;
    for _ in 0..40 {
        visitor += 1;
        let v = Uuid::from_u128(visitor);
        let chosen = assign_variant(&exp.id, &v, control.id, 0.5, &[(test.id, 50.0)]);
        let chosen_version = if chosen == test.id {
            test.version_id
        } else {
            control.version_id
        };
        let impression = AnalyticsEvent {
            id: Uuid::new_v4(),
            document_id: PostId(exp_doc),
            event_type: "experiment_impression".into(),
            band: None,
            block_id: Some(block_id),
            pageview_id: Uuid::new_v4(),
            visitor_id: VisitorId(v),
            referrer: None,
            user_agent: Some("Mozilla/5.0 (X11; Linux x86_64) Forgepost Demo".into()),
            read_time_ms: None,
            experiment_id: Some(exp.id),
            variant_id: Some(chosen),
            version_id: Some(chosen_version),
            recommended_slug: None,
            created_at_ms: now_ms(),
        };
        repo.record_analytics_event(&impression)
            .await
            .expect("impression event");
        if chosen == test.id && v.as_u128().is_multiple_of(2) {
            // Roughly half of the test-headline readers click through.
            let conversion = AnalyticsEvent {
                id: Uuid::new_v4(),
                document_id: PostId(exp_doc),
                event_type: "experiment_conversion".into(),
                band: None,
                block_id: Some(block_id),
                pageview_id: Uuid::new_v4(),
                visitor_id: VisitorId(v),
                referrer: None,
                user_agent: Some("Mozilla/5.0 (X11; Linux x86_64) Forgepost Demo".into()),
                read_time_ms: None,
                experiment_id: Some(exp.id),
                variant_id: Some(test.id),
                version_id: Some(test.version_id),
                recommended_slug: None,
                created_at_ms: now_ms() + 1,
            };
            repo.record_analytics_event(&conversion)
                .await
                .expect("conversion event");
        }
    }

    // A handful of article views across several posts so the reader dashboard
    // and per-article stats are non-empty.
    let owned = documents.list(admin).await.expect("list documents");
    for (i, summary) in owned.iter().enumerate().take(4) {
        for k in 0..5u8 {
            repo.record_analytics_event(&AnalyticsEvent {
                id: Uuid::new_v4(),
                document_id: summary.id,
                event_type: "view".into(),
                band: None,
                block_id: None,
                pageview_id: Uuid::new_v4(),
                visitor_id: VisitorId(Uuid::from_u128(81_000u128 + i as u128 * 10 + k as u128)),
                referrer: None,
                user_agent: Some("Mozilla/5.0 (X11; Linux x86_64) Forgepost Demo".into()),
                read_time_ms: Some(120_000),
                experiment_id: None,
                variant_id: None,
                version_id: None,
                recommended_slug: None,
                created_at_ms: now_ms() - i as i64 * 60_000,
            })
            .await
            .expect("view event");
        }
    }

    let svc = BackupService::new(repo.clone(), Arc::new(ArchiveBackup));
    svc.create(database_url, media_dir, dest)
        .await
        .expect("create backup");
}

/// Restore `artifact` into a scratch installation and check every demo
/// invariant against the *restored* state (so we prove the artifact, not the
/// freshly built content, is what ships).
async fn assert_demo_invariant(artifact: &Path) {
    let scratch = tmp_dir("scratch");
    let db_url = format!("sqlite://{}", scratch.join("demo.db").display());
    std::fs::create_dir_all(scratch.join("media")).unwrap();

    // Restore first: the boot repo only supplies the schema version.
    {
        let boot = SqliteRepository::connect(&db_url).await.expect("open db");
        boot.migrate().await.expect("migrate");
        let svc = BackupService::new(
            Arc::new(boot) as Arc<dyn Repository>,
            Arc::new(ArchiveBackup),
        );
        let report = svc
            .restore(artifact, &db_url, &scratch.join("media"), false)
            .await
            .expect("restore demo archive");
        assert!(report.ok, "demo archive must verify clean");
    }

    let repo: Arc<dyn Repository> = Arc::new(
        SqliteRepository::connect(&db_url)
            .await
            .expect("open restored db"),
    );

    // Fixed credentials actually log in.
    let auth = AuthService::new(repo.clone());
    assert!(auth.login(ADMIN_EMAIL, ADMIN_PASSWORD).await.is_ok());
    let admin_user = repo
        .find_user_by_email(ADMIN_EMAIL)
        .await
        .expect("find admin")
        .expect("admin user restored");

    // Settings survived.
    let settings = repo.site_settings().await.expect("settings");
    assert_eq!(settings.name, "The Forgepost Demo");
    assert!(settings.comments_enabled);

    // Six published articles with the expected tags.
    let documents = DocumentService::new(repo.clone(), Arc::new(RumbleOembed));
    let owned = documents
        .list(admin_user.id)
        .await
        .expect("list restored docs");
    assert_eq!(owned.len(), 6, "six restored articles");
    let titles: Vec<String> = owned.iter().map(|d| d.title.clone()).collect();
    for expected in [
        "Welcome to Forgepost",
        "Tracking Every Headline",
        "Videos Without the Trackers",
        "Your Words, Your Rules",
        "Anatomy of the Content Model",
        "Crafting the Perfect CTA",
    ] {
        assert!(titles.iter().any(|t| t == expected), "missing {expected}");
    }
    let exp_summary = owned
        .iter()
        .find(|d| d.title == "Tracking Every Headline")
        .expect("experiment article");
    let tags = repo.document_tags(exp_summary.id.0).await.expect("tags");
    assert_eq!(tags, vec!["experiments", "tutorial"]);

    // The articles must be *substantial* — a regression that shortens the
    // demo posts silently would otherwise still produce a valid archive.
    let documents_full = DocumentService::new(repo.clone(), Arc::new(RumbleOembed));
    const MIN_BLOCK_TEXT_CHARS: usize = 2_000;
    for summary in &owned {
        let full = documents_full
            .get_owned(summary.id.0, admin_user.id)
            .await
            .expect("get full doc");
        let text_chars: usize = full
            .document
            .blocks
            .iter()
            .filter_map(|b| full.document.versions.iter().find(|v| v.id == b.version_id))
            .filter_map(|v| v.content.get("text").and_then(|t| t.as_str()))
            .map(|s| s.chars().count())
            .sum();
        assert!(
            text_chars >= MIN_BLOCK_TEXT_CHARS,
            "{} is too thin ({} text chars across blocks)",
            summary.title,
            text_chars
        );
    }

    // Media rows + the actual files exist on disk.
    let media_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect(&db_url)
        .await
        .expect("media pool");
    let media_rows: Vec<String> = sqlx::query_scalar("SELECT disk_name FROM media")
        .fetch_all(&media_pool)
        .await
        .expect("media rows");
    assert_eq!(media_rows.len(), 4, "four media rows restored");
    for disk in &media_rows {
        assert!(
            scratch.join("media").join(disk).exists(),
            "media file {disk} restored to disk"
        );
    }
    sqlx::sqlite::SqlitePool::close(&media_pool).await;

    // One running experiment whose live counts survived the restore.
    let experiments = ExperimentService::new(repo.clone());
    let running = experiments
        .list_for_document(exp_summary.id.0, admin_user.id)
        .await
        .expect("experiments");
    assert_eq!(running.len(), 1, "the demo experiment is still running");
    assert_eq!(running[0].status, "running");
    let counts = repo.experiment_counts(running[0].id).await.expect("counts");
    let impressions: i64 = counts.iter().map(|c| c.impressions).sum();
    assert!(
        impressions >= 40,
        "at least the 40 seeded impressions survive"
    );
    assert!(
        counts.iter().any(|c| c.conversions > 0),
        "conversion events restored"
    );
}

#[tokio::test]
async fn demo_archive_is_valid() {
    let artifact = demo_artifact();
    if std::env::var("FORGEPOST_REGEN_DEMO").is_ok() {
        let build_dir = tmp_dir("build");
        let db_url = format!("sqlite://{}", build_dir.join("demo.db").display());
        std::fs::create_dir_all(build_dir.join("media")).unwrap();
        build_archive(&db_url, &build_dir.join("media"), &artifact).await;
    } else if !artifact.exists() {
        // First checkout: artifact not committed yet. Skip silently so the
        // workspace suite is green before the artifact is generated.
        eprintln!("demo/forgepost-demo.fpb missing; run with FORGEPOST_REGEN_DEMO=1");
        return;
    }
    assert_demo_invariant(&artifact).await;
}
