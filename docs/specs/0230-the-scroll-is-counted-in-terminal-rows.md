<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0230 — the scroll is counted in terminal rows

Status: implemented
Implemented in: 2026-08-02
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md
        (S8 — the 2× geometry that made a whole-line scroll too coarse,
        and the `w` toggle that has to hold a row still across it)

## Background

`scroll_offset` names the first visible *document line*. In wire mode a
document line is two terminal rows (spec 0225 S8), so the finest scroll
the main pane can express is two rows, and `w` cannot always put the
cursor back on the row it was drawn on. Two cases, both reported from
use:

- Turning the wire rows **on** from an odd offset. Holding the cursor
  needs the pane to start on a wire row — half a document line — so the
  offset was halved with integer division and the cursor slid up a row.
- Turning them **off** near the top. Holding the cursor needs twice as
  many document lines above it as were on screen, and near the top of
  the document they do not exist. `scroll_offset` stopped at 0 and the
  cursor slid up by the shortfall.

## Goals

- **G1.** The vertical scroll can name any terminal row, including one
  in the middle of a document line and one above the document's first
  line.
- **G2.** `w` holds the cursor's terminal row in every case.

## Non-goals

- **N1.** No sub-row scrolling. A terminal row is the finest unit a
  terminal has.
- **N2.** Nothing for the side panes. `override_scroll` and
  `manage_scroll` index rows that are one terminal row each, so the free
  `clamp_scroll_to_visible` they share stays exactly as it is; only the
  main pane stops calling it.
- **N3.** No scrollback above the document as a browsing position. The
  blank rows exist only because a toggle put them there, and the first
  downward scrolling that reaches them spends them (S4).

## Specification

- **S1.** `App` gains `scroll_skip: isize`, the fractional part of the
  vertical scroll in terminal rows. Positive means that many terminal
  rows of the line at `scroll_offset` are cut off the top of the pane;
  negative means that many blank rows are drawn above the document's
  first line.

  The pair is read and written only through three methods on `App`:
  `scroll_top()` (the top edge, in terminal rows from the top of
  document line 0, signed), `set_scroll_top(top)` (the only writer of
  the pair, which splits `top` back into a whole line and a remainder),
  and `terminal_row_of(row)` (where a document row is drawn, relative to
  the top of the main area). `row_height()` — 2 in wire mode, 1
  otherwise — is the conversion factor all three share.

  Normalization leaves `scroll_skip` in `0..row_height()` except at the
  very top, where there is no whole line left to borrow from and it goes
  negative. That is not a special case in the code: `set_scroll_top` is
  a signed division, and the negative branch falls out of it.

- **S2.** `toggle_wire` measures `terminal_row_of(cursor)`, flips
  `wire`, and calls `set_scroll_top` with the value that reproduces it.
  Both background cases are the same line of arithmetic.

- **S3.** The main pane's cursor clamp becomes
  `App::clamp_scroll_to_cursor(row)`, which works in terminal rows and
  replaces the two `clamp_scroll_to_visible(&mut self.scroll_offset, …)`
  calls (`clamp_pan_offset` and `render_main_pane`).

  It requires the cursor's *document* row to be on screen and
  deliberately not its wire row. A cursor on the pane's last terminal
  row is where the user put it; scrolling the document to reveal a byte
  row under it would undo the very thing S2 exists to preserve.

- **S4.** `pan_vertical` (Ctrl-Up/Ctrl-Down, wheel) is restated in
  terminal rows: it steps by `step * row_height()` and clamps to
  `0 ..= content_rows - main_area.height`. Panning therefore lands back
  on whole lines rather than carrying a half-line offset forever, and
  the lower bound of 0 is what spends the blank rows above the document.
  The document-line arithmetic it had was equivalent for
  `scroll_skip == 0`, so no pan behavior changes without one.

- **S5.** `render_main_pane` builds one more document line than before —
  `ceil((main_area.height + scroll_skip) / row_height())` — because a
  half-line scroll leaves a partial line at each end and both are drawn.
  The `flat_map` that draws is untouched; the skip is applied once to
  the finished `Vec<Line>`, dropping `scroll_skip` leading rows or
  prepending `-scroll_skip` blank ones.

  Everything keyed by *window index* — `window_styles`,
  `heat_displays`, `row_overridden`, `partner_cell`, the cursor-row
  test — is unaffected, because the window still starts at
  `scroll_offset`. This is the whole reason the skip is applied at the
  end rather than folded into the window.

- **S6.** `main_pane_line_idx` adds `scroll_skip` back before dividing,
  and returns `None` for a row above the document. A click on a blank
  row names no line.

- **S7.** `document_pane_height` keeps its whole-line answer and its
  remaining callers — the page keys, `max_visible_line_len`, the heat
  range. Those want "how many lines fit", where a partially drawn line
  is not one, and none of them is sensitive to a row of slack.

## Alternatives considered

### Round the toggle and accept the two cases

The shipped behavior before this spec, recorded in spec 0225 S8 as two
accepted imperfections. Rejected on report: a `w` that scrolls is a `w`
that loses the reader's place, which is exactly what the toggle was
given its rescale to avoid, and the accepted cases turn out to be
common — half of all offsets are odd.

### Pin the *node* rather than the row

Record the cursor node and re-derive a `scroll_offset` that puts it
back. This is what the whole-line scroll already does; it cannot express
the answer, so it rounds. The problem is the unit, not the target.

### Keep the pane's top on a whole line and move the cursor instead

Let `w` scroll to the nearest whole line and move the cursor to whatever
row that lands on. Rejected: the cursor is the user's, not the view's.
Spec 0229 had just finished making folds move the cursor *onto what the
user acted on*; moving it because a display toggle could not do
arithmetic is the opposite.

## Test plan

1. `toggling_wire_mode_keeps_the_cursor_on_its_terminal_row` — the row
   is held in both directions, from an even offset (no skip needed) and
   from an odd one (`scroll_skip == 1`), and a click on the pane's first
   row still names the half-drawn line it belongs to.
2. `turning_wire_mode_off_near_the_top_pads_above_the_first_line` —
   `scroll_skip == -3`, a click on a blank row is `None`, and moving
   down until the cursor reaches the pane's bottom spends one row of the
   padding.
3. The existing pan, page and click tests — unchanged, since they all
   run with `scroll_skip == 0`, where every restatement above is
   arithmetically the old one.

## Measured outcome

No measurement; the arithmetic is a handful of integer operations per
frame either way, and `render_main_pane` builds at most one extra
document line. 619 protolens tests pass.
