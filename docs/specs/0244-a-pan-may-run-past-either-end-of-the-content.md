<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0244 — a pan may run past either end of the content

Status: implemented
Implemented in: 2026-08-05
App: protolens
Refs: docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (the pan is
        bounded by the content and not by the cursor; the
        `Top`/`Bot`/`%` viewport label),
        docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md
        (S8: a wire-mode document line is two terminal rows),
        docs/specs/0230-the-scroll-is-counted-in-terminal-rows.md
        (`scroll_skip` — the signed remainder that already lets the main
        pane's top edge go negative)

## Background

Vertical panning stops dead at both ends of the content. `pan_vertical`
ends in `.clamp(0, max_top)` with `max_top = content - pane_height`
(`navigation.rs`), and the side panes' `pan_by_step_clamped` floors at
`0` with the same upper bound. So the first line can never leave the
pane's top row, and the last can never leave its bottom.

The main pane already knows how to draw a viewport whose top edge is
*above* the document. Spec 0230 made `scroll_skip` signed precisely so
that a `w` toggle can hold the cursor's terminal row still, and negative
means "blank rows above the document's first line": `render_main_pane`
prepends them (`render.rs`, the `scroll_skip` match) and `visible_row_at`
maps a click on one to no line (`mouse.rs`). Only `w` can reach such a
position today.

That is the visible defect. Press `w` near the top of the document to
land above line 0, then pan up: the clamp snaps the viewport back down to
0 in one step. The pan refuses a position the renderer draws correctly
and the user is already looking at.

## Goals

- **G1.** A vertical pan may put the pane's top edge anywhere from *one
  terminal row of content left on the pane's last row* to *one left on
  its first*. Both ends keep exactly one terminal row of content on
  screen.
- **G2.** All three pannable panes behave the same way: main, override,
  manage.

## Non-goals

- **N1.** The help overlay (`help_scroll`) and the command bar are
  untouched. Neither has a vertical pan — the overlay scrolls by `j`/`k`
  over a fixed text, the command bar pans horizontally only.
- **N2.** Moving the cursor does **not** end an over-pan. The clamps
  (`clamp_scroll_to_cursor`, `clamp_scroll_to_visible`) stay minimal
  nudges: they move the viewport only far enough that the cursor or the
  highlight is on screen, and if it already is — blank rows and all —
  they move it not at all. So an over-panned viewport survives cursor
  movement within it, and is left only by panning back or by moving far
  enough to need the room.

## Specification

### Step 0 — a viewport is one value

A pure refactor, no behavior change, landed on its own. Without it the
rest of this spec adds a second field beside an existing one at each of
the two side panes, and "these two are really one value" has to be
remembered at seven reset sites and four window slices — two of which
are already copies of each other, and one of which
(`manage_max_visible_line_len`) feeds the *horizontal* pan clamp, where
getting it wrong has no visible cause.

- **S1.** `PaneScroll { index: usize, skip: isize }` — a `Copy`,
  `Default` struct in `tui/pane_scroll.rs`, meaning exactly what the main
  pane's `scroll_offset`/`scroll_skip` pair means today: `index` is the
  first content row drawn; `skip` is the signed remainder in terminal
  rows — positive, that many rows of `index`'s row are cut off the top;
  negative, that many blank rows are drawn above it. Its whole API:

  ```
  top(row_height) -> isize                       // index * row_height + skip
  set_top(top, row_height)                       // the only writer; normalizes
  window(pane_height, row_height, total) -> (blank: usize, Range<usize>)
  ```

  `window` is the derivation currently open-coded at every draw site: how
  many blank rows to prepend, and which content rows to draw after them.

- **S2.** The main pane's `scroll_offset`/`scroll_skip` become one
  `scroll: PaneScroll`. `scroll_top`/`set_scroll_top` stay, as one-line
  wrappers that supply `self.row_height()`.

- **S3.** `override_scroll` and `manage_scroll` become `PaneScroll`s too,
  with `row_height == 1` and a `skip` that is still always `0` at this
  step. Their seven reset sites become `PaneScroll::default()`, and
  `SearchOrigin` carries a `PaneScroll` by value rather than a `usize`,
  so that step 1 needs no further change at either.

- **S4.** The four open-coded window slices — `render_override_pane`,
  `render_manage_pane`, `override_max_visible_line_len`,
  `manage_max_visible_line_len` — call `window` instead. `render_main_pane`'s
  own `scroll_skip` match does too.

### Step 1 — the bounds

- **S5.** One shared bound, `pan_top_bounds(content_rows, pane_height)
  -> (min_top, max_top)`, in terminal rows measured from the top of line
  0, where `content_rows = content_lines * row_height`:

  ```
  min_top = min(1 - pane_height, 0)
  max_top = max(content_rows - 1, 0)
  ```

  `min_top` puts the document's first terminal row on the pane's last
  row; `max_top` puts its last terminal row on the pane's first. Stated
  in *terminal rows*, not lines, so that in wire mode (`row_height == 2`,
  spec 0225 S8) a bound may fall between a line and its wire row and cut
  the pair in half. That is deliberate: `w` holds the cursor's *terminal*
  row still across the toggle, so a bound expressed in whole lines would
  be a position `w` can reach and a pan cannot — the very asymmetry this
  spec exists to remove. Both ends are clamped against `0` for the
  degenerate cases — a pane shorter than one row, and empty content — so
  that a pan over nothing is a no-op rather than a negative range.

- **S6.** `pan_vertical` clamps to `(min_top, max_top)` instead of
  `(0, content - pane_height)`. Everything else about it is unchanged: it
  still works through `scroll_top`/`set_scroll_top`, so a pan started from
  a half-line `w` offset still lands back on whole lines.

- **S7.** The two side panes' `override_pan_vertical`/`manage_pan_vertical`
  go through `PaneScroll::set_top` and the same `pan_top_bounds`, so all
  three panes share one rule. `pan_by_step_clamped` thereby loses both of
  its vertical callers and is left to the two horizontal pans it also
  serves; its doc comment drops the Ctrl-Up/Ctrl-Down sentence.

- **S8.** `clamp_scroll_to_visible` takes a `&mut PaneScroll` and works on
  the signed top, mirroring `clamp_scroll_to_cursor`. Without this a
  highlight moved into `[index, index + height)` can still be off screen,
  because the blank rows push the last of those rows past the bottom edge.

- **S9.** Each side pane's renderer prepends the blank rows `window`
  reports and draws that many fewer content rows, as `render_main_pane`
  already does. A click on a blank row selects nothing, as
  `visible_row_at` already decides for the main pane.

- **S10.** `viewport_label` takes the signed top and reports `All` only
  when the whole document really is on screen:

  ```
  All  if top <= 0 and top + height >= total
  Top  if top <= 0
  Bot  if top + height >= total
  N%   otherwise, N = top * 100 / (total - height)
  ```

  Its `first_visible: usize` parameter becomes an `isize`, so the three
  callers pass the pane's top edge rather than a floored row index.
  Today's first test is `total <= height`, which is true of a short
  document however far it has been panned — the label would read `All`
  with the document's last line alone on the pane's first row. Under an
  over-pan `Top` and `Bot` can both be true at once (a document shorter
  than the pane, panned so that it hangs off neither edge), and `All` is
  the right name for that; the ordering above says so. The percentage
  needs no clamp: the three earlier arms leave it only when
  `0 < top < total - height`, so the quotient is already in `1..100` and
  the denominator is already non-zero.

## Alternatives considered

**Make the side panes' `scroll` an `isize`.** Fewer fields, but it moves
the `.max(0) as usize` cast to all eight sites that use the value as an
index into the candidate/entry list, where getting it wrong is a panic
rather than a misdraw. The `(index, skip)` pair keeps the invariant in
one place — and it is the shape the main pane already proved out.

**Add the two skip fields directly, and skip step 0.** The smaller diff,
and the one this spec was first drafted with. Rejected: it leaves seven
`= 0` resets that each have to remember a second line, and copies the
blank-row arithmetic into four draw sites. Step 0 touches the same
number of places, but each becomes one call, and it leaves one shape for
the idea instead of the two that arise from converting only the side
panes.

**Bound the over-pan at a full pane of blank, letting the content leave
entirely.** Rejected: a pane showing nothing gives the user nothing to
aim at to pan back, and no way to tell an over-panned pane from an empty
document.

**Round the bounds to whole document lines, so that a wire-mode line is
never split from its wire row.** Prettier at the two extremes, and
wrong — `w` restores
the cursor's terminal row, so it can put the top edge on an odd row that
a line-granular pan then refuses to move from. Rounding at the bound
would also make the last pan step a different size from every other one.
The bound is in terminal rows, and half a pair on screen at the very
extreme is the price.

**Relax only the top, as literally asked.** Rejected on symmetry: the
snap-back that motivates this is the clamp arriving abruptly, and the
bottom clamp arrives just as abruptly. Two different rules at the two
ends would be a second thing to remember.

## Test plan

Step 0 carries no test of its own beyond `pane_scroll.rs`'s unit tests
for `set_top`/`top` round-tripping and for `window` at each edge
(`skip` positive, zero, negative; a `total` shorter than the pane). Its
real check is that the existing suite passes unchanged — a step 0 that
needs a test rewritten has changed behavior, which it must not.

1. `pan_up_may_leave_blank_rows_above_the_first_line` — from the top of
   the document, Ctrl-Up yields a negative `scroll_top` and the rendered
   frame's first rows are empty.
2. `pan_up_stops_with_one_line_still_shown` — repeated Ctrl-Up settles at
   `min_top` with the first line on the pane's last row, never past it.
3. `pan_down_stops_with_one_line_still_shown` — the mirror at `max_top`.
4. `pan_up_after_a_wire_toggle_does_not_snap_back` — the reported
   defect: `w` near the top, then Ctrl-Up, moves the viewport up rather
   than jumping it to `0`.
5. `wire_mode_bounds_are_terminal_rows` — in wire mode the bound at each
   end may leave a document row without its wire row, or a wire row
   without its document row; a pan settles one terminal row past where a
   whole-line bound would have stopped it.
6. `a_wire_toggle_keeps_the_cursor_row_at_the_bounds` — the reason for
   test 5: `w` at an over-panned bound moves the cursor's terminal row
   not at all.
7. `override_pane_pans_past_both_ends` and
   `manage_pane_pans_past_both_ends` — G2.
8. `moving_the_highlight_pulls_the_viewport_no_further_than_needed` —
   S8, in both side panes: the highlight comes on screen and the blank
   rows that are not in its way stay.
9. `a_click_on_a_blank_row_selects_nothing` — S9.
10. `viewport_label_over_a_short_over_panned_document` — S10: `Bot` and
    `Top`, not `All`, once the document has been panned off an edge.

## Measured outcome

Step 0 landed as intended: `PaneScroll` (`protolens/src/tui/pane_scroll.rs`,
~145 lines with its own five unit tests) is now the only place that knows
an index and a signed remainder are one number, and the existing suite
passed unchanged across it — the whole of step 0's own risk. The seven
resets became `PaneScroll::default()`, the four open-coded window slices
became one `window` call each, and `SearchOrigin` carries the viewport by
value.

Two deliberate restraints kept step 0 behavior-neutral. `max_visible_line_len`
was left on `document_pane_height()` rather than converted to `window`,
because `window` counts terminal rows with `div_ceil` where
`document_pane_height` floors — converting would have changed wire mode
with an odd pane height. And `center_search_match` gained a `.max(0)`, so
centering a match can never manufacture blank rows.

Step 1 is 10 tests, all passing (693 in the crate, 0 failures). The bounds
came out where the arithmetic said they would: `min_top <= 0 <= max_top`
always holds, so the `.clamp()` cannot panic and no caller needs a guard.
The wire-mode ends land on odd terminal rows — 79 of 80 at the bottom,
`-9` at the top of a 10-row pane — which is exactly the point of counting
in terminal rows rather than document lines: `w` at either bound moves the
cursor's drawn row not at all.

The reported defect is fixed and covered by
`pan_up_after_a_wire_toggle_does_not_snap_back`: with the old floor at `0`,
Ctrl-Up after a wire toggle near the top threw the blank rows away and
moved the document *down* the screen.
