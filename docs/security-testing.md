# Security testing inventory

Living document per `old_docs/security_testing.txt` (Phase 17). The suite is
built as an assurance layer around the existing unit/E2E tests, split into
deterministic security regression tests (Layer 1) and proptest invariants
(Layer 2). Fuzzing (Layer 3) and CI hardening items that are not yet delivered
are tracked at the end.

Every row is a permanent, CI-run regression test: the priors from the security
reviews are expressed as invariants rather than one-off payload checks.

## How to run

```sh
cargo test --workspace                          # everything including security.rs
cargo test -p forgepost-server --test security  # Layer 1 security suite
cargo test -p forgepost-analytics --lib rate    # rate-limiter properties
cargo test -p forgepost-content --lib           # markdown properties
cargo test -p forgepost-infrastructure --lib    # slug / filesystem properties
```

Property suites use proptest with a bounded case count (e.g. 64 cases for the
huge-key rate-limiter property) to stay fast enough for every-PR CI.

## Layer 1 — Security regression tests

`crates/server/tests/security.rs` (shared harness in
`crates/server/tests/common/mod.rs`) plus unit tests where the boundary is
`pub(crate)` and unreachable from a separate integration target.

### Authorization matrix

| Boundary   | Test                                                         | Assertion |
|------------|--------------------------------------------------------------|-----------|
| API        | `anonymous_api_matrix_rejected`                              | every `/api/*` route → 401 anonymous |
| Pages      | `anonymous_pages_redirect_to_login`                          | every `/admin` page → 303 `/login`; `/admin/media` → 401 |
| Guarded    | `owner_session_unlocks_guarded_surface`                      | owner session reaches guarded GET surface (CSRF gate still on top) |

### CSRF

| Boundary | Test                                                        | Assertion |
|----------|-------------------------------------------------------------|-----------|
| API      | `mutating_api_routes_enforce_csrf`                          | every mutating `/api/*` route: missing/wrong/valid-token × no-session/session cases as designed |
| Forms    | `page_forms_accept_token_field_and_reject_forgery`          | POST `/admin/new` honors the `csrf_token` form field; forge → 403 |

### Sessions / cookies

| Boundary    | Test                                                        | Assertion |
|-------------|-------------------------------------------------------------|-----------|
| Persistence | `raw_session_token_is_never_persisted`                      | DB stores only the SHA-256 hash, never the raw token |
| Hardening   | `session_cookie_carries_hardening_attributes`               | HttpOnly, SameSite, Path, Max-Age |
| TLS mode    | `secure_flag_is_set_when_tls_configured`                    | Secure flag present under HTTPS |
| Lifecycle   | `logout_invalidates_the_session_everywhere`                 | token unusable after logout |
| Substitution| `forged_and_substituted_tokens_are_rejected`                | hash token ≠ token; cross-user token rejected |

### Proxy / IP handling

`mod ip_tests` in `crates/server/src/routes.rs` (unit: `resolve_client_ip` is
`pub(crate)`). The middleware-level XFF-bypass-against-rate-limiting regression
already lives in `crates/server/tests/api.rs`.

| Case                      | Test                                        | Assertion |
|---------------------------|---------------------------------------------|-----------|
| No peer                   | `no_peer_resolves_to_unknown_even_with_a_forged_header` | header never honored without a socket peer |
| Untrusted peer            | `untrusted_peer_ignores_forged_x_forwarded_for`        | spoofed header ignored |
| Trusted proxy             | `trusted_proxy_uses_first_forwarded_entry`              | first xff entry used |
| Whitespace/trailing comma | `trusted_proxy_normalizes_whitespace_and_handles_trailing_commas` | trimmed |
| No header                 | `trusted_proxy_without_header_falls_back_to_proxy_ip`   | falls back to peer |
| Prefix matching           | `proxy_matching_is_prefix_based_not_host_based`         | out-of-range peer untrusted |
| IPv6                      | `ipv6_peer_is_preserved_verbatim`                       | preserved verbatim |
| Chain                     | `realistic_chain_keeps_originating_address`             | correct address from chain |

### Uploads

| Boundary    | Test                                                        | Assertion |
|-------------|-------------------------------------------------------------|-----------|
| Content type| `uploaded_type_comes_from_magic_bytes_not_declared_type`    | stored type from magic bytes, never the client's declared type |
| Filename    | `client_filename_is_ignored_and_storage_is_uuid`            | stored under a UUID, client name discarded |
| Serving     | `served_media_has_sniffed_type_and_nosniff`                 | `X-Content-Type-Options: nosniff` + sniffed type on read-back |
| Names       | `adversarial_media_names_are_404`                           | traversal/double-extension names → 404 |

