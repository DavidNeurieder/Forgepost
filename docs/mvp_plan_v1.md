# OpenPublish MVP Plan v1

## 1. Vision

Reframed around the wedge. The product is **block-level experimentation over immutable content**: "publish → A/B test → measure → improve," where every headline, image, CTA, and paragraph is a measurable, experimentable object.

The blog is the *thin host* for the optimization loop, not the product itself. The genuinely new thing (the "zero to one") is block-level experiments as overlays on an immutable, versioned document tree — canonical content is never mutated. No one ships this natively and self-hosted today.

It must also be **intuitive for non-technical creators**: installable in minutes with one command, dashboard readable at a glance.

## 2. Success test (revised)

The question being tested: *"Will creators engage with a measurable optimization loop?"*

Success is measured by engagement with the loop on a small set of higher-traffic creators, not raw writer count:

- A handful of creators (5–10) with meaningful traffic install it.
- Experiments run weekly; each active article has ≥1 experiment in its first month.
- Experiments reach a decision (probabilistic report or promoted winner) instead of lingering.
- Per-block analytics drive ≥1 concrete article improvement per creator per month (e.g., "Section 3 completion +12%").
- A non-technical writer goes from install to first published article and first running experiment without reading any docs.
- **Onboarding gate:** scripted acceptance test — first-time user installs solo mode and runs one experiment in ≤15 minutes, no docs (measured in M5).

If creators check dashboards and act on the numbers, the foundation is proven.

## 3. MVP scope

### Lead with the wedge

1. **Block-level experiments (primary)** — headline, image, CTA, and paragraph variants as overlays on immutable block versions.
2. **Per-block analytics (primary)** — completion, reading time, retention attributed to blocks and sections, not just whole articles.
3. **Thin blog host** — users, profiles, Markdown editor, publish, tags/categories, comments, RSS, themes. Just enough to power the loop.
   - **Editor scope (hard cap):** author in Markdown with live preview; Markdown is parsed into the block tree on save, and blocks remain the canonical stored form. No drag-drop block editor, no plugin/theme engine in the MVP.
   - **Theme scope (hard cap):** one bundled default theme + CSS-variable customization; no theme-loading API in the MVP.
4. **Conversion goals** — signup-form and CTA blocks, simple funnel analytics, email-provider integration (Listmonk/Mailcoach/Mailchimp/Brevo/ConvertKit).
5. **Creator competition (simple)** — reinforces the "compete" loop:
   - **Server mode:** leaderboards — growth, retention, best experimenter.
   - **Solo mode:** "beat your own best" — per-article score vs. the creator's rolling average, improvement streaks, no social comparison needed.
6. **Painless install & intuitive UX** — non-technical is a pillar, not a feature:
   - **Two install paths, one product:**
     - **Solo mode** — a single self-contained binary with embedded SQLite (`./openpublish serve`, `/setup` wizard at `localhost:8080`). No Docker, no database, no terminal config. Ideal for one writer.
     - **Server mode** — one-command install (`install.sh`) that provisions Docker + Postgres + app behind the scenes; auto-HTTPS via Caddy. For shared/community servers.
   - Web-based setup wizard: admin account, site name/theme, first post — no config files, no env editing.
   - Plain-language analytics with tooltips; raw metrics collapsed behind "show technical details."
   - Guided first-experiment wizard that explains the report in one sentence.
   - Progressive disclosure — the lab never blocks basic publishing.
7. **Open-source foundation** — AGPL-3.0 core, monorepo.

### Explicitly out (extension layers)

Federation · global search engine · video hosting · books · AI writing assistant · plugin marketplace · social network · email sending (via provider API instead).

## 4. Architecture

```
openpublish/
├── crates/
│   ├── server        # Axum API + core engine
│   ├── content       # document/block model (immutable versions)
│   ├── analytics     # event collection + per-block metrics
│   ├── experiments   # A/B engine (Bayesian + sequential)
│   ├── protocol      # (deferred)
│   └── federation    # (deferred)
├── frontend/         # SvelteKit (thin blog + dashboard)
├── migrations/       # per-driver SQLx migrations: migrations/postgres + migrations/sqlite
├── docker/           # all-in-one compose (app + Postgres + Caddy auto-TLS) + install.sh
└── docs/
```

