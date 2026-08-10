# Add-on plan: themes + home page layouts

Status: **planned** (this document is the specification; nothing shipped yet).
Follows: `single_binary_plan.md` (the current server-rendered architecture).
Related: settings store `site.url`/`site.tagline`; SEO head + `/robots.txt`,
`/sitemap.xml`, RSS base URLs all derive from `canonical_base`.

## 1. Why

The public site currently has **one design for everything**: a single Askama
home page (`templates/home.html`) and a hardcoded theme registry (`THEMES` in
`crates/server/src/pages.rs`). A "one fits all" design cannot serve different
blogs:

- Different blogs want different looks (light editorial, magazine, card grid,
  etc.) without editing Rust and recompiling.
- `cargo build` compiles Askama templates and `include_str!`-embeds static
  assets, so there is **no runtime override path** today.
- The `theme` setting already stores an arbitrary string, but `validate_settings`
  rejects anything not in the fixed `THEMES` list.

**Goal:** admin-installable **add-ons** for two orthogonal concerns:

1. **Themes** — whole-site appearance, delivered as CSS. Swap the look of every
   public page (and the admin pages) by changing a folder.
2. **Home page layouts** — the structure/markup of the main page, delivered as a
   template. Swap between built-in layouts and third-party ones.

Add-ons are **trusted admin-installed content** (the blog owner drops folders on
their own server); no sandboxing beyond path validation is required.

## 2. Decisions (locked)

1. **Runtime template engine: `minijinja`** (runtime Jinja2, pure Rust,
   auto-escaping). Its `{{ }}` / `{% if %}` / `{% extends %}` / `{% block %}`
   syntax matches the Askama templates already in the repo, so layout authors
   write in a familiar style. It is used for the **public shell + home page
   only** in this plan; admin/editor/stats pages stay on Askama. The article
   page migrates to the same engine in a later phase (see §11) — until then,
   layout add-ons only affect the home page.
2. **Install mechanism: drop-folder + restart.** The registry scans the add-ons
   directory once at startup into `AppState`. No hot-reload, no admin upload UI,
   no zip handling in this plan.
3. **Convention over manifest.** Add-ons are plain folders, discovered by
   convention. No JSON/TOML manifest to write:
   - `addons/themes/<id>/theme.css` — a theme add-on.
   - `addons/layouts/<id>/home.html` — a home layout add-on (optional
     `base.html`, `macros.html` alongside).
   - `<id>` must match `^[a-z0-9_-]{1,64}$`.
