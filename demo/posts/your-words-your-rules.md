# Your Words, Your Rules

Blog without a backup is a recipe for heartbreak. Every writer has a version of
this story: the platform stalls, the account gets locked, the export button
misbehaves, and suddenly a year of posts is a support ticket. Forgepost takes a
different stance. Your words live in a file you own, and getting them into a
portable, verifiable archive is one command.

{{img:archive:Archive unlocked}}

## What "your words" means when you self-host

Self-hosting buys you ownership of your *content*, but ownership is not the
same as safety. A single hard drive, a single `missed backup` in a script, a
single careless `rm` — the failure modes are mundane, and they are precisely
the failures the writing tools we grew up on made us forget exist. The antidote
is the same one for any serious dataset: a routine that produces restorable,
*tested* archives, on a schedule, to somewhere that isn't the same disk.

Forgepost makes the routine small enough to actually do it. The whole blog —
posts, blocks, versions, tags, settings, users, experiments, and every
analytics event — is a single SQLite file. Add the media directory next to it
and you have described the entire installation in two locations. An archive of
both is a faithful copy of the blog, not an approximation.

## The `.fpb` archive, mechanically

`forgepost backup create` produces a single self-describing ZIP archive with a
`.fpb` extension. The contents are:

- **`manifest.json`** — the format version (`format_version`), the Forgepost
  version that made the archive, the database schema version at backup time,
  and the list of media files inside.
- **`database.sqlite`** — not a raw copy of the live file. Taking a backup
  while the server is writing is a classic trap: the database uses a
  write-ahead log, and copying the main file alone can capture stale pages and
  leave out frames still sitting in the WAL. The backup command instead takes a
  consistent snapshot with SQLite's `VACUUM INTO` — a moment-in-time copy that
  is internally consistent even while the live database keeps working. The
  staging directory, and then the archive, receive that snapshot.
- **`media/*`** — every file from the media directory that is actually
  referenced by the database, stored under its safe UUID-based disk name.
- **`checksums.sha256`** — a `sha256sum`-style manifest over every entry in the
  archive.

Before the command declares success, it *verifies its own work*: it re-opens
the archive, checks every checksum, and runs a `PRAGMA integrity_check` over
the snapshot. A backup that fails its verification is deleted rather than
silently presented as good. The one-command rule of thumb: if `backup create`
does not print an OK verdict, the archive does not count.

> A backup you cannot restore is not a backup.

## Verifying before you trust

`forgepost backup verify` is the tool for the moment you actually need an
archive: it reads `manifest.json`, compares the format and schema versions with
your current installation, cross-checks every checksum, and runs the integrity
check on the database snapshot. Two kinds of failures come out of it:

- **Checksum or integrity failures** — the archive is damaged (a truncated
  upload, a bad disk). The report says so explicitly.
- **Version mismatches** — the archive was made by a Forgepost with a
  different schema or format version. The archive is probably fine, but it is
  not safely restorable into this binary, and the report says so rather than
  letting a restore fail confusingly halfway through.

Because the format is versioned and the check is explicit, an old archive
never *silently* becomes unreadable — it becomes loudly explainable.

## Restoring: the part most backup tools skimp on

A backup you cannot restore is not a backup. Restores are where the tool earns
or squanders its trust, and the restore path here is designed around one
principle: *never destroy the thing you are replacing.*

- **Dry-run first.** `forgepost backup restore archive.fpb --dry-run` runs the
  full verification and tells you exactly what a real restore would do, and
  writes nothing.
- **Explicit confirmation.** A real restore requires `--yes`. The server
  should be stopped first; restoring underneath a running server is not
  something the tool tries to make safe.
- **A rollback by default.** The pre-restore database is preserved next to the
  live file as `<name>.before-restore-<timestamp>`. If the restored state is
  somehow not what you wanted, the old world is still on disk, one rename away.
- **Media merge, not media nuke.** Restoring copies the archive's media files
  into your media directory additively. Files already present keep their
  names; nothing in your existing media is deleted by a restore. Archive
  files are written with a temp-then-rename so a crash mid-restore cannot
  leave a half-written image behind.

The version guard from `verify` runs again inside `restore` as a hard check: a
mismatched archive is refused before anything is written, no matter how the
command was invoked.

## A three-two-one rhythm that fits a solo blog

The discipline that actually protects a blog is boring and cheap:

1. **Three copies** — the live database, a local `.fpb` archive, and one off
   site.
2. **Two media** — the database and the media directory go into the same
   archive, so one `.fpb` is the whole blog.
3. **One command** — `forgepost backup create --output forgepost-$(date +%s).fpb`
   on a weekday cron, seconds of work, verified before it reports success.

When the monthly restore drill comes (and it should — a restore you never
practice is a backup you have never proven), the script is two lines:
`backup verify` on the cloud copy, then `backup restore --dry-run` against a
throwaway database to prove the end-to-end path works without touching the
real one.

## Emergencies are not the only reason

Archives earn their keep on happy days too, because the same file is how you
**move**:

- Trying a new domain or a fresh server? Ship a `backup create`, restore it,
  point the reverse proxy at the new install.
- Poking at the database in a way you might regret? Snapshot first, experiment
  confidently, and let the rollback be the safety net.
- Handing someone a copy of the blog? One `.fpb` is a complete, self-verifying
  deliverable — no folder zips, no "make sure you got the WAL too".

This very article, the experiment in the post next to it, and the images
bundled in this demo all travelled through that pipeline: the demo you are
reading was itself packaged as an archive and restored into a fresh database.
Proof, in the hand, that the loop closes.