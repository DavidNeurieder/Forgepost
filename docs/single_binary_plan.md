# Single-binary plan: drop SvelteKit, serve everything from Rust

Status: **implemented** (this document is the record of what shipped; see
"Deviations from the plan" below for the few implementation differences).
Supersedes: the two-process frontend architecture described in `mvp_plan_v6.md` and
the README deployment section.

## 1. Why

The current setup serves the app as **two stacked applications**:

- **Dev:** Rust (`cargo run`) + Vite (`npm run dev`) + a `/api` proxy.
- **Prod:** nginx splits `/api`, `/articles`, `/rss`, `/setup` to the Rust binary
  and everything else to a second Node process (`adapter-node`), all to keep the
  `SameSite=Lax` session cookie same-origin.
- **Redundancy:** `/articles/[slug]` is rendered twice — by SvelteKit
  (`frontend/src/routes/articles/[slug]/`) *and* by the Rust server
  (`crates/server/src/routes.rs` "Public: published articles"). In production nginx
  serves the Rust copy, so the SvelteKit reader page is dead weight.

This contradicts the plan's own promise of "solo = downloaded binary,
`./openpublish serve` handles everything" (`docs/mvp_plan_v6.md` §5.3).

**Goal:** `./openpublish serve` serves the public blog, admin dashboard,
setup/login, JSON API, RSS, static assets, and (optionally) HTTPS — one process,
no nginx, no npm, no config files. The JSON `/api/*` surface is preserved: the
engine must stay reachable headless (`docs/mvp_plan_v6.md` §5.4).

## 2. Decisions (locked)

1. **Delete `frontend/`** — SvelteKit app, Vite, Vitest, svelte-check, the
   frontend-targeted Playwright harness, TS/`$lib` code, fixtures, mocks. `cargo`
   becomes the only build toolchain.
2. **Rust renders all pages**; the `/api/*` JSON handlers stay unchanged.
3. **Templates: Askama** (compile-time, type-safe, auto-escaped, partials).
4. **Interactivity: htmx** — vendored ~14 KB script served as a static asset; the
   server returns HTML fragments. Keeps today's UX (editor live preview, dynamic
   experiment variants, smooth comment submit) with no client build step.
5. **TLS in the binary — tiers 1 + 2**:
   - Tier 2 (automatic): `rustls-acme` with `DirCache` (TLS-ALPN-01, no port 80
     required for issuance).
   - Tier 1 (bring-your-own): `axum-server` `tls-rustls` reading PEMs, with
     reload-on-renewal.
   - nginx/Caddy become unnecessary (optional TLS-only front).
6. **Analytics tracker** reimplemented as a small vanilla-JS file
   (`/static/tracker.js`) — browser-side tracking is unavoidable and is required
   by the `/api/events` ingestion path.
7. **Visual design preserved** — port `frontend/src/app.css` and the favicon.

## 3. Architecture

```
openpublish serve
├── /api/*           JSON (unchanged, headless contract)
├── /                home page (redirects to /setup when no owner)
├── /setup           setup wizard (GET form / POST action)
├── /login           login (GET form / POST action)
├── /admin           dashboard (auth required)
├── /admin/editor/{id}        editor (auth required)
├── /admin/stats/{id}         analytics + experiments (auth required)
├── /articles/{slug}          public article page (HTML, was JSON)
├── /articles/{slug}/comments comment form action
├── /rss             RSS (unchanged)
├── /static/*        app.css, favicon, htmx.js, tracker.js
└── [tls]            --tls-domain (auto-LE) | --tls-cert/--tls-key (BYO)
```

- Page handlers call the same repository/auth logic as the existing API handlers
  but return HTML and use POST-REDIRECT-GET.
- Mutating forms carry a hidden `csrf_token` field; `verify_csrf` accepts either
  that field or the existing `x-csrf-token` header (API clients keep the header).

## 4. New dependencies (crates/server)

| Crate | Version | Feature(s) | Use |
|---|---|---|---|
| `askama` | latest (0.14+) | — | templates |
| `axum-server` | 0.8 | `tls-rustls` | TLS bind + reload |
| `rustls-acme` | 0.15 | `axum` | automatic Let's Encrypt |
| `aws-lc-rs` | — | — | rustls crypto provider (install default in `main`) |
| `rcgen` | dev | — | self-signed certs for TLS tests |

Note: install the crypto provider once via
`rustls::crypto::aws_lc_rs::default_provider().install_default()` before serving.

## 5. Phase 1 — Template layer + static assets

- Add `askama`; create `crates/server/templates/`:
  - `base.html` — `<head>` (title, viewport, favicon, `app.css`), site header/nav
    (brand → `/`, Blog → `/`, Admin → `/admin`), `<main>`, flash partial.
  - `home.html`, `setup.html`, `login.html`, `dashboard.html`, `editor.html`,
    `stats.html`, `article.html`, `comment_form.html`, `flash.html`.
