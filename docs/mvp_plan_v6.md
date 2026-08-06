# OpenPublish MVP Plan v6

## 0. Lean framing

This plan follows Build-Measure-Learn. The **riskiest assumption is demand** (creators will engage with a measurable optimization loop), not feasibility. So the plan is re-sequenced: validate demand before building, build only what tests the hypothesis, and gate progress on measured learning — not on shipping features.

Two hypotheses, written before code:

- **Value hypothesis:** creators who publish regularly will run more experiments and improve their articles more when every headline, image, CTA, and paragraph is measurable and testable — because publishing becomes an optimization game.
- **Growth hypothesis:** 10–15 active creators can be hand-recruited from 2–3 named communities (Ghost/WordPress/IndieWeb writers, self-hosters); the product spreads by observable improvement ("their completion went up after that headline test").
- **Optimization-learning hypothesis:** when creators opt in to share anonymous experiment outcomes, the engine's priors and thresholds improve for everyone ("share outcomes → smarter experiments"). Seed data comes from concierge tests and our own installs until the cohort grows.

**Innovation accounting:** establish a baseline → tune the loop → decide pivot/persevere at the defined gates (G0, G1, G1b, G2).

## 1. Vision

The product is **block-level experimentation over immutable content**: "publish → A/B test → measure → improve," where every headline, image, CTA, and paragraph is a measurable, experimentable object. The blog is the thin host for the loop; the loop is the product. No one ships this natively and self-hosted today.

It must also be **intuitive for non-technical creators**: installable in minutes with one command, dashboard readable at a glance.

## 2. Hypotheses & success test

### 2.1 Value hypothesis

Creators act on measured feedback: they run experiments weekly, reach decisions, and make ≥1 concrete per-block improvement per month. Evidence: "Section 3 completion +12%" after a rewrite.

### 2.2 Growth hypothesis

Hand-recruited creators from 2–3 named communities adopt the loop; word-of-mouth follows demonstrated improvement. Distribution is deliberately manual at this stage — no organic-growth plan exists yet, and that's acceptable. Competition is the retention lever: opt-in anonymized aggregate sharing powers cross-install benchmarks and leaderboards, ranked on normalized metrics (improvement, completion, retention, experiment activity) — never raw views.

### 2.3 Success test (value loop)

- 3–10 creators with meaningful traffic install it (solo mode).
- Each active article has ≥1 experiment in its first month.
- Experiments reach a decision (probabilistic report, promoted winner, or "no improvement") instead of lingering.
- Per-block analytics drive ≥1 concrete improvement per creator per month.
- Non-technical onboarding gate: first-time user installs solo mode and runs one experiment in ≤15 minutes, no docs.
- **G2 cohort evidence:** each launch creator provides direct improvement evidence in an interview — before/after completion on a promoted article, plus a one-line statement of how the data changed their writing. Hypothesis validation = cohort feedback + local evidence, not central scraping.

### 2.4 Pivot signals

- **Pivot (problem pivot):** after G2, if creators install but don't run second experiments, the "optimization game" premise is wrong — pivot to a different creator pain point surfaced in Phase 0.
- **Pivot (narrow):** if per-block granularity isn't the draw but plain article A/B is, collapse granularity to whole-article tests (schema already supports it).
- **Persevere:** if the value loop metrics are met, invest in post-validation features.

## 3. Phase 0 — Customer discovery (before any code)

**Do not start M0 until Phase 0 passes its GO/NO-GO gate.**

