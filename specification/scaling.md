# Scaling notes

How Forgepost renders today, why it is built for a single self-hoster, what
ceils it hits under "much traffic", and how large blogs solve the same
problems.

Status: notes / analysis only. No code changes planned for now — revisit when
traffic actually becomes a problem.

---

## 1. Current architecture

- **Server-rendered at request time.** The page skeletons are Askama templates
  compiled into the binary. The article body is *not* pre-rendered: every
  request loads blocks + immutable versions from SQLite and runs
  `render_html` (`crates/server/src/routes.rs` `article_view`, ~line 161; each
  block is rendered individually for the analytics tracking wrapper).
- **Single embedded SQLite file** (`crates/server/src/repository.rs:256`).
  WAL journal mode, connection pool capped at 5 (`max_connections(5)`). Writes
  serialize; reads are concurrent under WAL.
- **Everything shares the one DB**: content, sessions, comments, FTS5 search
  index, experiment state, and analytics events.
- **RSS re-renders every document per request** (`routes.rs:710`, `article_html`
  per feed item).
- **Editor live preview** parses + renders on every keystroke via
  `/api/render` (`routes.rs:458`), using the *same* `parse_markdown` +
  `render_html` as the public page.
- **Per-visitor experiment variants** are resolved per request
  (`assigned_variants`, `routes.rs:195`) — the article body differs between
  visitors, which prevents naive whole-page caching.
- **Static assets** are the only cached thing: immutable + `max-age=31536000`
  (`pages.rs:2076`).

## 2. Ceilings under much traffic

1. **One-writer database.** All writes (sessions, comments, search index,
   experiments, analytics events) serialize on the single SQLite file. A busy
   blog's tracking events are a constant single-row write stream.
2. **No page cache.** Article + RSS HTML is re-rendered on every hit. Cheap per
   doc, but pure CPU per request with zero reuse.
3. **Write-heavy analytics path.** Every page view POSTs tracking events into
   the content DB — the most likely first bottleneck.
4. **Per-visitor variants** make it impossible to cache one HTML blob for all
   readers without a strategy for the experimented block.

The app itself is **stateless** (sessions live in the DB, not process memory),
so multiple instances could share load — but embedded SQLite can only have one
writer, which is the hard blocker for horizontal scaling.

## 3. What would make it scale (read-heavy blog, in order of payoff)

1. **Reverse proxy + CDN in front** (nginx/Caddy/Cloudflare): `Cache-Control`
   / `ETag` on article pages and RSS. Static assets already immutable-cacheable.
2. **Cache the control article HTML in-process**, keyed by document version,
   invalidated on save/publish/promote. Resolve the experimented block
   separately (small cached map of `(block, variant) -> html`) so the control
   body is shared and only the variant block is swapped in per visitor.
3. **Batch analytics writes**: buffer events in memory and flush in bulk, or
   push them to a separate store — keep the write-heavy path off the content DB.
4. **Split storage**: content (read-mostly) stays in SQLite; sessions/analytics
   go elsewhere. Or move to Postgres when SQLite is the ceiling — sqlx already
   abstracts both.
5. **Horizontal scaling** (only once the DB is shared/external): run multiple
   stateless instances behind a load balancer.

## 4. How big blogs solve it

- **Static generation (SSG)** — HTML rendered at build/deploy time, served from
  CDN + object storage. GitHub Pages, engineering blogs, Substack public pages.
  The origin never sees a reader request.
- **CDN full-page caching** — Medium, The Verge, WordPress.com cache rendered
  HTML at the edge (Cloudflare/Fastly/Varnish); origin renders only on miss.
  Combined with `Cache-Control`/`ETag` and `Vary`/cache keys per page variant.
- **Render-once / cache-many** — hot articles held as rendered HTML in memory
  or a KV store keyed by content version; origin hit = memory lookup, not a
  re-render. Invalidation is content-versioned.
- **Incremental / edge rendering** — Next.js ISR or Workers-on-the-edge:
  generate ahead of time, re-render on schedule or on-demand when content
  changes (webhook from the CMS).
- **Decoupling reads from writes** — search index (Elasticsearch/Algolia) and
  analytics/email fan-out updated asynchronously via queues; the content DB
  does writes + canonical storage, reads come from cache/CDN.
- **Personalization without breaking the cache** — big sites cache the control
  shell and render the variant bit client-side, or use `Vary: Cookie` / cache
  keys that include the segment, or edge-side includes to inject the
  personalized block at the edge.
- **Degrade anonymous traffic** — logged-out readers get cached pages; only
  logged-in requests hit dynamic code.

## 5. Takeaway for Forgepost

The pattern for a dynamic blog at scale is: **generate once, cache at the edge,
version the cache by content, move writes/analytics/search off the read path,
and keep personalization to a small swappable fragment.** Forgepost could adopt
the cheap end of this (CDN + content-versioned HTML cache + batched analytics)
without redesigning anything — the stateless server and block-versioned content
model are already shaped for it.

## 6. Recommendations & scaling

"Read next" (`crates/server/src/recommender.rs`) adds three costs to watch.
All are fine at blog scale and cheap to fix later; none blocks the current
design.

1. **Article pages grew from fixed to O(all published posts) work.** Every
   article view runs `list_published_with_tags` (`repository.rs:544`) plus up to
   3 `get_document` calls for the recommended cards. Previously those queries
   only ran on home/tag pages. Fix later: cache the published-with-tags list
   keyed by a publish-version counter (bump on save/publish/unpublish), or
   precompute the related map at write time instead of read time.
2. **More events on the analytics write bottleneck.** Each recommendation card
   fires an impression event and clicks fire one per click through the same
   single-writer `analytics_events` path. Same mitigation as the general
   analytics backlog: batch writes or push them off the content DB.
3. **The future interest engine needs a `visitor_id` index.** Phase 2 will score
   candidates from a visitor's own `analytics_events`; today the table is only
   indexed on `document_id`-led columns (`0003_analytics.sql`), so a
   per-visitor query would be a full scan. Add
   `CREATE INDEX ... ON analytics_events(visitor_id, created_at_ms)` (and
   likely `visitor_id, event_type`) when the personalized engine ships.
