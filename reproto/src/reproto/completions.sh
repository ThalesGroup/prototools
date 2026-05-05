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
        ["--pb-path"]=1      ["-I"]=1
        ["--output-root"]=1  ["-O"]=1
        ["--proto-variant"]=1
        ["--use-variant"]=1
        ["--seed"]=1         ["-s"]=1
        ["--prune"]=1        ["-p"]=1
        ["--go-root"]=1
        ["--graph"]=1
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

    # Reconstruct the true current token from the raw line/point, mirroring
    # the sed patch used for prototext.  When the cursor sits in whitespace
    # before COMP_WORDS[COMP_CWORD], bash reports that next token as the
    # current word even though the real incomplete value is empty.  Detect
    # this by comparing the raw current token against words[cword]: if they
    # differ, insert an empty sentinel and push cword forward.
    local raw_cur="${COMP_LINE:0:$COMP_POINT}"
    raw_cur="${raw_cur##* }"
    if [[ "${raw_cur}" != "${words[$cword]}" ]]; then
        words=("${words[@]:0:$cword}" "" "${words[@]:$cword}")
    fi

    # Build COMP_WORDS string manually so that empty tokens survive as ""
    # (which split_arg_string in Click parses as an empty string).
    # Plain "${words[*]}" would swallow empty slots, shifting COMP_CWORD.
    comp_words_str=""
    for (( i=0; i<${#words[@]}; i++ )); do
        [[ $i -gt 0 ]] && comp_words_str+=" "
        if [[ -z "${words[$i]// }" ]]; then
            comp_words_str+='""'
        else
            comp_words_str+="${words[$i]}"
        fi
    done

    #echo "-> cword=$cword comp_words_str=$comp_words_str" >> /tmp/aaa
    response="$(env COMP_WORDS="$comp_words_str" COMP_CWORD=$cword _REPROTO_COMPLETE=bash_complete $1)"
    #echo "<- $response" >> /tmp/aaa

    for completion in $response; do
        IFS=',' read type value <<< "$completion"
        if [[ $type == 'dir' ]]; then
            COMPREPLY=(); compopt -o dirnames
        elif [[ $type == 'file' ]]; then
            COMPREPLY=(); compopt -o default
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
