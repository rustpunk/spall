//! Shell completion script generation.

use crate::SpallCliError;

/// Generate a completion script for the requested shell.
pub fn generate_completions(shell: &str) -> Result<String, SpallCliError> {
    match shell {
        "bash" => Ok(generate_bash()),
        "zsh" => Ok(generate_zsh()),
        "fish" => Ok(generate_fish()),
        _ => Err(SpallCliError::Usage(format!(
            "Unsupported shell: {}. Supported: bash, zsh, fish",
            shell
        ))),
    }
}

fn generate_bash() -> String {
    r#"#!/bin/bash
# spall bash completion

_spall() {
    local cur prev words cword
    _init_completion || return

    # Count how many non-option words we have before current
    local non_opt=()
    for w in "${words[@]:1:$((cword-1))}"; do
        [[ "$w" != -* ]] && non_opt+=("$w")
    done

    local count=${#non_opt[@]}

    if [[ $count -eq 0 ]]; then
        # Top-level: suggest API names and built-in commands
        COMPREPLY=( $(compgen -W "api completions history __complete $(spall api list 2>/dev/null | grep -v '^Registered' | awk '{print $1}')" -- "$cur") )
    elif [[ $count -eq 1 ]]; then
        local api="${non_opt[0]}"
        # Complete operations from __complete helper
        local ops
        ops=$(spall __complete "$api" "$cur" 2>/dev/null)
        if [[ -n "$ops" ]]; then
            COMPREPLY=( $(compgen -W "$ops" -- "$cur") )
        fi
    fi
}

complete -F _spall spall
"#
    .to_string()
}

fn generate_zsh() -> String {
    r#"#compdef spall
# spall zsh completion

_spall() {
    local curcontext="$curcontext" state line
    typeset -A opt_args

    _arguments -C \
        '1: :->_spall_apis' \
        '*:: :->_spall_ops'

    case "$state" in
        spall_apis)
            local apis
            apis=$(spall api list 2>/dev/null | grep -v '^Registered' | awk '{print $1}')
            _alternative 'apis:apis:('$apis')'
            ;;
        spall_ops)
            local api="$line[1]"
            local ops
            ops=$(spall __complete "$api" "" 2>/dev/null | tr '\n' ' ')
            _alternative 'ops:ops:('$ops')'
            ;;
    esac
}

compdef _spall spall
"#
    .to_string()
}

fn generate_fish() -> String {
    r#"# spall fish completion

function __spall_apis
    spall api list 2>/dev/null | grep -v '^Registered' | awk '{print $1}'
end

function __spall_ops
    set -l api (commandline -opc)[2]
    spall __complete "$api" "" 2>/dev/null
end

complete -c spall -n '__fish_is_first_token' -a 'api completions history __complete'
complete -c spall -n '__fish_is_first_token' -a '(__spall_apis)'
complete -c spall -n 'test (count (commandline -opc)) -eq 2' -a '(__spall_ops)'
"#
    .to_string()
}
