# OpenPublish

Self-hosted block-level experimentation for creators.

OpenPublish is a small blogging engine whose real product is the
**publish → measure → experiment → improve** loop. Every headline, paragraph,
image, and call-to-action is a measurable, testable object. You write in
Markdown, publish, watch where readers drop off, then A/B test alternative
content on a single block and let a Bayesian engine decide when a variant is a
clear winner.

Version **0.1.0** — an AGPL-3.0 solo-mode MVP built for a single self-hoster.

## Features

- **Thin blog host** — Markdown editor that parses to a block tree, publish /
  unpublish, tags, comments with moderation, RSS, and one clean theme.
- **Per-block analytics** — privacy-lean browser tracking (banded scroll depth,
  completion, read time, block impressions), estimated reach and drop-off per
  block, honestly labeled ("estimated") because blockers and JS-disabled
  readers are undercounted.
- **Block experiments** — A/B test alternative content on any block. Stable
  per-visitor traffic split, impression/conversion tracking, and a
  **Bayesian sequential test**: exact `P(beats control)`, credible intervals, a
  spending-bound-corrected confidence threshold, a no-winner stopping rule, and
  automatic promotion of the winning variant.
- **Solo mode** — one binary + embedded SQLite, Argon2 password hashing,
  session cookies, CSRF protection, rate-limited analytics API, and
  `openpublish export` for backups.

## Project layout

```
crates/content      Markdown → block tree → HTML, immutable block versions
crates/analytics    Event ingestion, per-block and per-article aggregations
crates/experiments  Pure Bayesian engine + traffic-split assignment (no I/O)
crates/server       Axum app: routes, auth, repository (SQLite), auto-decider
frontend            SvelteKit admin dashboard, editor, and article view
migrations          SQLite schema (0001 … 0004)
docs                MVP plan and milestones
```

## Requirements

- **Rust** 1.85+ (edition 2024). Check with `rustc --version`.
- **Node.js** 20+ and `npm` (for the admin dashboard frontend). Check with
  `node --version`.
- Nothing else — the database is embedded SQLite, so there is no separate
  database server to install.

## 1. Install the server binary

Build from source (a release build of the single `openpublish` binary):

```sh
git clone https://github.com/DavidNeurieder/my_blog.git
cd my_blog
cargo build --release --bin openpublish
```

The binary lands at `target/release/openpublish`. Verify it:

```sh
./target/release/openpublish --help
```

> Optional: copy it somewhere on your `PATH` so you can run `openpublish`
> anywhere:
>
> ```sh
> cp target/release/openpublish /usr/local/bin/
> # or install directly:
> cargo install --path crates/server
> ```

## 2. Start the server

```sh
./target/release/openpublish serve
```

On first start this:

1. creates `openpublish.db` in the current directory,
2. runs the SQLite migrations (`migrations/0001 … 0004`),
3. spawns the background experiment auto-decider,
4. listens on `127.0.0.1:8080`.

To use a different database file or port:

```sh
./target/release/openpublish serve --database-url sqlite:///srv/openpublish/data.db --addr 0.0.0.0:8080
```

