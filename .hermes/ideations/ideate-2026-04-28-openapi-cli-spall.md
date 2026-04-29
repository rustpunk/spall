# Ideation: spall
_Dynamic OpenAPI 3.x CLI — break free, hit the endpoint._
_Generated: 2026-04-28 | Prior ideation: none_

---

## Refactoring

### Consolidate Phase1/Phase2 flag lookup behind `MergedMatches`
- **Where:** `spall-cli/src/http.rs` lines 44–99; `spall-cli/src/execute.rs` lines 413–437
- **What:** `http.rs` manually reimplements the Phase2-then-Phase1 fallback closure for every field (`timeout`, `retry`, `follow`, `insecure`, etc.). `execute.rs` already defines a `MergedMatches` struct but `http.rs` does not use it.
- **Why it fits:** Every new global flag requires duplicating the fallback logic. Extracting `MergedMatches` into a shared module (e.g., `spall-cli/src/matches.rs`) makes adding a `--spall-*` field a one-line change.
- **Effort:** S

### Normalize parameter extraction in the resolver
- **Where:** `spall-core/src/resolver.rs` lines 186–304
- **What:** `resolve_one_parameter` contains an 80+ line `match &p { Parameter::Query { ... }, Parameter::Header { ... }, ... }` block. Field extraction (`name`, `required`, `style`, `explode`, `schema_ref`, `extensions`) is repeated identically for all four parameter kinds.
- **Why it fits:** `resolver.rs` is the most complex core module; reducing this match to a location-based branch collapses ~60 lines to ~10 and eliminates a common source of drift when new fields are added.
- **Effort:** M

### Extract `ResolvedSchema::empty()` and header-redaction helpers
- **Where:** `spall-core/src/resolver.rs`; `spall-cli/src/execute.rs` lines 595–653
- **What:** `ResolvedSchema` is constructed with 25+ default fields in multiple places. In `execute.rs`, `record_history` takes 9 positional parameters and duplicates the header-redaction loop for request and response headers.
- **Why it fits:** `ResolvedSchema` is a stable IR type; a `Default` impl or constructor centralizes field defaults. A `redact_headers` helper removes the duplicated loop in `record_history`.
- **Effort:** S

---

## Documentation

### Add `///` docs to public IR types
- **Where:** `spall-core/src/ir.rs` (all `pub struct` and `pub enum` definitions)
- **What:** `ResolvedParameter`, `ResolvedSchema`, `ResolvedRequestBody`, `ResolvedResponse`, `HttpMethod`, and `ParameterLocation` have zero doc comments. `style`, `explode`, `is_recursive`, and `additional_properties` are not self-explanatory without context.
- **Why it fits:** These types are the primary API boundary between `spall-core` and `spall-cli`. Documenting them removes a daily friction for anyone working across crates.
- **Effort:** S

### Add crate-level doc comments to `spall-core` and `spall-config` roots
- **Where:** `spall-core/src/lib.rs`; `spall-config/src/lib.rs`
- **What:** Both crate roots re-export every module with only `pub mod cache;` style lines and no `//!` synopsis. A new contributor opening these files gets no map of the public surface.
- **Why it fits:** Onboarding currently requires reverse-engineering from `spall-design.md`. A two-sentence blurb per re-export maps the crate without writing a whole ARCHITECTURE.md.
- **Effort:** XS

### Create a concise `ARCHITECTURE.md` at project root
- **Where:** New file, root of repo
- **What:** `spall-design.md` is 786 lines and buried in the repo. A short `ARCHITECTURE.md` (100–150 lines) covering the two-phase parse, crate boundaries, and IR/cache design would serve as the first-stop developer doc.
- **Why it fits:** Contributors currently open `spall-design.md` and must read past the feature map to find the architecture. A compact doc reduces the barrier to entry for code reviews and bug fixes.
- **Effort:** S

---

## Scope Gaps

### `spall-core/src/loader.rs` still rejects URLs with a stale Wave 1 error
- **Where:** `spall-core/src/loader.rs` lines 46–50
- **What:** `load_raw` returns `SpallCoreError::InvalidSource("URL sources not yet supported in Wave 1: {}")`. The project is in Wave 3; `spall-cli/src/fetch.rs` already implements full HTTP fetching with TTL/ETag caching. The core library's error message is actively misleading.
- **Why it fits:** This is a spec/code drift bug. Either delete `load_raw` entirely (CLI already bypasses it for URLs) or update it to delegate to the fetch layer.
- **Effort:** XS

