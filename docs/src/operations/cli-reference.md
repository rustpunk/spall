# CLI Reference

## Top-Level Commands

```
spall [OPTIONS] <COMMAND>
```

| Command | Description |
|---------|-------------|
| `spall api ...` | Manage registered APIs |
| `spall auth ...` | Authentication commands |
| `spall history ...` | Request/response history |
| `spall completions ...` | Generate shell completion scripts |
| `spall <api> <operation> [args]` | Execute an API operation |

## `spall api`

| Subcommand | Description |
|------------|-------------|
| `spall api add <name> <source>` | Register a new API |
| `spall api list` | List registered APIs |
| `spall api remove <name>` | Unregister an API |
| `spall api refresh [<name>]` | Refresh cached remote spec |
| `spall api refresh --all` | Refresh all cached remote specs |
| `spall api discover <url>` | Discover and register an API via RFC 8631 |

## `spall auth`

| Subcommand | Description |
|------------|-------------|
| `spall auth status <api>` | Show auth status for an API |
| `spall auth login <api>` | Initiate OAuth2 PKCE login (stub) |

## `spall history`

| Subcommand | Description |
|------------|-------------|
| `spall history list` | List recent requests |
| `spall history show <id>` | Show full request details |
| `spall history clear` | Erase all history |

## Global Flags

See [Global Flags](../usage/global-flags.md) for the full list of `--spall-*` options.

## Next Steps

- [Request History](history.md)
- [Shell Completions](shell-completions.md)