Environment-variable equivalents: `DATABASE_URL` and `OPENPUBLISH_ADDR` (see
the [Configuration](#configuration) table). Set `RUST_LOG=debug` for verbose
logging.

Verify the server is up:

```sh
curl -s http://127.0.0.1:8080/health
# {"status":"ok"}
```

## 3. Install and run the admin dashboard

The dashboard is a separate SvelteKit app. For development:

```sh
cd frontend
npm install
npm run dev
```

This starts Vite on http://127.0.0.1:5173. The dev server proxies `/api`
requests to the Rust server (configurable via `OPENPUBLISH_API`, default
`http://127.0.0.1:8080`), so cookies stay same-origin and auth works without
any CORS setup.

## 4. First-run setup

With both processes running, open http://127.0.0.1:5173. Because no user
exists yet you are sent to **/setup**, where you create the admin account
(email + password). From then on `/setup` is locked and you log in at `/login`.

## 5. Production deployment (reverse proxy)

The Rust server is the API **and** the public blog; it does not serve the
admin dashboard's static files. For a single hostname you run a reverse proxy
(here: nginx) that forwards `/api`, `/articles`, `/rss`, `/health`, and
`/setup` to the Rust server, and everything else to the dashboard. Because
both origins share the hostname, the `SameSite=Lax` session cookie works
without CORS.

First give the dashboard a production adapter. The project ships with
`adapter-auto`, which emits nothing unless a platform adapter is installed.
For self-hosting, install the Node server adapter:

```sh
cd frontend
npm install -D @sveltejs/adapter-node
npm run build
```

This writes a self-contained Node server to `frontend/build`. Run it
(listens on 127.0.0.1:3000):

```sh
node build
```

If you don't want a second process, the Vite dev server (`npm run dev`) is
fine behind nginx for a single-creator blog, but the built adapter is the
supported production path.

Example `/etc/nginx/sites-available/myblog`:

```nginx
server {
    listen 80;
    server_name example.com;

    # Public blog + API + RSS go to the Rust server
    location /api/        { proxy_pass http://127.0.0.1:8080; }
    location /articles/   { proxy_pass http://127.0.0.1:8080; }
    location /rss         { proxy_pass http://127.0.0.1:8080; }
    location /health      { proxy_pass http://127.0.0.1:8080; }
    location /setup       { proxy_pass http://127.0.0.1:8080; }  # setup status probe

    # Dashboard (SvelteKit Node server)
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Run the Rust server on loopback only — nginx is the public face:

```sh
./target/release/openpublish serve --addr 127.0.0.1:8080
```

Then `nginx -s reload`. Enable HTTPS with a free Let's Encrypt certificate
before going public; the session cookie is `HttpOnly` and `SameSite=Lax`, but
not `Secure`.

The public blog and RSS are also served directly by the Rust server, so even
without the dashboard you get:

- Public article: `http://127.0.0.1:8080/articles/your-slug`
- RSS feed: `http://127.0.0.1:8080/rss`
- Health check: `http://127.0.0.1:8080/health`

## Workflow

1. **Write** — from the dashboard, click *New document*. The editor stores
   Markdown and renders a live block preview. The first block is the headline;
   every paragraph, image, and CTA becomes its own block.
2. **Publish** — give the post a slug and publish. It appears on the public
   route and the RSS feed.
3. **Measure** — open *Stats* for a document. You see views, unique readers
   (estimated), average reading time, completion, a scroll-depth funnel, and a
   per-block drop-off table: where do readers leave?
4. **Experiment** — on the Stats page, open *Create experiment*: pick the block
   to test, choose what share of visitors see variants, and add replacement
   content. Control (the current block) is created automatically.
5. **Watch and decide** — the live report shows impressions, conversions,
   conversion rate, and P(beats control) per variant as the Bayesian posterior
   updates. You can decide manually (promote the best, conclude no improvement,
   stop) or let the background auto-decider apply the sequential-test rules —
   it promotes a variant once it clears the spending-bound-corrected threshold,
   or concludes no-improvement when the variant is (near-)certain to lose.
6. **Promote** — a decision repoints the live block to the winning immutable
   version, so the article changes immediately.

Analytics and experiments share one goal model in the MVP: a "completion" is a
visitor who scrolled to the end of the article.

## Configuration

The `openpublish` binary is configured with CLI flags or environment variables:

| Flag / var | Default | Meaning |
|---|---|---|
| `--database-url` / `DATABASE_URL` | `sqlite://openpublish.db` | SQLite URL or file path |
| `--addr` / `OPENPUBLISH_ADDR` | `127.0.0.1:8080` | Bind address |
| `RUST_LOG` | `info` | Log verbosity, e.g. `debug`, `openpublish=debug` |

The frontend reads `OPENPUBLISH_API` (default `http://127.0.0.1:8080`) as the
API origin for the dev proxy.

### Export

```sh
./target/release/openpublish export                 # dump JSON to stdout
./target/release/openpublish export --output db.json
```

The export covers the full database, including experiments and their
decisions, and is the intended backup/migration path.

## Testing

```sh
cargo test --workspace          # engine golden+property tests, unit, API integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cd frontend && npm run check    # svelte-check (type checks all routes)
cd frontend && npm run build
cd frontend && npm run test     # Vitest: unit (src/lib) + component (@testing-library/svelte)
```

The `experiments` crate's correctness tests are golden (hand-computed beta
probabilities) plus property tests (posterior sanity, sample-size
concentration, no-winner/stop correctness, assignment honesty).

The frontend test suite covers the tracker (`src/lib/tracker.test.ts`) and the
API client (`src/lib/api.test.ts`) at the unit level, plus component tests for
every route that talks to the API (editor, article, dashboard, stats, login,
setup). Run a single suite with:

```sh
cd frontend && npx vitest run src/lib          # unit only
cd frontend && npx vitest run src/routes       # component only
```

### End-to-end tests

`npm run test:e2e` drives a real headless browser through the full creator
journey against a real server: first-run setup, write + publish, read + comment
as a visitor, moderation, analytics, an experiment lifecycle, and logout/login.
Playwright spawns the `openpublish` binary and the Vite dev server on free ports
with a throwaway SQLite database (a fresh server every run), so no setup is
needed beyond building the binary:

```sh
cargo build --bin openpublish
cd frontend && npx playwright install chromium   # first time only
cd frontend && npm run test:e2e
```

To point Playwright at an already-built binary instead of compiling on the
spot, set `OPENPUBLISH_BIN=/path/to/openpublish`. Specs live in
`frontend/e2e/`.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).

See [CHANGELOG.md](CHANGELOG.md) for release history and
[docs/mvp_plan_v6.md](docs/mvp_plan_v6.md) for the product plan and the
pivot/persevere gates.
