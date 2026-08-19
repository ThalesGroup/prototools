<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0333 — a double-click folds what it names

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0284-the-heat-cue-is-a-control.md (the two click zones,
        and the rule that a pair may not straddle them),
      docs/specs/0332-folding-is-a-question-about-the-bytes.md (`z` is
        `toggle_cursor_fold`, and what "foldable" means),
      docs/specs/0242-the-selection-is-a-span-of-characters.md (the
        selection the text zone's double-click used to make),
      docs/specs/0129-protolens-main-pane-line-select-copy.md (the
        gesture this supersedes)

## Background

Folding is the most-used gesture in the main pane — spec 0142 gave it a
two-column click target for that reason, and spec 0332 gave it eleven
keys. From the mouse there is exactly one way in: the fold marker in the
left margin, two columns wide, at the far left of a row that may be
indented forty columns.

So a reader working with the mouse has to travel to the margin and back
for every fold, while the row they actually care about is under the
pointer already. The double-click on that row currently spends itself on
selecting the row's text for copying — a gesture worth far less, in a
pane whose whole purpose is navigating a tree.

## Goals

- **G1.** A double-click on a main-pane row's own text does what `z`
  does: toggle the fold of the node that row belongs to.
- **G2.** A double-click that turns into a drag is not that gesture. The
  drag's selection stands and no fold happens.

## Non-goals

- **N1.** *Keeping the row selection on the same gesture.* One zone, one
  meaning — a double-click that both folded a subtree and selected a row
  would be two gestures wearing one name, which is the reason spec 0284
  gives for the text/cue split in the first place, and the reason spec
  0242 already took the `t`/`o` proxy off this gesture. Selecting text
  with the mouse remains the drag, which is the gesture that says *this
  span*, and `select_current_line` therefore loses its only caller and
  goes.

- **N2.** *A drag-cancel on the cue zone.* The cue's double-click is
  unchanged, drag or no drag. Its target is eight columns wide at the
  right-hand end of a row and nothing there is selectable (spec 0242
  S11), so a drag out of it expresses nothing to protect.

- **N3.** *A depth on the mouse.* No modifier spells `Z` or a digit.
  Those are absolute settings over a whole subtree; a pointer names one
  node, and the keyboard is where an absolute is worth learning.

## Specification

- **S1.** On `Up(Left)` with `pending_double_click == Some(Text)`, the
  main pane clears the selection and runs `toggle_cursor_fold()` — the
  same two steps, in the same order, that pressing `z` runs (the clear
  is `handle_key`'s `keeps_the_selection` gate, which `z` does not
  pass). No positioning step is needed: the first click of the pair put
  the caret on the row, and `toggle_cursor_fold` acts on the cursor.

- **S2.** A leaf answers `not foldable`, exactly as `z` does. The
  gesture is `z`, including when `z` refuses.

- **S3.** G2's test is `select_engaged`, and it needs no new state. The
  `Down` arm sets `select_engaged = false` on every plain main-pane
  click, and within a button-down the only writer is `drag_caret_to` —
  so at `Up`, `select_engaged` *is* "a drag arrived since the press".
  When it is set, the pair does nothing at all and the drag's selection
  is left standing.

- **S4.** Everything spec 0284 decided about *which* zone a click is in
  stands untouched: the fold marker still forms no pair (it acts on
  every click), a pair still may not straddle the text and cue zones,
  and the columns past a short row's text are still the text zone.

## Alternatives considered

**Put the fold on the cue's zone and leave the text alone.** The cue is
drawn only on rows that have one, is hidden entirely by the default
`i` mode (spec 0331), and already means something else. It is the wrong
half of the row for the pane's most common action.

**Keep the selection and put the fold on a modifier-double-click.**
`Shift` is eaten by most terminals before protolens sees it (the reason
`Ctrl` is its alias for a main-pane click at all), and `shift_click_to`
deliberately forms no pair. A gesture that half the terminals cannot
deliver is not a binding.

**Fold on a single click anywhere on the row.** A single click is how
the caret is placed; there would be no way left to point at a row
without restructuring it.

## Test plan

1. `a_double_click_on_a_row_folds_it_like_z` — a double-click and a `z`
   leave the same `folded` bit *and* the same `document_lines()`, twice
   over, so the second pair opens what the first closed; and the
   gesture leaves `selection_span()` empty, which is the half of "it is
   `z`" that the fold bit does not cover.
2. `a_double_click_that_dragged_selects_instead_of_folding` — `Down`,
   `Down`, `Drag`, `Up` keeps the drag's selection and toggles no fold.
3. `a_double_click_on_a_leaf_says_not_foldable` — S2.
4. `the_two_zones_of_one_row_mean_different_things` — the same node,
   pointed at two columns apart, folds or opens the override pane.
5. `a_click_on_the_text_then_on_the_cue_is_not_a_pair` — neither the
   pane nor the fold, in either order.
6. `double_click_on_the_fold_marker_toggles_twice_and_opens_nothing` —
   spec 0284 S7's rule, now non-vacuous (see below).

## Measured outcome

Implemented as specified. `select_current_line` lost its only caller
and was deleted; nothing else moved.

**The change found a test that had been passing for the wrong reason.**
`double_click_on_the_fold_marker_toggles_twice_and_opens_nothing`
aimed at `indent_len + 1`, one column short of the fold field — the
sibling test three functions above it uses `indent_len +
HEAT_FIELD_WIDTH`, which is the correct 2. So both clicks landed in the
gutter, which is the *text* zone; the pair selected the row, and all
four of the test's assertions ("toggles twice, back where it started",
"opens no pane") held vacuously about a gesture that never touched the
marker. Giving the text zone a visible effect is what exposed it. The
column is fixed and the test now exercises what it claims.

Two ordering facts the tests had to be written around, both consequences
of the fold changing the row it is measured on:

- A cue's drawn column must be re-read after a fold. The suffix is
  appended to the text (spec 0284 S2), so shortening the text slides it
  left — a column captured before the fold is no longer on the cue.
- A `last_click = None` is needed between two pairs on the same row, or
  the third click pairs with the second.
