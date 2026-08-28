# Changelog

All notable changes to Forgepost are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-08-15

Video embeds (a new `video` block kind with click-to-load rendering,
privacy-first embed URLs, and first-class SEO), a show/hide password toggle on
the login and setup forms, and a hardening pass on the server's security
surfaces.

### Added

- **Video blocks** — a Markdown line that is exactly one YouTube or Rumble URL
  (`watch`/`shorts`/`embed`/`live`/`youtu.be`, Rumble watch or embed links) or
  a raw HTML `<iframe>` line parses into a `video` block. A URL embedded in
  prose stays a paragraph; iframe attributes are whitelisted (`src`, `title`,
  `width`, `height`) and `src` must be http(s), so a hand-crafted tag can
  never smuggle `javascript:`.
- **Click-to-load rendering** — a video block renders as a button with a lazy
  thumbnail and a play badge and *no iframe*, so the reader's browser never
  contacts the provider until they choose to play. Clicking swaps in the
  iframe via the new `/static/embed.js` (privacy-host YouTube embeds,
  `referrerpolicy="no-referrer"`, `allowfullscreen`).
- **Thumbnails** — YouTube's thumbnail is derived from the video id
  (`i.ytimg.com/vi/<id>/hqdefault.jpg`); Rumble's title and thumbnail are
  fetched once, best-effort, from Rumble's oEmbed endpoint at save time (3s
  timeout, non-fatal, idempotent — never minting new block versions).
- **Video SEO** — articles with a video block gain `og:video`,
  `og:video:type`, `og:video:secure_url`, and a JSON-LD `VideoObject` node
  (name, description, thumbnailUrl, uploadDate, embedUrl, contentUrl).
- **Editor** — an **Insert video** button opens a dialog for a URL or an
  `<iframe>` snippet and inserts it at the cursor; the blocks↔markdown
  round-trip preserves video blocks.
- Videos are not experimentable (their content is a single immutable URL).
- **Backup & restore** — `forgepost backup create` seals the database (a
  `VACUUM INTO` snapshot) and every media file into a single `.fpb` ZIP archive
  (`manifest.json` with format/schema versions, `checksums.sha256`), then
  self-verifies the result (integrity check + checksum pass). `backup verify`
  reports an archive's manifest, schema, checksums, and database integrity;
  `backup restore` merges media additively, replaces the live database, keeps
  the pre-restore database as `<name>.before-restore-<timestamp>`, refuses to
  write without `--yes`, and supports `--dry-run`. Archive format is versioned
  (`format_version`) so a future schema bump can refuse a mismatched restore.
  New `BackupRepo`/`BackupGateway` ports, `BackupService`, and the
  `ArchiveBackup` gateway (`crates/application`, `crates/infrastructure`), with
  disaster-recovery roundtrip tests in `crates/infrastructure/tests/backup_roundtrip.rs`.
- **Bundled demo blog** — `forgepost demo` installs a ready-made blog from the
  committed `demo/forgepost-demo.fpb` archive (it is an ordinary backup): six
  published articles with tags and bundled images, seeded analytics views, and
  a **live A/B experiment** on the "Tracking Every Headline" headline with
  40 assignment-consistent impressions. Fixed login
  `admin@example.com` / `demo-password`. The artifact is validated on every
  test run by `crates/server/tests/demo.rs` (restore + assert) and rebuilt
  deterministically with `FORGEPOST_REGEN_DEMO=1`.
- **Show/hide password** — an eye toggle reveals/obscures the password on
  `/login` and on both `/setup` password fields (`password` and `confirm`),
  with `aria-pressed`/`aria-label` state and a keyboard-visible focus ring.

### Changed

- Workspace version bumped to **0.2.0**.
- `crates/server/Cargo.toml` gains `reqwest` (main deps, json + rustls-tls) for
  the oEmbed fetch; video-block diffs compare by provider/id/url identity so
  refreshed metadata never creates a new immutable version.

### Security

- **Rate limits now key on the socket peer, never on forwarded headers** — a
  single attacker cannot spoof its way around the login or comment-service
  limit with a forged `X-Forwarded-For`; the header is only honored when the
  peer is a trusted proxy.
