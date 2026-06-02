# Authentication

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

### Bearer with Secret URL

```toml
[auth]
kind = "Bearer"
token_url = "keyring://spall/github-token"
```

### OAuth2 (Authorization Code + PKCE)

For APIs that require interactive browser login, configure the IDP endpoints:

```toml
[auth]
kind = "OAuth2"
client_id = "your-app-client-id"
auth_url = "https://idp.example.com/oauth/authorize"
token_url = "https://idp.example.com/oauth/token"
scopes = ["read:user", "repo"]
```

Then run `spall auth login <api>` once:

```bash
$ spall auth login github
Open the following URL in your browser to authorize spall:

    https://idp.example.com/oauth/authorize?response_type=code&client_id=...

Waiting for the authorization callback on 127.0.0.1:53217 ...
Successfully signed in to 'github'. Tokens stored locally.
```

Spall binds a one-shot loopback listener (random port), receives the OAuth2 callback,
exchanges the authorization code at `token_url` using the PKCE verifier, and persists
the `access_token` + `refresh_token` to `$XDG_CACHE_HOME/spall/oauth2/<api>.json`
(mode `0600`). On every subsequent request spall refreshes the access token
automatically when it is within 30 seconds of expiry; if the refresh fails you'll
be asked to run `spall auth login` again.

> **Note:** OAuth2 tokens are session state owned by spall — they live in the cache
> dir, not in `hasp`. The `auth.token_url` field has a different meaning for the
> `OAuth2` kind (the IDP token endpoint) than for `Bearer`/`ApiKey` (a hasp URL).

## Secret URLs with `hasp`

Spall integrates with [`hasp`](https://github.com/rustpunk/hasp) for fetching secrets from multiple backends via URL-style references. This is the **recommended** way to manage credentials. The default build includes three backends:

| Field | Auth Kinds | Example URL |
|-------|------------|-------------|
| `token_url` | `Bearer`, `ApiKey` | `env://MY_TOKEN`, `file:~/secrets/api.key`, `keyring://spall/api-token` |
| `password_url` | `Basic` | `env://ALICE_PASSWORD`, `file:/run/secrets/password`, `keyring://spall/alice-pass` |
| `client_secret_url` | `OAuth2` (confidential clients) | `env://CLIENT_SECRET` |

> For `kind = "OAuth2"`, `token_url` is the **IDP token endpoint** (e.g.
> `https://idp.example/oauth/token`), not a hasp URL. See "OAuth2 (Authorization
> Code + PKCE)" above.

### URL Schemes

| Scheme | Format | Use Case |
|--------|--------|----------|
| `env://` | `env://VAR_NAME` | CI, Docker, local overrides |
| `file://` | `file:///absolute/path` or `file:~/relative` | Kubernetes secret mounts, dotfiles |
| `keyring://` | `keyring://service/entry` | macOS Keychain, GNOME Keyring, Windows Credential Manager |

`hasp` is enabled by default. If you need additional backends (HashiCorp Vault, AWS Secrets Manager, GCP Secret Manager, Azure Key Vault, 1Password, Bitwarden), build with the `hasp-full` feature:

```bash
cargo install spall-cli --features hasp-full
```

### Examples

**Fetch a Bearer token from the OS keyring:**

```toml
[auth]
kind = "Bearer"
token_url = "keyring://spall/github-token"
```

**Read an API key from a file mount (Docker / Kubernetes):**

```toml
[auth]
kind = "ApiKey"
token_url = "file:///run/secrets/api_key"
location = "header"
header_name = "X-Api-Key"
```

**Read a Basic password from an environment variable:**

```toml
[auth]
kind = "Basic"
username = "alice"
password_url = "env://ALICE_PASSWORD"
```

## Inline Tokens (Discouraged)

You can embed a token directly in the config file:

```toml
[auth]
kind = "Bearer"
token = "ghp_xxxxxxxx"
```

Spall will accept it but prints a warning recommending `token_url` or `token_env` instead.

## Credential Hygiene

All credentials are wrapped in `secrecy::SecretString`:

- Memory is zeroized on drop.
- Debug output prints `[REDACTED]`.
- History redacts sensitive headers (`Authorization`, `Cookie`, `X-Api-Key`, etc.).
- `--spall-debug` wire logs also redact secrets.

## Next Steps

- [Config Layout](../config/layout.md)
- [Request History](../operations/history.md)