### OAuth2 PKCE and OS keyring are spec'd Wave 3 but unimplemented
- **Where:** `spall-cli/src/auth/oauth2.rs` (all stubs); `spall-cli/src/commands/auth.rs` lines 61–63; `spall-cli/src/auth/mod.rs` line 102
- **What:** The Wave 3 feature map promises "OAuth2 Authorization Code w/ PKCE" and "OS keyring integration." `oauth2.rs` contains only token-injection and a `#[allow(dead_code)]` `fetch_client_credentials` stub. `auth login` is a no-op with a TODO.
- **Why it fits:** These are committed Wave 3 features tracked in the spec. Leaving them as stubs makes the auth UX incomplete for any API that requires interactive login.
- **Effort:** L

### `x-cli-*` extension vocabulary is incomplete
- **Where:** `spall-core/src/extensions.rs` lines 1–72; `spall-design.md` section 13
- **What:** Only `x-cli-name`, `x-cli-hidden`, and `x-cli-group` are parsed. The design doc notes that "wait loops, body shorthand, and auto-config are Wave 3+", but the current Wave 3 implementation does not include them and there are no TODOs or tracking comments.
- **Why it fits:** Restish compatibility is incomplete. Users migrating from Restish will hit silently ignored extension keys.
- **Effort:** M

---

## Feature Enrichment

### REPL lacks tab completion and reverse-history search
- **Where:** `spall-cli/src/repl.rs` lines 11–92
- **What:** The REPL uses `rustyline::DefaultEditor` with only four hard-coded commands (`help`, `history`, `quit`, `exit`). No completion for API names, operations, or history entries. Reverse-history search (`Ctrl+R`) is not enabled.
- **Why it fits:** Rustyline already supports custom `Completer` implementations and history search. The dependency is paid for; adding a completer that queries the registry turns the REPL from a demo into a usable tool.
- **Effort:** M

### History is list-only — no search or filter
- **Where:** `spall-cli/src/history.rs` (schema supports filtering); `spall-cli/src/commands/history.rs` (only exposes `list`, `show`, `clear`)
- **What:** The SQLite schema stores `api`, `operation`, `method`, `url`, and `status_code`, but the CLI only allows listing by limit or fetching by ID. Users cannot search by API name, status code, or URL substring.
- **Why it fits:** One more subcommand (`history search --api <n> --status 200`) with a few `LIKE` queries makes the history feature significantly more useful for debugging.
- **Effort:** S

### Pagination is Link-header-only
- **Where:** `spall-cli/src/paginate.rs` lines 1–89
- **What:** The paginator only parses RFC 5988 `Link: rel=next`. Many APIs (Stripe, Shopify, GraphQL REST wrappers) rely on cursor or offset parameters embedded in the JSON body.
- **Why it fits:** The `Paginator` struct already encapsulates page traversal and concatenation. Adding a `Strategy` enum (`LinkHeader`, `Cursor { jmespath: String, param: String }`) is a natural extension that doesn't break the existing flow.
- **Effort:** M

---

## New Features (scope-aligned)

### Request chaining — the core Wave 3 gap
- **Where:** Spec: `spall-design.md` Wave 3 table ("Chaining: Capture response values, feed into next request"); Code: zero implementations in `src/`
- **What:** Add `--spall-chain <jmespath>` or a REPL pipe syntax (`api op1 | api op2 --id $prev`) that extracts a value from a JSON response and feeds it into the next request as a parameter.
- **Why it fits:** Explicitly committed in the Wave 3 spec. The existing `--spall-filter` and `--spall-paginate` prove the CLI can manipulate JSON responses between request and output; chaining is the same pipeline with the output becoming input.
- **Effort:** L

### `spall api show <name>` — inspect registered API config
- **Where:** `spall-cli/src/commands/api.rs` lines 36–48
- **What:** `api list` only prints `name` and `source`. Users cannot see `base_url`, `default_headers`, `auth.kind`, `proxy`, or `profiles` without opening `~/.config/spall/apis/*.toml`.
- **Why it fits:** A natural extension of the existing `api` subcommand tree. The `ApiEntry` struct already carries all this data; printing it as a formatted TOML or table is trivial.
- **Effort:** XS

---

## Quality

### Replace ad-hoc `eprintln!` with structured logging
- **Where:** `spall-cli/src/execute.rs`, `spall-cli/src/preview.rs`, `spall-cli/src/fetch.rs`, `spall-core/src/cache.rs`
- **What:** Zero use of `tracing` or `log`. ~30 `eprintln!`/`println!` calls handle diagnostics, warnings, and debug output. This breaks scripted consumers and makes `--spall-debug` unreliable.
- **Why it fits:** As async features multiply (pagination, retry, cache misses), spans and structured event fields would make debugging significantly easier. `tracing` is the standard for async Rust.
- **Effort:** M

### `spall-config` has zero tests
- **Where:** `spall-config/src/` (6 source files, 0 test files); `spall-config/tests/` (does not exist)
- **What:** Registry parsing, credential resolution, TOML deserialization, and auth mapping are completely untested. Error variants like `ConfigNotFound`, `TomlParse`, and `CredentialResolution` are never exercised.
- **Why it fits:** Config parsing handles secrets and routing. A malformed TOML or unexpected env var can silently change behavior. Unit tests for `registry.rs` and `credentials.rs` are high-value and low-effort.
- **Effort:** M