- **Backend:** Rust + Axum + Tokio + SQLx (compile-time checked SQL). **Storage is driver-agnostic via a repository layer:** PostgreSQL (server mode) or SQLite embedded (solo mode). The binary is built with `--features postgres` or `--features sqlite`; the `content`/`analytics`/`experiments` logic is shared and driver-independent.
- **Frontend:** SvelteKit.
- **Analytics:** browser → Rust event API → DB (SQLite solo, Postgres server mode); ClickHouse later.
- **License:** core AGPL-3.0; SDKs MIT/Apache-2.0 later.
- **Setup:** two install paths — solo mode is a single downloaded binary (`./openpublish serve`); server mode is `install.sh` (Docker + Postgres + Caddy with auto-TLS). Both expose a first-boot `/setup` wizard (admin bootstrap). All config has safe defaults — no config files required for 90% of installs.
- **Growth path + backups:** `./openpublish export` produces a portable dump (content + versions + events + settings). It is the backup mechanism for non-technical users and the documented path to migrate a solo SQLite install to Postgres server mode (export → import). No lock-in between modes.

## 5. Data model

The schema below is carried forward verbatim from `docs/mvp_plan.md` §5.1, with two v1 additions driven by the reframe:

- **`experiment_decisions`** — append-only conclusions: chosen variant, probability, sample counts, timestamp. Reports survive re-runs and feed the "best experimenter" leaderboard.
- **`blocks.updated_at`** — per-block change signal for "this block improved after experiment" tracking.

### 5.1 Content storage (database schema)

**Principles:** store the semantic document tree in the DB and derive all formats (HTML/EPUB/PDF/search) from it · granularity stops at block level (no sentence/word rows) · `block_versions` are immutable (insert-only, never UPDATE) · block-level analytics enabled.

**Tables:**

