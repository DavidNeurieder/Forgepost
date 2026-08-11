#!/usr/bin/env bash
#
# Forgepost nightly database export (JSON backup of the whole DB).
#
# SQLite runs in WAL mode, so this concurrent read is safe while the server
# is serving. Run standalone (works without the server), or via the
# forgepost-backup.timer service.
#
set -euo pipefail

DB="${FORGEPOST_BACKUP_DB:-sqlite:///var/lib/forgepost/forgepost.db}"
BACKUP_DIR="${FORGEPOST_BACKUP_DIR:-/var/lib/forgepost/backups}"
RETENTION_DAYS="${FORGEPOST_BACKUP_RETENTION_DAYS:-30}"

mkdir -p "$BACKUP_DIR"
/opt/forgepost/forgepost export \
	--database-url "$DB" \
	--output "$BACKUP_DIR/db-$(date +%F).json"

find "$BACKUP_DIR" -type f -name 'db-*.json' -mtime +"$RETENTION_DAYS" -delete
