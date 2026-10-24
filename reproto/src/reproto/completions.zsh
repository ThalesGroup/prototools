#compdef %(prog_name)s

# Printed by `_REPROTO_COMPLETE=zsh_source reproto` (spec 0422 S3): Click's
# zsh script, which also adds reproto's own candidate types. `arg` is a path
# relative to the current directory, listed by its last component; `arg_I` a
# path relative to an -I root, possibly outside it, listed whole. Either,
# ending in '/', takes no suffix, so Tab descends into the directory.
#
# Derived from Click's (click/shell_completion.py, Copyright 2014 Pallets,
# BSD-3-Clause). A Click source template: printf-style placeholders, so a
# literal percent sign must be written twice.

%(complete_func)s() {
    local -a completions
    local -a completions_with_descriptions
    local -a response
    local -a arg_files arg_files_disp arg_dirs arg_dirs_disp
    local -a arg_I_files arg_I_dirs
    (( ! $+commands[%(prog_name)s] )) && return 1

    response=("${(@f)$(env COMP_WORDS="${words[*]}" COMP_CWORD=$((CURRENT-1)) \
%(complete_var)s=zsh_complete %(prog_name)s)}")

    # Click completes the value of --option=value alone, and compadd -U
    # replaces the whole word: give the option back with -p. _path_files
    # matches instead, so it takes the option as an ignored prefix.
    local opt=''
    [[ $PREFIX == -*=* ]] && opt="${PREFIX%%%%=*}="

    for type key descr in ${response}; do
        if [[ "$type" == "plain" ]]; then
            if [[ "$descr" == "_" ]]; then
                completions+=("$key")
            else
                completions_with_descriptions+=("$key":"$descr")
            fi
        elif [[ "$type" == "dir" ]]; then
            [[ -n $opt ]] && compset -P '*='
            _path_files -/
        elif [[ "$type" == "file" ]]; then
            [[ -n $opt ]] && compset -P '*='
            _path_files -f
        elif [[ "$type" == "arg" ]]; then
            if [[ "$key" == */ ]]; then
                arg_dirs+=("$key")
                arg_dirs_disp+=("${${key%%/}:t}/")
            else
                arg_files+=("$key")
                arg_files_disp+=("${key:t}")
            fi
        elif [[ "$type" == "arg_I" ]]; then
            if [[ "$key" == */ ]]; then
                arg_I_dirs+=("$key")
            else
                arg_I_files+=("$key")
            fi
        fi
    done

    if [ -n "$completions_with_descriptions" ]; then
        _describe -V unsorted completions_with_descriptions -U -p "$opt"
    fi

    if [ -n "$completions" ]; then
        compadd -U -p "$opt" -V unsorted -a completions
    fi

    (( $#arg_dirs )) && compadd -U -p "$opt" -S '' -d arg_dirs_disp -a arg_dirs
    (( $#arg_files )) && compadd -U -p "$opt" -d arg_files_disp -a arg_files
    (( $#arg_I_dirs )) && compadd -U -p "$opt" -S '' -a arg_I_dirs
    (( $#arg_I_files )) && compadd -U -p "$opt" -a arg_I_files
    return 0
}

if [[ $zsh_eval_context[-1] == loadautofunc ]]; then
    # autoload from fpath, call function directly
    %(complete_func)s "$@"
else
    # eval/source/. command, register function for later
    compdef %(complete_func)s %(prog_name)s
fi
