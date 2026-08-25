<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0358 — persistent selection

Status: implemented
Implemented in: 2026-08-25
App: protolens
Refs: docs/specs/0242-the-selection-is-a-span-of-characters.md (the
      selection model this spec amends);
      docs/specs/0131-….md (`Ctrl-c` is the only copy);
      docs/specs/0357-script-select-and-search-directives.md (the
      script `select:` directive that motivated this change)

## Background

Spec 0242 S3 clears the selection on every non-selection key. The rule
was borrowed from terminals: you move the caret, the selection vanishes,
you must re-select to copy. It is convenient interactively — you never
have to explicitly dismiss a selection — but it makes the script
`select:` directive (spec 0357) useless: the highlighted line disappears
on the first keypress the presenter makes.

Additionally, `Ctrl-c` copies the cursor's whole line when no selection
is engaged (spec 0131 §G1). That fallback is surprising — the user
presses `Ctrl-c` to dismiss something (terminal muscle memory) and
silently overwrites the clipboard with a line they did not ask for. With
a persistent selection the confusion is worse: the user may not realise
the selection is still active and believe they are copying the current
line.

## Goals

- **G1.** The selection persists across caret movement. Moving the
  caret does not clear the selection.
- **G2.** The selection is dismissed explicitly: by `Esc`, by a bare
  click, or by `script_reset` (next script step).
- **G3.** `Ctrl-c` copies the active selection when one is engaged.
  With no selection engaged it is a no-op (no clipboard write, no
  message).
- **G4.** The script `select:` directive sets a selection that survives
  the presenter navigating the document — it stays until the next step.

## Non-goals

- **N1.** No change to what is selected. The anchor+caret model and all
  the selection-extension keys are unchanged.
- **N2.** No change to when `script_reset` clears the selection. It
  already calls `clear_selection()` and that is correct.

## Specification

### S1 — Remove the blanket clear

Spec 0242 S3 reads: "Any main-pane key that is not one of the four
selection keys of S4 and not `Ctrl-c` clears `select_anchor` before it
runs."

That sentence is **deleted**. The call to `clear_selection()` that
implements it is removed from the key dispatch fallthrough.

The selection is now cleared only by the explicit gestures in S2.

### S2 — Explicit dismissal

The selection is cleared by:

- **`Esc`** in the main pane — already clears it (spec 0129 §G3 /
  existing `clear_selection()` call in `handle_esc`); no change needed.
- **A bare click** (`Down` without a following `Drag`, i.e. the
  `Up`-with-no-drag arm) — already calls `clear_selection()` via
  `handle_click`; no change needed.
- **`script_reset`** — already calls `clear_selection()`; no change
  needed.

No new dismissal gestures are added. The `Shift`-motion keys that extend
the selection are unchanged; they do not dismiss it.

### S3 — `Ctrl-c` with no selection is a no-op

`copy_current_selection_or_line` is renamed to
`copy_current_selection`. The fallback branch that copies the cursor's
whole line is **removed**. When `selection_span()` returns `None`,
`Ctrl-c` does nothing — no clipboard write, no message.

### S4 — Interaction with selection-extension keys

The selection-extension keys (`Shift-Left`/`H`, `Shift-Right`/`L`,
`Shift-Up`/`K`, `Shift-Down`/`J`) anchor if unanchored then move the
caret. This is unchanged. With persistent selection, pressing one of
these while a selection is already engaged extends it from its existing
anchor — the anchor is not reset.

This is already the correct behaviour: `anchor_selection` only sets the
anchor when `select_anchor` is `None`.

### S5 — Visual interaction between selection and caret

The selection highlight (blue background) and the cursor-row highlight
(dark grey row tint) may now coexist on different rows simultaneously.
This is already supported by the renderer — no change needed.

### S6 — `Ctrl-c` message

When the selection is engaged, `Ctrl-c` copies it and reports as before
(spec 0242 S13): `N character(s)` for a within-line selection, `N
line(s)` for a multi-line one.

When no selection is engaged, `Ctrl-c` does nothing and shows no
message.

### S7 — Shift-Ctrl and Shift-Alt selection chords

