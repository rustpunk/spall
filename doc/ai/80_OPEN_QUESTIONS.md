# Open Questions

## High Priority

### What is the intended config source priority?

- Why it matters: docs and code may disagree about whether per-API TOML, inline `[[api]]`, or `spec_dirs` wins when names collide.
- Files/modules: `spall-config/src/registry.rs`, `spall-config/src/sources.rs`, `docs/internal/spall-design.md`.
- Suggested resolution: add or update tests that assert source priority, then update user/internal docs.

### What is the intended proxy precedence?

- Why it matters: CLI network behavior changes user-visible routing and security expectations.
- Files/modules: `spall-cli/src/http.rs`, `docs/src/config/proxy.md`, CLI tests.
- Suggested resolution: compare implementation, docs, and tests; decide whether env, profile/global config, or explicit CLI flags win in each case.

### Is "no unwrap in library crates" strict or scoped?

- Why it matters: root `CLAUDE.md` states no `.unwrap()` in library crates, but current source reportedly has a mutex-lock `unwrap()` in `spall-core::validator`.
- Files/modules: `CLAUDE.md`, `spall-core/src/validator.rs`.
- Suggested resolution: decide whether to allow internal invariant unwraps, then update root/local guidance.

## Medium Priority

### Which older planning docs are still authoritative?

- Why it matters: `docs/internal` includes scaffold and wave plans that may describe planned behavior rather than current implementation.
- Files/modules: `docs/internal/*`, root planning files, current source.
- Suggested resolution: add status notes to stale plans or prefer current docs/source in future onboarding updates.

### Should `spall-core` still depend on `url`?

- Why it matters: unused dependencies add maintenance and supply-chain surface.
- Files/modules: `spall-core/Cargo.toml`, `spall-core/src/*`.
- Suggested resolution: run `cargo machete` or direct search in a source-changing task; do not remove without normal dependency-change approval.

### What is the long-term role of `execute_legacy_path`?

- Why it matters: parallel request paths can drift in behavior.
- Files/modules: `spall-cli/src/execute.rs`, CLI e2e tests.
- Suggested resolution: document why it remains or consolidate only with parity tests.

### Is URL path parameter replacement intentionally unencoded in `spall-openapi::build_request`?

- Why it matters: changing this affects request URLs and compatibility.
- Files/modules: `spall-openapi/src/builder.rs`, `spall-cli/src/execute.rs`, request tests.
- Suggested resolution: add explicit tests and docs for current behavior before changing.

## Low Priority

### Should `doc/ai` be included in mdBook or remain agent-only?

- Why it matters: affects discoverability and maintenance workflow.
- Files/modules: `doc/ai`, `docs/src/SUMMARY.md`.
- Suggested resolution: keep agent-only unless humans want it published.

### Should `reserve/` have local agent guidance?

- Why it matters: it is a placeholder crate and probably should not receive routine implementation edits.
- Files/modules: `reserve/`.
- Suggested resolution: leave without local `AGENTS.md` unless package-reservation work resumes.
