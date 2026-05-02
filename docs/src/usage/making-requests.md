# Making Requests

Once an API is registered, every OpenAPI operation becomes a subcommand. Parameter types, enums, and defaults are preserved from the spec.

## Path Parameters

Path parameters are supplied as positional arguments in the order they appear in the path template.

Given a path `/pets/{petId}`:

```bash
spall petstore get-pet-by-id 1
```

Path parameters are always required. If you omit one, clap prints a usage error before any network activity occurs.

## Query Parameters

Query parameters become `--flags`. Given an operation with `in: query` params `status` and `limit`:

```bash
spall petstore find-pets-by-status --status available --limit 10
```

If the spec declares an enum for `status`, spall registers it as clap `possible_values` so typos are caught immediately:

```bash
spall petstore find-pets-by-status --status availbale
# error: invalid value 'availbale' for '--status'
#   [possible values: available, pending, sold]
```

Defaults from the spec are honored:

```bash
# If limit defaults to 20 in the spec, this is equivalent:
spall petstore find-pets-by-status --status available
```

## Header Parameters

Header parameters become `--header-{name}` flags:

```bash
spall petstore create-order --header-x-request-id abc-123
```

## Cookie Parameters

Cookie parameters become `--cookie-{name}` flags:

```bash
spall petstore login --cookie-session abc123
```

Spall collects all cookie params and sends them as a single `Cookie` header.

## Injecting Arbitrary Headers

For headers not declared in the spec, use `--spall-header` (repeatable):

```bash
spall petstore get-pet-by-id 1 --spall-header "X-Custom: value" --spall-header "X-Another: 42"
```

## Overriding the Server URL

Use `--spall-server` to target a different base URL for a single request:

```bash
spall petstore get-pet-by-id 1 --spall-server https://staging.petstore.io
```

The resolution order is:

1. `--spall-server` CLI flag
2. Per-API config `base_url`
3. Operation-level `servers` from the spec
4. Spec-level `servers`
5. Fallback `/`

## Deprecation Warnings

If an operation is marked `deprecated: true` in the spec, spall prints a `[DEPRECATED]` banner in the help text. The operation still works — this is a heads-up, not a gate.

## Next Steps

- [Request Bodies](request-bodies.md)
- [Response Output](response-output.md)
- [Global Flags](global-flags.md)
- [Chaining Requests](repl.md#request-chaining)
