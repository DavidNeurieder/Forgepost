# Forgepost

Self-hosted block-level experimentation for creators.

Forgepost is a small blogging engine whose real product is the
**publish → measure → experiment → improve** loop. Every headline, paragraph,
image, and call-to-action is a measurable, testable object. You write in
Markdown, publish, watch where readers drop off, then A/B test alternative
content on a single block and let a Bayesian engine decide when a variant is a
clear winner.

Version **0.2.0** — an AGPL-3.0 solo-mode MVP built for a single self-hoster.

## Features

- **Thin blog host** — Markdown editor that parses to a block tree, publish /
  unpublish, tags, image uploads (served from `/media`), **Markdown import**
  (drop in an existing `.md` — or a `.zip` bundling it with its images — from
  the dashboard to create a reviewable draft; title/tags read from front
  matter), opt-in comments (disabled by default) with moderation, RSS,
  full-text search (SQLite FTS5, with as-you-type prefix matching and snippet
  highlighting), and one clean theme. SEO is first-class: per-post social
  cards (Open Graph/Twitter image + dimensions, canonical, JSON-LD) and a
  site-wide **default image** you can upload in Settings as the fallback.
- **Video embeds** — a line that is exactly one YouTube or Rumble URL (or a
  raw `<iframe>` line) becomes a video block, rendered as a **click-to-load**
  box: a lazy thumbnail and zero third-party requests until the reader chooses
  to play. YouTube's thumbnail is derived from the video id; Rumble's title
  and thumbnail are fetched once, best-effort, via oEmbed at save time. Embeds
  render privacy-first (`youtube-nocookie.com`, `referrerpolicy="no-referrer"`)
  and articles gain `og:video` + a JSON-LD `VideoObject`.
- **Per-block analytics** — privacy-lean browser tracking (banded scroll depth,
  completion, read time, block impressions), estimated reach and drop-off per
  block, honestly labeled ("estimated") because blockers and JS-disabled
  readers are undercounted.
- **Block experiments** — A/B test alternative content on any block. Stable
  per-visitor traffic split, impression/conversion tracking, and a
  **Bayesian sequential test**: exact `P(beats control)`, credible intervals, a
  spending-bound-corrected confidence threshold, a no-winner stopping rule, and
  automatic promotion of the winning variant.
- **Keep reading** — after each article, up to three related-post cards ranked
  by shared tags (the most recent posts backfill the list). Impressions and
  clicks are tracked into analytics to feed a future personalized
  recommendation engine.
- **Traffic sources** — each article's views are bucketed into Search / Direct
  / Community on the Stats page from the referrer captured per event.
- **Game-feel dashboard** — the admin dashboard leads with the week's
  most-read post, per-post **Views (7d)** with a **Δ vs last week** column,
  and a nudge pointing at the post with the worst read-through.
- **Share tracking** — a Share button on every article (native sheet or
  clipboard copy) reports `share_click` events shown as a Shares stat.
- **Solo mode** — one binary + embedded SQLite, Argon2 password hashing,
  session cookies, CSRF protection, rate-limited analytics API,
  `forgepost export` for backups, and in-process HTTPS (Let's Encrypt
  auto-renewal or bring-your-own certificates).

## Project layout

```
crates/content      Markdown → block tree → HTML, immutable block versions
crates/analytics    Event ingestion, per-block and per-article aggregations
crates/experiments  Pure Bayesian engine + traffic-split assignment (no I/O)
crates/server       Axum app: page routes, API, auth, repository, TLS
crates/server/templates   Askama templates (all pages server-rendered)
crates/server/static      app.css, favicon, tracker.js (embedded in the binary)
e2e                 Playwright end-to-end suite against the built binary
migrations          SQLite schema (0001 … 0008)
docs                Website (GitHub Pages, static)
```

The whole app — public blog, admin dashboard, JSON API, RSS, static assets, and
TLS — is one process. There is no Node.js server in production; Node is only
used to drive the Playwright test suite.

## Requirements

- **Rust** 1.85+ (edition 2024). Check with `rustc --version`.
- **Node.js** 20+ and `npm`, only for the end-to-end tests.
- Nothing else — the database is embedded SQLite, so there is no separate
  database server to install.

## 1. Install the server binary

Build from source (a release build of the single `forgepost` binary):

```sh
git clone https://github.com/DavidNeurieder/Forgepost.git
cd Forgepost
cargo build --release --bin forgepost
```

The binary lands at `target/release/forgepost`. Verify it:

```sh
./target/release/forgepost --help
```

> Optional: copy it somewhere on your `PATH` so you can run `forgepost`
> anywhere:
>
> ```sh
> cp target/release/forgepost /usr/local/bin/
> # or install directly:
> cargo install --path crates/server
> ```

## 2. Start the server

```sh
./target/release/forgepost serve
```

On first start this:

1. creates `forgepost.db` in the current directory,
2. runs the SQLite migrations (`migrations/0001 … 0008`),
3. spawns the background experiment auto-decider,
4. listens on `127.0.0.1:8080`.

To use a different database file or port:

```sh
./target/release/forgepost serve --database-url sqlite:///srv/forgepost/data.db --addr 0.0.0.0:8080
```

