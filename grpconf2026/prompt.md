<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Code review: `grpconf2026/teleprompt`

## What the script does

`teleprompt` is a self-contained Bash readline REPL that drives a live terminal
demo from a flat text file.  It loads all commands upfront, lets the
presenter step forward (ENTER), browse (Up/Down/PgUp/PgDn), edit in place,
duplicate or delete a step (F2/F3), and save back to the file (Ctrl-S).
It handles multi-line commands (backslash continuation), re-colorizes the
command after ENTER, tracks exit status, and recovers from Ctrl-Z / fg.


## Name

**Implemented:** renamed to `teleprompt` — the verb form reads naturally as a
command (`teleprompt script.sh`) and the audience gets it instantly.


## Speaker-notes feature

**Implemented:** lines starting with `##` are kept in `commands[]`/`display_commands[]`
(they survive Ctrl-S unchanged) but are never executed.  Navigation (Up/Down/PgUp/PgDn)
skips over them in the direction of travel.  On ENTER at a note, the script advances
to the next non-note entry.  Startup fast-forwards past any leading notes.


## Findings

### 1. `TMPDIR` shadows the system variable

```bash
TMPDIR=$(mktemp -d)
```

`TMPDIR` is an XDG-inherited convention: many tools (`mktemp` itself,
`python`, `cargo`, …) honour it as the temp-file root.  Clobbering it with
a subdirectory means any child process that calls `mktemp` will nest inside
the demo's own temp tree — and the `trap … rm -rf "$TMPDIR"` at exit will
silently delete those files too.  Use a private name: `_DEMO_TMPDIR` or
`demo_tmpdir`.

### 2. `calculate_lines` is inaccurate for multi-line commands

The function has two branches: if the string contains a literal newline it
counts newlines; otherwise it estimates wrapping.  But for multi-line
commands the string always contains newlines (the loader preserves them in
`display_commands`), so the wrap branch is never reached for them.  The
newline-count branch counts continuation lines correctly but ignores the
width of individual continuation lines — a very long continuation line that
wraps will be undercounted.  In practice the commands in `presentation.sh`
are short, so this is latent rather than live.

### 3. `grep -c` in `calculate_lines` forks a subprocess on every prompt

```bash
local count=$(echo -n "$str" | grep -c $'\n')
```

This is called from `_update_prompt` → `calculate_lines` on every ENTER.
Pure-bash newline counting (`${str//[!$'\n']/}` then `${#count}`) avoids
the fork and is cleaner.  Low priority for a demo script, worth fixing if
the script is reused in a latency-sensitive context.

### 4. Duplicate logic in `browse_up` / `browse_down` / `fast_up` / `fast_down`

All four functions open with the same two lines:

```bash
display_commands[$index]="$READLINE_LINE"
commands[$index]=$(echo "$READLINE_LINE" | tr -d '\n' | sed 's/\\  */ /g')
```

And all close with the same three:

```bash
READLINE_LINE="${display_commands[$index]}"
READLINE_POINT=${#READLINE_LINE}
_redraw_prompt
```

Extract a `_save_current` helper and a `_load_current` helper (or a single
`_navigate_to N` function).  `fast_up`/`fast_down` differ only in the delta
(`-= 10` / `+= 10`) and the clamp — they can share one body.

### 5. `sed 's/\\  */ /g'` is subtly wrong

The intent is "strip trailing backslash and collapse to a space", but the
regex `\\  *` matches a literal backslash followed by one-or-more spaces —
it does not strip a trailing backslash that has no following space (which is
the normal case: the backslash is the last character on the line before the
newline).  The loader strips trailing backslashes correctly (`${line%\\}`);
the execution-conversion path should use `tr`+`sed` in the same style, or
simply `sed 's/\\\n/ /g'` on the display string.

### 6. Comment-splitting (`car`/`cdr`) is applied to the display string, not the execution string

```bash
car=${display_cmd%%#*}
cdr=${display_cmd#"$car"}
```

This splits on the first `#` in the entire display string, which breaks on
commands containing a `#` inside a quoted string or heredoc.  For the
current `presentation.sh` content this is harmless, but it is a known
fragility worth a comment.  The variable names `car`/`cdr` (Lisp head/tail)
are obscure for a shell script — `cmd_body`/`cmd_comment` would read better.

### 7. `eval "$usercmd_exec"` vs `bash -c`

`eval` runs in the current shell, which is generally correct here (so that
`export` and `cd` affect subsequent commands).  The caveat is that a
malformed command can corrupt the shell state in ways that `bash -c` would
not.  For a demo script driving trusted content this is acceptable; worth a
comment so future editors know the choice was deliberate.

### 8. Cursor-column query races against slow commands

```bash
IFS='[;' read -s -d R -p $'\033[6n' _ _row col </dev/tty
```

This DSR/CPR round-trip assumes the terminal processes the escape and
replies before any subsequent output arrives.  For commands that produce a
lot of output quickly (e.g. `reproto`) the CPR reply can arrive interleaved
with the command's stdout, causing `col` to be misread.  The symptom is a
spurious blank line or a missing newline after the command.  A robust fix
reads the reply in a loop discarding non-CSI bytes, but that is
significantly more complex; a pragmatic alternative is to flush stdout
(`tcdrain`) before sending the escape.

### 9. `save_commands` overwrites with display versions only

```bash
printf "%s\n" "${display_commands[@]}" > "$CONFIG_FILE"
```

If the user edits a multi-line block down to a single line and then saves,
the file is updated correctly.  But if the user navigated away without
pressing ENTER, `display_commands[$index]` still holds the original — the
unsaved edit in `$READLINE_LINE` is lost.  `save_commands` should sync
`display_commands[$index]="$READLINE_LINE"` before writing, just as
`browse_up`/`browse_down` do.

### 10. `TMPDIR` cleanup trap fires before readline cleanup

The `trap … rm -rf "$TMPDIR"` is set before readline bindings are
established.  If the EXIT trap fires mid-readline (e.g. on `kill`), the
temp dir is gone but readline state may be dirty.  `stty sane` should be
part of the EXIT trap alongside the `rm -rf`.


## Priority

| # | Severity | Notes |
|---|----------|-------|
| 1 | Medium — correctness | `TMPDIR` shadowing — **fixed** |
| 9 | Medium — data loss | `save_commands` drops unsaved edit — **fixed** |
| 5 | Low — latent bug | `sed` regex misses trailing backslash — **fixed** (`sed 's/\\\n/ /g'`) |
| 4 | Low — maintainability | duplicated navigation boilerplate — **fixed** (`_save_current`/`_load_at`) |
| 10 | Low — robustness | EXIT trap missing `stty sane` — **fixed** |
| 2 | Low — latent | `calculate_lines` undercounts wrapped continuations |
| 6 | Low — fragility | `#`-split on quoted strings; opaque names |
| 3 | Negligible | subprocess in `calculate_lines` |
| 7 | Info | `eval` vs `bash -c` — deliberate, needs comment |
| 8 | Info — demo-specific | CPR race; acceptable in practice |