- **Layered security assurance suite** — a deterministic regression suite
  (authorization matrix, CSRF table, session/cookie lifecycle, proxy/IP
  handling, upload hardening) backed by proptest invariants over the rate
  limiter, markdown rendering, slugify, and ZIP import. See
  `docs/security-testing.md`; CI now runs `cargo audit` so a dependency
  vulnerability fails a merge.
- **Server-verified experiment assignment** — attribution is derived, never
  client-asserted: the events endpoint recomputes the deterministic variant
  assignment from the visitor cookie and rejects a variant the visitor was not
  actually given (400). Validated experiment events now record `version_id`,
  the exact immutable version the assigned variant pointed at, so conversion
  history is reproducible against the version pool.
- **One running experiment per block** — a partial unique index forbids
  starting a second experiment on a block that already has a running one; the
  server maps the violation to a clean `409 Conflict`.
- **Latch-guarded conclusions** — `conclude_experiment` transitions status in
  the same transaction that records the decision and rolls back if the latch
  is already closed, so a racy or double conclusion can never append a second
  decision row (the API already short-circuits at the service layer).

### Internal

- Extracted the monolithic server into `domain`/`application`/`infrastructure`
  crates (repository traits, service layer) and pinned e2e dashboard
  assertions to the posts table.
- `crates/server/src/main.rs` grows `backup` and `demo` subcommands.

## [0.1.0] - 2026-08-14

First release. The learn-MVP (publish + measure + experiment + dashboard) plus
the post-MVP wave — single-binary rendering, HTTPS, SEO, recommendations,
traffic sources, the game-feel dashboard, and share tracking — as a solo-mode
single binary.

### Added

**M1 — Thin blog host + activation**

- Argon2 password hashing, session cookies, CSRF protection, and a `/setup`
  wizard that creates the first admin account and then locks.
- Markdown editor that parses to an immutable block tree (`heading`,
  `paragraph`, `image`, `call_to_action`, `quote`, `code`, `divider`) with a
  live preview, plus a `POST /api/render` live preview endpoint.
- Documents with tags, publish/unpublish, public article rendering, comments
  with moderation, and an RSS feed.
- UI-created posts get their slug from the title while still a draft, instead
  of the old `untitled` placeholder; published slugs stay stable.
- `forgepost export` for JSON backups of the full database.
- AGPL-3.0 license and workspace scaffolding.
- Admin **Settings** page (`/admin/settings`) to change the blog name (shown
  in the header, page titles, home page, and RSS feed) and pick a site-wide
  theme: system (auto), light, dark, sepia, or solarized. Themes are applied
  via a `data-theme` attribute; the home page gains a header **Log in** link
  for anonymous visitors.
- **Default blog image** — the Settings page accepts an uploaded site-wide
  image (`/media/…`) used as the fallback social-card image for articles, the
  home page, and tag pages when a post has no image of its own; relative upload
  paths are absolutized so crawlers always see an absolute URL.
- **Post card thumbnails** — home and tag pages show each post's first
  resolvable image as a linked, lazy-loaded card thumbnail (12×8rem desktop,
  6.5×4.33rem mobile). The dashboard gained a **Created** column and its list
  is ordered newest-first by creation.

**M2 — Per-block analytics**

- Browser tracking script: banded scroll depth, article completion, read time,
  and per-block impressions, delivered through a rate-limited analytics API.
- Per-article stats (views, unique readers, average reading time, completion,
  scroll-depth funnel) and a per-block drop-off dashboard.
- Estimated numbers are explicitly labeled: blockers and JS-disabled readers
  are undercounted by design.
- **"Keep reading" recommendations** — every article ends with up to three
  related-post cards (before the comments), ranked by shared tags with the most
  recent posts as backfill. New `recommendation_impression` /
  `recommendation_click` events record what readers are shown and open
  (`analytics_events.recommended_slug`), and the ranking lives in
  `crates/server/src/recommender.rs` behind a visitor-aware signature ready for
  a future personalized engine.
- **Traffic sources** — the Stats page breaks down each article's views into
  Search / Direct / Community buckets (`classify_referrer` in
  `crates/server/src/analytics.rs`), using the `Referer` header that was
  already captured per event. Direct counts no-referrer and same-site visits;
  Search is a small allow-list of well-known engines.
