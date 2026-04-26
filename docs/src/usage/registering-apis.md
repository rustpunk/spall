# Registering APIs

Before you can call an API, spall needs to know where its OpenAPI 3.x spec lives.

## Add an API

```bash
spall api add <name> <source>
```

The source can be a local file path or a URL:

```bash
# Local file
spall api add internal ./specs/internal-api.yaml

# Remote URL
spall api add petstore https://petstore.swagger.io/v2/swagger.json
```

This creates `~/.config/spall/apis/{name}.toml`. The spec is fetched, cached, and indexed immediately.

## List registered APIs

```bash
spall api list
```

Output:

```text
Registered APIs:
  petstore             https://petstore.swagger.io/v2/swagger.json
  internal             ./specs/internal-api.yaml
```

## Remove an API

```bash
spall api remove petstore
```

This deletes `~/.config/spall/apis/petstore.toml` and invalidates the IR cache.

## Refresh a cached spec

Remote specs are cached with a TTL and conditional GET (ETag). If the spec has changed on the server, refresh it:

```bash
# Refresh one API
spall api refresh petstore

# Refresh everything
spall api refresh --all
```

Refresh also invalidates the IR cache so the next request rebuilds it from the new spec.

## Discover an API from a base URL

If a server advertises its OpenAPI spec via RFC 8631 `service-desc` link relation, spall can probe and auto-register:

```bash
spall api discover https://api.example.com
```

Spall will follow `Link: <...spec...>; rel="service-desc"` headers, derive a name from the spec title, and register it just like `api add`.

## Auto-scanning spec directories

You can point spall at a directory full of spec files instead of registering each one manually:

```toml
# ~/.config/spall/config.toml
spec_dirs = [
    "~/.config/spall/specs",
]
```

Files in these directories are auto-registered. Names are derived from the filename minus extension:

| Filename | Registered name |
|----------|----------------|
| `petstore.json` | `petstore` |
| `my-internal-api.yaml` | `my-internal-api` |
| `v2_billing.yml` | `v2-billing` |

Priority (highest → lowest):

1. `apis/*.toml` files
2. `[[api]]` inline entries in `config.toml`
3. `spec_dirs` scanned files

Lower-priority entries with duplicate names are discarded.

## Next Steps

- [Making Requests](making-requests.md)
- [Config Layout](../config/layout.md)