- Port CSS: `crates/server/static/app.css`; favicon → `static/favicon.svg`;
  htmx → `static/htmx.min.js` (vendored, no CDN); tracker → `static/tracker.js`.
- Serve `/static/{file}` via `include_str!`-embedded assets and a small route.

## 6. Phase 2 — Page routes, forms, auth

- New module `crates/server/src/pages.rs`; `lib.rs` mounts page routes and removes
  the old non-`/api` JSON aliases (`/setup`, `/articles/{slug}`, comments).
- Routes and behavior:
  - `GET /` — published post list; redirect → `/setup` when `setup_complete=false`.
  - `GET/POST /setup` — wizard; redirect → `/login` when complete; POST creates the
    owner, sets cookie, redirects → `/admin`.
  - `GET/POST /login` — form; redirect → `/setup` when incomplete; → `/admin` when
    already authed.
  - `POST /logout` — small form button; clears session, redirect → `/login`.
  - `GET /admin` — posts table + pending-comment moderation queue; 401 → `/login`.
  - `GET/POST /admin/editor/{id}` (+ `POST .../publish`) — title/tags/markdown
    form, Save/Publish actions, status badge from server state; 401 → `/login`.
  - `GET /admin/stats/{id}` — stats cards, scroll funnel, drop-off table,
    experiments list, create-experiment form; experiment actions
    (start/stop/decide/promote/no-winner) as POST buttons; 401 → `/login`.
  - `GET /articles/{slug}` — full HTML page (see Phase 4); comments + comment form.
  - `POST /articles/{slug}/comments` — public, no CSRF (matches current API).
- **CSRF:** extend `verify_csrf` to accept a hidden `csrf_token` form field or the
  `x-csrf-token` header. Embed the session token in every authenticated form.
