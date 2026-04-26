# Installation

Spall ships as a single static binary with no runtime dependencies.

## Download

Pre-built binaries are available for Linux, macOS, and Windows on the releases page.

```bash
# Linux / macOS
curl -sL https://github.com/rustpunk/spall/releases/latest/download/spall-$(uname -s)-$(uname -m) -o spall
chmod +x spall
sudo mv spall /usr/local/bin/

# Verify
spall --version
```

## Build from Source

Requires Rust 1.75+.

```bash
git clone https://github.com/rustpunk/spall.git
cd spall
cargo build --release --workspace

# Binary will be at:
# ./target/release/spall
```

## Shell Completion Setup

Generate completion scripts for your shell:

```bash
# Bash
spall completions bash > /etc/bash_completion.d/spall

# Zsh
spall completions zsh > "${fpath[1]}/_spall"

# Fish
spall completions fish > ~/.config/fish/completions/spall.fish
```

## Next Steps

- [Your First Request](first-request.md) — register an API and make a call.
