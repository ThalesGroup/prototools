<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pane: help and session chrome

*last verified: 2026-08-03*

## Executive summary

A handful of session-level surfaces don't belong to any one pane above:
the startup splash, the `F1` help overlay, the always-present global
command/message row, and suspending. What unifies them
is that each works identically regardless of which "real" pane
(main/override-select/manage) currently has focus — they're checked
centrally, ahead of any focus-specific key dispatch, rather than being
threaded individually into every pane's own key handler.

## Technical detail

### Centrally-checked, focus-independent keys

`Ctrl-Z` (suspend), `:` (open the command line) and `F1` (open help) are
all checked at the very top of the key-handling entry point, before
dispatch branches on which pane has focus. This ordering is deliberate,
not incidental: these concerns apply uniformly to the whole
session regardless of what the user happens to be doing inside a pane,
so implementing them as an early-exit check avoids needing the same
checks duplicated (and kept in sync) inside every pane's own key
handler. It is also what makes `:quit` reachable on a document with no
nodes at all, where the main pane's own dispatch returns immediately.

### Quitting is a command, not a keypress

`:quit` (or its unambiguous prefix `:q`) is the only way out of
protolens; no bare letter quits, and there is no confirmation flow to
cancel (spec 0236). The letter that used to quit was `q`, which is also
the letter every pane-like surface trains a user to press to close
*itself* — so the same reflex either closed a pane or ended the session
depending on focus. Freeing `q` entirely, and leaving `Esc` as the one
key that closes a pane, removes the ambiguity at the source rather than
guarding against it with a two-keypress confirmation. The cost is that
quitting now takes five keystrokes instead of two; the benefit is that
no keystroke can end the session by accident, which is what the
confirmation was there to buy in the first place.

### Suspend leaves the terminal exactly as a clean exit would

`Ctrl-Z` doesn't just raise `SIGTSTP` — it first runs the same terminal
teardown a normal exit runs (draining any mouse events still queued so
they don't leak into the shell as escape-sequence garbage on resume,
restoring cooked mode, leaving the alternate screen), and on resume
re-establishes raw mode/alternate screen/mouse capture and forces a full
redraw, since another program may have used the terminal while
protolens was stopped. This mirrors — and deliberately reuses — the same
cleanup path a panic hook uses, so there is exactly one "return the
terminal to a sane state" implementation serving three different exit
paths (normal exit, suspend, panic).

### Help is static text, not generated from key bindings

The `F1` help overlay's content is a hand-maintained block of text, not
derived from the key-dispatch match arms it documents. This is a
conscious trade-off: generating it from the dispatch table would
guarantee the two never drift apart, but would also produce documentation
ordered and phrased however the code happens to be organized internally,
rather than grouped the way a user actually thinks about the tool's
features (movement, folding, search, overrides, management). The
maintenance cost of updating both when a binding changes is accepted in
exchange for help text that reads as a guide rather than a dump of match
arms.

### Local statuslines vs. the global command/message row

There is no longer one shared "status bar" — each pane (main,
override-select, manage) carries its own single-row local statusline at
its own bottom edge, showing only that pane's own cursor/selection state:
positional path, resolved type, byte range, and line number for the main
pane; the target field's own path and a candidate-list ruler for the
override-select pane; the highlighted entry's origin path and a row
ruler for the manage pane. This mirrors vim's own split-window
statusline convention — each window's local statusline reflects that
window's own buffer state, never another window's or anything
session-wide — and focus is conveyed purely by that row's own accent
color, not by a separate border.

Session-wide concerns — an active `:`/`/`/`?` text-entry buffer, or a
passive `self.message` notice — instead share exactly one row, fixed at
the very bottom of the whole screen regardless of how many panes are
open: the global command/message row (see
[command-line.md](command-line.md)). This is the dividing line: a pane's
own position/selection info belongs to that pane's own statusline,
because it means nothing outside that pane; a command or a message
belongs to the session, because e.g. a `:save-overrides` confirmation
isn't about whichever pane happened to have focus when it completed.

Recomputing a local statusline is as cheap as the old shared status bar
was — one node's worth of lookups per pane per frame, not a
document-wide scan — and its content stays directly actionable for the
same reason: it's either useful for orienting the cursor within that
pane, or feeds some other cursor-relative action in that same pane (like
the main pane's byte range, which the default `:extract` filename
embeds).
