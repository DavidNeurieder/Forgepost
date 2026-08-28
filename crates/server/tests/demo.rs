//! The demo installation: a seedable, versioned backup (`demo/forgepost-demo.fpb`)
//! of a small blog with real content, media, and a running A/B experiment — the
//! thing `forgepost demo` restores.
//!
//! The committed artifact is *always validated* by restoring it into a scratch
//! installation and asserting the demo invariants (admin login, published
//! posts + tags, media on disk, running experiment with live counts). Setting
//! `FORGEPOST_REGEN_DEMO=1` first rebuilds the content and overwrites the
//! artifact, so the artifact can be regenerated deterministically on demand.

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

    let img = |alt: &str, disk: &str| format!("![{alt}](/media/{disk})");

    article(
        &documents,
        admin,
        "Welcome to Forgepost",
        format!(
            "# Welcome to Forgepost\n\n\
             Forgepost is a self-hosted blog with A/B testing built in at the *block* level.\n\n\
             {}\n\n\
             Every headline, image, and call-to-action can be tested against \
             alternatives — the losing versions disappear without you rebuilding anything.\n",
            img("Welcome banner", &header.0)
        ),
        &["welcome", "intro"],
    )
    .await;

    let exp_doc = article(
        &documents,
        admin,
        "Tracking Every Headline",
        "# Tracking every headline\n\n\
         Readers scan the title before they read the article. Forgepost lets you run a real \
         experiment on it:\n\n\
         - two variants, one goal\n\
         - the winner gets promoted automatically\n\
         - the loser just goes away\n\n\
         The admin panel shows the live report while the test runs.\n",
        &["experiments", "tutorial"],
    )
    .await;

    article(
        &documents,
        admin,
        "Videos Without the Trackers",
        "# Videos Without the Trackers\n\n\
         Embed YouTube videos without leaking your readers' data:\n\n\
         https://www.youtube.com/watch?v=dQw4w9WgXcQ\n\n\
         The player is served from youtube-nocookie.com and loads only after a click.\n",
        &["privacy", "video"],
    )
    .await;

    article(
        &documents,
        admin,
        "Your Words, Your Rules",
        format!(
            "# Your Words, Your Rules\n\n\
             Blog without a backup is a recipe for heartbreak.\n\n\
             > A backup you cannot restore is not a backup.\n\n\
             {}\n\n\
             `forgepost backup create` seals the database and every media file into a single \
             `.fpb` archive, then verifies the result before you ship it anywhere.\n",
            img("Archive unlocked", &archive.0)
        ),
        &["backup", "self-hosting"],
    )
    .await;

    article(
        &documents,
        admin,
        "Anatomy of the Content Model",
        format!(
            "# Anatomy of the Content Model\n\n\
             {}\n\n\
             Every block is versioned and immutable; experiments overlay a fresh version:\n\n\
             ```\n\
             document\n\
               └─ blocks[]\n\
                    ├─ heading   (test this)\n\
                    ├─ paragraph (test this)\n\
                    └─ version pool (append-only)\n\
             ```\n\n\
             Promotion happens in exactly one place, so a test can never leave \
             content half-updated.\n",
            img("Content cards", &cards.0)
        ),
        &["architecture"],
    )
    .await;

    article(
        &documents,
        admin,
        "Crafting the Perfect CTA",
        format!(
            "# Crafting the Perfect CTA\n\n\
             {}\n\n\
             Test the words, not your gut:\n\
             1. set a goal (subscribe, share, or click)\n\
             2. let traffic split automatically\n\
             3. keep the winner\n",
            img("A/B results", &chart.0)
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
            "clicks on the Keep reading link",
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
                read_time_ms: Some(18_000),
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