The readline/Emacs motion keys gain `Shift-Ctrl-` and `Shift-Alt-`
counterparts that extend the selection instead of merely moving the
caret. Each is `extend_selection(motion)` — anchor if unanchored, run
the motion, call `show_caret`.

The character-wise motions use `selection_caret_left` /
`selection_caret_right` (spec 0242 S5), not `caret_left` /
`caret_right`, because the latter fold or unfold at a voluntary end
(spec 0199 S5/S6), violating spec 0242 S6.

| chord | motion | note |
|---|---|---|
| `Shift-Ctrl-n` | `move_down` | same row motion as `J` / `Shift-Down` |
| `Shift-Ctrl-p` | `move_up` | same row motion as `K` / `Shift-Up` |
| `Shift-Ctrl-f` | `selection_caret_right` | one character right |
| `Shift-Ctrl-b` | `selection_caret_left` | one character left |
| `Shift-Ctrl-e` | `caret_to_line_end` | to end of line |
| `Shift-Ctrl-a` | `caret_to_line_start` | to start of line |
| `Shift-Alt-f` / `Shift-Alt-l` | `caret_word_right` | one word right |
| `Shift-Alt-b` / `Shift-Alt-h` | `caret_word_left` | one word left |

All eight are placed in the existing `Ctrl`/`Alt` character block in
`handle_main_key`, ahead of their unshifted counterparts, guarded by
`key.modifiers.contains(SHIFT)`. The `Shift-Alt-` arms use `shift_alt`
for consistency with the existing pan-key guards.

## Alternatives considered

### Clear on every motion, keep the line-copy fallback

The pre-existing behaviour. Works for interactive use but breaks the
script `select:` directive entirely.

### Keep the line-copy fallback, only drop the blanket clear

The line-copy fallback is a footgun with persistent selection: a user
who moved the caret away from the highlighted line and presses `Ctrl-c`
expecting to copy the current line would instead copy the stale
selection. Removing the fallback makes the command's meaning
unambiguous.

## Test plan

1. `a_plain_motion_no_longer_drops_the_selection` — after a `Shift-`
   selection, pressing `j` leaves `selection_span()` non-None. Amends
   spec 0242 test-plan item 2.
2. `esc_still_clears_the_selection` — `Esc` after a selection yields
   `None`.
3. `bare_click_still_clears_the_selection` — `Down`+`Up` with no drag
   yields `None`.
4. `ctrl_c_with_no_selection_is_a_noop` — `Ctrl-c` with no selection
   writes nothing to the clipboard and leaves `self.message` unchanged.
5. `ctrl_c_still_copies_an_active_selection` — `Ctrl-c` with an
   engaged selection copies and reports as before.
6. `script_select_survives_navigation` — after `select: true` applies,
   pressing `j` leaves the selection span intact.
7. `shift_ctrl_chords_extend_the_selection` — each of the eight new
   chords in S7 produces a non-None `selection_span` and does not clear
   the selection.
8. `reuse lint` passes.

### S8 — `select_caret`: the stored moving end

To make S1 possible — plain motions do not alter the span — the moving
end of the selection can no longer be derived from `cursor_column` at
render time. A new field `select_caret: Option<CursorPos>` is added to
`App`.

`selection_span` reads `select_caret` instead of
`(cursor_line(), cursor_column)`. Every gesture that extends the
selection writes `select_caret = Some(cursor_pos())` after performing
its motion:

- `extend_selection` — covers all `Shift`-motion and S7 chords.
- `drag_caret_to` — covers mouse drag and `Shift`-click.
- `select_sweep_hit` — covers the multi-row search hit (spec 0274 S12).
- `script_apply_select` — sets `select_caret` to `last` (the end of the
  line including any `; shadowed_scalar` suffix) before restoring
  `cursor_column` to where `node:`/`search:` left it, achieving a
  full-line span while leaving the caret position unchanged.

`clear_selection` clears `select_caret` alongside `select_anchor`.

## Measured outcome

1224 unit tests + 25 integration tests green; `reuse lint` clean.

The `select_caret` field (S8) was not in the original draft: it emerged
from the implementation of `script_apply_select` (spec 0357), which
needs a full-line selection span while leaving the visible caret at the
position `search:` established. The stored moving end makes that
representable without touching the render path.
