//! Backup service: create, verify, and restore versioned snapshot archives
//! (`.fpb`) per `docs/backup.md` and the disaster-recovery policy.
//!
//! Restore is "replace, with a preserved rollback": the live database is
//! copied to `<db>.before-restore-<timestamp>` before the archive's snapshot
//! becomes active, so a bad restore always has an escape hatch. Nothing is
//! touched until the archive has passed every check (format, schema
//! compatibility, sha256 checksums, SQLite integrity).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ports::{BackupGateway, BackupManifest, BackupRepo, Repository};
use crate::services::ServiceError;
use forgepost_content::now_ms;

/// Result of an in-archive validation pass (also what `verify` reports).
#[derive(Debug, Clone)]
pub struct BackupReport {
    pub path: PathBuf,
    pub format_version: u32,
    pub schema_version: i64,
    pub media_files: usize,
    pub size_bytes: u64,
    pub ok: bool,
}

pub struct BackupService {
    repo: Arc<dyn BackupRepo>,
    gateway: Arc<dyn BackupGateway>,
}

impl BackupService {
    pub fn new(repo: Arc<dyn Repository>, gateway: Arc<dyn BackupGateway>) -> Self {
        Self { repo, gateway }
    }

    /// Seal a consistent snapshot of the live database plus every media file
    /// into a versioned `.fpb` archive at `dest`, then verify the result.
    pub async fn create(
        &self,
        database_url: &str,
        media_dir: &Path,
        dest: &Path,
    ) -> Result<BackupReport, ServiceError> {
        let parent = dest.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .map_err(|e| ServiceError::Internal(format!("cannot create output dir: {e}")))?;

        let stage = std::env::temp_dir().join(format!("forgepost-backup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&stage)
            .map_err(|e| ServiceError::Internal(format!("cannot create stage dir: {e}")))?;
        let result = self
            .create_inner(database_url, media_dir, dest, &stage)
            .await;
        std::fs::remove_dir_all(&stage).ok();
        result
    }

    async fn create_inner(
        &self,
        database_url: &str,
        media_dir: &Path,
        dest: &Path,
        stage: &Path,
    ) -> Result<BackupReport, ServiceError> {
        let created_at_ms = now_ms();
        let schema_version = self.repo.schema_version().await?;

        // 1. Consistent, self-validating database snapshot.
        let db_snap = stage.join(BackupManifest::DATABASE_ENTRY);
        self.gateway
            .snapshot_database(database_url, &db_snap)
            .await?;
        self.gateway.integrity_check(&db_snap).await?;
        let db_bytes = std::fs::read(&db_snap)?;

        // 2. Media inventory (sorted, sanitized names).
        let media = self.gateway.read_media_dir(media_dir)?;

        // 3. Manifest + checksums.
        let manifest = BackupManifest {
            format_version: BackupManifest::CURRENT_FORMAT_VERSION,
            forgepost_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version,
            created_at_ms,
            database: BackupManifest::DATABASE_ENTRY.to_string(),
            media: media.iter().map(|(name, _)| name.clone()).collect(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest)
            .map_err(|e| ServiceError::Internal(format!("serialization error: {e}")))?;

        let mut checksums = String::new();
        for (name, bytes) in
            std::iter::once((BackupManifest::MANIFEST_ENTRY.to_string(), &manifest_bytes))
                .chain(media.iter().map(|(name, bytes)| (name.clone(), bytes)))
        {
            checksums.push_str(&format!("{}  {name}\n", self.gateway.sha256_hex(bytes)));
        }
        // The database snapshot is hashed too, though it always lives in the
        // archive as `database.sqlite`.
        checksums.push_str(&format!(
            "{}  {}\n",
            self.gateway.sha256_hex(&db_bytes),
            BackupManifest::DATABASE_ENTRY
        ));

        // 4. Seal everything.
        let mut entries: Vec<(&str, &[u8])> = vec![
            (BackupManifest::MANIFEST_ENTRY, &manifest_bytes),
            (BackupManifest::DATABASE_ENTRY, &db_bytes),
        ];
        entries.extend(media.iter().map(|(n, b)| (n.as_str(), b.as_slice())));
        entries.push((BackupManifest::CHECKSUM_ENTRY, checksums.as_bytes()));
        self.gateway.write_archive(dest, &entries)?;

        // 5. The archive must satisfy its own contract.
        let report = self.verify(dest).await?;
        if !report.ok {
            return Err(ServiceError::Internal(format!(
                "backup failed self-verification at {}",
                dest.display()
            )));
        }
        Ok(report)
    }

    /// Validate an archive without touching the live database: manifest and
    /// format version, schema compatibility against the current store,
    /// sha256 checksums, and SQLite integrity of the embedded snapshot.
    pub async fn verify(&self, path: &Path) -> Result<BackupReport, ServiceError> {
        let entries = self.gateway.read_archive(path)?;
        let manifest = self.parse_manifest(&entries)?;
        let current_schema = self.repo.schema_version().await?;

        let mut ok = true;
        if manifest.format_version != BackupManifest::CURRENT_FORMAT_VERSION {
            ok = false;
        }
        let schema_match = manifest.schema_version == current_schema;
        if !schema_match {
            ok = false;
        }

        self.gateway.verify_checksums(&entries)?;

        let db_bytes = entries
            .iter()
            .find(|(name, _)| *name == BackupManifest::DATABASE_ENTRY)
            .ok_or_else(|| ServiceError::Validation("backup is missing database snapshot".into()))?
            .1
            .clone();
        let tmp = std::env::temp_dir().join(format!(
            "forgepost-integrity-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        self.gateway.replace_database(&tmp, &db_bytes)?;
        let integrity = self.gateway.integrity_check(&tmp).await;
        std::fs::remove_file(&tmp).ok();
        integrity?;

        Ok(BackupReport {
            path: path.to_path_buf(),
            format_version: manifest.format_version,
            schema_version: manifest.schema_version,
            media_files: manifest.media.len(),
            size_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            ok,
        })
    }

    /// Restore an archive. With `dry_run = true` only the validation pass runs
    /// (this is what the CLI's `--dry-run` flag wires up). Otherwise the
    /// previous database is preserved as `<db>.before-restore-<ts>` and media
    /// files are merged additively before the snapshot becomes active.
    pub async fn restore(
        &self,
        path: &Path,
        database_url: &str,
        media_dir: &Path,
        dry_run: bool,
    ) -> Result<BackupReport, ServiceError> {
        let mut report = self.verify(path).await?;
        if !report.ok {
            let detail = format!(
                "archive is not compatible with this installation \
                 (format_version={}, schema to restore ={}, current schema ={}); \
                 refusing to write before the report says ok",
                report.format_version,
                report.schema_version,
                self.repo.schema_version().await?
            );
            return Err(ServiceError::Validation(detail));
        }
        if dry_run {
            report.ok = true;
            return Ok(report);
        }

        let entries = self.gateway.read_archive(path)?;
        let manifest = self.parse_manifest(&entries)?;
        let db_bytes = entries
            .iter()
            .find(|(name, _)| *name == BackupManifest::DATABASE_ENTRY)
            .expect("verify already required the database snapshot")
            .1
            .clone();

        // Media first, database last: the snapshot swap is the pivot point, so
        // a failed media write never leaves a half-restored installation.
        for name in &manifest.media {
            let bytes = entries
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, b)| b.clone())
                .ok_or_else(|| ServiceError::Validation(format!("archive is missing {name}")))?;
            self.gateway.write_media_file(media_dir, name, &bytes)?;
        }

        let db_path = db_path_from_url(database_url);
        // Preserve the previous database as a consistent snapshot, not a raw
        // file copy: the live DB runs in WAL mode, so `fs::copy` of the main
        // file would silently drop uncheckpointed `-wal` frames.
        if self.gateway.path_exists(&db_path) {
            let rollback = db_path.with_file_name(format!(
                "{}.before-restore-{}",
                db_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                now_ms()
            ));
            self.gateway
                .snapshot_database(database_url, &rollback)
                .await?;
        }
        self.gateway.replace_database(&db_path, &db_bytes)?;
        Ok(report)
    }

    fn parse_manifest(
        &self,
        entries: &[(String, Vec<u8>)],
    ) -> Result<BackupManifest, ServiceError> {
        let bytes = entries
            .iter()
            .find(|(name, _)| *name == BackupManifest::MANIFEST_ENTRY)
            .map(|(_, b)| b)
            .ok_or_else(|| ServiceError::Validation("backup is missing manifest.json".into()))?;
        let manifest: BackupManifest = serde_json::from_slice(bytes)
            .map_err(|e| ServiceError::Validation(format!("corrupt manifest.json: {e}")))?;
        Ok(manifest)
    }
}

impl BackupReport {
    /// One line per check for the CLI's pretty report.
    pub fn summary_lines(&self) -> Vec<String> {
        let size = if self.size_bytes >= 1024 {
            format!("{:.1} KiB", self.size_bytes as f64 / 1024.0)
        } else {
            format!("{} B", self.size_bytes)
        };
        vec![
            format!("format:   v{}", self.format_version),
            format!("schema:   {}", self.schema_version),
            format!("objects:  {} media files", self.media_files),
            format!("size:     {}", size),
        ]
    }
}

/// Convert `sqlite://<path>` (the only scheme the CLI uses) to a filesystem path.
fn db_path_from_url(database_url: &str) -> PathBuf {
    let trimmed = database_url.trim().trim_start_matches("sqlite://");
    if trimmed.starts_with("file:") {
        trimmed.trim_start_matches("file:").into()
    } else {
        trimmed.into()
    }
}