### Experiment attribution (M3, analytics review)

Attribution is server-derived, not client-asserted: the assignment is
deterministic per (experiment, visitor), so the events endpoint recomputes it
from the visitor cookie and rejects anything the browser reports that it was
not actually given. Validated experiment events also carry `version_id`, the
exact immutable version the assigned variant pointed at, so conversion history
can be reproduced against the version pool.

| Boundary            | Test                                            | Assertion |
|---------------------|-------------------------------------------------|-----------|
| Assignment firewall | `experiment_events_require_assigned_variant`    | unassigned variant impression/conversion → 400; valid event 204 and its row records `version_id` |
| One-per-block guard | `experiment_rejects_second_running_on_block`    | starting a second experiment on a block with a running one → 409 (partial unique index) |
| Idempotent conclude | `experiment_conclude_is_idempotent`             | second decide is a no-op; second promote → 409; exactly one decision row |

The system-level `creator_journey_end_to_end` test posts only assignment-aware
visitors and asserts the live report counts impressions/conversions per variant;
`visitor_assignment` asserts the served DOM `variant_id` equals the deterministic
assignment.

## Layer 2 — Property tests (proptest)

### Rate limiter — `crates/analytics/src/lib.rs`

| Property | Test                                                        |
|----------|-------------------------------------------------------------|
| A: first N allowed, N+1 denied | `rate_limiter_first_n_allowed_then_denied` |
| B: key isolation              | `rate_limiter_keys_are_isolated` |
| C: window expiry              | `rate_limiter_window_expires` |
| D: arbitrary clock            | `rate_limiter_never_panics_on_any_clock` |
| E: huge keys saturate safely  | `rate_limiter_record_saturates_and_huge_keys_are_safe` |

### Markdown / HTML — `crates/content/src/markdown.rs`

| Invariant | Test                                                        |
|-----------|-------------------------------------------------------------|
| Executable URL schemes blocked at the renderer (incl. `\n`/`\t` obfuscation) | `executable_url_schemes_are_blocked_at_the_renderer` |
| Arbitrary Markdown never injects live markup (`<script>`, event handlers, scheme attributes) | `render_of_arbitrary_markdown_never_injects_live_markup` |
| Arbitrary Markdown round-trips parse+merge without panicking | `parse_and_merge_never_panic_on_arbitrary_source` |

`is_safe_url` policy (applied to inline-link hrefs and image src/href): strip
BOM/tab/newline before the lowercase scheme check, allow http(s), relative,
`//`-protocol-relative, anchors, mailto and inert `data:image/*`; reject
`javascript:`, `vbscript:`, `file:`, and HTML/SVG-in-data schemes and
`data:application/xhtml`.

### Slugs — `crates/infrastructure/src/sqlite.rs`

| Invariant | Test                                                        |
|-----------|-------------------------------------------------------------|
| Any title → URL-safe slug grammar | `slugify_produces_safe_url_component_for_any_title` |

Note: slugs intentionally keep Unicode alphanumerics (`À` → `à`); the property
asserts the resulting grammar, not ASCII-only output.

### Filesystem / ZIP import — `crates/infrastructure/src/filesystem.rs`

| Invariant | Test                                                        |
|-----------|-------------------------------------------------------------|
| Front matter never panics, body recovered | `front_matter_parse_never_panics` |
| Unresolvable image refs are lossless | `rewrite_with_no_successful_resolution_is_lossless` |
| Post extraction never panics; base dir stays safe | `extract_post_never_panics_and_base_dir_is_safe` |
| Hard limits (size, entries) enforced | `extract_post_rejects_oversized_and_overpopulated_archives` |

## Deferred / not yet delivered

- **Layer 3 fuzzing** (Sprint 4): `cargo-fuzz` targets for markdown, slug,
  upload filenames, ZIP import, URL validation, HTML sanitizer. Deferred until
  a nightly toolchain policy is chosen.
- **`cargo deny`** licenses/bans (Phase 15); **`cargo llvm-cov`** coverage
  targets (Phase 16).
- **Nightly long fuzz runs + crash artifact retention** (Phase 14).
- `cargo audit` — **delivered** as a CI job (see `.github/workflows/ci.yml`):
  a dependency vulnerability fails CI rather than being discovered later.