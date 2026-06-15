# AGENTS.md

## Purpose

`spall-openapi` defines Spall's transport-neutral request/response contract.

## Responsibilities

- Build `HttpRequestSpec` from resolved operations and typed inputs.
- Define neutral request, response, status, pagination, and streaming types.
- Provide transport-neutral auth contributors.
- Stream JSON response items without buffering whole pages.

## Public APIs And Entry Points

- `build_request`
- `HttpRequestSpec`, `Headers`, `RequestBody`, `MultipartField`, `MultipartValue`
- `ResponseStream`, `Status`
- `DataPath`, `Paginator`, `parse_rfc5988`
- `ItemStream`, `JsonSkimmer`, `StreamLimits`, `StreamError`
- `bearer`, `basic`, `api_key`, `oauth2_access_token`, `oauth2_client_credentials_request`

## Internal Module Map

- `builder.rs`: pure operation/args/body to request spec.
- `request.rs`: neutral request/body descriptors.
- `response.rs`: streaming response wrapper.
- `status.rs`: neutral HTTP status newtype.
- `datapath.rs`: item location parsing.
- `links.rs`, `paginate.rs`: Link header pagination.
- `stream.rs`: bounded forward-only JSON parser and item iterator.
- `auth/`: neutral request mutation helpers.

## Dependency Rules

- Do not add `reqwest`, `tokio`, `clap`, `spall-config`, or normal `*-sys` dependencies.
- Do not perform network, filesystem, stdin, or credential-resolution work here.
- Keep concrete transport in `spall-cli`.

## Invariants

- Request URLs carry no query string; query pairs stay separate for transport encoding.
- `Headers` are lowercased by local helpers; preserve that contract when inserting manually.
- Multipart file values are descriptors, not buffered file contents.
- Do not override an existing `content-type`.
- Pagination follows `Link: rel=next` lazily.
- Preserve `StreamLimits`, max nesting, max item bytes, and max buffered bytes.

## Common Mistakes

- Buffering whole response pages in `ItemStream`.
- Adding concrete HTTP or async behavior for convenience.
- Resolving secrets or OAuth login flows here.
- Setting multipart content-type without a transport-generated boundary.

## Local Commands

- `cargo test -p spall-openapi`
- `cargo test -p spall-openapi --test stream_bound`
- `cargo run -p spall-openapi --example embed`
- `cargo tree -p spall-openapi -e normal`

## Documentation Updates

Update `../doc/ai/30_DESIGN_RULES.md`, `../doc/ai/60_PERFORMANCE_NOTES.md`, and `../doc/ai/AI_CHANGELOG.md` for transport-boundary or streaming changes.

## Unclear / Ask Human

- Confirm before changing path-parameter encoding behavior.
- Confirm before expanding RFC 5988 parser semantics beyond current tests.

## Evidence

`src/lib.rs`, `src/builder.rs`, `src/request.rs`, `src/stream.rs`, `tests/stream_bound.rs`, `examples/embed.rs`, root CI `openapi-purity` job.
