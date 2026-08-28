//! Backup format primitives for the application's [`BackupGateway`] port.
//!
//! The archive is a versioned, deflated ZIP (`.fpb`):
//!
//! ```text
//! manifest.json      format/forgepost/schema version + media inventory
//! database.sqlite    consistent snapshot (VACUUM INTO) of the SQLite store
//! media/<name>       every file referenced by the `media` table
//! checksums.sha256   sha256 of each entry above (sha256sum-style lines)
//! ```
//!
//! Database snapshots are taken with SQLite's online `VACUUM INTO`, which
//! produces a consistent representation of the committed state even while the
//! live server is writing — never a raw `fs::copy` of the database file.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use forgepost_application::ports::{BackupGateway, BackupManifest, RepositoryError};
use sha2::{Digest, Sha256};

/// Stateless backup gateway.
#[derive(Debug, Default, Clone)]
pub struct ArchiveBackup;

/// `zip` is an infrastructure concern; surface its errors as `RepositoryError::Io`.
fn zip_err(err: zip::result::ZipError) -> RepositoryError {
    RepositoryError::Io(std::io::Error::other(err.to_string()))
}

/// Escape a SQL string literal (single-quote doubling) for `VACUUM INTO`.
fn sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

/// One archive entry we never expect to collide with user content.
const MEDIA_PREFIX: &str = "media/";
/// Regex-free sanity check for a media disk name: `<uuid>.<ext>`, no separators.
fn is_safe_disk_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 64
        || name.chars().any(|c| matches!(c, '/' | '\\'))
        || name.contains("..")
    {
        return false;
    }
    matches!(name.rsplit_once('.'), Some((uuid, ext)) if !uuid.is_empty() && !ext.is_empty())
}

/// Validate that `name` is a single path segment belonging under `dir`.
fn join_inside(dir: &Path, name: &str) -> Result<PathBuf, RepositoryError> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(RepositoryError::InvalidInput(format!(
            "unsafe media name {name:?}"
        )));
    }
    Ok(dir.join(name))
}

#[async_trait::async_trait]
impl BackupGateway for ArchiveBackup {
    async fn snapshot_database(
        &self,
        database_url: &str,
        dest: &Path,
    ) -> Result<(), RepositoryError> {
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        // `VACUUM INTO` refuses to overwrite an existing file.
        std::fs::remove_file(dest).ok();
        let target = sql_literal(&dest.to_string_lossy());
        sqlx::query(&format!("VACUUM INTO '{target}'"))
            .execute(&pool)
            .await?;
        Ok(())
    }

    async fn integrity_check(&self, file: &Path) -> Result<(), RepositoryError> {
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(&format!(
            "sqlite://{}",
            file.to_string_lossy()
        ))?
        .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let result: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await?;
        sqlx::sqlite::SqlitePool::close(&pool).await;
        if result.trim() != "ok" {
            return Err(RepositoryError::InvalidInput(format!(
                "database failed integrity check: {result}"
            )));
        }
        Ok(())
    }

    fn read_media_dir(&self, media_dir: &Path) -> Result<Vec<(String, Vec<u8>)>, RepositoryError> {
        let mut out = Vec::new();
        let mut names: Vec<String> = match std::fs::read_dir(media_dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        names.sort();
        for name in names {
            if !is_safe_disk_name(&name) {
                // Foreign files (logs, stray exports) belong to the operator's
                // directory; they are not part of a Forgepost backup.
                continue;
            }
            let bytes = std::fs::read(media_dir.join(&name))?;
            out.push((format!("{MEDIA_PREFIX}{name}"), bytes));
        }
        Ok(out)
    }

    fn write_archive(&self, dest: &Path, entries: &[(&str, &[u8])]) -> Result<(), RepositoryError> {
        let file = std::fs::File::create(dest)?;
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            writer.start_file(*name, opts).map_err(zip_err)?;
            writer.write_all(bytes)?;
        }
        writer.finish().map_err(zip_err)?;
        Ok(())
    }

