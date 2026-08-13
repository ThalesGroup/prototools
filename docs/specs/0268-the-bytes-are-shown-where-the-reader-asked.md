<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0268 — the bytes are shown where the reader asked

Status: implemented
Implemented in: 2026-08-09
Amended 2026-08-13: **S8, `Ctrl-w` puts the bytes away.** N1's single
        run is cleared by `w` or `W` on a row *inside* it, which means
        the gesture has to be aimed at a run the reader may have
        scrolled away from — and after a `W` on a large subtree the run
        can be longer than the screen. `Ctrl-w` sets the span to `None`
        whatever is showing and wherever the caret is, through
        `set_wire_span` so that the caret keeps its terminal row as the
        pane grows back (S6). On nothing showing it is a no-op, not an
        error: "the bytes are away" is the state it names, not a
        transition it performs. It is not one of spec 0242 S3's
        selection-keepers — it does not read the selection — and
        `keeps_the_selection` already excludes it, `Ctrl-w` not being
        `w`.
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (the
        wire row itself, and S8's whole-pane `w` this replaces),
        docs/specs/0230-….md (`PaneScroll`'s signed `skip`, which is
        what makes a half-drawn row representable),
        docs/specs/0244-a-pane-scrolls-and-pans-as-one.md (the one
        vertical viewport all of this goes through),
        docs/specs/0259-the-viewport-holds-its-place-across-a-splice.md
        (`AnchorLine`, reused here for the same staleness reason),
        docs/specs/0242-a-selection-is-an-anchor-and-a-caret.md (the
        selection `w` reads)

## Background

`w` shows the bytes under *every* line in the document or under none.
That is the wrong unit. A reader looking at one field wants that
field's bytes, and pays for them by halving the pane — 25 lines of
context instead of 50 — for a screenful of hex they did not ask about.
The interesting case is almost always a handful of lines: one record
whose length looks wrong, one packed run, one subtree being compared
against a schema.

## Goals

- **G1.** `w` turns the bytes on under the lines the reader is looking
  at — the selection, or the caret's own line — and nothing else.
- **G2.** `W` does the same for a whole subtree, so "show me this
  message's bytes" is one key rather than a selection gesture.
- **G3.** Rows outside the shown run keep costing one terminal row, so
  the pane loses only the height it was asked to spend.

## Non-goals

- **N1. Several disjoint runs.** One `w`/`W` *replaces* what is shown
  rather than adding to it, so there is exactly one run at any moment.
  This is what makes the geometry three numbers instead of a data
  structure (S4), and it is also the behavior that can be explained in
  one sentence: the bytes are wherever you last asked for them.
- **N2. The wire row's own drawing.** Spec 0225's four hues, spec
  0229's `\x` label and every byte of `tui/wire.rs` are untouched. This
  spec decides only *which rows get one*.
- **N3. Persisting the run.** It is a display attribute like
  `annotations`, not something an override file or `:export` knows
  about.

## Specification

- **S1. The shown run is one span, anchored to nodes.**

  ```rust
  struct WireSpan { first: WireAnchor, last: WireAnchor }
  struct WireAnchor { node: usize, line: AnchorLine }
  ```

  `App::wire` becomes `Option<WireSpan>`; `None` is today's "off".
  `AnchorLine` is spec 0259's, unchanged and reused for its own
  reason: a splice renumbers every line below it, so a span held as
  line numbers slides off the rows it was pointing at. `Footer` — "the
  node's last line, whatever the count has become" — is also what
  makes `W` on a node that is still baking cover the lines that arrive
  later, which the reader's own example (`W` at the root, then read on)
  requires.

  Resolved to absolute lines by `absolute_start(node) + line`, and
  from there to a *visible row* range by `visible_row_of_line`. Folds
  and splices both bump `structural_version`, so the resolved pair is
  cached against it and costs two descents per structural change
  rather than two per query.

- **S2. `w` acts on lines, and the caret's line decides.**

  With a selection, the span's ends are the first and last selected
  lines. With none, both ends are the caret's own line. Either way the
  new state is the negation of the state of the line *under the
  caret* — the selection widens what the decision applies to, never
  what decides. Since a `w` replaces the span outright, that is exactly
  two outcomes: the span becomes the target (the caret's line was not
  shown), or it becomes `None` (it was).

  The caret rather than the span's first line, because the reader can
  see where the caret is and a selection may run off the top of the
  screen. The two differ only when the caret's line and the selection's
  first line disagree about their current state, which is precisely
  when reading the first one would do the opposite of what was asked.

  "Unless the caret is outside the selection" needs no code: spec 0242
  made the caret the selection's own moving end, so it is always one of
  the two lines the span is built from.

  Lines, not nodes, because the only place the two differ is a packed
  record — one node, many element lines — and there the line is the
  better unit: an element's own bytes are what the reader is pointing
  at. Nothing else needs the distinction, since every other node draws
  one line per line.

- **S7. `w` and `W` leave the selection standing.**

  Spec 0242 S3 clears the selection on every main-pane key that is not
  one of the four `Shift`-motions or `Ctrl-c`. `w`/`W` read the
  selection, so they join that list — otherwise S2's selection case
  could never be reached through the keyboard at all, the selection
  being gone before the gesture ran.

- **S3. `W` acts on a subtree.**

  The target is the deepest node containing every selected line — with
  no selection, the caret line's own node. The span is that node's
  header through its footer, which is its whole subtree, because a
  node's lines are contiguous and its descendants' lie between them.
  The same two outcomes as S2, read off the target's header line.

  The common ancestor is found by climbing from the two end nodes
  until the paths meet, which is O(depth) — 13 on the reference
  corpus.

- **S3a. A packed record's element is a line, not a subtree.**

  Spec 0216 S22 collapses a packed run into one arena node, but that
  node is not something the reader can see: its elements are drawn as
  the message's own fields — `vals: 5` at the same indent as `a: 42`,
  no header, no brace. So for `W` an element *line* is the unit:

  - one element (no selection, or a selection within its line) is a
    **leaf**, so `W` on it does what `w` does — that one line;
  - two or more elements are **siblings**, so the subtree they name is
    the *message* holding them, which reaches the run's other
    elements, the message's other fields, and any second packed record
    beside this one.

  Both ends resolving to one unbracketed node of several lines is
  exactly this case and no other, so it is one condition rather than a
  packed-record concept spread through the navigation code.

- **S4. A row's height is a function of the row, and the function is
  three numbers.**

  This is what S1's single-span rule buys. With the shown run resolved
  to visible rows `tall`, the map from a row to the terminal row it
  starts on is

  ```
  offset(row) = row + (row.clamp(tall.start, tall.end) - tall.start)
  ```

  — arithmetic, no lookup, invertible in the same form. It replaces
  `App::row_height()`'s scalar at every site that multiplied by it:
  `PaneScroll::{top, set_top, window}`, `terminal_row_of`,
  `clamp_scroll_to_cursor`, `pan_vertical`, `center_row`,
  `restore_scroll_anchor`, the click hit-test, and the viewport label.

  The three methods on `PaneScroll` take the map rather than a scalar,
  and the side panes pass the empty one. Keeping one implementation
  matters more here than the scalar did: `set_top` and `top` must stay
  exact inverses or the caret drifts a row per toggle, and that is one
  property to prove once, not twice.

- **S5. `document_pane_height` answers for the viewport it has.**

  It is "how many document lines fit", which is no longer a division.
  It becomes the number of rows from the current top that fit in the
  pane's terminal rows — still O(1) through S4's map, still at least
  1. Its callers are the page keys and the auto-fold expansion
  budgets, and all of them want the count that is actually on screen.

- **S6. The toggle still holds the caret's terminal row.**

  Spec 0225 S8's rule and its two consequences (turning the bytes on
  from an odd row starts the pane mid-line; turning them off near the
  top leaves blank rows above line 0) are unchanged — measure the
  caret's drawn row, change the span, put it back. It now matters
  more, not less: a `w` that changes the height of rows *above* the
  caret moves it further than the old whole-pane toggle ever did.

