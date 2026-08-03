<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0229 — a fold moves the cursor onto what it folded

Status: implemented
Implemented in: 2026-08-02
App: protolens
Refs: docs/specs/0142-protolens-cursor-on-closing-brace.md (G6.2 — the
        narrower cursor rule this replaces, and `cursor_line_in_node`);
        docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md
        (S8 — the same "hold what the user is looking at still"
        obligation, for the `w` toggle)

## Background

Folding or unfolding a node left the cursor wherever it was, except in
the one case where folding would have hidden the line the cursor sat on
(spec 0142 G6.2). Two consequences, both reported from use:

- After collapsing a subtree with the mouse, the next keystroke acted on
  a node elsewhere in the document — often one now scrolled off screen.
- Clicking a fold marker deliberately did *not* move the highlight (a
  2026-07-18 decision, recorded in `clicking_the_fold_marker_focuses_
  the_main_pane_without_moving_the_cursor`). The marker was treated as a
  pure display control. It is not: it changes the shape of the document
  at one specific node.

## Goals

- **G1.** Whatever gesture reaches `toggle_fold`, the cursor ends on the
  node whose shape just changed.
- **G2.** That node stays on the terminal row it was already on.

## Non-goals

- **N1.** No new pinning machinery. G2 is a property the existing row
  arithmetic already has (S2); if it needed code it would need a cache
  of screen positions, which is the thing spec 0210 removed.
- **N2.** Nothing about *which* node a gesture names. A click on a fold
  marker already identifies one; `h`/`zc` already identify one. This
  spec only says what happens to the cursor afterwards.

## Specification

- **S1.** `App::toggle_fold(idx)` sets the cursor to `idx`. When `idx`
  is already the cursor it resets `cursor_line_in_node` to 0 instead,
  because `set_cursor` bumps `cursor_moves` and a fold in place is not a
  cursor movement for anything watching that counter. The reachable ways
  in — a
  fold-marker click, `h` at a node's `Home`, `zc`/`zo` — are all
  gestures aimed at `idx`.

  This subsumes spec 0142 G6.2, which moved the cursor only when folding
  would have stranded it on a line about to be hidden. `idx` is the
  nearest still-visible ancestor of every such line, so the same move
  happens, for a reason that also covers the rest.
  `is_strict_descendant`, whose only caller was that narrower test, is
  deleted.

- **S2.** Nothing pins the view, because nothing needs to. A node's
  displayed row is a count of the visible rows *before* it, and folding
  `idx` only ever hides lines after `idx`'s header. So `idx` is drawn on
  the row it was already on, and `folds_changed`'s scroll clamp finds
  the new cursor already in view and leaves `scroll_offset` alone.

  This is a fact about `visible_row_of_line(absolute_start(idx))`, not
  an accident of the current clamp: it holds for unfold as well as fold,
  and for a cursor arriving from anywhere in the document.

## Alternatives considered

### Leave the cursor and pin the view instead

Record the toggled node's screen row, then restore it after the fold.
Rejected on both halves: G2 is already true without it (S2), and the
cursor half is the part the user was actually asking for — a fold with
the highlight left behind means the next keystroke acts somewhere
invisible.

### Keep the marker click a pure display control

The 2026-07-18 position. It was wrong in the same way for the marker as
for the keyboard, and the fix is the same one: whatever gesture reaches
`toggle_fold`, the node whose shape changed is where the user is looking.

## Test plan

1. `clicking_the_fold_marker_focuses_the_main_pane_and_selects_the_node`
   — the renamed and inverted 2026-07-18 test: keyboard focus still
   moves to the main pane, and now the cursor lands on the folded node.
2. The existing spec 0142 fold tests — unchanged, since S1 produces the
   same move they already assert in the stranded-cursor case.

## Measured outcome

No measurement; a behavior change with no new work per frame. `set_cursor`
is one arena lookup and a caret clamp.
