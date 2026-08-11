<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0275 — the command line's caret goes where you click

Status: implemented
Implemented in: 2026-08-11
App: protolens
Refs: docs/specs/0147-protolens-status-message-command-line-split.md (the
        one global command/message row), docs/specs/0127-protolens-pan-all-panes.md
        (that row pans, and auto-follows the caret),
        docs/specs/0190-the-activity-dot-reports-the-highest-live-tier.md
        (column 0 of the global row is the dot, not the text),
        docs/specs/0194-the-cursor-is-a-caret.md (the main pane's own
        click-to-caret inversion)

## Background

Every other text the reader can put a caret in answers a click. The main
pane has done so since spec 0194 S7: `set_caret_from_click` inverts the
column-to-screen mapping and puts the caret under the pointer. The
command line — `:` commands and `/`/`?` patterns — does not. A click
anywhere on it is swallowed: `handle_mouse`'s `Down(Left)` chain tests
`main_interactive`, `over_main` and `over_side`, and `over_cmd` is
computed only for the pan branch above it, so the click falls off the
end of the chain and nothing happens.

The gap shows up on exactly the buffers where it costs most. A `:export`
line with a long path, or a regex being tuned after spec 0273, is edited
by walking the caret there with `Left`/`Alt-b` — while the character to
fix is on screen, under the pointer, in a row that is already
hit-tested for the wheel.

## Goals

- **G1.** A left click on the command row, while a command or search
  line is open, puts the caret on the clicked character.
- **G2.** It is correct when the row is panned — the case the reader
  called out, and the only one whose arithmetic is not the identity.

## Non-goals

- **N1.** No selection, and so no drag and no double-click. The command
  line has no selection model at all (spec 0242's span is the main
  pane's), and inventing one to give a drag something to do is a much
  larger change than the one asked for. A drag over the row therefore
  does nothing, as it does today.

- **N2.** A click on the row while it shows a *status message* does not
  place a caret. There is no caret to place: `command_buffer` is `None`,
  the row is output rather than a field, and spec 0147 gives the two the
  same row precisely because only one of them is live at a time.

- **N3.** The click does not move focus, because there is nowhere to
  move it from. While `command_buffer.is_some()`, `handle_key` routes
  every key to `handle_command_key` ahead of any focus-specific
  dispatch — the command line is modal, not focused.

## Specification

- **S1.** A `Down(Left)` inside `cmd_area` with `command_buffer.is_some()`
  sets `command_cursor`. It is the last arm of `handle_mouse`'s existing
  `Down(Left)` chain; the three areas are disjoint, so no arm above it
  changes meaning.

- **S2.** The inversion is `render_command_row`'s mapping read backwards,
  and must stay that way if either side moves. That mapping draws
  character `pos` of `cmd_text` at `cmd_area.x + (pos -
  command_pan_offset)`, where `cmd_text` is the prefix character
  (`:`, `/`, `?`) followed by the buffer. So a click at column `col`
  names `pos = (col - cmd_area.x) + command_pan_offset`, and

      command_cursor = min(pos - 1, buffer length in chars)

  The `- 1` is the prefix, which is part of the drawn text and not part
  of the buffer. The caret lands *before* the clicked character, as a
  caret between characters must.

  `cmd_area` already excludes spec 0190's activity dot (it is
  `global_row[1]`), so the dot is outside the hit test and needs no term
  of its own.

- **S3.** The pan is carried by that one `+ command_pan_offset` and
  nothing else. Both the pan and the caret are counted in *characters*
  of `cmd_text` — `pan_spans` skips characters, `set_cursor_position`
  advances one column per character — so the inversion is exact rather
  than approximate, and no width table is consulted.

- **S4.** A click past the end of the text clamps to the end of the
  buffer, and a click on the prefix character clamps to its start. Both
  fall out of S2's `min` and `saturating_sub`; both are what a line
  editor does, and the alternative — refusing a click that named no
  character — is a click that visibly does nothing.

- **S5.** The pan does not move in response. `render_command_row`
  re-pans only when the caret is outside the visible window, and a
  caret derived from a visible column is inside it by construction. The
  one case that could move it is S4's clamp from past the right edge,
  which moves the caret *left*, further inside.

## Alternatives considered

**Store the caret's screen column at render time and compare against
it.** `render_command_row` already computes the caret's `x`; caching a
column-to-character table beside it would make the click a lookup rather
than an inversion. Rejected: the mapping is one addition, and a cached
table is a second copy of it that goes stale whenever a frame is skipped
— the click handler runs on an event, not on a frame.

**Handle the click in `render`'s coordinate space by re-deriving
`cmd_area` from `terminal_size`.** Rejected for the reason the field
exists: `cmd_area` is `None` exactly when the row is empty, and that
`None` is also the "nothing to click" answer. Re-deriving the rectangle
would lose it.

## Test plan

1. `a_click_on_the_command_line_moves_the_caret_there` — with a short
   `:` buffer and no pan, a click on the n-th character leaves
   `command_cursor` at `n`, and a click on the prefix leaves it at 0.
2. `a_click_on_a_panned_command_line_accounts_for_the_pan` — with a
   buffer long enough that auto-follow has panned it, a click on the
   leftmost visible column names the character the reader can see there,
   not the character with that index.
3. `a_click_past_the_end_of_the_command_line_goes_to_the_end` — the
   caret clamps to the buffer's length rather than past it.
4. `a_click_on_a_status_message_places_no_caret` — N2: with
   `command_buffer` `None` the click leaves the command state alone.

## Measured outcome

Nothing to measure — the whole of it is one added arm in
`handle_mouse`'s `Down(Left)` chain and one inversion,
`App::command_click`, on an event the terminal was already reporting.

The pan turned out to be the easy half rather than the touchy one the
request expected. It is a single added term because
`render_command_row`, `pan_spans` and `set_cursor_position` all already
count the same unit — characters of `cmd_text`, one per column — so the
mapping to invert was an addition and not a layout.
