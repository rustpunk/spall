# Spall

> Break free. Hit the endpoint.

**spall** is a dynamic CLI tool that parses OpenAPI 3.x specifications at runtime and generates fully-featured command-line interfaces for making API requests — with validation, auth, colored output, and schema-aware help.

A *spall* is the fragment that breaks free from a corroding metal surface and flies. Your request — shaped by the spec, launched from the terminal, sent across the gap.

Think **Restish, but Rust**.

## Features

- **Dynamic CLI from OpenAPI specs** — no codegen required.
- **Runtime spec loading** from file path or URL.
- **Two-phase parsing** for fast startup and rich per-operation help.
- **Schema-aware argument validation** and typed flags.
- **Colored, formatted response output** with TTY detection.
- **Request history**, **shell completions**, **pagination**, **REPL shell**, and **JMESPath filtering**.

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
```

## Next Steps

- [Installation](getting-started/installation.md) — download and set up spall.
- [Your First Request](getting-started/first-request.md) — register an API and make a call.
- [Key Concepts](getting-started/concepts.md) — understand how spall works.
