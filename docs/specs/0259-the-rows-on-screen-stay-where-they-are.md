<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0259 — the rows on screen stay where they are

Status: implemented
Implemented in: 2026-08-08
App: protolens
Refs: docs/specs/0249-a-large-document-answers-the-user-first.md (S10,
        S10a — this spec is their implementation), docs/specs/0255-the-document-finishes-itself-while-nobody-waits.md
        (the bake that moves the rows), docs/specs/0257-the-first-pane-does-not-wait-for-the-last-line.md
        (which put a bake in every session), docs/specs/0244-a-pan-may-run-past-either-end-of-the-content.md
        (`PaneScroll`, the viewport this spec re-anchors)

## Background

`PaneScroll.index` is a document row number (spec 0244). A bake step
(spec 0255) renders a subtree that was drawn as one folded row and
splices in its real body, so every row below the splice gets a new
number. The viewport keeps its old number. The rows under the reader
therefore move.

Measured on `bounded_repeated_message_fixture(3)`, a four-node fixture
whose stops hold one line each — the smallest possible instance of the
problem:

| | before the bake step | after |
|---|---|---|
| document rows | 9 | 10 |
| cursor's document row | 6 | 7 |
| **cursor's terminal row** | **5** | **7** |
| viewport top | 0 | 0 |

The viewport did not move and the content did. On googleapis a single
stop's body is thousands of rows, and there are 7 771 stops.

Before spec 0257 this needed a root override plus a scroll to reach.
Spec 0257 made every session open with a bake in progress — 5.9 s of it
on googleapis — so this is now the first thing that happens to every
reader of a large file.

## Goals

- **G1.** A splice does not move the rows the reader can see. The row
  drawn at the top of the pane before a splice batch is drawn at the top
  of the pane after it.
- **G2.** The rule is the same for a bake step and for a confirm. Both
  are splice batches and neither is allowed to scroll the document.
- **G3.** An anchor whose node stops being rendered resolves to
  something, never to row 0 and never to a panic.

## Non-goals

- **N1. Stable absolute row numbers.** They are not stable and cannot
  be: spec 0249 S10 accepted that the number beside a row changes while
  a bake runs. What this spec fixes is the row's *position on screen*.
- **N2. Keeping the caret on a fixed terminal row.** When a bake lands
  between the top of the pane and the caret, the caret is genuinely
  further down the document than the rows above it, and it must move
  down the screen to say so. Anchoring the caret instead would scroll
  every row above it off the top — the defect this spec exists to fix,
  inverted. See "Alternatives considered".
- **N3. The side panes.** The override and manage panes are lists whose
  rows a splice does not renumber.

## Specification

- **S1. The viewport's anchor is a row's owning node, captured at draw
  time.** `render_main_pane` already resolves the window's first row to
  a `LinePos` (spec 0222 S3); that pair — node plus which of the node's
  own lines — is stored on `App` alongside the viewport's `skip`. The
  arena is immutable (spec 0216), so a node index stays valid for the
  life of the document and nothing has to be invalidated.

  Captured from the frame rather than at the start of a splice batch
  because a batch has several entry points (`splice_override` standalone,
  the outermost `render_overrides`) and one exit (`finalize_override_batch`).
  The last drawn frame *is* the current viewport — spec 0245 draws one
  for every event that changes anything — so the frame is the honest
  place to read it.

- **S2. A footer is anchored from the end, every other row from the
  start.** A bracketed node draws exactly two of its own rows: its
  header at `line_in_node = 0` and its closing brace at
  `lines_total - 1`. A bake changes `lines_total`, so a footer's stored
  `line_in_node` is stale the moment the body it closes arrives, and
  restoring it verbatim lands somewhere in the middle of the new body.

  This is not a corner case. A reader who has scrolled to the bottom of
  a bounded document is sitting on a stack of closing braces, and that is
  where `G` puts them.

  A flat node's row count does not change under a bake, so its rows are
  anchored from the start with no special case.

- **S3. The anchor is restored at the top of `finalize_override_batch`,
  before the pan clamp.** Resolve the anchor to an absolute row, ask
  `visible_row_of_line` for its visible row, and `set_scroll_top` it back
  to the terminal row it had. `clamp_pan_offset` then runs as it does
  today.

  The order is what composes the two rules. The clamp scrolls to the
  caret only when the caret is *outside* the pane; with the anchor
  already restored, a bake that landed above the viewport leaves the
  caret on the terminal row it had, so the clamp finds it inside and
  moves nothing. The clamp only acts in the case N2 describes — content
  grew between the top of the pane and the caret, and the caret has been
  pushed off the bottom — where scrolling the minimum to recover it is
  the right answer.

  Restoring *after* the clamp would undo it and leave the caret off
  screen.

- **S4. An anchor on a node that is no longer rendered climbs to its
  nearest rendered ancestor** (spec 0249 S10a). An `FqdnField` override
  can match a node's parent and flatten it to `bytes`, after which the
  child has no row to anchor to. Walk `parent[]` until a rendered slot is
  found; the root is always rendered, so this terminates, over a chain 13
  deep on googleapis.

