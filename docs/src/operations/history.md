# Request History

Spall records every request to a local SQLite database (`cache/history.db`). This is useful for debugging, auditing, and replay.

## Listing History

```bash
spall history list
```

Output:

```text
  42  2025-04-25 14:32  GET     200   124ms  petstore  get-pet-by-id
  41  2025-04-25 14:30  POST    201   312ms  petstore  add-pet
  40  2025-04-25 14:28  GET     404    89ms  github    get-repo
```

## Searching History

Filter by API name, status code, method, URL substring, or date:

```bash
spall history search --api petstore --status 200 --limit 5
spall history search --method POST --since 2025-04-01
spall history search --url "/pets/" --limit 10
```

All filters are optional and combined with AND logic. `--since` accepts dates in `YYYY-MM-DD` format.

## Viewing a Single Request

```bash
spall history show 42
```

This prints the method, URL, status code, duration, request headers, and response headers. Sensitive headers are redacted.

## Replaying a Request

```bash
# Replay the most recent request
spall --spall-repeat

# Replay a specific request by ID
spall history show 42 --spall-repeat
```

Replay reconstructs the exact method, URL, headers, and body from the history record, then re-executes it.

## Clearing History

```bash
spall history clear
```

This deletes all rows and vacuums the database.

## Privacy Notes

- Request and response headers are recorded, but sensitive headers (`Authorization`, `Cookie`, `X-Api-Key`, etc.) are stored as `[REDACTED]`.
- Request bodies are **not** stored in history.
- The history database is local to your machine and never transmitted.

## Next Steps

- [CLI Reference](cli-reference.md)
- [Shell Completions](shell-completions.md)
