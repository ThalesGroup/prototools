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
    declare -A lookup=(
        ["--pb-path"]=1
        ["-I"]=1
        ["-s"]=1
    )

    for ((i=1; i<$word_count; i++)); do
        if [[ ${COMP_WORDS[i]} == "=" && ${lookup[${COMP_WORDS[i-1]}]} ]]; then
            unset "words[$i]"
            if (( i < $COMP_CWORD )); then
                ((cword--))
            fi
        fi
    done

    #echo "-> $((cword - removed_before)) ${words[@]}" >> /tmp/aaa
    response="$(env COMP_WORDS="${words[*]}" COMP_CWORD=$cword _REPROTO_COMPLETE=bash_complete $1)"
    #echo "<- ${response[@]}" >> /tmp/aaa

    for completion in $response; do
        IFS=',' read type value <<< "$completion"
        if [[ $type == 'dir' ]]; then
            COMPREPLY=(); compopt -o dirnames
        elif [[ $type == 'file' ]]; then
            COMPREPLY=(); compopt -o filenames
        elif [[ $type == 'plain' ]]; then
            COMPREPLY+=($value)
        elif [[ $type == 'arg' ]]; then
            COMPREPLY+=($value); compopt -o nospace -o filenames
        fi
    done
    #echo "<-- ${COMPREPLY[@]}" >> /tmp/aaa

    return 0
}

_reproto_completion_setup() {
    complete -o nosort -F _reproto_completion reproto
}

_reproto_completion_setup