```
users
  id              uuid pk
  username        text unique
  email           text unique
  password_hash   text
  display_name    text
  bio             text
  avatar_asset_id uuid fk assets
  created_at      timestamptz

documents            -- article, landing page, about page; NEVER hard-deleted
  id              uuid pk
  author_id       uuid fk users
  content_type    text   -- 'article' | 'landing_page'
  slug            text   -- url path, unique per author
  status          text   -- 'draft' | 'published' | 'archived'
  published_at    timestamptz
  created_at      timestamptz
  updated_at      timestamptz

blocks               -- ordered nodes inside a document
  id                   uuid pk
  document_id          uuid fk documents
  type                 text   -- heading, paragraph, image, code, quote, table, cta, signup_form, list
  position             int    -- order in document
  published_version_id uuid fk block_versions  -- current published snapshot (nullable)
  created_at           timestamptz
  updated_at           timestamptz   -- v1: per-block change signal

block_versions         -- IMMUTABLE content snapshots
  id         uuid pk
  block_id   uuid fk blocks
  status     text   -- 'published' | 'draft' | 'archived'
  content    jsonb  -- typed payload: {text}, {markdown}, {url}, {alt}, ...
  asset_id   uuid fk assets   -- for image/media blocks
  created_by uuid fk users
  created_at timestamptz

assets
  id           uuid pk
  owner_id     uuid fk users
  kind         text   -- image | file
  storage_path text
  mime_type    text
  size_bytes   bigint
  created_at   timestamptz

experiments          -- overlays; never mutate canonical blocks
  id                   uuid pk
  document_id          uuid fk documents
  name                 text
  goal                 text   -- max_completion | max_ctr | max_signups
  status               text   -- 'running' | 'concluded' | 'archived'
  confidence_threshold numeric
  concluded_at         timestamptz
  created_at           timestamptz

experiment_variants   -- each variant POINTS TO an existing block_version
  id             uuid pk
  experiment_id  uuid fk experiments
  block_id       uuid fk blocks
  version_id     uuid fk block_versions   -- same immutable pool as published content
  traffic_weight int                       -- relative split
  is_control     boolean

experiment_decisions  -- v1: append-only experiment conclusions
  id            uuid pk
  experiment_id uuid fk experiments
  winner_version_id uuid fk block_versions
  probability   numeric   -- P(winner beats control) at conclusion
  impressions   int       -- per-variant sample counts (jsonb)
  promoted      boolean   -- whether winner was auto-promoted to published
  created_at    timestamptz

analytics_events      -- append-only; PARTITIONED by month
  id            bigserial pk
  event         text   -- page_view | article_scroll | article_read | experiment_impression |
                       -- experiment_conversion | signup | download
  document_id   uuid fk documents
  block_id      uuid fk blocks
  experiment_id uuid fk experiments
  variant_id    uuid fk experiment_variants
  visitor_id    text   -- anonymous cookie/stable hash
  referer       text
  user_agent    text
  payload       jsonb  -- {percentage}, {duration}, {completion}, ...
  created_at    timestamptz

tags
  id   uuid pk
  name text unique

document_tags          -- many-to-many; categories are parent tags
  document_id uuid fk documents
  tag_id      uuid fk tags
  pk (document_id, tag_id)

comments
  id          uuid pk
  document_id uuid fk documents
  author_id   uuid fk users
  parent_id   uuid fk comments   -- threads
  body        text
  status      text   -- 'visible' | 'hidden' | 'spam'
  created_at  timestamptz

follows                -- follow creators / subscribe to a document
  follower_id uuid fk users
  followee_id uuid fk users
  document_id uuid fk documents   -- nullable; set when following a document
  created_at  timestamptz
  pk (follower_id, followee_id, document_id)

leads                  -- signup-form conversions (M4)
  id            uuid pk
  document_id   uuid fk documents
  email         text
  source_event  uuid fk analytics_events
  conversion_state text  -- 'subscribed' | 'downloaded' | 'purchased'
  provider_id   text   -- subscriber id returned by the email provider
  created_at    timestamptz
```

- **Identity:** plain UUID + version column; no content-addressable hashes in the MVP (add for federation/replication later).
- **Driver-parameterized schema:** the logical schema above is identical in both modes; DDL differs per driver. Postgres uses `jsonb`, `bigserial`, and monthly range partitioning; SQLite stores JSON as TEXT (queried with `json_extract`) and `INTEGER PRIMARY KEY AUTOINCREMENT`, with no partitioning. Migrations are maintained per driver (`migrations/postgres`, `migrations/sqlite`); partial unique indexes and `ON DELETE SET NULL` behave the same in both.
- **Metrics:** aggregate on read with SQL over `analytics_events`; no scheduled pre-aggregation jobs in the MVP.
- **Indexes:** `analytics_events(document_id, event, created_at)`, `blocks(document_id, position)`, `block_versions(block_id, status)`.
- **Constraints (enforced in DB, not just app code):**
  - Partial unique index on `blocks(published_version_id) WHERE published_version_id IS NOT NULL` — exactly one published version per block.
  - Unique `(author_id, slug)` on `documents`.
  - Unique `(experiment_id, block_id)` on `experiment_variants` — one variant per block per experiment.
- **Delete semantics:** documents transition to `status: 'archived'`, never hard-deleted (preserves history + analytics). `analytics_events` FK refs use `ON DELETE SET NULL` so events survive doc/block cleanup. RSS, leaderboards, and funnels are derived queries, not tables.

### 5.2 Events & experiment statistics (updated for low traffic)

