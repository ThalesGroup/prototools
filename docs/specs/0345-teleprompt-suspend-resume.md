<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0345 — teleprompt: correct Ctrl-Z / fg suspend-resume

Status: implemented
Implemented in: 2026-08-23
App: teleprompt (bin/teleprompt)
Refs: docs/specs/0344-teleprompt-built-in-header.md (introduced the
      persistent Python readline coproc whose interaction with job
      control this spec fixes)

## Background

Pressing Ctrl-Z while a demo command is running (e.g. `header … && blue
…`) stops teleprompt.  After `fg` the terminal is stuck — readline's
prompt never reappears, and the only way out is Ctrl-C.

The bug was introduced when the persistent Python readline coproc was
added (spec 0344 / commit 311da26).  The bash-only predecessor handled
suspend correctly because `read -e` (bash's built-in readline) owns the
tty and is naturally interrupted and re-prepped by the kernel's job
control.  With the coproc the tty is managed by Python's C readline
library; bash no longer drives it directly.

## Two distinct suspend sites

Ctrl-Z can fire at two points in teleprompt's main loop, and each has a
different failure mode.

### Site A — during `eval` (command execution)

Python is idle in `_read_field()` blocking on the request pipe (fd 0).
Bash is running the child command via `eval`.

On Ctrl-Z the kernel sends SIGTSTP to the foreground process group.
The child command stops; bash detects the stopped child and stops itself
via job control.  Python's `_read_field()` read() is also stopped.
`_sigtstp_c` never fires because Python is not inside readline.

On `fg` SIGCONT is delivered to the process group.  Everything restarts.
Bash's CONT trap fires (`resume_after_fg=1`).  Bash continues after
`eval`, loops back to `_tp_send` + `_tp_recv`.  Python's read() restarts,
reads the new prompt, calls readline.

**Observed failure:** readline's prompt never appears.  Root cause: the
parent shell's job-control bookkeeping resets the tty to its saved state
when it hands the foreground back, which undoes `stty -ixon -echoctl`
that teleprompt set at startup.  Python's readline then preps the
terminal for raw input but cannot fix `-ixon` because it does not know
it was wanted.  The result is a terminal that looks correct to readline
but behaves wrongly (Ctrl-S freezes output, `^C` echoes, etc.), and in
practice the prompt either does not appear or appears but is unresponsive.

**Required fix (S1):** restore `stty -ixon -echoctl` in bash's CONT trap,
before Python calls readline again.

### Site B — during `_tp_recv` (readline prompt)

Python is inside `readline()`.  Bash is blocked in `_tp_recv` reading
from the coproc reply pipe (fd 3).

On Ctrl-Z SIGTSTP is delivered to the process group.  Python's
`_sigtstp_c` fires: it calls `rl_deprep_terminal()`, resets SIGTSTP to
`SIG_DFL`, and calls `kill(0, SIGTSTP)` — stopping the whole group
including itself.  Bash's pipe `read()` is also stopped.

On `fg` SIGCONT is delivered.  Python resumes inside `_sigtstp_c`:
re-arms SIGTSTP, calls `rl_prep_terminal(1)`, `rl_redisplay()`, and
returns into readline's inner loop — which then blocks on read() from
the tty waiting for the user to press Enter.

Bash's pipe `read()` restarts (SA_RESTART applies to pipes).  Bash
resumes in `_tp_recv` — correctly waiting for Python's reply.

**Observed failure (pre-existing, minor):** bash's CONT trap
(`resume_after_fg=1`) does not fire because the pipe `read()` restarts
transparently without EINTR; bash never sees SIGCONT as an asynchronous
event.  So when the user presses Enter and readline returns, bash
executes the command instead of skipping it.

**Required fix (S2):** detect the resume-from-readline case on the bash
side without relying on the CONT trap.  The mechanism already in place
(`resume_after_fg` flag + CONT trap) is correct in *intent* but fails in
site B because SA_RESTART suppresses the trap.  A reliable alternative is
to have Python signal the resume explicitly — see S2 below.

Additionally, after `rl_redisplay()`, Python's `_sigtstp_c` currently
prints `[Resumed demo: hit ENTER to continue]` to /dev/tty.  This message
is unnecessary if the fix makes the resume seamless and should be removed.

## Goals

- **G1.** After Ctrl-Z during command execution (site A) and subsequent
  `fg`, readline reappears normally — the prompt is drawn, keystrokes
  are accepted, and Enter runs the next command.
- **G2.** After Ctrl-Z during the readline prompt (site B) and subsequent
  `fg`, readline reappears without requiring a manual Enter press.
- **G3.** In neither case is the command that was active when Ctrl-Z was
  pressed executed on resume.

## Non-goals

- **N1.** Nested suspend (Ctrl-Z while already stopped) is not handled.
- **N2.** SIGTSTP sent by means other than the terminal (e.g. `kill -TSTP`)
  is not handled — only keyboard Ctrl-Z is in scope.
- **N3.** The tty geometry (`COLUMNS`) is not re-probed on resume; the
  value from startup is reused.

## Specification

### S1 — Restore tty settings in the CONT trap (fixes site A)

Change the bash CONT trap from:

```bash
trap 'resume_after_fg=1' CONT
```

to:

```bash
trap 'stty -ixon -echoctl 2>/dev/null; resume_after_fg=1' CONT
```

This restores the two stty flags that teleprompt requires and that the
parent shell's job-control reset, before Python's readline is next
invoked.

### S2 — Seamless resume from readline (fixes site B)

Replace the "hit ENTER to continue" pattern with one that resumes without
a keypress:

**Python side (`_sigtstp_c`):**

After `rl_prep_terminal(1)` on resume:

1. Set `_rl_done = 1` to make readline return the current buffer.
2. Use `rl_signal_event_hook` to trigger the return without waiting for
   input: set `rl_signal_event_hook` to a one-shot C callback that sets
   `rl_done = 1` and clears itself, then send SIGUSR1 to `getpid()` with
   a no-op C handler (installed via `libc.signal`, which does not set
   SA_RESTART).  The SIGUSR1 interrupts readline's blocked `read()` with
   EINTR; readline calls `rl_signal_event_hook`; the hook sets `rl_done`;
   readline returns the current buffer.
3. Set a module-level flag `_resumed = True` and set `_nav = "resume"`.
4. Remove the `rl_redisplay()` call and the "Resumed demo" message.

**Python main loop:**

After `readline()` returns, if `_resumed` is True: send a `nav:resume:0`
reply to bash (payload = current buffer bytes, so bash can restore the
slot) and clear `_resumed`.  Do not skip the send — bash needs the reply
to unblock from `_tp_recv` before it can issue a fresh `_tp_send`.

**Bash side:**

Add a `resume` case to the nav dispatch:

```bash
resume)
    display_commands[$index]="$_nav_buf"
    join_continuations "$_nav_buf"
    commands[$index]="$_joined"
    _erase_and_redraw "$_old_display" "$_rl_cursor_row"
    ;;
```

This restores the slot from the returned buffer (preserving any edits the
user had made before Ctrl-Z) and redraws the prompt cleanly.

The `resume_after_fg` flag is still set by the CONT trap for site A
(where it works); it remains as the guard against executing the command
in that case.  For site B, the `resume` nav tag takes over — the command
is never delivered as a `line` tag.

### S3 — add `blue` command

Add `blue TEXT` as a built-in exported function alongside `header`:

```bash
blue() { printf '\e[1;94m%s\e[0m\n' "$*"; }
export -f blue
```

This is independent of the suspend fix but was deferred pending this
spec.

## Alternatives considered

**Send the resume reply before stopping (pre-stop send).** Have
`_sigtstp_c` call `_send("nav:resume:0", b"")` before `kill(0,
SIGTSTP)`, then on `fg` set `_resumed=True` and `rl_done=1` to suppress
the second reply.  Built and tested; failed.  The sequence bash→`_tp_send`
(pipe-buffered, succeeds immediately) → `_tp_recv` (blocks on fd 3) races
with the process group being stopped.  If bash sends `_tp_send` before
Python is resumed, bash then blocks in `_tp_recv` with Python still
stopped — identical deadlock to the original bug.  The timing window is
narrow but not zero.

**TIOCSTI to inject a character and wake readline.**  Would allow setting
`rl_done=1` and injecting a character without `rl_signal_event_hook`.
Not available on Linux 6.2+ without `CAP_SYS_ADMIN` (confirmed on the
development kernel 6.12.63 — `EINVAL`).  Ruled out.

**`rl_pending_input` to wake readline.**  `rl_pending_input` is checked
by readline *before* calling `read()`, not after EINTR.  Since readline
restarts its `read()` via SA_RESTART after SIGCONT, `rl_pending_input`
is never checked until the next keystroke arrives.  Ruled out.

**No-op CONT trap (`trap '' CONT`).** Prevents SIGCONT from interrupting
any bash `read()`, avoiding the EINTR-on-pipe issue.  But also prevents
`resume_after_fg` from being set, breaking site A's execution-skip
guard.  Ruled out.

## Test plan

1. `resume_during_exec` — run a slow command (e.g. `sleep 2`), press
   Ctrl-Z mid-sleep, `fg`: the readline prompt reappears, the command
   counter has not advanced.
2. `resume_during_readline` — press Ctrl-Z at the idle readline prompt,
   `fg`: readline reappears immediately without pressing Enter; the
   command counter has not advanced.
3. `resume_preserves_edits` — type a partial edit at the readline prompt,
   press Ctrl-Z, `fg`: the edited buffer is restored in the prompt.
4. `no_exec_on_resume` — in both cases above, confirm the command
   displayed at the time of Ctrl-Z is not executed on `fg`.
5. `stty_restored` — after `fg`, confirm `stty -a` shows `-ixon` and
   `-echoctl`; confirm Ctrl-S does not freeze output.

## Measured outcome

Implemented in `bin/teleprompt`.

S1: CONT trap changed to `stty -ixon -echoctl 2>/dev/null; resume_after_fg=1`.

S2: `_sigtstp_c` now:
- calls `rl_prep_terminal(1)` on resume (before the SIGUSR1 kick)
- arms `rl_signal_event_hook` with a one-shot callback that sets `rl_done=1`
  and NULLs itself
- installs a no-op SIGUSR1 handler via `libc.signal` (no SA_RESTART) and
  sends SIGUSR1 to itself to interrupt readline's `read()` with EINTR
- sets `_resumed = True`

Python main loop sends `nav:resume:0` when `_resumed` is set; bash `resume`
nav case calls `_erase_and_redraw` to redisplay the prompt cleanly.

S3: `blue()` function added and exported alongside `header`.