- **Flash messages:** POST → 303 redirect → `?flash=<key>`; the page renders the
  matching string ("Saved", "Published", "Thanks! Your comment is awaiting
  moderation.", etc.) so e2e-asserted text survives the redirect.

## 7. Phase 3 — Editor & stats interactivity (htmx)

- Editor live preview: textarea `hx-trigger="input changed delay:400ms"` → existing
  `POST /api/render`; render returned HTML fragment.
- Create-experiment form: Add/Remove variant rows via `hx-get` row fragments.
- Comment submit and experiment action buttons: `hx-post` with `hx-target`.
- All other flows remain plain form posts.
- Fallback (if htmx is dropped): plain form posts; live preview and dynamic
  variant rows are removed, everything else unchanged.

## 8. Phase 4 — Article page + tracker

- `GET /articles/{slug}` renders a full HTML page:
  - Header/brand/nav from `base.html`.
  - Article HTML from `openpublish_content::render_html`, preserving the
    experiment-variant overlay and the `data-block-id` / `data-experiment-id` /
    `data-variant-id` attributes the tracker depends on.
  - Approved comments + comment form.
  - `<script src="/static/tracker.js">`.
- `tracker.js` (vanilla port of `frontend/src/lib/tracker.ts`): on load posts a
  `view` event; once-per-band `banded_scroll` (25/50/75/100); `article_read` after
  100% scroll + 3 s dwell; IntersectionObserver `block_impression` per
  `[data-block-id]`; `experiment_impression`/`experiment_conversion` per
  `[data-experiment-id]`. Uses `navigator.sendBeacon('/api/events', …)` with a
  fresh `crypto.randomUUID()` `session_id`; failures swallowed.

## 9. Phase 5 — TLS (tiers 1 + 2)

- CLI/env args on `serve`:
  - `--tls-domain example.com` / `OPENPUBLISH_TLS_DOMAIN` (tier 2).
  - `--tls-cert path` + `--tls-key path` / `OPENPUBLISH_TLS_CERT` /
    `OPENPUBLISH_TLS_KEY` (tier 1).
  - `--tls-cache-dir path` (default `./tls`, tier 2 ACME cache).
  - `--no-http-redirect` (skip the `:80` → HTTPS redirect listener).
  - Precedence: `--tls-domain` > `--tls-cert/--tls-key` > plain HTTP.
- Tier 2 wiring: `AcmeConfig::new([domain]).cache_option(Some(DirCache::new(dir)))`
  + `AxumAcceptor`; TLS-ALPN-01 so issuance/renewal run on the TLS port itself.
- Tier 1 wiring: `RustlsConfig::from_pem_file(cert, key)`; reload on renewal via
  file-mtime watch or SIGHUP.
- When TLS is active:
  - `--addr` binds the TLS listener; optionally spawn a second `:80` listener that
    301s to `https://{host}` (opt out via `--no-http-redirect`).
  - Add `Secure` to `openpublish_session` and `opv` cookies (thread a
    `tls_active` flag into the cookie builders; `SameSite=Lax` already set).

## 10. Phase 6 — Tests

- Rust integration (keep `tests/api.rs`; add):
  - `tests/pages.rs` — page status codes; redirects (`/` → `/setup`, `/login` →
    `/setup`, `/setup` → `/login`, admin 401 → `/login`); flash markers; CSRF via
    hidden field; article page contains rendered HTML + `data-*` attributes +
    tracker include; comment flow; save/publish flow.
  - `tests/tls.rs` — generate a self-signed cert with `rcgen` (dev-dep); assert
    HTTPS 200 with `danger_accept_invalid_certs(true)`, `Secure` cookie flag, and
    the HTTP→HTTPS redirect. ACME issuance itself is manual/e2e (Let's Encrypt
    staging / a real domain) — CI tests the wiring, not issuance.
- E2E: rewrite the 7 Playwright flows against the Rust binary on a temp DB
  (reuse the current scenario list: first-run setup, create+publish, reader +
  comment, approve, analytics > 0, experiment lifecycle, logout/login). Playwright
  still needs Node as a runner, but no build or dev server.
- Update CI: remove frontend build/test/e2e jobs; add `cargo test` (new tests) and
  the rewritten e2e job. Keep `cargo fmt --check` and clippy `-D warnings`.

## 11. Phase 7 — Cleanup, CI, docs

- Delete `frontend/`.
- `.gitignore`: drop frontend entries; add `tls/` cache dir if committed.
- README: quickstart → one binary (`cargo run -- serve`); testing → Rust tests +
  e2e; deployment → systemd unit + `--tls-domain`, nginx section reduced to
  "optional TLS front".
- `docs/mvp_plan_v6.md`: note the frontend architecture change.

## 12. Resulting run steps

```sh
# build once
cargo build --release

# dev
cargo run -- serve

# prod — automatic HTTPS (Let's Encrypt, auto-renew)
./openpublish serve --tls-domain example.com --addr 0.0.0.0:443

# prod — bring-your-own certs
./openpublish serve --tls-cert cert.pem --tls-key key.pem --addr 0.0.0.0:443
```

One file, one process, auto-renewing HTTPS, no nginx, no npm. The SQLite DB is
created/migrated on first boot (`--database-url`, default `sqlite://openpublish.db`).

## 13. What stays identical

- Repository, schema/migrations, content/analytics/experiments engines.
- Every `/api/*` JSON endpoint (headless contract), RSS, auth model
  (cookie session + CSRF token).
- Redirect semantics for first-run setup.

## 14. Effort estimate

~1–2 weeks part-time:
- Templates + static assets: ~1 day
- Page routes + forms/CSRF/redirects: ~3–4 days
- Editor/stats pages (htmx): ~2 days
- Article page + tracker.js: ~1 day
- TLS (tiers 1 + 2): ~1–2 days
- Tests (pages + tls + e2e rewrite) + CI + docs + cleanup: ~2–3 days

## 15. Deviations from the plan (as implemented)

- **htmx (Phase 3) was dropped.** The editor live preview uses a small vanilla-JS
  `fetch('/api/render')` debounce; the create-experiment form has one fixed
  variant field; comment submit and experiment actions are plain form posts.
  This is the plan's own documented fallback, minus the removed preview.
- **Templates differ slightly from the list:** `flash.html`/`comment_form.html`
  are not separate files (flash is a `base.html` partial and the comment form is
  inline in `article.html`); `base.html` nav is brand + Dashboard/RSS when
  authed (the "Blog" link is the brand itself).
- **Slug fix:** the editor regenerates a draft's slug from its title on save, so
  UI-created posts publish under their real title slug instead of the old
  `untitled` placeholder. The API's `PATCH` title update still never changes the
  slug (stable public URLs), and once a post is published its slug is frozen.
- **HTTPS redirect is a genuine 301** (the plan says 301; `Redirect::temporary`
  would have been a 307).
- **E2E harness lives at the repo-root `e2e/`** instead of `frontend/e2e/`, so
  `frontend/` could be deleted wholesale as decided.
- **TLS tests** verify with `add_root_certificate` (the trusted self-signed cert)
  rather than `danger_accept_invalid_certs(true)`, and cover the Secure-cookie
  contrast between HTTPS and plain HTTP plus the redirect target/query handling.
- **The auto-decider and ACME event-loop are background tasks** spawned by
  `serve`; there is no SIGHUP handling (cert reload is a 30 s mtime poll).
