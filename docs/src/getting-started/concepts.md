# Key Concepts

## Two-Phase Parse

Spall uses a two-phase argument parser so that `--help` feels instant even when the underlying spec is large.

**Phase 1 — Index scan (~1ms)** reads only your config files (`~/.config/spall/config.toml`, `apis/*.toml`, and any `spec_dirs`). It registers each API as a clap subcommand stub with `disable_help_flag(true)` so that `--help` falls through.

**Phase 2 — Spec load (~50–200ms)** happens only when you actually invoke an API. The spec is loaded from cache or fetched from its source, `$ref`s are resolved, and a full clap command tree is built. If the spec is unreachable, spall falls back to a lightweight cached `SpecIndex` so you still see the operation list.

## Dynamic Command Building

Unlike tools that generate static Rust code from a spec, spall constructs clap `Command` and `Arg` objects at runtime. This means:

- No recompilation when an API changes.
- Schema enums become clap `possible_values`.
- Defaults from the spec become clap `default_value`.
- No generated code bloat.

## Parameter Namespacing

OpenAPI allows a path param `id` and a query param `id` on the same operation. Spall namespaces them internally while preserving user-facing names:

| OpenAPI `in` | Internal ID | User-facing |
|--------------|-------------|-------------|
| path | `path-id` | positional argument |
| query | `query-id` | `--id` |
| header | `header-id` | `--header-id` |
| cookie | `cookie-id` | `--cookie-id` |

## IR Cache

After the first parse, spall serializes the resolved spec to a compact binary format (postcard) keyed by SHA-256 of the raw spec bytes. On subsequent runs it skips YAML/JSON parsing and `$ref` resolution entirely. Cache invalidation is automatic when the spec content changes or when spall upgrades its IR format.

## Credential Stack

Auth resolution follows a strict priority chain so that scripts, CI, and interactive sessions can all coexist:

1. `--spall-auth` CLI override
2. Per-API config `[auth]` section
3. Environment variable (`SPALL_<API>_TOKEN`)
4. Interactive prompt (Basic auth only, TTY)

All credentials are wrapped in `secrecy::SecretString` — they are zeroized on drop and redacted from debug output and history.

## Exit Codes

Spall returns structured exit codes so scripts can branch on outcome:

| Code | Meaning |
|------|---------|
| 0 | Success (2xx response) |
| 1 | CLI usage error |
| 2 | Network / connection error |
| 3 | Spec loading / parsing error |
| 4 | HTTP 4xx response |
| 5 | HTTP 5xx response |
| 10 | Request body / parameter validation failed |

## Next Steps

- [Registering APIs](../usage/registering-apis.md)
- [Making Requests](../usage/making-requests.md)
- [Config Layout](../config/layout.md)
