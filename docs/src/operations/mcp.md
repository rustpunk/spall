# MCP Server

`spall mcp <api>` serves every operation in a registered OpenAPI spec as
a [Model Context Protocol][mcp-spec] tool over stdio. Drop the binary
into a Claude Desktop or ChatGPT Apps config and the AI client can call
your API with no integration code.

[mcp-spec]: https://modelcontextprotocol.io/specification/2025-06-18

## What it does

Given an API you've already added with `spall api add petstore <spec-url>`:

```bash
spall mcp petstore        # serves on stdio, default transport
```

Each `ResolvedOperation` becomes one MCP tool. The tool's `inputSchema`
is generated from the operation's parameters and request body; on
`tools/call`, spall dispatches through the same request pipeline used by
`spall <api> <op>` (auth chain, default headers, proxy, retries).

Wire it into Claude Desktop with this entry in
`~/Library/Application Support/Claude/claude_desktop_config.json`:

```jsonc
{
  "mcpServers": {
    "spall-petstore": {
      "command": "spall",
      "args": ["mcp", "petstore"]
    }
  }
}
```

Restart Claude; the tools appear in the sidebar.

## Usage

```text
spall mcp <api>
    [--spall-transport stdio|http]
    [--spall-port <N>]           # HTTP only; default 8765
    [--spall-bind <addr>]        # HTTP only; default 127.0.0.1
    [--spall-allowed-origin <origin>]  # HTTP only; repeatable
    [--spall-include <tag>]      # repeatable
    [--spall-exclude <tag>]      # repeatable
    [--spall-max-tools <N>]
    [--spall-list-tags]
```