    fn read_archive(&self, path: &Path) -> Result<Vec<(String, Vec<u8>)>, RepositoryError> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;
        let mut out = Vec::with_capacity(archive.len());
        for idx in 0..archive.len() {
            let mut entry = archive.by_index(idx).map_err(zip_err)?;
            let name = entry.name().to_string();
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes)?;
            out.push((name, bytes));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn verify_checksums(&self, entries: &[(String, Vec<u8>)]) -> Result<(), RepositoryError> {
        let Some(sum) = entries
            .iter()
            .find(|(n, _)| n == BackupManifest::CHECKSUM_ENTRY)
        else {
            return Err(RepositoryError::InvalidInput(
                "backup is missing checksums.sha256".into(),
            ));
        };
        let mut expected: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let spec = String::from_utf8(sum.1.clone())
            .map_err(|_| RepositoryError::InvalidInput("checksums file is not utf-8".into()))?;
        for line in spec.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((hash, name)) = trimmed.split_once("  ") else {
                return Err(RepositoryError::InvalidInput(format!(
                    "malformed checksum line: {trimmed:?}"
                )));
            };
            expected.insert(name.to_string(), hash.trim().to_lowercase());
        }
        for (name, bytes) in entries {
            if name == BackupManifest::CHECKSUM_ENTRY {
                continue;
            }
            let want = expected.get(name).ok_or_else(|| {
                RepositoryError::InvalidInput(format!("no checksum recorded for {name}"))
            })?;
            let got = self.sha256_hex(bytes);
            if &got != want {
                return Err(RepositoryError::InvalidInput(format!(
                    "checksum mismatch for {name}"
                )));
            }
        }
        Ok(())
    }

    fn sha256_hex(&self, bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn write_media_file(
        &self,
        media_dir: &Path,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), RepositoryError> {
        std::fs::create_dir_all(media_dir)?;
        let disk_name = name.strip_prefix(MEDIA_PREFIX).unwrap_or(name);
        if !is_safe_disk_name(disk_name) {
            return Err(RepositoryError::InvalidInput(format!(
                "unsafe media name {name:?}"
            )));
        }
        let path = join_inside(media_dir, disk_name)?;
        atomic_write(&path, bytes)
    }

    fn replace_database(&self, dest: &Path, bytes: &[u8]) -> Result<(), RepositoryError> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Stale write-ahead-log artifacts from the previous live database
        // would be replayed against the restored file; remove them first.
        let wal = wal_path(dest);
        std::fs::remove_file(&wal).ok();
        std::fs::remove_file(shm_path(dest)).ok();
        atomic_write(dest, bytes)
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

fn wal_path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-wal");
    PathBuf::from(p)
}

fn shm_path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-shm");
    PathBuf::from(p)
}

