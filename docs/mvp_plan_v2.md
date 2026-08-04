# OpenPublish MVP Plan v2

## 0. Lean framing

This plan follows Build-Measure-Learn. The **riskiest assumption is demand** (creators will engage with a measurable optimization loop), not feasibility. So the plan is re-sequenced: validate demand before building, build only what tests the hypothesis, and gate progress on measured learning — not on shipping features.

Two hypotheses, written before code:

- **Value hypothesis:** creators who publish regularly will run more experiments and improve their articles more when every headline, image, CTA, and paragraph is measurable and testable — because publishing becomes an optimization game.
- **Growth hypothesis:** 10–15 active creators can be hand-recruited from 2–3 named communities (Ghost/WordPress/IndieWeb writers, self-hosters); the product spreads by observable improvement ("their completion went up after that headline test").

**Innovation accounting:** establish a baseline → tune the loop → decide pivot/persevere at the defined gates (G0, G1, G1b, G2).

## 1. Vision

The product is **block-level experimentation over immutable content**: "publish → A/B test → measure → improve," where every headline, image, CTA, and paragraph is a measurable, experimentable object. The blog is the thin host for the loop; the loop is the product. No one ships this natively and self-hosted today.

It must also be **intuitive for non-technical creators**: installable in minutes with one command, dashboard readable at a glance.

## 2. Hypotheses & success test

### 2.1 Value hypothesis

Creators act on measured feedback: they run experiments weekly, reach decisions, and make ≥1 concrete per-block improvement per month. Evidence: "Section 3 completion +12%" after a rewrite.

### 2.2 Growth hypothesis

Hand-recruited creators from 2–3 named communities adopt the loop; word-of-mouth follows demonstrated improvement. Distribution is deliberately manual at this stage — no organic-growth plan exists yet, and that's acceptable.

### 2.3 Success test (value loop)

- 3–10 creators with meaningful traffic install it (solo or server mode).
- Each active article has ≥1 experiment in its first month.
- Experiments reach a decision (probabilistic report, promoted winner, or "no improvement") instead of lingering.
- Per-block analytics drive ≥1 concrete improvement per creator per month.
- Non-technical onboarding gate: first-time user installs solo mode and runs one experiment in ≤15 minutes, no docs.

### 2.4 Pivot signals

- **Pivot (problem pivot):** after G2, if creators install but don't run second experiments, the "optimization game" premise is wrong — pivot to a different creator pain point surfaced in Phase 0.
- **Pivot (narrow):** if per-block granularity isn't the draw but plain article A/B is, collapse granularity to whole-article tests (schema already supports it).
- **Persevere:** if the value loop metrics are met, invest in post-validation features.

## 3. Phase 0 — Customer discovery (before any code)

**Do not start M0 until Phase 0 passes its GO/NO-GO gate.**

- **Interviews (20–30):** creators who publish 1+/week on Ghost, WordPress, Medium, Substack, or personal sites. Questions target their measurement habits: do they track anything beyond views? Have they ever A/B tested a headline (even manually)? What would make them obsessive about a metric?
- **Look for hacked workflows:** spreadsheets, manual headline swaps, guessing. Existing hacks = demand signal. No evidence of hacks = strong NO-GO signal.
- **Concierge pre-MVP (optional, recommended):** manually run headline/completion experiments for 2–3 real creators on their existing blogs. Watch for obsession ("I want to publish more because I can improve my score"). A weekend of manual work validates the loop before months of automation.
- **Deliverables:** interview summary, validated/rejected value hypothesis, updated growth hypothesis, GO/NO-GO decision, and a list of 5–10 warm launch creators recruited during interviews.
- **Gate G0:** ≥60% of interviewees show existing measurement/optimization behavior (hacks or expressed pain). If not, revise the hypothesis before building.

## 4. MVP scope (re-sequenced)

### The learning MVP (M0–M3): publish + measure + experiment + dashboard

1. **Thin blog host** — users, profiles, Markdown editor (live preview → parsed to block tree), publish, tags, comments (moderation), RSS, one bundled theme + CSS variables. Editor and theme scope hard-capped (no drag-drop editor, no theme engine).
2. **Per-block analytics** — views, unique readers, reading time, scroll depth, completion, retention, referral source; block-level drop-off first-class.
3. **Block experiments** — headline/image/CTA/paragraph overlays; traffic split; probabilistic reporting + sequential-test promotion; no-winner stopping rule.
4. **Painless install & UX** — solo mode (single binary + SQLite), server mode (`install.sh` → Docker + Postgres + Caddy), `/setup` wizard, plain-language dashboard, first-experiment wizard, `export` for backups + solo→server migration.
5. **Open-source foundation** — AGPL-3.0, monorepo, CI, license compliance.

### Post-validation (explicitly deferred, not MVP)

- **Conversion goals / funnels / email-provider integration** (v1 M4) — assumes the loop exists; build only after G2 passes.
- **Leaderboards / "compete" layer** (v1 M5) — social competition is a growth experiment, not a value test; solo-mode "beat your own best" stays minimal or deferred.
- **Managed hosting, heavy polish, books, search, video, federation, plugin marketplace** — unchanged from "explicitly out."

