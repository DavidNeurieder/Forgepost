//! Disaster-recovery round trip: seed a live installation, create a backup,
//! restore it elsewhere, and assert the storage surface matches — including
//! the replace-with-rollback behavior for an existing target database.

use std::path::PathBuf;
use std::sync::Arc;

use forgepost_application::ports::*;
use forgepost_application::services::backup::BackupService;
use forgepost_domain::model::Media;
use forgepost_infrastructure::backup::ArchiveBackup;
use forgepost_infrastructure::sqlite::SqliteRepository;

async fn repo(db_url: &str) -> SqliteRepository {
    SqliteRepository::connect(db_url).await.expect("open db")
}

async fn migrate(repo: &SqliteRepository) -> i64 {
    repo.migrate().await.expect("migrate");
    repo.schema_version().await.expect("schema version")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forgerten-fp-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn media_bytes() -> Vec<u8> {
    b"\x89PNG demo bytes".to_vec()
}

async fn seed(r: &SqliteRepository, media_dir: &std::path::Path) {
    let alice = r
        .create_first_user("alice@example.com", "Alice", "hash")
        .await
        .expect("user");
    let doc1 = r
        .create_document(alice.id, "Alpha Post")
        .await
        .expect("doc");
    r.set_document_tags(doc1.document.id, &["tutorial".into(), "rust".into()])
        .await
        .expect("tags");
    r.publish_document(doc1.document.id).await.expect("publish");
    let doc2 = r
        .create_document(alice.id, "Beta Notes")
        .await
        .expect("doc");
    r.publish_document(doc2.document.id).await.expect("publish");

    let disk_name = format!("{}.png", uuid::Uuid::new_v4());
    std::fs::write(media_dir.join(&disk_name), media_bytes()).unwrap();
    r.insert_media(&Media {
        id: uuid::Uuid::new_v4(),
        disk_name,
        content_type: "image/png".into(),
        size_bytes: media_bytes().len() as i64,
        sha256: ArchiveBackup.sha256_hex(&media_bytes()),
        created_at_ms: forgepost_content::now_ms(),
    })
    .await
    .expect("media");

    r.set_setting("site.name", "Roundtrip Blog")
        .await
        .expect("setting");
    r.set_setting("site.tagline", "round trip me")
        .await
        .expect("setting");
    r.set_setting("comments.enabled", "1")
        .await
        .expect("setting");
}

async fn assert_restored(db_url: &str, media_dir: &std::path::Path) {
    let r = repo(db_url).await;
    assert_eq!(migrate(&r).await, 9, "schema restored");
    let settings = r.site_settings().await.expect("settings");
    assert_eq!(settings.name, "Roundtrip Blog");
    assert!(settings.comments_enabled);

    let owner = r
        .find_user_by_email("alice@example.com")
        .await
        .expect("query")
        .expect("owner missing after restore");
    let docs = r.list_documents(owner.id).await.expect("list");
    let mut titles: Vec<String> = docs.iter().map(|d| d.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, vec!["Alpha Post", "Beta Notes"]);

    let media_rows = media_rows(&r).await;
    let disk_name = media_rows[0].clone();
    assert!(
        media_dir.join(&disk_name).exists(),
        "restored media file must exist on disk"
    );
}

async fn media_rows(r: &SqliteRepository) -> Vec<String> {
    sqlx::query_scalar("SELECT disk_name FROM media ORDER BY disk_name")
        .fetch_all(r.pool())
        .await
        .expect("media rows")
}

#[tokio::test]
async fn create_verify_restore_round_trip() {
    let src_dir = tmp_dir("src");
    let src_url = format!("sqlite://{}", src_dir.join("blog.db").display());
    let src_media = src_dir.join("media");
    std::fs::create_dir_all(&src_media).unwrap();

    let r = repo(&src_url).await;
    migrate(&r).await;
    seed(&r, &src_media).await;
    let src_schema = r.schema_version().await.expect("schema");

    let svc = BackupService::new(Arc::new(r) as Arc<dyn Repository>, Arc::new(ArchiveBackup));
    let dest = src_dir.join("blog-2026-08-28.fpb");
    let report = svc
        .create(&src_url, &src_media, &dest)
        .await
        .expect("create");
    assert!(report.ok);
    assert_eq!(report.schema_version, src_schema);

    // Archive contains manifest, snapshot, checksums, and the media file.
    let entries = ArchiveBackup.read_archive(&dest).unwrap();
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&BackupManifest::MANIFEST_ENTRY));
    assert!(names.contains(&BackupManifest::DATABASE_ENTRY));
    assert!(names.contains(&BackupManifest::CHECKSUM_ENTRY));
    assert_eq!(names.iter().filter(|n| n.starts_with("media/")).count(), 1);

    // verify() independently ok.
    let verify = svc.verify(&dest).await.expect("verify");
    assert!(verify.ok);

    // Restore into a brand-new database file.
    let dst_dir = tmp_dir("dst");
    let dst_url = format!("sqlite://{}", dst_dir.join("revived.db").display());
    let dst_media = dst_dir.join("media");
    std::fs::create_dir_all(&dst_media).unwrap();
    let restored = svc
        .restore(&dest, &dst_url, &dst_media, false)
        .await
        .expect("restore");
    assert!(restored.ok);
    assert_restored(&dst_url, &dst_media).await;

    // dry-run must not create anything.
    let dry_dir = tmp_dir("dry");
    let dry_url = format!("sqlite://{}", dry_dir.join("nope.db").display());
    svc.restore(&dest, &dry_url, &dry_dir.join("media"), true)
        .await
        .expect("dry-run");
    assert!(
        !dry_dir.join("nope.db").exists(),
        "dry-run wrote a database"
    );
}

