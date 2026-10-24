# Printed by `_REPROTO_COMPLETE=fish_source reproto` (spec 0422 S4): Click's
# fish script, which also offers reproto's own candidate types, `arg` and
# `arg_I` (paths relative to the current directory or to an -I root), as
# they are. fish adds no space after a candidate ending in '/'.
#
# Derived from Click's (click/shell_completion.py, Copyright 2014 Pallets,
# BSD-3-Clause). A Click source template: printf-style placeholders, so a
# literal percent sign must be written twice.

function %(complete_func)s;
    set -l token (commandline -t);
    set -l response (env %(complete_var)s=fish_complete COMP_WORDS=(commandline -cp) \
COMP_CWORD=$token %(prog_name)s);

    # Click completes the value of --option=value alone, and fish keeps only
    # the candidates that start with the whole token: give the option back.
    set -l opt '';
    if string match -q -- '-*=*' $token;
        set opt (string replace -r -- '=.*' '=' $token);
    end;

    for completion in $response;
        set -l metadata (string split -m 1 "," $completion);

        if test $metadata[1] = "dir";
            __fish_complete_directories $metadata[2] | string replace -r -- '^' $opt;
        else if test $metadata[1] = "file";
            __fish_complete_path $metadata[2] | string replace -r -- '^' $opt;
        else if contains -- $metadata[1] plain arg arg_I;
            echo $opt$metadata[2];
        end;
    end;
end;

complete --no-files --command %(prog_name)s --arguments \
"(%(complete_func)s)";