/// fsync + atomic rename so a crash never leaves a torn file at `dest`.
fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<(), RepositoryError> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        dest.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into()),
        uuid::Uuid::new_v4()
    ));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(err) => {
            std::fs::remove_file(&tmp).ok();
            Err(err.into())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use forgepost_application::ports::BackupManifest;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "forgepost-backup-test-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_entries(names: &[&str], entries: &[(String, Vec<u8>)]) {
        let got: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(got, names, "archive entries differ from expectation");
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        let gw = ArchiveBackup;
        assert_eq!(
            gw.sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn archive_round_trips_entries() {
        let gw = ArchiveBackup;
        let dir = tmp_dir("archive");
        let zip_path = dir.join("out.fpb");
        gw.write_archive(
            &zip_path,
            &[
                ("manifest.json", b"{\"a\":1}\n"),
                ("database.sqlite", b"\x00\x01sqlite"),
                ("media/x.png", b"png-bytes"),
            ],
        )
        .unwrap();
        let entries = gw.read_archive(&zip_path).unwrap();
        assert_entries(
            &["database.sqlite", "manifest.json", "media/x.png"],
            &entries,
        );
        assert_eq!(&entries[2].1, b"png-bytes");
    }

    #[test]
    fn checksums_verify_when_intact_and_fail_on_tamper() {
        let gw = ArchiveBackup;
        let data = [("a-filename.txt".to_string(), b"hello".to_vec())];
        let sum = format!("{}  a-filename.txt\n", gw.sha256_hex(b"hello"));
        let mut entries = data.to_vec();
        entries.push(("checksums.sha256".to_string(), sum.into_bytes()));
        gw.verify_checksums(&entries).unwrap();

        // Bit flip in the data must fail verification.
        let mut tampered = entries.clone();
        tampered[0].1[0] ^= 0xff;
        let err = gw.verify_checksums(&tampered).unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidInput(_)));

        // Missing checksums entry must fail.
        let err = gw
            .verify_checksums(&[("x".to_string(), vec![])])
            .unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidInput(_)));
    }

    #[test]
    fn checksums_reject_missing_entry() {
        let gw = ArchiveBackup;
        let err = gw
            .verify_checksums(&[("database.sqlite".to_string(), vec![1])])
            .unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidInput(_)));
    }

    #[test]
    fn media_dir_reads_safe_files_and_skips_foreign_ones() {
        let gw = ArchiveBackup;
        let dir = tmp_dir("mediadir");
        std::fs::write(dir.join("abc-123.png"), vec![1, 2, 3]).unwrap();
        std::fs::write(dir.join("notes.txt"), vec![9]).unwrap();
        // A file inside the media dir that fails the sanity check (no ext).
        std::fs::write(dir.join("orphan"), vec![7]).unwrap();
        let media = gw.read_media_dir(&dir).unwrap();
        assert_eq!(
            media,
            vec![
                ("media/abc-123.png".to_string(), vec![1, 2, 3]),
                ("media/notes.txt".to_string(), vec![9]),
            ]
        );
    }

    #[test]
    fn media_write_rejects_path_traversal() {
        let gw = ArchiveBackup;
        let dir = tmp_dir("mediawrite");
        for bad in ["../evil.png", "a/b.png", ".hidden.png", "..png"] {
            let err = gw.write_media_file(&dir, bad, b"x").unwrap_err();
            assert!(
                matches!(err, RepositoryError::InvalidInput(_)),
                "name {bad:?} should be rejected"
            );
        }
        gw.write_media_file(&dir, "media/abc-123.png", b"ok")
            .unwrap();
        assert_eq!(std::fs::read(dir.join("abc-123.png")).unwrap(), b"ok");
    }

    #[tokio::test]
    async fn snapshot_integrity_round_trip() {
        let gw = ArchiveBackup;
        let dir = tmp_dir("snapshot");
        let db_url = format!("sqlite://{}", dir.join("live.db").display());

        // Seed a live database.
        {
            let opts = sqlx::sqlite::SqliteConnectOptions::from_str(&db_url)
                .map_err(|e| panic!("bad url: {e}"))
                .unwrap()
                .create_if_missing(true);
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await
                .unwrap();
            sqlx::query("CREATE TABLE t (v TEXT)")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO t VALUES ('hello')")
                .execute(&pool)
                .await
                .unwrap();
            let v: String = sqlx::query_scalar("SELECT v FROM t")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(v, "hello");
            sqlx::sqlite::SqlitePool::close(&pool).await;
        }
        assert!(dir.join("live.db").exists());

        // Snapshot into a sibling file and validate it in isolation.
        let snap = dir.join("snap.db");
        gw.snapshot_database(&db_url, &snap).await.unwrap();
        gw.integrity_check(&snap).await.unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite://{}", snap.display()))
            .await
            .unwrap();
        let v: String = sqlx::query_scalar("SELECT v FROM t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(v, "hello");
        sqlx::sqlite::SqlitePool::close(&pool).await;

        // A torn file fails integrity_check.
        std::fs::write(&snap, b"not a database").unwrap();
        assert!(gw.integrity_check(&snap).await.is_err());
    }

    #[tokio::test]
    async fn replace_database_is_atomic_and_drops_wal_shm() {
        let gw = ArchiveBackup;
        let dir = tmp_dir("replace");
        let db = dir.join("app.db");
        std::fs::write(&db, b"old").unwrap();
        std::fs::write(dir.join("app.db-wal"), b"stale-wal").unwrap();
        std::fs::write(dir.join("app.db-shm"), b"stale-shm").unwrap();

        gw.replace_database(&db, b"new-content").unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), b"new-content");
        assert!(
            !dir.join("app.db-wal").exists(),
            "stale -wal must be cleared"
        );
        assert!(
            !dir.join("app.db-shm").exists(),
            "stale -shm must be cleared"
        );
    }

    #[test]
    fn manifest_constants_self_consistent() {
        assert_eq!(BackupManifest::DATABASE_ENTRY, "database.sqlite");
        assert_eq!(BackupManifest::MANIFEST_ENTRY, "manifest.json");
        assert_eq!(BackupManifest::CHECKSUM_ENTRY, "checksums.sha256");
    }
}
