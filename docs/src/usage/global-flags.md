# Global Flags

All internal spall flags use the `--spall-*` prefix so they never collide with API parameters.

## Output and Formatting

| Flag | Short | Description |
|------|-------|-------------|
| `--spall-output` | `-O` | Output format: `json`/`pretty`, `raw`, `yaml`, `table`, `csv`, or `@file` |
| `--spall-download` | `-o` | Save response body to a file |
| `--spall-verbose` | `-v` | Print request/response headers to stderr |
| `--spall-debug` | | Wire-level debug logging (redacts secrets) |
| `--spall-time` | | Include request/response timing in verbose output |
| `--filter` | | JMESPath filter expression for JSON responses |

## Network Control

| Flag | Short | Description |
|------|-------|-------------|
| `--spall-server` | `-s` | Override base URL for this request |
| `--spall-timeout` | `-t` | Timeout in seconds (default: 30) |
| `--spall-retry` | | Retry count for failed requests (default: 1, max: 3) |
| `--spall-follow` | `-L` | Follow HTTP redirects (default: off) |
| `--spall-max-redirects` | | Maximum redirects (default: 10) |
| `--spall-insecure` | | Skip TLS certificate verification |
| `--spall-ca-cert` | | Path to custom CA certificate |
| `--spall-proxy` | | HTTP/SOCKS proxy URL |
| `--spall-no-proxy` | | Disable proxy for this request |

## Request Modification

| Flag | Short | Description |
|------|-------|-------------|
| `--spall-header` | `-H` | Inject a non-sensitive header (repeatable) |
| `--spall-auth` | `-A` | Pass-through auth token/header |
| `--spall-content-type` | `-c` | Override request content type |

## Execution Control

| Flag | Short | Description |
|------|-------|-------------|
| `--spall-dry-run` | | Print curl equivalent without executing |
| `--spall-preview` | | Show resolved URL, headers, and body without sending |
| `--spall-paginate` | | Auto-follow `Link` header pagination |
| `--spall-repeat` | | Replay the most recent request from history |
| `--profile` | | Active config profile (e.g., `staging`, `production`) |

## Examples

### Verbose request with timing

```bash
spall petstore get-pet-by-id 1 --spall-verbose --spall-time
```

### Staging override with custom header

```bash
spall petstore get-pet-by-id 1 \
  --spall-server https://staging.petstore.io \
  --spall-header "X-Debug: true"
```

### Retry with redirect following

```bash
spall petstore get-pet-by-id 1 --spall-retry 3 --spall-follow
```

### Replay last request

```bash
spall --spall-repeat
spall history show 42 --spall-repeat
```

## Next Steps

- [CLI Reference](../operations/cli-reference.md)
- [Exit Codes](../operations/exit-codes.md)
