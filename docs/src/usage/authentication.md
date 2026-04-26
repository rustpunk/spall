# Authentication

Spall supports Bearer tokens, Basic auth, API keys, and OAuth2 pass-through. Auth resolution follows a strict priority chain so that scripts, CI, and interactive sessions can coexist.

## Quick Pass-Through with `--spall-auth`

For one-off testing, pass a token or credentials directly:

```bash
# Bearer token
spall github get-user octocat --spall-auth "Bearer ghp_xxxxxxxx"

# Basic auth (user:pass)
spall internal get-data --spall-auth "Basic alice:secret"

# Shorthand basic (no space, one colon)
spall internal get-data --spall-auth "alice:secret"

# Bare token (treated as Bearer)
spall github get-user octocat --spall-auth "ghp_xxxxxxxx"
```

`--spall-auth` is the highest-priority auth source. It overrides everything else for that single request.

## Environment Variables

If `--spall-auth` is omitted, spall looks for `SPALL_<API>_TOKEN` (hyphens become underscores):

```bash
export SPALL_GITHUB_TOKEN=ghp_xxxxxxxx
spall github get-user octocat
```

## Per-API Config

Store auth settings in `~/.config/spall/apis/{name}.toml`:

```toml
source = "https://api.example.com/openapi.json"
base_url = "https://api.example.com"

[auth]
kind = "Bearer"
token_env = "MY_API_TOKEN"
```

Supported `kind` values: `Bearer`, `Basic`, `ApiKey`, `OAuth2`.

### API Key

```toml
[auth]
kind = "ApiKey"
token_env = "MY_API_KEY"
location = "header"        # or "query"
header_name = "X-Api-Key"  # ignored when location = "query"
query_name = "api_key"     # ignored when location = "header"
```

### Basic Auth

```toml
[auth]
kind = "Basic"
username = "alice"
password_env = "ALICE_PASSWORD"
```

If `password_env` is not set and stdin is a TTY, spall prompts for the password interactively.

## Inline Tokens (Discouraged)

You can embed a token directly in the config file:

```toml
[auth]
kind = "Bearer"
token = "ghp_xxxxxxxx"
```

Spall will accept it but prints a warning recommending `token_env` or a keyring instead.

## Credential Hygiene

All credentials are wrapped in `secrecy::SecretString`:

- Memory is zeroized on drop.
- Debug output prints `[REDACTED]`.
- History redacts sensitive headers (`Authorization`, `Cookie`, `X-Api-Key`, etc.).
- `--spall-debug` wire logs also redact secrets.

## Next Steps

- [Config Layout](../config/layout.md)
- [Request History](../operations/history.md)