- **Probabilistic reporting (default).** Use a Bayesian beta-binomial posterior. At any point, report *P(variant B beats control)* — e.g., "B likely better, 63%" — rather than waiting for a confident winner. The dashboard always shows a decision-relevant signal, which keeps the loop alive on small samples.
- **Sequential testing (promotion).** When confidence crosses `confidence_threshold` under sequential analysis (spending-bound adjusted — no peeking-inflation), auto-promote by creating a new published `block_version` and archiving the experiment. Minimum sample size (≥100 impressions/variant) guards promotion but can be overridden when the creator is running with probabilistic reporting on.
- **No-winner stopping rule.** Every experiment gets a decision. Conclude as "no improvement" (archive with the control retained) when *P(variant beats control) < 5%* or after a configurable max duration (e.g., 30 days). Prevents low-traffic experiments lingering forever.
- **Traffic split:** `hash(visitor_id, experiment_id)` weighted by `traffic_weight`; weights are finalized before changes (mid-run re-weighting invalidates buckets).
- **Scroll tracking:** banded `article_scroll` (25/50/75/100%) + one `article_read` per session; events range-partitioned by month with a configurable retention window (Postgres server mode only — solo SQLite mode needs no partitioning or retention).
- **Block attribution:** map scroll depth to block boundaries from stored block order at read time — approximate in MVP (documented assumption).
- **Privacy note:** visitor tracking (cookie + referer + user agent) is opt-out via a per-site consent flag; self-hosted owners decide.

**Render flow:** documents → ordered blocks → resolve `published_version_id` → substitute running-experiment variant by `hash(visitor_id, experiment_id)` weighted by `traffic_weight` → render.

### 5.3 Security & trust

- **Auth:** argon2 password hashing; http-only session cookies; CSRF protection; per-user scoping on every API route (no IDOR).
- **Analytics API is a public write endpoint** — rate-limit by visitor/IP and validate payload shape/size; treat it as unauthenticated input.
- **Comment spam:** moderation queue via `comments.status` ('visible' | 'hidden' | 'spam'), rate-limited posting.
- **Uploads:** validate mime/type/size on `assets`; serve via a dedicated route (no path traversal).
- **Honest numbers for non-technical users:** label unique-reader and bot-filtered figures as "estimated"; ad-blockers will undercount and the UI should say so.

## 6. Milestones

### M0 — Scaffolding

- Cargo workspace: `server`, `content`, `analytics`, `experiments` crates.
- Repository/storage abstraction layer; `postgres` + `sqlite` features and per-driver migrations.
- SvelteKit frontend shell; all-in-one Docker compose (app + Postgres + Caddy) + `install.sh`; single-binary solo build; CI; AGPL license; repo structure.

### M1 — Thin blog host

- Auth, users, profiles (argon2, session cookies, CSRF).
- Markdown editor with live preview → parses to the Block/BlockVersion model on save; blocks are the canonical stored form.
- Publish articles, tags/categories, comments (moderation queue), RSS, one bundled theme + CSS-variable customization.
- First-boot `/setup` wizard: admin account, site name/theme, first post — no config files.
- Explicitly framed as the host for the loop, not the product.

### M2 — Per-block analytics (the wedge, part 1)

- Event collection API + browser tracking (scroll depth, completion, read time).
- **Per-block and per-article** aggregations: views, unique readers, avg reading time, completion %, retention, referral source.
- Dashboard surfaces block-level drop-off first-class (e.g., "readers leave at Section 3").
- Plain-language metric labels + tooltips; default dashboard view hides raw numbers behind "show technical details."

### M3 — Block experiments (the wedge, part 2)

- Create an experiment on any block (headline, image, CTA, paragraph).
- Traffic split + stable variant assignment per visitor.
- **SSR/hydration consistency:** variant assignment is computed server-side with the visitor cookie set *before* first render, so SvelteKit hydration never mismatches.
- Impression/conversion tracking.
- Bayesian probabilistic reporting + sequential-test promotion → new published BlockVersion; `experiment_decisions` recorded.
- **Stats-engine correctness (explicit deliverable):** golden + property tests — simulated experiments with known ground truth, no-better-than-control cases, sequential spending-bound behavior, and the no-winner stopping rule. A silent stats bug would poison every decision the platform makes.

### M4 — Conversion goals

- Signup-form and CTA blocks, landing pages.
- Simple funnel analytics (visit → read → signup → download → purchase).
- Email-provider integration via API (track conversions; provider handles sending).

