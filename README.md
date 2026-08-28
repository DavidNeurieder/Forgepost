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
- **Read next** — after each article, up to three related-post cards ranked
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
- **Backup & restore** — `forgepost backup create` seals the database and every
  media file into a single self-verifying `.fpb` archive (`forgepost backup
  verify` / `restore`, with a same-destination rollback of the pre-restore
  database). A bundled **demo blog** (six articles, images, and a live A/B
  experiment) installs in one command with `forgepost demo`.

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
./forgepost serve --addr 127.0.0.1:8080 --public-host example.com
```

`--public-host` sets the origin used for canonical/OG/RSS/sitemap links (you can
also set **Site URL** under Settings in the admin UI — that takes precedence).
Without either, an untrusted `Host` header is never echoed into those links:
the server falls back to `localhost`, so a spoofed header can't poison SEO or
feed output.

The server refuses to serve plain HTTP on a non-loopback address unless you
pass `--insecure-http` (session cookies lack `Secure` under plain HTTP). Keep
the loopback setup above, or **only** use `--insecure-http` on a private LAN
you control.

To keep rate limiting per-visitor and per-account behind a reverse proxy, tell
the server to trust the proxy's `X-Forwarded-For`:

```sh
./forgepost serve --addr 127.0.0.1:8080 --public-host example.com \
    --trusted-proxy 127.0.0.1/32
```

with nginx:

```nginx
server {
    listen 80;
    server_name example.com;
    location / {
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_pass http://127.0.0.1:8080;
    }
}
```

Forgepost never trusts a client-supplied `X-Forwarded-For`; a forged header
cannot mint a fresh rate-limit budget.

The public blog and RSS are served by the binary itself:

- Public article: `http://127.0.0.1:8080/articles/your-slug`
- RSS feed: `http://127.0.0.1:8080/rss`
- Health check: `http://127.0.0.1:8080/health`

### Security defaults

- **Login throttling** (10 failed attempts per client+account per window) and
  comment spam throttling (10 per client per window) are on by default and
  enforced before any password or comment work happens.
- All rate limiting keys on the socket peer, never on forwarded headers, so a
  single attacker cannot spoof its way around a limit without a trusted proxy
  in front.
- The admin/setup endpoints cannot be replayed: `setup` is atomic (concurrent
  first-login races lose), every authenticated request is CSRF-checked, and
  passwords are Argon2-hashed.
- Session cookies get `Secure` under HTTPS and `SameSite=Lax`; visitors get an
  anonymized `opv` cookie for traffic counting (see privacy link in the footer).

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
| `--media-dir` / `FORGEPOST_MEDIA_DIR` | next to the database | Where uploaded media is stored (served at `/media`) |
| `--public-host` / `FORGEPOST_PUBLIC_HOST` | HTTPS domain (ACME) | Origin for canonical/RSS/OG links when Site URL is unset |
| `--trusted-proxy` / `FORGEPOST_TRUSTED_PROXY` | none | Reverse-proxy IP/CIDR whose `X-Forwarded-For` is honored for rate limiting (repeatable or comma-separated) |
| `--insecure-http` | off | Allow plain HTTP on a non-loopback address (cookies lose `Secure`) |
| `RUST_LOG` | `info` | Log verbosity, e.g. `debug`, `forgepost=debug` |

TLS precedence: `--tls-domain` > `--tls-cert`/`--tls-key` > plain HTTP.

### Export

```sh
./target/release/forgepost export                 # dump JSON to stdout
./target/release/forgepost export --output db.json
```

The export covers the full database, including experiments and their
decisions, and is the intended backup/migration path.

### Backup & restore

```sh
./target/release/forgepost backup create                      # forgepost-<ts>.fpb
./target/release/forgepost backup verify forgepost-<ts>.fpb   # integrity report
./target/release/forgepost backup restore forgepost-<ts>.fpb --yes
```

Point any command at a different database or media directory with
`--database-url` / `--media-dir` (both also come from `DATABASE_URL` and
`FORGEPOST_MEDIA_DIR`). A `.fpb` archive contains a `manifest.json`
(format/schema versions), a `VACUUM INTO`-taken snapshot of the database, the
media files, and a `checksums.sha256`; it always self-verifies after creation.

`restore` replaces the live database and merges media files into the media
directory, then verifies the result. **Stop the server first** — a backup taken
while the server is writing is still crash-safe, but restoring a live database
under a running server is not supported. A real restore refuses to run without
`--yes` (`--dry-run` reports what a restore would do); the pre-restore database
is preserved next to it as `<name>.before-restore-<timestamp>`.

### The demo blog (one command)

```sh
./target/release/forgepost demo
```

installs a ready-made blog into `forgepost-demo.db` and **starts the server on
http://127.0.0.1:8080** — that single command is the whole quick start. The
demo ships six long-form articles (Markdown sources live in `demo/posts/`), the
bundled demo images, per-post tags, seeded analytics views, and a **live A/B
experiment** on the "Tracking Every Headline" headline with a populated report.
Log in at http://127.0.0.1:8080/admin with **admin@example.com / demo-password**.

`forgepost demo --no-serve` installs the content without starting the server;
`--addr 0.0.0.0:8080` (or `FORGEPOST_ADDR`) moves the listener. The demo is an
ordinary backup archive, so `backup restore` of `demo/forgepost-demo.fpb` is
identical to `demo --no-serve`.

## Testing

```sh
cargo test --workspace          # engine golden+property tests, unit, API integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The Rust suite covers the repository, every `/api/*` endpoint, all
server-rendered pages (`tests/pages.rs`), the full creator journey over a real
socket (`tests/system.rs`), and TLS in the binary with a self-signed certificate
(`tests/tls.rs`). Backup roundtrips are covered in
`crates/infrastructure/tests/backup_roundtrip.rs`, and the bundled demo archive
(`demo/forgepost-demo.fpb`) is validated on every test run by restoring it and
asserting its content — rebuild it with `FORGEPOST_REGEN_DEMO=1 cargo test
-p forgepost-server --test demo`. The `experiments` crate's correctness tests
are golden
(hand-computed beta probabilities) plus property tests (posterior sanity,
sample-size concentration, no-winner/stop correctness, assignment honesty).

Security testing is layered: a deterministic regression suite
(`tests/security.rs` in `crates/server`, covering the authorization matrix,
CSRF table, session/cookie lifecycle, proxy/IP handling, and upload
hardening) backed by proptest invariants over the rate limiter, markdown
rendering, slugs, and ZIP import. Everything maps back to the plan in
[docs/security-testing.md](docs/security-testing.md); CI also runs
`cargo audit` so a dependency vulnerability fails a merge instead of
surfacing later.

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