4. **Two built-in layouts ship.** `classic` (today's home page, unchanged markup)
   and `cards` (the blog-style redesign: masthead with tagline, entry cards with
   excerpt, tags, reading time, date). Both ship embedded in the binary so the
   feature is selectable out of the box and the default behavior is unchanged.
5. **`classic` is the default layout and the fallback.** If the configured
   add-on layout is missing/broken at render time, log a warning and render
   `classic`.
6. **Settings gain `home.layout`** alongside `theme`. Both selects list built-ins
   **plus** discovered add-ons; validation checks against the registry, not the
   old fixed consts.

## 3. Architecture

```
forgepost serve
├── /static/themes/{name}.css   GET  theme CSS (built-in or add-on) [new]
├── /                            home page (runtime-rendered) [changed]
├── /admin/settings              theme + home layout selects [changed]
└── addons/                      registry root (default ./addons, --addons-dir)
    ├── themes/<id>/theme.css
    └── layouts/<id>/home.html  (+ optional base.html, macros.html)
```

- A new `Addons` registry is built once at startup and stored in `AppState`
  (`crates/server/src/lib.rs`). It exposes:
  - `theme_ids() -> Vec<(String, String)>` — `(id, label)` for built-ins +
    add-ons, used by the settings select and validation.
  - `layout_ids() -> Vec<(String, String)>` — same for home layouts.
  - `theme_css(id) -> Option<String>` — CSS content (embedded for built-ins,
    read from disk for add-ons, mtime-cached).
  - `render_home(layout, ctx) -> Result<String, RenderError>` — renders the
    home page through minijinja; unknown/missing add-on template falls back to
    `classic`.
- The registry **never maps a raw request path to disk**. Ids are validated
  against the `[a-z0-9_-]` rule and looked up in the known sets; anything else
  returns 404.

## 4. Add-on registry (new module `crates/server/src/addons.rs`)

```rust
pub struct Addons {
    themes: HashMap<String, Theme>,     // id -> Theme { label, css: Arc<str> }
    layouts: HashMap<String, Layout>,   // id -> Layout { label, source: TemplateSource }
}
```

- `Addons::load(addons_dir: Option<&Path>) -> Addons` — scans
  `<dir>/themes/*` and `<dir>/layouts/*`; skips invalid ids and unreadable
  folders (logged). When `--addons-dir` is unset, defaults to `./addons` and
  tolerates the directory being absent.
- Built-ins are registered first, add-ons after, so an add-on with a colliding
  id (e.g. a `dark` theme folder) is ignored with a warning — built-ins win.
- `theme_css` returns embedded built-in CSS or the add-on file contents. Add-on
  CSS is read once at startup (cache); edits require a restart (Decision 2).
- `render_home` builds a minijinja `Environment` per request (or keeps a cached
  environment per layout): registers the shared `base.html` shell, the layout's
  `home.html`, and any add-on-provided `macros.html`/`base.html` override.
  Auto-escaping is enabled for `.html` templates. A failed render falls back to
  `classic`.

## 5. Themes

### 5.1 Built-in themes move out of `app.css`

Today all theme palettes live as `:root[data-theme="…"]` blocks in
`crates/server/static/app.css`. This plan:

- Keeps **shared/layout styles and the `:root` light defaults** in `app.css`
  (so pages remain legible even before/without the theme stylesheet).
- Moves the per-theme variable blocks into embedded per-theme files:
  `crates/server/static/themes/system.css`, `light.css`, `dark.css`, `sepia.css`,
  `solarized.css`. `system.css` contains only the dark `prefers-color-scheme`
  override (light is the `:root` default); the others carry their
  `[data-theme="…"]` blocks.
- `static_file` (or a new route, §7) serves them.

### 5.2 Serving theme CSS

New route `GET /static/themes/{name}.css`:

- `name` in built-in set → embedded CSS, `Content-Type: text/css`.
- `name` in add-on registry → `addons/themes/<id>/theme.css` contents.
- else → 404.

### 5.3 Page inclusion

Both shells — the Askama `templates/base.html` (admin) and the runtime shell
(public) — emit, after `app.css`:

```html
<link rel="stylesheet" href="/static/themes/{{ theme }}.css">
```

The `data-theme="{{ theme }}"` attribute on `<html>` is unchanged, so tests that
assert `data-theme="system"` / `data-theme="sepia"` keep passing.

### 5.4 Settings integration

- `SettingsTemplate.themes` is built from `Addons::theme_ids()` instead of the
  `THEMES` const (`crates/server/src/pages.rs`); the const is removed.
- `validate_settings` accepts any id in the registry ("Unknown theme." for
  anything else, same message as today).

## 6. Home layouts

### 6.1 New setting

- `settings` table key `home.layout`, default `"classic"`. Read alongside
  `site.*` in `Repository::site_settings()` (which gains a `layout` field on
  `SiteSettings`); stored via `set_setting("home.layout", …)`.
- `SettingsForm` gains `layout`; missing/empty submissions keep the current
  value so old clients/forms don't reset it.
- `SettingsTemplate` gains `layouts: Vec<LayoutOption>` and the settings page a
  second select (§8).

### 6.2 Runtime shell and layouts

New module `crates/server/src/templates_runtime.rs` embeds, via `include_str!`:

- `templates_runtime/base.html` — the public shell, matching today's Askama
  `base.html` exactly (DOCTYPE, viewport, `title`, head meta block, favicon,
  app.css + theme css links, site header/nav with authed/admin links, flash
  partial). Uses Jinja `{% block %}` so layouts can override `title`/`head_meta`/
  `content`.
- `templates_runtime/classic_home.html` — byte-equivalent rendering of today's
  `templates/home.html` (keeps `<h1>{{ site_name }}</h1>`).
- `templates_runtime/cards_home.html` — the blog-style layout: masthead
  (`h1` site name, tagline, RSS link), then entries with title link, meta line
  (date, reading time, tag badges), and a clamped excerpt; empty state kept.

### 6.3 Home context (shared by all layouts)

The home handler builds one context used by every layout:

```json
{
  "site_name": "...", "tagline": "...", "theme": "...", "authed": false,
  "flash": "", "seo": { ... },
  "posts": [ { "title": "...", "slug": "...", "date": "2026-08-06",
               "iso_date": "2026-08-06", "excerpt": "...", "tags": ["..."],
               "reading_minutes": 4 } ]
}
```

### 6.4 Enriched posts (backend change in `pages.rs` `home_page`)

Today `HomePost` carries only title/slug/date from `list_published()`. The
handler now enriches each post using existing repository methods
(`get_document` + `document_tags`):

- **excerpt** — first body-text block, whitespace-collapsed, truncated to
  ~160–200 chars (parameterize the existing `page_meta_description` helper).
- **tags** — via `document_tags(document_id)`.
- **reading_minutes** — word count of all text blocks ÷ 220 wpm, min 1.
- **iso_date** — `published_at_ms` formatted as `YYYY-MM-DD` (new helper,
  shares `civil_from_days`).

For small blogs this is N+1 reads over SQLite; acceptable, and a joined query is
a later optimization (noted in §11).

### 6.5 Render fallback

`render_home` swallows template errors (missing/broken add-on `home.html`) with
a `tracing::warn!` and renders `classic`, so a bad add-on never takes the blog
down.

## 7. Routes

`crates/server/src/lib.rs`:

- `.route("/static/themes/{name}", get(routes::theme_css))` — new.
- `GET /` (home) — now runtime-rendered; handler reads `home.layout` and calls
  `Addons::render_home`.
- `AppState` gains `pub addons: Addons` (built in `app_with_config`; tests build
  it with an in-memory/absent add-ons dir).

`crates/server/src/main.rs`: `ServeArgs` gains

```
--addons-dir <PATH>   default ./addons   (env FORGEPOST_ADDONS_DIR)
```

## 8. Settings UI

`templates/settings.html` gains a "Home layout" select next to Theme:

```html
<field>
  <label for="layout">Home layout</label>
  <select id="layout" name="layout">
    {% for l in layouts %}
    <option value="{{ l.value }}"{% if l.selected %} selected{% endif %}>{{ l.label }}</option>
    {% endfor %}
  </select>
</field>
```

## 9. Tests

New `tests/addons.rs` (in-memory app + temp `addons/` dir):

- Theme CSS served for each built-in id (200, `text/css`, contains the theme's
  variable block).
- Add-on theme CSS served from `addons/themes/<id>/theme.css`.
- Unknown id and traversal (`..`/`%2e%2e`) → 404.
- Settings page lists built-in + add-on themes and both layouts; selecting an
  add-on theme/layout persists and renders.
- `classic` renders the current home markup (h1 + post list) — regression
  guard for existing pages tests.
- `cards` renders excerpt, tags, reading minutes, iso date.
- Invalid ids (`Bad!Name`) are ignored at scan; colliding add-on id loses to
  the built-in.
- A broken add-on `home.html` falls back to `classic` (page still 200).

Existing suites must stay green: `tests/pages.rs` (h1 + `data-theme`
assertions, settings option labels), `tests/api.rs` (`/rss`, `/robots.txt`,
`sitemap.xml` — unaffected), `tests/system.rs`, `tests/tls.rs`, lib unit tests.

## 10. Effort estimate

~2–3 days:

- Spec + registry module + `--addons-dir`: ~0.5 day
- Theme CSS split + `/static/themes` route + base.html links: ~0.5 day
- minijinja migration of base + home, built-in `classic`/`cards`, enriched
  post context: ~1 day
- Settings (`home.layout`) + validation + select: ~0.5 day
- Tests (`tests/addons.rs`), regression fixes, clippy, docs: ~0.5–1 day

## 11. Open items / later phases

- **Article page on the runtime engine** — migrate `templates/article.html`
  (comments, JSON-LD, tracker include) so add-ons can restyle articles; layout
  add-ons then own `article.html` too.
- **Hot-reload** — rescan the add-ons dir on an interval or on each request with
  an mtime cache, removing the restart requirement.
- **Zip upload in the admin UI** — accept a `.zip` add-on, validate archive
  paths, unpack under `addons/`.
- **Excerpt via one joined query** — replace the per-post `get_document` N+1
  with a `list_published_details()` repository method.
- **Theme label/description** — if a manifest is ever wanted, start with an
  optional `addon.toml` (label, author, description) falling back to the folder
  name.