#[tokio::test]
async fn restore_preserves_rollback_of_existing_database() {
    let src_dir = tmp_dir("rb-src");
    let src_url = format!("sqlite://{}", src_dir.join("blog.db").display());
    let src_media = src_dir.join("media");
    std::fs::create_dir_all(&src_media).unwrap();
    let r = repo(&src_url).await;
    migrate(&r).await;
    seed(&r, &src_media).await;
    let svc = BackupService::new(Arc::new(r) as Arc<dyn Repository>, Arc::new(ArchiveBackup));
    let dest = src_dir.join("snapshot.fpb");
    svc.create(&src_url, &src_media, &dest)
        .await
        .expect("create");

    // Pre-existing target with different content.
    let dst_dir = tmp_dir("rb-dst");
    let dst_url = format!("sqlite://{}", dst_dir.join("live.db").display());
    let dst_media = dst_dir.join("media");
    std::fs::create_dir_all(&dst_media).unwrap();
    let live = repo(&dst_url).await;
    migrate(&live).await;
    live.create_first_user("zed@example.com", "Zed", "hash")
        .await
        .expect("user");

    svc.restore(&dest, &dst_url, &dst_media, false)
        .await
        .expect("restore");

    // Rollback copy exists and still holds the previous content.
    let backups: Vec<PathBuf> = std::fs::read_dir(&dst_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains(".before-restore-"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(backups.len(), 1, "exactly one rollback file");
    let rollback = repo(&format!("sqlite://{}", backups[0].display())).await;
    assert!(
        rollback
            .find_user_by_email("zed@example.com")
            .await
            .unwrap()
            .is_some(),
        "rollback holds the pre-restore user"
    );

    // The live target now has the archive's content, not Zed.
    let now_live = repo(&dst_url).await;
    assert!(
        now_live
            .find_user_by_email("zed@example.com")
            .await
            .unwrap()
            .is_none(),
        "restore must replace, not merge"
    );
    assert!(
        now_live
            .find_user_by_email("alice@example.com")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn tampered_archive_fails_verification() {
    let dir = tmp_dir("tamper");
    let url = format!("sqlite://{}", dir.join("blog.db").display());
    let media = dir.join("media");
    std::fs::create_dir_all(&media).unwrap();
    let r = repo(&url).await;
    migrate(&r).await;
    seed(&r, &media).await;
    let svc = BackupService::new(Arc::new(r) as Arc<dyn Repository>, Arc::new(ArchiveBackup));
    let dest = dir.join("good.fpb");
    svc.create(&url, &media, &dest).await.expect("create");

    // Rewrite one media entry without updating its checksum.
    let mut entries = ArchiveBackup.read_archive(&dest).unwrap();
    for (name, bytes) in entries.iter_mut() {
        if name.starts_with("media/") {
            bytes[0] ^= 0xff;
        }
    }
    let tampered = dir.join("bad.fpb");
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    ArchiveBackup.write_archive(&tampered, &refs).unwrap();

    let err = svc.verify(&tampered).await.unwrap_err();
    assert!(
        err.to_string().contains("checksum mismatch"),
        "tamper should be caught by checksums, got: {err}"
    );

    // Restore refuses to touch the target for a tampered archive.
    let dst = format!("sqlite://{}", dir.join("denied.db").display());
    assert!(
        svc.restore(&tampered, &dst, &dir.join("m2"), false)
            .await
            .is_err()
    );
    assert!(
        !dir.join("denied.db").exists(),
        "restore must not write on failure"
    );
}