### M5 — Leaderboard + polish

- Server mode: creator rankings — fastest growth, best retention, best experimenter (from `experiment_decisions`). Solo mode: "beat your own best" views (per-article vs. rolling average, improvement streaks).
- Solo mode: download page + `./openpublish serve` quickstart. Server mode: one-command `install.sh`. `./openpublish export` (backups + SQLite→Postgres migration path). Onboarding checklist; first-post and first-experiment wizards; one-page non-technical README.
- **Onboarding acceptance gate:** scripted test — a first-time user installs solo mode and runs one experiment in ≤15 minutes with no docs; run as part of M5 acceptance.
- Note: true no-terminal installs need managed hosting — deferred; the MVP bar is one command + wizard.

## 7. Key design decisions to protect early

- **Experiments-first, metrics-first:** the blog is a thin host; the loop is the product.
- **Immutable BlockVersions + experiments as overlays** — foundation for everything later (federation, replication, rollback).
- **Store the semantic document; derive all formats** (HTML/EPUB/PDF/search) from it.
- **Probabilistic reporting built in from day one** so experiments work at low traffic.
- **Storage-agnostic core:** one shared engine behind a repository layer — SQLite solo binary and Postgres server mode, same logic, same features.
- **No lock-in between modes:** `export`/import makes backups and solo→server migration first-class.
- **Correctness of the stats engine is a first-class deliverable:** simulated-experiment tests, not just unit tests, for the Bayesian/sequential engine.
- **Zero-config defaults, plain-language UX, progressive disclosure** — non-technical is a pillar, not a feature.
- **API-first:** web/desktop/mobile/clients consume the same API.

## 8. Licensing & third-party compliance

### Project license strategy

- **Core server** (`crates/server`): **AGPL-3.0** — network copyleft (§13) so a company cannot take the platform, add proprietary features, and offer it as a closed SaaS.
- **Protocol / federation crates** (later): AGPL-3.0, with an Apache-2.0 re-license consideration before release.
- **Official clients** (desktop/mobile, later): AGPL-3.0.
- **SDKs:** Apache-2.0 or MIT (permissive to maximize adoption).
- **Themes / plugins:** creator's choice (per spec).
- Deliberate choice: **AGPL-3.0, not GPLv3** — plain GPLv3 does not require source disclosure for network services; AGPL does. All components below are compatible with both.

### Third-party components — all permissive or separate services

| Component | License | Notes |
|---|---|---|
| Rust | MIT OR Apache-2.0 | compatible |
| Axum | MIT OR Apache-2.0 | compatible |
| Tokio | MIT | compatible |
| SQLx | MIT OR Apache-2.0 | compatible |
| Svelte / SvelteKit | MIT | compatible |
| Vite | MIT | compatible |
| Caddy | Apache-2.0 | compatible |
| SQLite (solo mode) | public domain (SQLite) / MIT (`libsqlite3-sys`) | compatible |
| PostgreSQL | PostgreSQL License (permissive) | separate service |
| ClickHouse (later) | Apache-2.0 | separate service |
| Listmonk (optional) | AGPL-3.0 | API integration only |
| Mailcoach / Mailchimp / Brevo / ConvertKit | proprietary SaaS | API calls only, no redistribution |

No listed component imposes copyleft on our code. "Use under AGPL" licenses *our* code; third-party code remains under its own license with notices preserved.

### Compliance checklist (before first release)

- **Dependency audit:** add `cargo-deny` (and `cargo-license`) to CI to catch any GPL/LGPL *transitive* crates; LGPL is acceptable (library carve-out), GPL deps need review.
- **License notices:** ship `LICENSE` (AGPL-3.0) + a `THIRD_PARTY_NOTICES` file with all third-party attributions.
- **Docker images:** include the project license + third-party notices inside the image; respect the base image (Alpine/Debian) license.
- **SDK boundary:** keep the AGPL core behind the API; permissive SDKs reference it over HTTP so SDK consumers are not drawn into copyleft.