### Add unit tests for `spall-cli/src/validate.rs` and `--spall-filter` e2e
- **Where:** `spall-cli/src/validate.rs`; `spall-cli/tests/` (no `filter_e2e.rs` exists)
- **What:** Preflight and response validation logic is only checked by one expensive e2e test (`validate_e2e.rs`). `filter.rs` has only 3 inline tests and no e2e for the `--spall-filter` flag.
- **Why it fits:** Validation blocks HTTP calls; bugs in preflight logic are expensive to catch via e2e alone. A dedicated unit test harness (mock `ResolvedOperation` + `ArgMatches`) would provide fast feedback.
- **Effort:** M

---

## DX / UX

### Config TOML silently ignores typos and unknown fields
- **Where:** `spall-config/src/sources.rs` (serde structs for `SpallConfig`, `ApiToml`, etc.)
- **What:** The serde structs do not use `#[serde(deny_unknown_fields)]`. If a user types `baseurl` instead of `base_url`, the field is silently discarded and the user gets confusing default behavior with no error.
- **Why it fits:** This is a direct user foot-gun. Adding `deny_unknown_fields` is a single attribute change that turns silent misconfiguration into a clear error message.
- **Effort:** XS

### Auth resolution falls through to `None` and fires unauthenticated requests
- **Where:** `spall-cli/src/auth/mod.rs` lines 106–124; `spall-cli/src/commands/auth.rs` lines 28–34
- **What:** The auth `resolve` chain returns `Ok(None)` when no credential is found, then the request is fired without authentication. The user receives a raw HTTP 401 from the server instead of a CLI-side error like "Auth required for 'foo'. Set SPALL_FOO_TOKEN or configure auth in ~/.config/spall/apis/foo.toml."
- **Why it fits:** Improving the error boundary here aligns with the project's exit-code conventions (exit code 1 for usage/auth errors, 4 for HTTP 4xx). It would also make `spall auth status` more useful by showing the expected env var name.
- **Effort:** S

### Shell completions are static and hard to install
- **Where:** `spall-cli/src/completions.rs`; `spall-cli/src/main.rs` lines 441–444
- **What:** The completions command dumps raw shell scripts to stdout and expects manual redirecting to dotfiles. It also parses `spall api list` output with `grep | awk`, which will break if the list format changes. There is no `--install` flag, no `--output` path, and no support for operation descriptions in completions.
- **Why it fits:** This is a pure CLI ergonomics win. Adding `--install` to write to `~/.config/fish/completions/` (or equivalent) with a `--shell` flag would make completions actually usable, not an exercise in manual redirection.
- **Effort:** S

---

## Hygiene

### Global `unused = "allow"` lint masks dead code
- **Where:** `spall-cli/Cargo.toml` line 59; `spall-cli/src/commands/mod.rs`; `spall-cli/src/commands/api.rs`
- **What:** `[lints.rust] unused = "allow"` suppresses all unused warnings for the binary crate. Per-file `#![allow(unused_imports)]` attributes in `commands/mod.rs` and `commands/api.rs` compound the issue.
- **Why it fits:** Removing the global override and running `cargo fix` would surface legitimate dead code. Example: `spall-cli/src/auth/oauth2.rs` line 14 marks `fetch_client_credentials` with `#[allow(dead_code)]` because it is unused in the auth chain.
- **Effort:** XS

### Stale design-doc dependencies (`crossterm`, `jsonschema`)
- **Where:** `spall-design.md` lines 88–94; no corresponding entries in any `Cargo.toml`
- **What:** The design doc lists `crossterm = "0.29"` and `jsonschema = "0.46"` as dependencies. Neither crate is referenced in any `Cargo.toml` or source file. `jsonschema` is also conspicuously absent from `spall-core/src/validator.rs` which implements manual validation.
- **Why it fits:** Stale dependency references in the primary design doc create confusion when evaluating whether a crate is actually in use. Removing them keeps the doc truthful.
- **Effort:** XS

---

## Open questions
- What is the intended behavior when `spall auth login` is eventually implemented — browser PKCE redirect, or device-code flow?
- Does the history SQLite schema need migration support beyond `PRAGMA user_version`, or is V1 sufficient for the foreseeable future?
- Is request chaining intended to work across APIs (e.g., `api1 op1 | api2 op2`) or only within a single API?

## Out of scope
- OpenAPI 3.1 support (Wave 4, explicitly deferred)
- Daemon mode / plugin system / WASM (Wave 4, deferred)
- Rewrite to use `openapiv3-extended` or `oas3` crate — the project chose `openapiv3` v2 intentionally and flagged re-evaluation for Wave 4
- Mock server (Wave 4, deferred)
