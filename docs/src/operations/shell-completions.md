# Shell Completions

Spall can generate completion scripts for bash, zsh, and fish. Because operations are loaded dynamically from specs, the completion scripts query spall itself at runtime to suggest API names and operation IDs.

## Bash

```bash
spall completions bash > /etc/bash_completion.d/spall
# or user-local:
spall completions bash > ~/.local/share/bash-completion/completions/spall
```

## Zsh

```bash
spall completions zsh > "${fpath[1]}/_spall"
```

## Fish

```bash
spall completions fish > ~/.config/fish/completions/spall.fish
```

## How It Works

The generated scripts are thin wrappers around spall's `__complete` hidden subcommand:

```bash
spall __complete <api> <partial-word>
```

This loads the spec (or its cached index if offline) and prints matching operation IDs and parameter names. Completion is fast even for large APIs because it uses the lightweight `SpecIndex` cache.

## Next Steps

- [CLI Reference](cli-reference.md)
