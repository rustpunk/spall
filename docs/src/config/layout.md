# Config Layout

Spall stores all configuration under your platform's config directory (typically `~/.config/spall/` on Linux, `~/Library/Application Support/spall/` on macOS, `%APPDATA%\spall\` on Windows).

## Directory Structure

```text
~/.config/spall/
├── config.toml          # Global settings
├── apis/
│   ├── github.toml      # Per-API overrides
│   └── petstore.toml
├── specs/               # Optional auto-scan directory
│   ├── internal-api.yaml
│   └── partner-api.json
└── cache/
    ├── <hash>.raw       # Cached remote spec bytes
    ├── <hash>.raw-meta  # TTL + ETag metadata
    ├── <hash>.ir        # Compiled IR (postcard)
    ├── <hash>.idx       # Lightweight SpecIndex
    ├── <hash>.meta      # IR cache metadata (SHA-256 + version)
    └── history.db        # SQLite request history
```

## Global Config (`config.toml`)

```toml
# Register APIs inline
[[api]]
name = "github"
spec = "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.json"

[[api]]
name = "petstore"
spec = "https://petstore.swagger.io/v2/swagger.json"

# Auto-scan directories
spec_dirs = [
    "~/.config/spall/specs",
]

[defaults]
output = "json"    # json | pretty | raw | yaml | table | csv
color = "auto"     # auto | always | never

[defaults.proxy]
url = "http://proxy:8080"
```

## Per-API Config (`apis/{name}.toml`)

Created automatically by `spall api add`. You can edit these to add auth or overrides:

```toml
source = "https://petstore.swagger.io/v2/swagger.json"
base_url = "https://staging.petstore.io"

[auth]
kind = "Bearer"
token_url = "env://PETSTORE_TOKEN"

[headers]
X-Client = "spall-cli"

[profiles]

[profiles.staging]
base_url = "https://staging.petstore.io"

[profiles.production]
base_url = "https://petstore.io"
```

The `auth` table supports inline tokens, environment variables, and `hasp` secret URLs (`env://`, `file://`, `keyring://`). See [Authentication](../usage/authentication.md) for the full priority chain and URL scheme reference.

### Per-API fields

| Field | Type | Description |
|-------|------|-------------|
| `source` | string | Spec file path or URL (required) |
| `base_url` | string | Override the spec's server URL |
| `proxy` | string | HTTP/SOCKS proxy URL for this API |
| `auth` | table | Auth configuration (tokens, env vars, `hasp` secret URLs) |
| `headers` | table | Headers added to every request |
| `profiles` | table | Named environment overlays |

## Cache

Spall manages the `cache/` directory automatically. You should not need to edit these files directly.

- **Raw cache** (`*.raw`, `*.raw-meta`) stores fetched spec bytes with TTL and ETag for conditional GET. Proxy settings are respected during spec fetches.
- **IR cache** (`*.ir`) stores the resolved spec in postcard format for instant reload.
- **Index cache** (`*.idx`) stores a lightweight `SpecIndex` for degraded `--help` when the spec is unreachable.
- **History** (`history.db`) is a SQLite database of recent requests.

## Next Steps

- [Profiles](profiles.md)
- [Authentication](../usage/authentication.md)
