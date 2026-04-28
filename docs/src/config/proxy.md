# Proxy Support

Spall routes requests through HTTP and SOCKS proxies. Proxies can be configured via CLI flags, per-API config files, global defaults, or standard environment variables.

## Resolution Priority

Spall resolves the effective proxy URL using the following priority (highest first):

1. **`--spall-no-proxy`** — disables proxying entirely for the request.
2. **`--spall-proxy <url>`** — CLI override.
3. **Per-API `proxy`** in `~/.config/spall/apis/{name}.toml`.
4. **Profile `proxy`** in the active per-API profile.
5. **Global default `proxy`** in `~/.config/spall/config.toml`.
6. **Environment variables** — `HTTPS_PROXY`, then `HTTP_PROXY`, then `ALL_PROXY`.
7. **Direct connection** — no proxy.

## CLI Flags

```bash
# Use a proxy for this request only
spall petstore get-pet-by-id 1 --spall-proxy http://proxy:8080

# Disable proxy for this request (ignore config/env)
spall petstore get-pet-by-id 1 --spall-no-proxy

# SOCKS5 proxy
spall petstore get-pet-by-id 1 --spall-proxy socks5://localhost:1080
```

## Config File

### Global default proxy

```toml
# ~/.config/spall/config.toml
[defaults.proxy]
url = "http://proxy.corp.internal:8080"
```

### Per-API proxy

```toml
# ~/.config/spall/apis/internal.toml
source = "https://api.internal.example/openapi.json"
proxy = "http://proxy.corp.internal:8080"
```

### Profile-level proxy

```toml
# ~/.config/spall/apis/internal.toml
source = "https://api.internal.example/openapi.json"

[profiles.staging]
proxy = "http://staging-proxy:8080"

[profiles.production]
proxy = "http://prod-proxy:8080"
```

## Environment Variables

Spall reads the standard proxy environment variables when no config or CLI value is set:

| Variable | Description |
|----------|-------------|
| `HTTPS_PROXY` | Preferred for HTTPS destinations |
| `HTTP_PROXY` | Fallback |
| `ALL_PROXY` | Fallback for any protocol |
| `NO_PROXY` | Comma-separated list of hosts to bypass |

```bash
export HTTPS_PROXY=http://proxy:8080
export NO_PROXY=localhost,127.0.0.1,.local

# Direct — proxy is skipped for localhost and *.local
cd spall petstore get-pet-by-id 1
```

### `NO_PROXY` format

- `*` — bypass all proxies.
- `example.com` — exact match.
- `.example.com` — matches `example.com` and `*.example.com`.
- Values are case-insensitive.

## Authentication

Embed credentials directly in the proxy URL:

```bash
--spall-proxy http://user:password@proxy:8080
```

> **Note:** The password is passed directly to `reqwest` and is not logged by spall. Use `--spall-debug` with care; reqwest may still include the URL in wire logs.

## Next Steps

- [Config Layout](layout.md)
- [Profiles](profiles.md)
- [Global Flags](../usage/global-flags.md)
