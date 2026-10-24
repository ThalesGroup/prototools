#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

_reproto_completion() {
    local IFS=$'\n'
    local response
    local words=("${COMP_WORDS[@]}")
    local cword=$COMP_CWORD
    local word_count=${#COMP_WORDS[@]}
    # Options that consume a value argument (used for = stripping and for
    # detecting a missing empty slot before the current word).
    declare -A takes_value=(
        ["--desc-root"]=1    ["-I"]=1
        ["--pb-path"]=1
        ["--proto-out"]=1    ["-O"]=1
        ["--output-root"]=1
        ["--proto-variant"]=1
        ["--use-variant"]=1
        ["--seed"]=1         ["-s"]=1
        ["--prune"]=1        ["-p"]=1
        ["--go-root"]=1
        ["--graph"]=1
        ["--fdp-plugin"]=1
        ["--phase2-plugin"]=1
        ["--dump-resolved-features"]=1
    )

    for ((i=1; i<$word_count; i++)); do
        if [[ ${COMP_WORDS[i]} == "=" && ${takes_value[${COMP_WORDS[i-1]}]} ]]; then
            unset "words[$i]"
            if (( i < $COMP_CWORD )); then
                ((cword--))
            fi
        fi
    done

    # Reconstruct the true current token from the raw line/point.
    # If '/' were in COMP_WORDBREAKS, bash would split "docs/sp" into
    # ["docs","/","sp"] — so words[$cword]="sp" but the full incomplete
    # token is "docs/sp".  We reconstruct it from COMP_LINE regardless.
    local raw_cur="${COMP_LINE:0:$COMP_POINT}"
    raw_cur="${raw_cur##* }"   # everything after the last space

    # When the cursor sits in whitespace, raw_cur="" but words[$cword] is the
    # next token — insert an empty sentinel so Click sees an empty incomplete.
    # Don't insert sentinel for path-split case (raw_cur ends with words[$cword]).
    if [[ -z "$raw_cur" && -n "${words[$cword]}" ]]; then
        words=("${words[@]:0:$cword}" "" "${words[@]:$cword}")
    fi

    # Collapse the path-split fragments back into a single token for Click.
    # Replace words from token_start..cword with raw_cur as one token.
    local token_start=$cword
    local acc=""
    for (( i=cword; i>=1; i-- )); do
        acc="${words[$i]}${acc}"
        if [[ "$acc" == "$raw_cur" ]]; then
            token_start=$i
            break
        fi
    done

    local new_words=()
    for (( i=0; i<token_start; i++ )); do
        new_words+=("${words[$i]}")
    done
    local new_cword=${#new_words[@]}
    new_words+=("$raw_cur")

    # Build COMP_WORDS string — empty tokens encoded as "" for Click.
    comp_words_str=""
    for (( i=0; i<${#new_words[@]}; i++ )); do
        [[ $i -gt 0 ]] && comp_words_str+=" "
        if [[ -z "${new_words[$i]}" ]]; then
            comp_words_str+='""'
        else
            comp_words_str+="${new_words[$i]}"
        fi
    done
    cword=$new_cword

    response="$(env COMP_WORDS="$comp_words_str" COMP_CWORD=$cword _REPROTO_COMPLETE=bash_complete $1)"

    local has_arg=false
    local has_arg_I=false
    local arg_values=()
    local arg_I_values=()
    for completion in $response; do
        IFS=',' read type value <<< "$completion"
        if [[ $type == 'dir' ]]; then
            COMPREPLY=(); compopt -o dirnames
        elif [[ $type == 'file' ]]; then
            COMPREPLY=(); compopt -o default
        elif [[ $type == 'plain' ]]; then
            COMPREPLY+=($value)
        elif [[ $type == 'arg' ]]; then
            arg_values+=("$value")
            has_arg=true
        elif [[ $type == 'arg_I' ]]; then
            arg_I_values+=("$value")
            has_arg_I=true
        fi
    done

    if $has_arg; then
        # CWD-relative paths: use -o filenames so readline strips the common
        # path prefix in the menu (showing last leg only) and stats each entry
        # to append '/' to directories.  Strip our own trailing '/' first so
        # readline's stat-based re-addition doesn't double it.
        local has_dir=false
        for value in "${arg_values[@]}"; do
            if [[ $value == */ ]]; then
                COMPREPLY+=("${value%/}")
                has_dir=true
            else
                COMPREPLY+=("$value")
            fi
        done
        compopt -o filenames
        $has_dir && compopt -o nospace
    fi

    if $has_arg_I; then
        # -I-relative paths: candidates may be outside CWD so readline's stat
        # (triggered by -o filenames) would be unreliable.  Put the full paths
        # directly in COMPREPLY without -o filenames; the menu shows full paths
        # rather than last legs, but insertion and trailing slashes are correct.
        COMPREPLY+=("${arg_I_values[@]}")
        local has_dir_I=false
        for value in "${arg_I_values[@]}"; do
            [[ $value == */ ]] && has_dir_I=true
        done
        $has_dir_I && compopt -o nospace
    fi

    return 0
}

_reproto_completion_setup() {
    complete -o nosort -F _reproto_completion reproto
}

_reproto_completion_setup