- **S5. No anchor is captured while a preview overlay is held.** With an
  overlay, a display row and a committed visible row are different
  numbers, and `visible_row_of_line` answers in the second. The overlay
  is a live preview the user is actively driving, and it covers a
  contiguous span for as long as a pane is open — restoring a viewport
  underneath it is not worth a second coordinate system. No anchor means
  the old behavior, which is what happens today.

## Alternatives considered

### Anchor the caret, as spec 0249 S10 wrote it

S10 says to hold the caret's slot and put it back on the terminal row it
occupied. That is right in the two cases it was written for — a bake
entirely above the viewport, or entirely below the caret — and wrong in
the two it was not:

- **A bake between the top of the pane and the caret.** Holding the
  caret still pushes the top row *up* and off the screen. Every row the
  user was reading moves, to keep still the one row that had a reason to
  move.
- **The caret above the viewport**, which an `Alt` pan produces and which
  the code deliberately supports (`pan_vertical` is unclamped). A bake
  between the caret and the viewport does not move the caret at all, so a
  caret anchor computes a correction of zero while the whole screen
  shifts.

The top row's node is right in all four. The caret then keeps its
terminal row in exactly the cases where the document did not put content
above it, which is the property worth having.

### Make `PaneScroll.index` a node index

The viewport would need no restoring at all. Rejected: `PaneScroll` is
shared by three panes (spec 0244 S1) and the two side panes index lists
that have no nodes. It would also move the per-frame `visible_row_of_line`
descent from the splice path, where it happens once a batch, onto the
draw path, where it happens every frame.

### Suppress the caret clamp during a bake

Tried on paper as a way to stop the clamp fighting the anchor, and
unnecessary: with S3's ordering the clamp is already a no-op whenever the
anchor did its job. Adding a flag would also have to answer what a
confirm does, and the answer is that a confirm is a splice batch like any
other (G2).

## Test plan

1. `a_bake_above_the_viewport_holds_the_rows` — the measurement in the
   Background, as an assertion: the row drawn at the top of the pane is
   the same row before and after a bake step that lands above it, and the
   caret keeps its terminal row.
2. `an_anchor_on_a_footer_survives_its_body_arriving` — S2. Scroll so the
   pane's first row is a stop's closing brace, bake, and assert the same
   brace is still the first row. Fails without the from-the-end encoding
   by landing inside the new body.
3. `a_bake_below_the_viewport_moves_nothing` — the negative case, so the
   restore cannot pass by scrolling to a fixed place.
4. `a_confirm_holds_the_rows_too` — G2, through `splice_override` rather
   than `bake_step`.
5. `an_anchor_climbs_out_of_a_flattened_subtree` — S4, via an override
   that flattens the anchor's parent to `bytes`.
6. `a_baked_document_is_the_unbounded_document` and the spec 0257 suite
   must stay green: the anchor changes the viewport and nothing else.

## Measured outcome

On `bounded_repeated_message_fixture(3)`, with the viewport scrolled to
the last `Item` and a bake step landing above it — the Background's
measurement, re-taken in the one configuration where the defect can show
at all. (The Background's own numbers were taken with the pane at the top
of the document, where the anchor is row 0 and nothing above it can grow;
what slid there is the caret, and N2 says that is correct.)

| | before the bake step | after, without the anchor | after |
|---|---|---|---|
| document rows | 7 | 9 | 9 |
| cursor's document row | 5 | 7 | 7 |
| viewport top | 5 | 5 | 7 |
| **cursor's terminal row** | **0** | **2** | **0** |

The viewport moved by exactly the two rows the bake inserted above it,
and everything on screen stayed where it was drawn.

### A pre-existing defect the tests found

`splice_override` repaired the caret's `cursor_line_in_node` — stale for
the same reason a footer anchor is, the node it closes having just grown
— *after* calling `finalize_override_batch`. But the finalizer reads that
coordinate, through `clamp_pan_offset`, to decide whether the caret needs
scrolling into view, so it read one pointing into the body of the node
the brace closes and scrolled the pane by the difference. The repair now
precedes the finalize call. Covered by
`a_caret_on_a_brace_does_not_drag_the_viewport`, which is the only test
in the suite that fails if the two are swapped back.

### Mutation testing

Four mutations, each killed by a different test, all others green:

| mutation | killed by |
|---|---|
| delete `restore_scroll_anchor()` | all four anchor tests |
| `AnchorLine::Footer` resolves as `FromStart(0)` | `an_anchor_on_a_footer_survives_its_body_arriving` |
| never climb out of an unrendered node | `an_anchor_climbs_out_of_a_flattened_subtree` |
| repair the caret after the finalize, as before | `a_caret_on_a_brace_does_not_drag_the_viewport` |

The first round of this found two of the tests passing for the wrong
reason: with the caret on the anchored row, `clamp_scroll_to_cursor`
restores the viewport by itself and the anchor is never exercised. Both
now park the caret on the root's header — the `Alt`-pan state, a row no
splice below it can move — so only the anchor can hold the viewport.
