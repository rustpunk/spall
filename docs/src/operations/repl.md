# Interactive REPL

Spall includes an embedded REPL shell that keeps the registry resident in memory and lets you issue multiple commands without the ~50-200ms spec-load overhead on each call.

## Starting the REPL

```bash
spall repl
```

```text
spall REPL — type 'help' for commands, 'quit' or 'exit' to leave.
spall> 
```

## Commands Inside the REPL

Any input that is not a special command is parsed as a normal spall command (without the `spall` prefix):

```text
spall> api list
Registered APIs:
  petstore             https://petstore.swagger.io/v2/swagger.json

spall> petstore get-pet-by-id 1
{ ... }

spall> petstore get-pet-by-id 1 --spall-output csv
id,name,status
1,Rex,available
```

## Special Commands

| Command | Description |
|---------|-------------|
| `help` | Show available commands |
| `history` | List the last 20 recorded requests |
| `quit` / `exit` | Leave the REPL |

`Ctrl-C` interrupts the current prompt without exiting. `Ctrl-D` sends EOF and exits.

## REPL History

Command history is saved to `~/.cache/spall/repl_history` (or your platform equivalent) across sessions. Use the Up arrow to recall previous inputs.

## When to Use It

The REPL is useful for:

- Interactive API exploration — tab-like speed without tab-like setup.
- Poking at an API while debugging — no repeated spec fetches.
- Batch scripting small exploratory sequences without shell function wrappers.

## Request Chaining

The REPL supports pipe syntax to chain requests, passing the JSON response from one stage into the next via JMESPath expressions:

```text
spall> petstore get-pet-by-id 1 | update-pet --id id --status status
```

Each stage after the first is a chain expression: `operation --arg jmespath_expr ...`. The response from stage N-1 becomes the input for stage N.

If a pipe stage fails, the REPL prints a structured error showing the stage number, the failing expression, and a debug suggestion.

## Limitations

- The REPL uses `rustyline` for line editing. Complex multi-line JSON payloads should be written to a file and passed with `@file`.
- OAuth2 PKCE interactive flows are not yet implemented inside the REPL.

## Next Steps

- [CLI Reference](cli-reference.md)
- [Request History](history.md)