## Alternatives considered

### Keep `wire: bool` and add a separate per-node set

A `HashSet<usize>` of nodes showing bytes is the obvious shape, and it
is what makes the geometry expensive: the height of row *n* stops being
arithmetic and becomes a membership test, so `offset(row)` becomes a
sum over every row above it and the viewport math goes from O(1) to
O(document). A prefix-sum index over the visible rows would fix that
and would have to be rebuilt on every fold and every splice — of which
a bake does 70 000. The single-span rule is not a simplification of the
feature; it is the feature that can be afforded.

### Store the span as absolute line numbers

Cheapest to resolve and wrong within one bake step: the span would
stay on lines 40-60 while the content that was there moved to 900-920.
Spec 0259 already learned this for the viewport and already has the
type for it.

### `w` on a node rather than on a line

Uniform with `W`, and worse in the one case that differs: `w` on any
line of a packed run would light all of its elements, which is a
`W`-sized answer to a `w`-sized question. It also makes `w` on a
submessage header light the header *and* the footer with the children
between them dark — a span with a hole in it, which S4's arithmetic
cannot express.

### Let `w` extend the shown run instead of replacing it

Then the run is a set again, with N1's cost, and there is no gesture
that clears it short of walking back over every line. Replacing is
also what makes the state describable without reference to its
history.