## 5. Architecture

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
├── migrations/       # per-driver SQLx: migrations/postgres + migrations/sqlite
├── docker/           # all-in-one compose (app + Postgres + Caddy auto-TLS) + install.sh
└── docs/
```

- **Backend:** Rust + Axum + Tokio + SQLx. **Storage is driver-agnostic via a repository layer** — Postgres (server mode) or embedded SQLite (solo mode); binary built with `--features postgres` or `--features sqlite`; shared, driver-independent logic.
- **Frontend:** SvelteKit.
- **Analytics:** browser → Rust event API → DB (SQLite solo, Postgres server); ClickHouse later.
- **Setup:** solo = downloaded binary (`./openpublish serve`); server = `install.sh` (Docker + Postgres + Caddy auto-TLS). First-boot `/setup` wizard; safe defaults; no config files for 90% of installs.
- **Growth path:** `./openpublish export` = backups + documented SQLite→Postgres migration. No lock-in between modes.

## 6. Data model

Carried forward verbatim from `docs/mvp_plan_v1.md` §5–§5.3, including:

- Full schema: `users`, `documents` (never hard-deleted), `blocks`, immutable `block_versions`, `assets`, `experiments`, `experiment_variants`, `experiment_decisions` (v1), `analytics_events` (month-partitioned), `tags`/`document_tags`, `comments`, `follows`, `leads`; `blocks.updated_at`.
- Driver-parameterized DDL (jsonb/bigserial/partitioning vs JSON-as-TEXT/`json_extract`); DB-enforced constraints; delete semantics.
- §5.2 events & statistics: probabilistic reporting, sequential testing, no-winner stopping rule, traffic split, banded scroll events, block attribution, privacy note.
- §5.3 security & trust: argon2, session cookies, CSRF, rate-limited public analytics API, comment spam, upload validation, honest-number labeling.

## 7. Milestones & gates

### Phase 0 — Customer discovery

Interviews, hacked-workflow scan, optional concierge tests, hypotheses written, **Gate G0** (GO/NO-GO).

### M0 — Scaffolding

Workspace (`server`/`content`/`analytics`/`experiments`), repository/storage layer + `postgres`/`sqlite` features + per-driver migrations, SvelteKit shell, all-in-one compose + `install.sh` + solo binary build, CI, AGPL license.

### M1 — Thin blog host + activation

Auth (argon2/sessions/CSRF), Markdown editor → block tree, publish, tags, comments (moderation), RSS, one theme, `/setup` wizard, plain-language UX foundations, `./openpublish export` (backups).

**Gate G1 — Activation:** with warm launch creators recruited in Phase 0: ≥5 creators publish ≥2 posts each within 2 weeks, and ≥60% return to the dashboard. If they don't come back, the problem framing is wrong — fix before building analytics deeper.

### M2 — Per-block analytics

Event API + browser tracking (banded scroll, completion, read time); per-block and per-article aggregations; block-level drop-off dashboard first-class; "estimated" labeling.

**Gate G1b — Measurement:** creators voluntarily check the dashboard repeatedly and can state what they'd improve. (Soft gate; informs G2.)

### M3 — Block experiments

Experiments on any block; traffic split + stable assignment; SSR/hydration-consistent variant rendering; impression/conversion tracking; Bayesian probabilistic reporting + sequential-test promotion + no-winner rule; `experiment_decisions` recorded; **stats-engine correctness tests** (golden + property).

**Gate G2 — Value (pivot/persevere):**

- Persevere if: ≥3 creators each ran ≥1 experiment that reached a decision, and ≥1 promoted improvement per creator exists, and ≥1 creator reports acting on per-block data.
- Else pivot per §2.4 signals. No post-validation features are built until G2 passes.

### Post-validation (only after G2)

- **M4 — Conversion goals** (signup/CTA blocks, funnels, email-provider integration).
- **M5 — Competition layer + polish** (server-mode leaderboards, solo "beat your own best", onboarding docs, managed-hosting consideration).
- Ongoing: use the product's own A/B loop on its onboarding (self-experimentation).

## 8. Innovation accounting summary

| Gate | After | Trigger metric | Decision |
|---|---|---|---|
| G0 | Phase 0 | ≥60% of interviewees show optimization behavior | GO / revise hypothesis |
| G1 | M1 | ≥5 creators, ≥2 posts each, ≥60% dashboard return in 2 weeks | fix problem framing / continue |
| G1b | M2 | creators voluntarily check dashboard, can name next improvement | continue / deep-dive UX |
| G2 | M3 | ≥3 creators, experiments reach decisions, promoted improvements | **persevere → M4/M5** / pivot |

## 9. Growth & activation

- **Phase 0 recruits launch cohort:** 5–10 warm creators, already interview-engaged.
- Distribution is manual: 2–3 named communities (Ghost/WordPress/IndieWeb, self-hosters), direct outreach.
- Activation lever = non-technical install (solo binary + wizard); every install friction removed is an activation test, gated at G1.
- No paid acquisition, SEO, or marketplace before G2.

## 10. Key design decisions to protect early

- **Experiments-first, metrics-first:** the blog is a thin host; the loop is the product.
- **Immutable BlockVersions + experiments as overlays** — foundation for everything later.
- **Store the semantic document; derive all formats** from it.
- **Probabilistic reporting from day one** so experiments work at low traffic.
- **Storage-agnostic core** — SQLite solo and Postgres server, same logic.
- **No lock-in between modes** — `export`/import first-class.
- **Stats-engine correctness is a first-class deliverable** — simulated-experiment tests.
- **Non-technical is a pillar:** zero-config defaults, plain-language UX, progressive disclosure.
- **Build nothing that doesn't test the current hypothesis** — post-validation features are deferred until G2.

## 11. Licensing & third-party compliance

Carried forward from `docs/mvp_plan_v1.md` §8 verbatim:

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
