<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0304 — the caret brings the view with it

Status: implemented
Implemented in: 2026-08-16
App: protolens
Refs: docs/specs/0242-….md (spec 0242 S7 introduced `pan_to_caret` and
        the minimum-move rule, but bound it to selection motions only),
      docs/specs/0194-….md (`^` and `$`; the caret and its anchor),
      docs/specs/0199-….md (the arrow keys' edge behavior and the
        `Alt-Left`/`Alt-Right` word motions),
      docs/specs/0244-….md (`pan_offset` and the pane's horizontal
        viewport)

## Background

Spec 0242 S7 settled what should happen when the caret moves off the
side of the pane: `pan_to_caret` shifts `pan_offset` by the **minimum**
needed to bring `cursor_column` back inside the visible columns.
Minimum rather than centered, because extending a selection by one
character should move the view by one column.

That rule was wired to selection motions only. The same caret, moved
without `Shift`, can still leave the viewport, and then it is simply
gone: the user presses `$` on a long line and the caret is somewhere off
to the right with nothing on screen to show it, or walks right with the
arrow key past the last visible column and the view stays put.

Seven motions move `cursor_column` without panning:

- `Left`/`Right` when the caret is stepping within the line rather than
  leaving the node (`navigation.rs`, the two early-return arms);
- the anchor flip to `CaretAnchor::Home` / `CaretAnchor::End` — a move
  of the caret's *reported* column, which the block caret draws;
- `^` and `$` (spec 0194 S6), which jump to the caret bounds and so are
  the most likely of all to land off-screen;
- `Alt-Left` / `Alt-Right`, the word motions (spec 0199 S8).

There is nothing principled about the split. It is the set of call sites
that existed before spec 0242 and were not revisited when it landed.

## Goals

- **G1.** Every motion that changes `cursor_column` leaves the caret
  inside the visible columns, by the same minimum-move rule spec 0242 S7
  set for selection motions.

## Non-goals

- **N1.** Do not change the rule to centering. Spec 0242 S7's reasoning
  — a one-column move should cost a one-column pan — applies unchanged;
  `center_columns` stays reserved for search jumps, which land somewhere
  the reader has not been.
- **N2.** Do not touch vertical scrolling. `clamp_scroll_to_cursor`
  already runs on these paths; this spec is horizontal only.
- **N3.** Do not add panning to motions that leave the node (the
  `parent_move` / descend arms). Those change `self.cursor`, and the
  node-level move already re-establishes the viewport.
- **N4.** Do not introduce a "pan after every keystroke" hook in the
  dispatcher. Spec 0242 S7 put the call at the motion, and a blanket
  hook would fire on keys that move nothing — spec 0245 S2 counts a
  frame that changed nothing as a bug.

## Specification

- **S1.** `pan_to_caret` (in `selection.rs`) becomes `pub(super)` so
  `navigation.rs` can reach it. Its body is unchanged — the
  minimum-move rule of spec 0242 S7 is the rule this spec spreads, not
  one it revises.

- **S2.** `pan_to_caret()` is called at the seven sites named in the
  Background, each immediately after `desired_column` is set and before
  the arm returns:

  | site | motion |
  |---|---|
  | `caret_left`, in-line step | `Left` |
  | `caret_left`, anchor flip | `Left` onto `CaretAnchor::Home` |
  | `caret_right`, in-line step | `Right` |
  | `caret_right`, anchor flip | `Right` onto `CaretAnchor::End` |
  | `caret_to_line_start` | `^` |
  | `caret_to_line_end` | `$` |
  | `caret_word_left` | `Alt-Left` |
  | `caret_word_right` | `Alt-Right` |

  (Eight rows, seven described sites: `caret_left` and `caret_right`
  each contribute two.)

## Alternatives considered

### Call `pan_to_caret` once, centrally, after key dispatch

A single call at the end of `handle_key` would cover every present and
future motion with no per-site bookkeeping. Rejected: the main pane's
dispatch also handles keys that move no caret at all, and panning
unconditionally would either need a "did the column change" guard —
which is the per-site knowledge back again, in a worse place — or it
would defeat spec 0245 S2's `event_changed_nothing` accounting.

### Make `pan_to_caret` part of `clamp_caret_column`

Three of the eight sites already call `clamp_caret_column`, so folding
the pan into it would shorten the diff. Rejected: `clamp_caret_column`
is a *correctness* fixup that also runs when the caret's line changes
under it, and coupling a viewport effect to it would pan on paths that
did not move the caret horizontally at all.

## Test plan

1. `dollar_pans_to_the_end_of_a_long_line` — on a line wider than the
   pane, `$` leaves `cursor_column` inside `[pan_offset, pan_offset +
   usable)`.
2. `caret_pans_by_one_column_not_by_a_screenful` — stepping `Right` off
   the right edge moves `pan_offset` by exactly one, pinning the
   minimum-move rule at a non-selection site.
3. `word_motion_keeps_the_caret_visible` — `Alt-Right` repeated across a
   long line never leaves `cursor_column` outside the visible columns.
4. `caret_to_line_start_pans_back` — `^` after `$` on a long line
   returns `pan_offset` to 0.
