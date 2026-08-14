#!/usr/bin/env bash
#
# Forgepost nightly backup: JSON export of the whole DB plus a tarball of the
# uploaded media directory.
#
# SQLite runs in WAL mode, so the concurrent read is safe while the server
# is serving. Run standalone (works without the server), or via the
# forgepost-backup.timer service.
#
set -euo pipefail

DB="${FORGEPOST_BACKUP_DB:-sqlite:///var/lib/forgepost/forgepost.db}"
MEDIA_DIR="${FORGEPOST_BACKUP_MEDIA_DIR:-/var/lib/forgepost/media}"
BACKUP_DIR="${FORGEPOST_BACKUP_DIR:-/var/lib/forgepost/backups}"
RETENTION_DAYS="${FORGEPOST_BACKUP_RETENTION_DAYS:-30}"
BIN="${FORGEPOST_BACKUP_BIN:-/opt/forgepost/forgepost}"

mkdir -p "$BACKUP_DIR"
"$BIN" export \
	--database-url "$DB" \
	--output "$BACKUP_DIR/db-$(date +%F).json"

if [ -d "$MEDIA_DIR" ]; then
	tar -czf "$BACKUP_DIR/media-$(date +%F).tar.gz" -C "$(dirname "$MEDIA_DIR")" "$(basename "$MEDIA_DIR")"
fi

find "$BACKUP_DIR" -type f \( -name 'db-*.json' -o -name 'media-*.tar.gz' \) -mtime +"$RETENTION_DAYS" -delete