- **Game-feel dashboard** — the admin dashboard opens with a "This week"
  section: a most-read-post callout, a per-post **Views (7d)** + **Δ vs last
  week** column pair, and a completion nudge pointing at the post with the
  worst read-through once it has enough reads to judge.
- **Share tracking** — articles gain a **Share** button (native share sheet
  when available, clipboard copy otherwise) that reports a new `share_click`
  event; the Stats page shows a **Shares** stat card.

**M3 — Block experiments**

- `forgepost-experiments` crate: a pure Bayesian engine with exact
  `P(beats control)` (beta posteriors), equal-tailed credible intervals, a
  spending-bound-corrected sequential-test confidence threshold, a no-winner
  stopping rule, and a control-bias-free traffic-split assignment.
- Engine correctness tests: golden cases (hand-computed beta probabilities)
  and property tests (posterior sanity, sample-size concentration, stopping
  rules, assignment honesty).
- Experiment CRUD with start / stop / decide / promote / conclude-no-winner
  routes; control is created automatically from the block's current immutable
  version; each variant writes a new immutable version.
- Stable per-visitor variant assignment with SSR/hydration-consistent
  rendering and `data-experiment-id` / `data-variant-id` attributes.
- `experiment_impression` and `experiment_conversion` events; per-variant
  counts drive the live Bayesian report.
- Background auto-decider that polls running experiments and applies a decision
  once the stopping rules fire (promote winner or conclude no improvement).
- Experiment decisions recorded (winner, promoted version, effect size,
  confidence) and included in exports.
- Admin dashboard experiments section: create experiments, live report with
  P(beats control) bars, and manual Decide / Promote best / No improvement /
  Stop actions.

**Operations**

- In-process HTTPS with two tiers: automatic Let's Encrypt issuance + renewal
  (`--tls-domain`) and bring-your-own certificates (`--tls-cert`/`--tls-key`,
  reloaded on change), plus a configurable HTTP→HTTPS 301 redirect listener
  and `Secure` session/visitor cookies under TLS.
- `tests/pages.rs` (server-rendered page flows) and `tests/tls.rs` (self-signed
  cert over a real socket); the e2e harness moved to `e2e/` at the repo root
  and drives the compiled binary directly.

### Changed

- **Single-binary migration** — SvelteKit/Vite/Vitest (`frontend/`) is gone.
  Rust + Askama renders every page (home, setup, login, dashboard, editor,
  stats, article, error) with POST-REDIRECT-GET; the `/api/*` JSON surface is
  unchanged. The Playwright e2e harness moved to `e2e/` at the repo root and
  now drives the compiled binary directly. See `docs/single_binary_plan.md`.
- **SEO head for articles, home, and tags** — canonical URL, meta description,
  Open Graph (`og:image` + `og:image:width/height`) and Twitter
  (`summary` / `summary_large_image`) cards, and JSON-LD now carry the post's
  image and dimensions (first resolvable image, else the site default). All
  rendered images are lazy-loaded with `decoding="async"`.
- **Migrations run through 0008** — `0007_recommendations.sql` adds
  `analytics_events.recommended_slug`; `0008_recommendation_visitor_index.sql`
  adds `visitor_id`-led indexes for the future interest engine.
- **`specification/scaling.md`** — new "Recommendations & scaling" section
  documenting the per-article read-path cost of ranking, the extra analytics
  writes, and the visitor index.

### Fixed

- API integration test asserted paragraph blocks were not experimentable; the
  engine's intent (any heading, paragraph, image, or CTA) was restored in the
  test.

### Notes

- MVP goal model is single: a "completion" is scrolling to the end of the
  article. Custom goals are deferred until post-G2.
- The admin create-experiment form supports text-content blocks (heading,
  paragraph, CTA); image variants are creatable via the API.
- Postgres storage, privacy/opt-in sharing tiers, and network percentile
  leaderboards are explicitly deferred beyond the MVP gates (see
  `docs/mvp_plan_v6.md`).

[0.2.0]: https://github.com/DavidNeurieder/Forgepost/releases/tag/v0.2.0
[0.1.0]: https://github.com/DavidNeurieder/Forgepost/releases/tag/v0.1.0
