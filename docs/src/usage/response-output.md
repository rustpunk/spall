# Response Output

Spall automatically chooses an output format based on whether stdout is a TTY.

## Default Behavior

| Context | Format |
|---------|--------|
| TTY (interactive terminal) | Pretty-printed JSON with syntax highlighting |
| Pipe / file redirect | Raw JSON |

```bash
# Pretty JSON (TTY)
spall petstore get-pet-by-id 1

# Raw JSON (piped)
spall petstore get-pet-by-id 1 | jq '.name'
```

## Output Modes

Use `--spall-output` to override:

```bash
# Raw JSON
spall petstore get-pet-by-id 1 --spall-output raw

# YAML
spall petstore get-pet-by-id 1 --spall-output yaml

# Table (requires a JSON array of objects)
spall petstore find-pets-by-status --status available --spall-output table

# CSV (requires a JSON array of objects)
spall petstore find-pets-by-status --status available --spall-output csv
```

Table and CSV modes walk the array and collect all unique top-level keys as headers. If the response is not a JSON array, spall warns and falls back to pretty JSON.

## Saving Responses

Save the response body to a file without touching stdout:

```bash
spall petstore get-pet-by-id 1 --spall-download pet.json
```

Or use the `@file` syntax with `--spall-output`:

```bash
spall petstore get-pet-by-id 1 --spall-output @pet.json
```

Binary responses are streamed raw to the file. When writing to stdout on a TTY, spall emits a warning and suggests `--spall-download`.

## Filtering with JMESPath

Extract fields without installing `jq`:

```bash
spall petstore find-pets-by-status --status available --filter "[].name"
# ["Rex","Fluffy"]
```

If the filter expression is invalid, spall warns and falls back to the unfiltered response.

## Verbose Mode

Print request and response headers to stderr:

```bash
spall petstore get-pet-by-id 1 --spall-verbose
```

With `--spall-time`, the duration is included:

```bash
spall petstore get-pet-by-id 1 --spall-verbose --spall-time
```

Sensitive headers (`Authorization`, `Cookie`, `X-Api-Key`, etc.) are redacted.

## Next Steps

- [Pagination](pagination.md)
- [Request History](../operations/history.md)
