# Changelog

All notable changes to Forgepost are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Single-binary migration** — SvelteKit/Vite/Vitest (`frontend/`) is gone.
  Rust + Askama renders every page (home, setup, login, dashboard, editor,
  stats, article, error) with POST-REDIRECT-GET; the `/api/*` JSON surface is
  unchanged. The Playwright e2e harness moved to `e2e/` at the repo root and
  now drives the compiled binary directly. See `docs/single_binary_plan.md`.

### Added

- In-process HTTPS with two tiers: automatic Let's Encrypt issuance + renewal
  (`--tls-domain`) and bring-your-own certificates (`--tls-cert`/`--tls-key`,
  reloaded on change), plus a configurable HTTP→HTTPS 301 redirect listener
  and `Secure` session/visitor cookies under TLS.
- Editor live preview via `POST /api/render` (vanilla JS, no client build).
- UI-created posts get their slug from the title while still a draft, instead
  of the old `untitled` placeholder; published slugs stay stable.
- `tests/pages.rs` (server-rendered page flows), `tests/tls.rs` (self-signed
  cert over a real socket), and the relocated e2e suite.
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
- **"Keep reading" recommendations** — every article ends with up to three
  related-post cards (before the comments), ranked by shared tags with the most
  recent posts as backfill. New `recommendation_impression` /
  `recommendation_click` events record what readers are shown and open
  (`analytics_events.recommended_slug`), and the ranking lives in
  `crates/server/src/recommender.rs` behind a visitor-aware signature ready for
  a future personalized engine.

### Changed

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

## [0.1.0] - 2026-08-05

First release. The learn-MVP (publish + measure + experiment + dashboard) as a
solo-mode single binary.

### Added

**M1 — Thin blog host + activation**

- Argon2 password hashing, session cookies, CSRF protection, and a `/setup`
  wizard that creates the first admin account and then locks.
- Markdown editor that parses to an immutable block tree (`heading`,
  `paragraph`, `image`, `call_to_action`, `quote`, `code`, `divider`) with a
  live preview.
- Documents with tags, publish/unpublish, public article rendering, comments
  with moderation, and an RSS feed.
- `forgepost export` for JSON backups of the full database.
- AGPL-3.0 license and workspace scaffolding.

**M2 — Per-block analytics**

- Browser tracking script: banded scroll depth, article completion, read time,
  and per-block impressions, delivered through a rate-limited analytics API.
- Per-article stats (views, unique readers, average reading time, completion,
  scroll-depth funnel) and a per-block drop-off dashboard.
- Estimated numbers are explicitly labeled: blockers and JS-disabled readers
  are undercounted by design.

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

[0.1.0]: https://github.com/DavidNeurieder/my_blog/releases/tag/v0.1.0
