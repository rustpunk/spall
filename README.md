# spall

> Break free. Hit the endpoint.

**spall** is a dynamic CLI tool that parses OpenAPI 3.x specifications at runtime and generates fully-featured command-line interfaces for making API requests — with validation, auth, colored output, and schema-aware help.

A *spall* is the fragment that breaks free from a corroding metal surface and flies. Your request — shaped by the spec, launched from the terminal, sent across the gap.

Think **Restish, but Rust**.

## Features

- Dynamic CLI from OpenAPI specs — no codegen required.
- Runtime spec loading from file path or URL.
- Two-phase parsing for fast startup and rich per-operation help.
- Schema-aware argument validation and typed flags.
- Colored, formatted response output with TTY detection.
- Config profiles, credential resolution, and shell completion support.
- Response validation, pagination, request history, and spec autodiscovery.
- IR cache with `postcard` for fast repeated loads.
- YAML spec parsing via `serde_saphyr` (billion-laughs hardened).

## Status

Alpha — core request flow (Wave 1), QoL (Wave 2), and auth/discovery (Wave 3 Independent) are implemented and tested.

## Quick Usage

```bash
# Register an API
spall api add petstore https://petstore.swagger.io/v2/swagger.json

# List operations
spall petstore --help

# Make a request
spall petstore get-pet-by-id 1

# POST with a body
spall petstore add-pet --data '{"name":"Rex","status":"available"}'

# Output as table
spall petstore list-pets --spall-output table

# Use a profile
spall petstore get-pet-by-id 1 --profile staging

# Replay last request
spall --spall-repeat

# Discover spec from a base URL
spall api discover https://api.example.com
```

## Build / Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo doc --workspace --no-deps
```

## License

MIT OR Apache-2.0