- **Interviews (20–30):** creators who publish 1+/week on Ghost, WordPress, Medium, Substack, or personal sites. Questions target their measurement habits: do they track anything beyond views? Have they ever A/B tested a headline (even manually)? What would make them obsessive about a metric?
- **Look for hacked workflows:** spreadsheets, manual headline swaps, guessing. Existing hacks = demand signal. No evidence of hacks = strong NO-GO signal.
- **Concierge pre-MVP (optional, recommended):** manually run headline/completion experiments for 2–3 real creators on their existing blogs. Watch for obsession ("I want to publish more because I can improve my score"). A weekend of manual work validates the loop before months of automation. **These tests + the founders' own installs seed the first experiment-outcome priors** (dogfooding) until the opt-in learning tier has volume.
- **Deliverables:** interview summary, validated/rejected value hypothesis, updated growth hypothesis, GO/NO-GO decision, and a list of 5–10 warm launch creators recruited during interviews.
- **Gate G0:** ≥60% of interviewees show existing measurement/optimization behavior (hacks or expressed pain). If not, revise the hypothesis before building.

## 4. MVP scope (re-sequenced)

### The learning MVP (M0–M3): publish + measure + experiment + dashboard

1. **Thin blog host** — users, profiles, Markdown editor (live preview → parsed to block tree), publish, tags, comments (moderation), RSS, one bundled theme + CSS variables. Editor and theme scope hard-capped (no drag-drop editor, no theme engine).
2. **Per-block analytics** — views, unique readers, reading time, scroll depth, completion, retention, referral source; block-level drop-off first-class.
3. **Block experiments** — headline/image/CTA/paragraph overlays; traffic split; probabilistic reporting + sequential-test promotion; no-winner stopping rule.
4. **Painless install & UX** — solo mode (single binary + SQLite) is the **only MVP distribution**; server mode (`install.sh` → Docker + Postgres + Caddy) is deferred until a multi-author or hosted need is validated (post-G2). `/setup` wizard, plain-language dashboard, first-experiment wizard, `export` for backups.
5. **Local-first sharing (two tiers)** — installs may opt in (off by default, plain-language toggle) to share only derived, anonymous data; raw events, content, and visitor data never leave the install:
   - **Competition tier:** k-anonymized per-document summaries (≥10 readers) with a pseudonymous handle — powers the game.
   - **Optimization-learning tier:** anonymous experiment outcomes (winner, effect size, sample counts, confidence, goal, topic/length context) + metric distributions (completion/retention histograms) — never content, visitor data, emails, or handle-linked.
   - A local `share_ledger` records everything sent. The MVP ships both plumbings plus one "network percentile" line in the dashboard; the game UI and engine calibration from cohort data are post-G2.
6. **Open-source foundation** — AGPL-3.0, monorepo, CI, license compliance.

### Post-validation (explicitly deferred, not MVP)

