# OpenPublish MVP Plan

## 1. Vision

Open-source, self-hostable blogging platform with built-in content optimization: **publish → auto A/B test → measure → improve.** "GitHub for creators + analytics lab for blogs."

## 2. Success test

50–100 writers install it (`docker compose up`), publish regularly, check analytics daily, run experiments, and improve articles over time.

## 3. MVP scope

### In scope

1. **Self-hosted blog** — user accounts, profiles, Markdown editor, publish articles, tags/categories, comments, RSS feed, themes.
2. **Analytics engine** — views, unique readers, reading time, scroll depth, completion rate, returning readers, referral source, subscriber conversion.
3. **Automatic A/B testing** — headlines, images, CTAs; traffic split → measure → pick winner → promote automatically.
4. **Conversion goals** — signup forms, CTA blocks, landing pages, simple funnel analytics; email via provider integration (Listmonk/Mailcoach/Mailchimp/Brevo/ConvertKit).
5. **Creator leaderboard (simple)** — fastest growth, best retention, best experimenter.
6. **Open-source foundation** — AGPL-3.0 core, monorepo, public from day one.

### Explicitly out (extension layers)

Federation · global search engine · video hosting · books · AI writing assistant · plugin marketplace · social network.

## 4. Architecture

```
openpublish/
├── crates/
│   ├── server        # Axum API + core engine
│   ├── content       # document/block model
│   ├── analytics     # event collection + metrics
│   ├── experiments   # A/B engine
│   ├── protocol      # (deferred)
│   └── federation    # (deferred)
├── frontend/         # SvelteKit (publishing pages, dashboards)
├── migrations/       # SQLx (PostgreSQL)
├── docker/           # compose + Dockerfiles
└── docs/
```

- **Backend:** Rust + Axum + Tokio + SQLx (compile-time checked SQL) + PostgreSQL.
- **Frontend:** SvelteKit.
- **Analytics:** browser → Rust event API → PostgreSQL (MVP); ClickHouse later.
- **License:** core AGPL-3.0; SDKs MIT/Apache-2.0 later.

## 5. Data model (the differentiator)

- `Document` → `Block` (heading, paragraph, image, code block, quote, table, CTA, signup form) → **immutable `BlockVersion`**; block points at current published version (perfect history, rollback, easy federation later).
- `Experiment` references existing `BlockVersion`s as variants (overlays). Canonical content never mutated. On winner: create a new published `BlockVersion`, archive experiment.
- **Events:** `page_view`, `article_scroll`, `article_read`, `experiment_impression`, `experiment_conversion`, `signup`, `download`.
- **Derived metrics:** per-article and per-block aggregations (completion, reading time, retention, conversion).

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
- **Metrics:** aggregate on read with SQL over `analytics_events`; no scheduled pre-aggregation jobs in the MVP.
- **Indexes:** `analytics_events(document_id, event, created_at)`, `blocks(document_id, position)`, `block_versions(block_id, status)`.
- **Constraints (enforced in DB, not just app code):**
  - Partial unique index on `blocks(published_version_id) WHERE published_version_id IS NOT NULL` — exactly one published version per block.
  - Unique `(author_id, slug)` on `documents`.
  - Unique `(experiment_id, block_id)` on `experiment_variants` — one variant per block per experiment.
- **Delete semantics:** documents transition to `status: 'archived'`, never hard-deleted (preserves history + analytics). `analytics_events` FK refs use `ON DELETE SET NULL` so events survive doc/block cleanup. RSS, leaderboards, and funnels are derived queries, not tables.

### 5.2 Events & experiment statistics

- **Scroll tracking:** the client throttles `article_scroll` to banded events (25/50/75/100% depth) instead of every scroll; `article_read` carries duration + completion once per session. Reduces write volume on community servers.
- **Growth:** `analytics_events` is range-partitioned by month with a configurable retention window (e.g. auto-drop after 12 months). ClickHouse is the escape hatch if a server outgrows Postgres.
- **Block attribution:** "readers leave at Section 3" maps scroll-depth % to block boundaries computed from stored block order at read time — approximate in MVP (documented assumption).
- **Winner selection (M3):** two-proportion z-test on the experiment's goal metric (CTR, completion, or signup rate), defaulting to the Bayesian beta-binomial posterior if A/B→A/B/C extends naturally. Enforce a minimum sample size (e.g. ≥ 100 impressions per variant) before any promotion — never promote on a handful of visitors. Changing `traffic_weight` mid-run invalidates bucket assignments, so experiments are finalized before re-weighting.
- **Privacy note:** visitor tracking (cookie + referer + user agent) is opt-out via a per-site consent flag; self-hosted owners decide. Add a docs note, not a blocker.

**Render flow:** documents → ordered blocks → resolve `published_version_id` → substitute running-experiment variant by `hash(visitor_id, experiment_id)` weighted by `traffic_weight` → render.

## 6. Milestones

### M0 — Scaffolding

- Cargo workspace: `server`, `content`, `analytics`, `experiments` crates.
- SvelteKit frontend shell; Docker compose (Postgres); CI; AGPL license; repo structure.

### M1 — Core publishing

- Auth, users, profiles.
- Markdown editor backed by the Block/BlockVersion model.
- Publish articles, tags/categories, comments, RSS, basic themes.

### M2 — Analytics engine

- Event collection API + browser tracking (scroll depth, completion, read time).
- Aggregations: views, unique readers, avg reading time, completion %, returning readers, referral source.
- Dashboard UI (game-like feel).

### M3 — Experiments

- Create block-level A/B tests (headline, image, CTA).
- Traffic split + stable variant assignment per visitor.
- Impression/conversion tracking; confidence threshold → auto-promote winner to new BlockVersion.

### M4 — Conversion goals

- Signup-form and CTA blocks, landing pages.
- Simple funnel analytics (visit → read → signup → download → purchase).
- Email-provider integration via API (track conversions; provider handles sending).

### M5 — Leaderboard + polish

- Creator rankings: fastest growth, best retention, best experimenter.
- One-command `docker compose up` deploy; README + onboarding docs.

## 7. Key design decisions to protect early

- Immutable BlockVersions + experiments as overlays (foundation for everything later).
- Store semantic document, derive all formats (HTML/EPUB/PDF/search) from it.
- Analytics-first event model from day one.
- API-first: web/desktop/mobile/clients all consume the same API.