- `--spall-transport` selects the wire protocol:
  - `stdio` (default) for Claude Desktop / config-launched servers.
  - `http` for Streamable HTTP per MCP spec 2025-06-18 §HTTP; see
    [Running over HTTP](#running-over-http) below.
- `--spall-include <tag>` keeps only operations carrying that OpenAPI
  tag (repeatable; union semantics).
- `--spall-exclude <tag>` removes operations carrying that tag.
- `--spall-max-tools <N>` deterministically truncates the filtered
  registry to `N` tools when the spec exceeds the cap. See
  [Sizing your server](#sizing-your-server) for the ordering rule.
- `--spall-list-tags` loads the spec, prints a
  `tag\tcount\tsample-op-id` TSV to stdout, and exits without starting
  the server. Useful for crafting an `--spall-include` filter.
- Operations with no `tags` belong to a synthetic tag named `default` —
  you can include/exclude them by that name.

## Tool naming

Tool names come straight from the operation's `operationId`, sanitized
to fit MCP's allowed character class (`[A-Za-z0-9_./-]`, max 64 chars,
lowercased). For example:

| `operationId`     | tool name        |
|-------------------|------------------|
| `getPetById`      | `getpetbyid`     |
| `create user`     | `create-user`    |
| `Foo::Bar`        | `foo-bar`        |

If two sanitized names collide (extremely rare; the resolver
deduplicates `operationId` collisions on load), spall appends `-2`,
`-3`, etc.

## Auth

`tools/call` runs the standard spall auth resolution chain (env var →
hasp → OAuth2 stored token → config field). You must configure
credentials out-of-band before starting the server; MCP gives no
opportunity to prompt interactively.

### Per-tool auth profiles

Some APIs mix public-read and admin-write endpoints, or carry separate
keychain entries per operation class. Two surfaces let you pin
specific tools to a non-default `[profile.*]` block from the API's
config:

```bash
spall mcp github \
    --spall-auth-tool delete-repo=admin \
    --spall-auth-tool transfer-repo=admin
```

The flag is repeatable; `<tool>` matches either the sanitized tool
name from `tools/list` or the raw `operationId` from the spec.

Equivalently, declare the binding inline on the operation in your
spec via the extension `x-mcp-auth-profile`:

```yaml
paths:
  /repos/{id}:
    delete:
      operationId: delete-repo
      x-mcp-auth-profile: admin
      ...
```

When both forms target the same tool, the CLI flag wins.

Profiles named via either path are validated at server start; an
unknown profile name aborts startup with the list of configured
profiles so typos surface immediately.

## Tool annotations

Each entry in `tools/list` carries an `annotations` block with
client-confirmation hints derived from the HTTP method
([MCP spec 2025-06-18 §tools][mcp-tools]):

[mcp-tools]: https://modelcontextprotocol.io/specification/2025-06-18/server/tools

| Method            | readOnlyHint | destructiveHint | idempotentHint |
|-------------------|--------------|-----------------|----------------|
| GET / HEAD / OPTIONS / TRACE | true | false | true |
| PUT / DELETE      | false        | true            | true           |
| PATCH             | false        | true            | false          |
| POST              | (omitted)    | (omitted)       | (omitted)      |

`POST` is intentionally hint-free — the server cannot infer intent.
Override any hint with the operation-level `x-mcp-annotations`
extension, which merges field-by-field over the derived defaults:

```yaml
paths:
  /search:
    post:
      operationId: search
      x-mcp-annotations:
        readOnlyHint: true   # POST that is in fact read-only
        idempotentHint: true
      ...
```

Unknown keys (e.g. `openWorldHint`) pass through so future MCP spec
additions don't require a spall release.

The `title` annotation is auto-derived from the operation's
`summary` field — MCP clients (Claude Desktop, Cursor, ChatGPT Apps)
render this in their tool pickers as a human-readable display name
in place of the sanitized tool slug. An explicit
`x-mcp-annotations.title` in the spec overrides the summary-derived
default; if neither is present, the field is omitted (clients fall
back to the tool name).

Each tool entry also carries `_meta.spall.tags` with the OpenAPI tag
list — useful for clients that surface tags in their UI.

## Debugging

Pass `--spall-verbose` to `spall mcp <api>` (any transport) to dump
server-lifecycle and per-call diagnostics to **stderr**. Stdout
remains pure JSON-RPC; the verbose stream never crosses the protocol
channel, so it's safe to enable while clients are connected.

Each event is one stderr line prefixed with `[spall-mcp]`:

```text
[spall-mcp] kind=startup api=petstore transport=stdio tools=42 profiles=admin,readonly
[spall-mcp] kind=tools/call tool=getpetbyid profile=<default> method=GET url=/pets/{petId}
[spall-mcp] kind=http-request origin=https://app.example.com allowlist=https://app.example.com headers={...}
```

Profile names that appear in the `startup` line are the set spall
validated against your config. A profile only appears on a
`tools/call` line when a request actually triggered it — profile
resolution is lazy, so profiles you never invoke stay un-resolved
(and therefore can't leak via `expose_secret`).

### What is redacted

- **HTTP request headers** (case-insensitive name match):
  - `Authorization` → rendered as `Bearer [REDACTED]`,
    `Basic [REDACTED]`, or `[REDACTED]` for other schemes; the auth
    scheme is preserved so "wrong auth kind" is still debuggable.
  - `Cookie` → `[REDACTED]`.
  - `Proxy-Authorization` → same as `Authorization`.

  The list is hardcoded in `spall-cli/src/mcp/verbose.rs::REDACTED_HEADER_NAMES`
  and a unit-test drift guard asserts every entry actually triggers
  a redaction.

### What is NOT redacted in v1

This is the honest scope statement — do not assume the verbose
stream is safe to share verbatim:

- **URL query parameters.** The `tools/call` line emits the OpenAPI
  `path_template` (e.g. `/pets/{petId}`), not the rendered URL with
  query string. Path-segment values (the `{petId}` substitution) are
  not in the verbose log because the actual rendering happens
  downstream of the MCP dispatcher. A future version may render +
  redact the URL with `?api_key=[REDACTED]` semantics.
- **Request bodies** of the upstream API call.
- **Response bodies** and **response headers** of the upstream API
  call.
- **Custom organization-specific header names** outside the
  hardcoded list above. If your spec uses `X-Foo-Token` or similar
  for a credential, do not enable `--spall-verbose` in environments
  where stderr is captured to durable storage.
- **Browser CORS preflight rejections** never reach the per-request
  log; only POST requests that pass the CORS layer are visible.

If you need to share a verbose dump, pipe through
`--spall-verbose 2>&1 | tee debug.log` and review `debug.log`
manually before sharing. The redactor closes the most common leakage
path (Authorization headers) but is not exhaustive.

`--spall-verbose` is also wired on the request-execution path (when
you run `spall <api> <op>`) for header-trace debugging; the two uses
of the flag are independent and may be combined.

## Limitations

- **Tools only.** No MCP `resources` or `prompts` surfaces in v1.
- **Request/response only.** No progress streaming on long-running
  calls.
- **`oneOf` / `anyOf` / `allOf` are flattened.** Spall's resolver
  collapses schema composition on load, so each tool's `inputSchema`
  reflects a single resolved branch. If your spec relies heavily on
  polymorphism, the tool input shape may be coarser than the spec
  suggests.
- **Recursive schemas collapse.** Schemas that hit the `$ref` cycle /
  depth guard emit `{ "description": "cyclic schema omitted" }` in
  place; clients see a permissive empty schema.

## Running over HTTP

`--spall-transport http` switches the server from line-delimited
JSON-RPC over stdio to Streamable HTTP per [MCP spec 2025-06-18
§HTTP][mcp-http]. The wire shape:

- One POST endpoint at `/` (the bind root). Body is one JSON-RPC 2.0
  request frame; response is the matching reply as `application/json`.
- `Mcp-Session-Id` header is issued on `initialize` and required on
  every subsequent request. Sessions live for the process lifetime;
  restarting the server invalidates all existing sessions.
- Streaming (`text/event-stream`) responses are documented in the
  spec for long-running tools. spall's v1 tools are all
  request/response; the server returns JSON regardless of which
  format the client `Accept`s.

[mcp-http]: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports

```bash
# Localhost by default (MCP spec recommendation; mitigates DNS rebinding).
spall mcp petstore --spall-transport http --spall-port 8765

# Bind on all interfaces — combine with a reverse proxy that adds auth + TLS.
spall mcp petstore --spall-transport http --spall-port 8765 --spall-bind 0.0.0.0

# Pass --spall-port 0 to let the kernel pick a free port. The bound
# port is logged to stderr:
spall mcp petstore --spall-transport http --spall-port 0
# [spall-mcp] listening on http://127.0.0.1:54321/
```

### Origin allowlist (DNS rebinding mitigation)

The spec requires the server to validate the `Origin` header to block
DNS-rebinding attacks. spall's policy:

- **Allowlist set** (`--spall-allowed-origin <origin>`, repeatable):
  only listed origins succeed; all others get `403 Forbidden`. The
  CORS preflight layer is configured against the same list so
  browsers see a coherent preflight rejection rather than a generic
  CORS error.

  ```bash
  spall mcp petstore --spall-transport http \
      --spall-allowed-origin https://app.example.com \
      --spall-allowed-origin https://staging.example.com
  ```

- **Allowlist empty** (default): non-browser callers (no Origin
  header — curl, the MCP test client) and localhost browsers
  (`http://localhost[:N]`, `http://127.0.0.1[:N]`, `http://[::1][:N]`,
  same with `https`) succeed. Browsers with a remote Origin get
  `403`. This closes the DNS-rebinding hole where an attacker-
  controlled DNS record at `localhost.example.com → 127.0.0.1` could
  otherwise drive a victim's browser into the local server.

### Request body size

Capped at 16 MiB. Larger requests get HTTP `413 Payload Too Large`.
OpenAPI specs with very large multipart payloads should sit behind a
reverse proxy that handles streaming uploads, or run as stdio.

### TLS, auth on the HTTP endpoint

Both are deliberately **not** in scope for the spall server itself. The
expected deployment is a reverse proxy (Caddy / Nginx / Cloudflare /
fly proxy / etc.) that terminates TLS and adds auth, with spall
listening on a private port behind it. This matches the
`claude-desktop` / `chatgpt-apps` deployment pattern and keeps spall's
dep tree small.

## Sizing your server

MCP clients impose practical limits on how many tools they surface from
a single server. Claude Desktop in particular silently truncates near
**100 tools** (see [modelcontextprotocol/discussions/537][md-537]).
Stripe / AWS / GitHub-class specs blow well past this in one server.

[md-537]: https://github.com/orgs/modelcontextprotocol/discussions/537

Spall surfaces this in three ways:

1. **Startup warning.** When the filtered tool count exceeds 100, the
   server emits a stderr warning naming the most populated tags so you
   can pick a filter:

   ```text
   spall mcp: WARNING 247 tools exceeds the ~100-tool cap most MCP clients ...
   spall mcp: top tags by population: users=42, orgs=38, repos=37, gists=21, billing=18
   ```

2. **Discovery flag.** `--spall-list-tags` dumps every tag in the
   filtered registry as TSV without starting the server, so you can
   shape your `--spall-include` list ahead of time:

   ```text
   $ spall mcp github --spall-list-tags
   tag	count	sample-op-id
   actions	48	actions/list-workflow-runs
   billing	12	billing/get-shared-storage
   ...
   ```

3. **Auto-truncation.** `--spall-max-tools <N>` deterministically caps
   the registry. The ordering rule is:

   - Bucket each operation by its **first** tag in spec order
     (untagged operations land in `default`).
   - Sort buckets alphabetically.
   - Within each bucket, keep spec order.
   - Take the first `N`; ties on count are broken by spec order.

   The selected subset is stable across runs on the same spec — useful
   for predictable CI behavior. An operation that's truncated out has
   no way to come back without rerunning with a higher `N` or a
   different filter.

## Troubleshooting

### Claude Desktop only shows some of my tools

See [Sizing your server](#sizing-your-server). The startup warning is
your first signal; `--spall-list-tags` plus `--spall-include` or
`--spall-max-tools` are the levers.

### "Server disconnected" / corrupted JSON-RPC stream

Stdio MCP requires that **only JSON-RPC** is written to stdout. Spall's
server hot path uses `eprintln!` for diagnostics and never writes to
stdout outside of protocol replies. Sanity check:

```bash
echo '' | spall mcp <api>
```

The server should print its single-line stderr banner and exit on EOF
with zero stdout output.

### "Unknown argument"

Tool arguments are routed to the parameter location declared in the
spec. Pass each parameter by its **spec name** (not the `--query` /
`--header` flag used on the CLI). The reserved key `body` carries the
JSON request body when the operation declares one.