Environment-variable equivalents: `DATABASE_URL` and `FORGEPOST_ADDR` (see
the [Configuration](#configuration) table). Set `RUST_LOG=debug` for verbose
logging.

Verify the server is up:

```sh
curl -s http://127.0.0.1:8080/health
# {"status":"ok"}
```

## 3. First-run setup

Open http://127.0.0.1:8080. Because no user exists yet you are sent to
**/setup**, where you create the admin account (email + password). From then on
`/setup` is locked and you log in at `/login`.

## 4. Production deployment

No reverse proxy required. The server can terminate HTTPS itself, either with
certificates you supply or with automatic Let's Encrypt issuance and renewal.

### Automatic HTTPS (Let's Encrypt)

```sh
./forgepost serve --tls-domain example.com --addr 0.0.0.0:443
```

The binary obtains and renews a certificate automatically (TLS-ALPN-01, so no
port 80 is needed for issuance), and starts an HTTP listener on port 80 that
redirects to HTTPS. Redirect port and ACME cache directory are configurable
(see below).

### Bring-your-own certificates

```sh
./forgepost serve --tls-cert cert.pem --tls-key key.pem --addr 0.0.0.0:443
```

The certificate is watched and reloaded on change, so renewed certs are picked
up without a restart. Under HTTPS, `Secure` is added to the session and visitor
cookies.

### Plain HTTP behind a TLS front

If you keep an existing nginx/Caddy as a TLS front (optional — the binary does
not need it), run the server on loopback only:

```sh
./forgepost serve --addr 127.0.0.1:8080
```

and proxy everything to it:

```nginx
server {
    listen 80;
    server_name example.com;
    location / { proxy_pass http://127.0.0.1:8080; }
}
```

The public blog and RSS are served by the binary itself:

- Public article: `http://127.0.0.1:8080/articles/your-slug`
- RSS feed: `http://127.0.0.1:8080/rss`
- Health check: `http://127.0.0.1:8080/health`

## Workflow

1. **Write** — on the dashboard, click *New post*. The editor stores Markdown
   and renders a live block preview. The first block is the headline; every
   paragraph, image, and CTA becomes its own block. Saving sets the public URL
   (slug) from the title while the post is still a draft; once published the
   URL is stable.
2. **Publish** — click *Publish*. The post appears on the public route and the
   RSS feed, and the editor links to it.
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

The `forgepost` binary is configured with CLI flags or environment variables:

| Flag / var | Default | Meaning |
|---|---|---|
| `--database-url` / `DATABASE_URL` | `sqlite://forgepost.db` | SQLite URL or file path |
| `--addr` / `FORGEPOST_ADDR` | `127.0.0.1:8080` | Bind address (TLS listener when TLS is active) |
| `--tls-domain` / `FORGEPOST_TLS_DOMAIN` | — | Enable automatic Let's Encrypt HTTPS for this domain |
| `--tls-cert` / `FORGEPOST_TLS_CERT` | — | PEM certificate chain for bring-your-own HTTPS |
| `--tls-key` / `FORGEPOST_TLS_KEY` | — | Matching PEM private key (must be given with `--tls-cert`) |
| `--tls-cache-dir` / `FORGEPOST_TLS_CACHE_DIR` | `./tls` | ACME certificate cache directory |
| `--http-redirect-port` / `FORGEPOST_HTTP_REDIRECT_PORT` | `80` | Port for the HTTP→HTTPS redirect listener |
| `--no-http-redirect` | off | Do not start the redirect listener under TLS |
| `RUST_LOG` | `info` | Log verbosity, e.g. `debug`, `forgepost=debug` |

TLS precedence: `--tls-domain` > `--tls-cert`/`--tls-key` > plain HTTP.

### Export

```sh
./target/release/forgepost export                 # dump JSON to stdout
./target/release/forgepost export --output db.json
```

The export covers the full database, including experiments and their
decisions, and is the intended backup/migration path.

## Testing

```sh
cargo test --workspace          # engine golden+property tests, unit, API integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The Rust suite covers the repository, every `/api/*` endpoint, all
server-rendered pages (`tests/pages.rs`), the full creator journey over a real
socket (`tests/system.rs`), and TLS in the binary with a self-signed certificate
(`tests/tls.rs`). The `experiments` crate's correctness tests are golden
(hand-computed beta probabilities) plus property tests (posterior sanity,
sample-size concentration, no-winner/stop correctness, assignment honesty).

### End-to-end tests

`npm run test:e2e` drives a real headless browser through the full creator
journey against the real binary: first-run setup, write + publish, read + comment
as a visitor, moderation, analytics, an experiment lifecycle, and logout/login.
Playwright spawns `forgepost` on a free port with a throwaway SQLite database
(a fresh server every run), so no setup is needed beyond building the binary:

```sh
cargo build --bin forgepost
cd e2e && npx playwright install chromium    # first time only
cd e2e && npm run test:e2e
```

To point Playwright at an already-built binary instead of compiling on the
spot, set `FORGEPOST_BIN=/path/to/forgepost`. Specs live in `e2e/`.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).

See [CHANGELOG.md](CHANGELOG.md) for release history and
[old_docs/mvp_plan_v6.md](old_docs/mvp_plan_v6.md) for the product plan and the
pivot/persevere gates.