## Test plan

1. `w_with_no_selection_shows_only_the_caret_line` — one wire row in
   the frame, under the caret's row.
2. `w_over_a_selection_shows_exactly_those_lines`, and
   `the_caret_decides_and_the_selection_follows` for S2's rule about
   which line's state is read — a selection whose first line is lit
   while the caret's is not is the one case that tells them apart.
3. `w_inside_a_shown_run_turns_the_whole_run_off` — the reader's own
   example: `W` at the root, then `w` on one line, and no row in the
   frame has bytes under it.
4. `capital_w_shows_a_subtree_and_nothing_beside_it` — a sibling of
   the target subtree has no wire row.
5. `capital_w_with_a_selection_climbs_to_the_common_ancestor`, plus
   S3a's two halves:
   `capital_w_over_two_packed_elements_climbs_to_the_message` and
   `capital_w_on_one_packed_element_lights_that_element`.
6. `a_growing_subtree_keeps_its_bytes` — `W` on an auto-folded node,
   then bake it, and the lines that arrive are shown too (the
   `AnchorLine::Footer` half of S1).
7. `rows_outside_the_run_stay_one_terminal_row` — the pane shows
   `height - shown` more lines than the old whole-pane toggle did.
8. `offset_and_row_at_round_trip` — S4's map is a bijection on terminal
   rows, at every span position, including the empty span (property
   test in the style of `set_top_and_top_round_trip`).
9. The existing wire-mode viewport tests
   (`toggling_wire_mode_keeps_the_cursor_on_its_terminal_row`,
   `turning_wire_mode_off_near_the_top_pads_above_the_first_line`,
   `pan_up_after_a_wire_toggle_does_not_snap_back`,
   `wire_mode_bounds_are_terminal_rows`, `page_down_advances_by_the_
   halved_height`, `a_click_on_a_wire_row_selects_the_line_above_it`)
   restated against a span covering the whole document, which is what
   they were testing.

## Measured outcome

Implemented 2026-08-09. `App::wire` is `Option<WireSpan>`;
`App::row_height()` is gone and `RowHeights` (`tui/pane_scroll.rs`) has
taken its place at all ten sites S4 lists, with `FLAT_ROWS` for the
three side panes, which have no wire rows to make a row tall.

What the pane costs is now what was asked for. On a 10-row pane over a
24-line document, a three-line span leaves 7 lines of context where the
old whole-pane toggle left 5 — and the difference grows with the pane:
a 50-row terminal loses 3 rows instead of 25
(`rows_outside_the_run_stay_one_terminal_row`).

Three findings from implementation, all now in the specification:

- **The caret, not the span's first line** (S2, the reader's own
  amendment). The draft read the state off the first selected line,
  which does the opposite of what was asked whenever the caret's line
  and the first one disagree — and a selection can run off the top of
  the screen while the caret cannot.
- **Spec 0242 S3 would have eaten the gesture** (S7). Every main-pane
  key that is not a `Shift`-motion or `Ctrl-c` clears the selection
  *before* dispatch, so S2's selection case was unreachable from the
  keyboard until `w`/`W` joined that list. Nothing in the draft
  suggested the two specs met.
- **A packed element is a line to `W` as well** (S3a, the reader's
  second amendment). The draft let `W` fall through to the arena, so a
  selection of two elements named the run they share and a single
  element named the whole run — both of them answering with a node the
  reader cannot see on screen. S2 already took the same view for `w`;
  S3a says the two gestures agree about what an element is.

`AnchorLine::Footer` earns its place: `a_growing_subtree_keeps_its_bytes`
puts `W` on a bounded root, bakes it, and the run covers the rows that
arrive afterward. Held as line numbers the span would have stayed where
it was.

844 protolens tests pass, of which 30 in `tui/tests/wire.rs`.