- **Conversion goals / funnels / email-provider integration** (v1 M4) — assumes the loop exists; build only after G2 passes.
- **Full game (post-G2)** — cross-install leaderboards, tiers, streaks, via **pseudonymous handles** (claimed at the benchmark service; no real identity, no install accounts). Solo "beat your own best" stays in the MVP.
- **Engine calibration (post-G2)** — tune priors, sequential-test thresholds, and no-winner bounds from the opt-in optimization-learning tier; grows with cohort scale.
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
├── e2e/              # Playwright suite against the single binary
├── migrations/       # SQLx, single driver (SQLite) for the MVP
├── docker/           # (deferred — server mode post-G2)
└── docs/
```

- **Backend:** Rust + Axum + Tokio + SQLx. **Storage is driver-agnostic via a repository layer** — embedded SQLite for the MVP (one driver, one migration set, one test matrix). Postgres stays a later, documented addition through the same layer — no driver-specific macros or dual builds in the MVP.
- **Frontend:** server-rendered. The SvelteKit shell described in the original plan was removed in the single-binary migration; Rust + Askama renders every page (see `single_binary_plan.md`), and the `/api/*` JSON surface is preserved headless. The Playwright e2e harness in `e2e/` is the only remaining Node.
- **Analytics:** browser → Rust event API → SQLite (MVP); ClickHouse later.
- **Setup:** solo = downloaded binary (`./openpublish serve`). Server mode (`install.sh` → Docker + Postgres + Caddy auto-TLS) is deferred. First-boot `/setup` wizard; safe defaults; no config files for 90% of installs.
- **Growth path:** `./openpublish export` = backups + a documented SQLite→Postgres migration, ready when server mode ships. No lock-in by design.
- **Benchmark/learning service** — small, AGPL, self-hostable by anyone, with a documented aggregate-only protocol (a seed of the federation/discovery layer). It speaks two protocols: **competition aggregates** (pseudonymous summaries → percentiles/rankings) and **optimization-learning outcomes** (anonymous experiment results → calibrated engine params served back to installs). It never sees raw events.

### 5.4 Design constraints — big-company door stays open

We **do not target** big companies (no SSO, roles, SOC2, enterprise sales in the MVP). But the architecture must never make adoption by a large team or a CMS vendor structurally impossible. These are design boundaries, not features:

- **API-first core** — the server-rendered pages are a *client* of the same JSON API (see `single_binary_plan.md`); the engine (documents, experiments, decisions) is reachable headless by construction. Frontend-only features with no API surface are the door-closer to avoid.
- **Auth is a normal `users` table + session cookies** — no hard-coded single-user/singleton shortcuts anywhere; roles and SSO remain a later *extension point*, not built now.
- **No driver-specific SQL in the domain layer** — SQLite idioms (`json_extract`, SQLite-only functions) live behind the repository boundary; domain logic stays driver-neutral.
- **Experiment assignment is one reusable engine function** — served over the API *and* used by SSR/hydration. An external CMS could call the same endpoint; no second implementation path.
- **`EventSink` trait** — analytics writes go through an interface so a bulk store (ClickHouse) can replace the SQLite writer later without touching domain logic.
- **Consistent tenancy scoping** — documents/events/users are scoped by owner + install id from day one, so multi-tenant hosting later is a layer, not a redesign.
- **Licensing:** AGPL keeps self-hosting open for anyone; embedding/redistribution routes through a commercial license (already in §11). Big-company adoption is a licensing decision, never an architectural one.

## 6. Data model

Carried forward verbatim from `docs/mvp_plan_v1.md` §5–§5.3, including:

- Full schema: `users`, `documents` (never hard-deleted), `blocks`, immutable `block_versions`, `assets`, `experiments`, `experiment_variants`, `experiment_decisions` (v1), `analytics_events` (month-partitioned), `tags`/`document_tags`, `comments`, `follows`, `leads`; `blocks.updated_at`.
- Driver-parameterized DDL design retained, but **MVP DDL targets SQLite only** (JSON-as-TEXT/`json_extract`); Postgres DDL (jsonb/bigserial/partitioning) ships with the server mode post-G2. DB-enforced constraints; delete semantics.
- §5.2 events & statistics: probabilistic reporting, sequential testing, no-winner stopping rule, traffic split, banded scroll events, block attribution, privacy note.
- §5.3 security & trust: argon2, session cookies, CSRF, rate-limited public analytics API, comment spam, upload validation, honest-number labeling.
- **v3 additions:**
  - **`settings`** — key/value, incl. `analytics_sharing` (off by default) and `benchmark_handle` (pseudonymous handle for leaderboards).
  - **`share_ledger`** — audit trail: what aggregate, when, to which benchmark service.
  - **Aggregate sharing boundary (§5.3):** only derived, k-anonymized summaries (≥10 readers) are shareable; no content, emails, visitor IDs, IPs, or user agents; opt-in, off by default, fully reversible, with a local audit ledger.
- **v4 additions:**
  - **`settings`** gains `learning_sharing` (off by default) — the optimization-learning tier toggle.
  - **`share_ledger`** covers both tiers: competition summaries and learning outcomes.
  - **Learning-tier payload (§5.3):** per-experiment outcome (winner, effect size, N, confidence, goal, topic/length context) + metric distributions (completion/retention histograms). Fully anonymous — no handle linkage, no content, no visitor data, no emails. K-anonymity applies; engine params are only updated from data that clears the floor.

## 7. Milestones & gates

### Phase 0 — Customer discovery

Interviews, hacked-workflow scan, optional concierge tests, hypotheses written, **Gate G0** (GO/NO-GO).

### M0 — Scaffolding

Workspace (`server`/`content`/`analytics`/`experiments`), repository/storage layer on SQLite (repository pattern keeps Postgres a later addition), migrations, SvelteKit shell, solo binary build + `/setup`, CI, AGPL license.

### M1 — Thin blog host + activation

Auth (argon2/sessions/CSRF), Markdown editor → block tree, publish, tags, comments (moderation), RSS, one theme, `/setup` wizard, plain-language UX foundations, `./openpublish export` (backups).

**Gate G1 — Activation:** with warm launch creators recruited in Phase 0: ≥5 creators publish ≥2 posts each within 2 weeks, and ≥60% return to the dashboard. If they don't come back, the problem framing is wrong — fix before building analytics deeper.

### M2 — Per-block analytics

Event API + browser tracking (banded scroll, completion, read time); per-block and per-article aggregations; block-level drop-off dashboard first-class; "estimated" labeling.

**Gate G1b — Measurement:** creators voluntarily check the dashboard repeatedly and can state what they'd improve. (Soft gate; informs G2.)

### M3 — Block experiments

Experiments on any block; traffic split + stable assignment; SSR/hydration-consistent variant rendering; impression/conversion tracking; Bayesian probabilistic reporting + sequential-test promotion + no-winner rule; `experiment_decisions` recorded; **stats-engine correctness tests** (golden + property). **Sharing plumbing:** opt-in anonymized export for both tiers (competition aggregates + learning outcomes, k-anonymized, ledgered) + "network percentile" line in the dashboard. *(Deferrable — see §7.5.)*

**Gate G2 — Value (pivot/persevere):**

- Persevere if: ≥3 creators each ran ≥1 experiment that reached a decision, and ≥1 promoted improvement per creator exists, and ≥1 creator reports acting on per-block data.
- Else pivot per §2.4 signals. No post-validation features are built until G2 passes.

### Post-validation (only after G2)

- **M4 — Conversion goals** (signup/CTA blocks, funnels, email-provider integration).
- **M5 — Competition game + polish** (cross-install leaderboards, tiers, streaks via pseudonymous handles, onboarding docs, managed-hosting consideration).
- **Engine calibration from opt-in outcomes** — refine priors, thresholds, and no-winner bounds using the learning tier; validate against the simulated-experiment test suite.
- Ongoing: use the product's own A/B loop on its onboarding (self-experimentation).

## 7.5 Effort & risk

**Assumptions:** experienced full-stack engineer(s) comfortable in both Rust and web frontends; scope held strictly to this plan; solo/SQLite-only distribution.

### Effort (single experienced engineer)

| Milestone | Scope | Effort |
|---|---|---|
| **M0** Scaffolding | workspace, SQLite repository layer + migrations, SvelteKit shell, solo binary + `/setup`, CI, license | 1–2 wk |
| **M1** Thin blog host | auth/CSRF, **Markdown→block tree**, publish, tags, comments, RSS, one theme, plain-language UX, `export` | 4–6 wk |
| **M2** Per-block analytics | event API, tracking client, per-block aggregations, drop-off dashboard | 3–5 wk |
| **M3** Block experiments | engine, **Bayesian + sequential stats**, correctness tests, SSR hydration | 5–8 wk |
| Onboarding gate + buffer | wizard UX, acceptance test, fixes | 2–3 wk |
| **Total** | | **~15–24 weeks (~4–6 months)** |

A 2-person parallel team (backend/Rust + SvelteKit frontend) shortens wall-clock to ~3–4 months; with testing and the stats engine, budget **5–9 person-months**.

### Biggest implementation risks (ranked)

1. **Stats engine** — Bayesian beta-binomial with sequential spending bounds, no-winner rule, and min-sample logic is specialist math; wrong thresholds silently poison decisions. The correctness test suite is as much effort as the engine.
2. **Markdown ↔ block-tree with stable block identity** — block IDs must survive edits or per-block analytics and block experiments silently break. Subtle, fails silently.
3. **Scroll-to-block attribution ("readers leave at Section 3")** — layout-dependent and approximate by design; risk of plausible-but-wrong data creators trust.
4. **Non-technical UX + the 15-minute onboarding gate** — wizards and plain language are deceptively expensive; the gate makes them a hard requirement.
5. **Per-visitor experiments vs. SSR hydration + caching** — visitor cookie before first render; edge caching and hydration fight per-visitor rendering (flicker, cached variants, A/A artifacts).
6. **Analytics data quality** — bots, ad-blockers, duplicate events, session heuristics; the experiment engine consumes the same noisy stream, which inflates false positives.
7. **Sharing-tier privacy credibility** — k-anonymity, ledger, and off-by-default must be defensible; the audience audits privacy claims.

### Deferrable scope

**M3 two-tier sharing plumbing** is the most cuttable MVP piece — the game UI and engine calibration are post-G2 anyway. Deferring it (ship only the local `share_ledger` skeleton now) pulls ~1–2 weeks and the k-anonymity complexity off the critical path at zero cost to the value hypothesis. The "network percentile" dashboard line ships with it.

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
- **Storage-agnostic core** — repository layer + `export` keep a later Postgres path open; the MVP runs on embedded SQLite only (Postgres is a hypothesis, not MVP scope).
- **No lock-in between modes** — `export`/import first-class.
- **Stats-engine correctness is a first-class deliverable** — simulated-experiment tests.
- **Non-technical is a pillar:** zero-config defaults, plain-language UX, progressive disclosure.
- **Local-first competition:** raw data stays local; only opt-in, k-anonymized aggregates may be shared, under pseudonymous handles.
- **Two clearly separated sharing tiers:** competition (pseudonymous aggregates) vs optimization-learning (anonymous outcomes). The engine only improves with opt-in data; no default-on telemetry.
- **Build nothing that doesn't test the current hypothesis** — post-validation features are deferred until G2.
- **API-first core** — the frontend is a client of the API; the engine stays headless-capable (see §5.4).
- **No driver-specific SQL in the domain layer** — SQLite-only idioms stay behind the repository boundary (see §5.4).

## 11. Licensing & third-party compliance

Carried forward from `docs/mvp_plan_v1.md` §8 verbatim:

### Project license strategy

- **Core server** (`crates/server`): **AGPL-3.0** — network copyleft (§13) so a company cannot take the platform, add proprietary features, and offer it as a closed SaaS.
- **Protocol / federation crates** (later): AGPL-3.0, with an Apache-2.0 re-license consideration before release.
- **Official clients** (desktop/mobile, later): AGPL-3.0.
- **SDKs:** Apache-2.0 or MIT (permissive to maximize adoption).
- **Themes / plugins:** creator's choice (per spec).
- **Benchmark/learning service:** AGPL-3.0 with a self-hostable, documented aggregate-only protocol covering both tiers (competition + optimization-learning) — the platform's own collector must never become the closed super-instance.
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
| PostgreSQL | PostgreSQL License (permissive) | separate service — deferred (server mode post-G2) |
| ClickHouse (later) | Apache-2.0 | separate service |
| Listmonk (optional) | AGPL-3.0 | API integration only |
| Mailcoach / Mailchimp / Brevo / ConvertKit | proprietary SaaS | API calls only, no redistribution |

No listed component imposes copyleft on our code. "Use under AGPL" licenses *our* code; third-party code remains under its own license with notices preserved.

### Compliance checklist (before first release)

- **Dependency audit:** add `cargo-deny` (and `cargo-license`) to CI to catch any GPL/LGPL *transitive* crates; LGPL is acceptable (library carve-out), GPL deps need review.
- **License notices:** ship `LICENSE` (AGPL-3.0) + a `THIRD_PARTY_NOTICES` file with all third-party attributions.
- **Docker images:** include the project license + third-party notices inside the image; respect the base image (Alpine/Debian) license.
- **SDK boundary:** keep the AGPL core behind the API; permissive SDKs reference it over HTTP so SDK consumers are not drawn into copyleft.
