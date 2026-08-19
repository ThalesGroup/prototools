<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0328 — the node you are in has an edge

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0318-a-preview-ends-where-a-record-ends.md (the
        preview's bar, its column and its two colors — this generalizes
        it),
      docs/specs/0193-the-fold-marker-lives-in-a-gutter.md
        (`fold_margin` and `marker_column`, the one rule for the fold
        column),
      docs/specs/0260-a-fold-nobody-has-read-says-so.md (the five-color
        scale the current-node bar borrows its color from),
      docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (the
        wire row, which builds its own left margin and is why both bars
        break),
      docs/specs/0194-the-cursor-is-a-caret.md (the brace-pair
        highlight, which answers a different question and stays)

## Background

Three things are missing from the fold column, and they are one change
because they are all the same column.

**The current node has no extent on screen.** The caret sits on one row.
Which rows belong to the node that row is part of — where its body
starts and where it closes — is answerable only by counting
indentation, and on a deep document with the header scrolled off it is
not answerable at all. Spec 0194 gave the cursor node's braces a
highlight, which says where the two ends are when both are on screen and
nothing when they are not.

**A preview's bar breaks whenever bytes are shown.** Spec 0318 S7 draws
`│` down the fold column of every overlay row, and `w`/`W` (spec 0268)
insert a hex row under each of those rows. The hex row is built by
`wire.rs`'s own `margin` helper, which emits `FOLD_FIELD_WIDTH + indent`
spaces and never goes through `margin_spans` — so the bar is drawn on
every other terminal row and reads as a dotted line that means nothing.

**A truncated preview does not say where it stops.** The bar's color
says *that* something was withheld (spec 0318 S5's violet), but the
rendering below it ends at a closing brace like any other, and the
reader has to look back up at a one-cell hue to know the brace is not
the node's.

## Goals

- **G1.** The current node draws a continuous bar down the fold column,
  from directly under its own fold triangle to its closing brace, in the
  triangle's own color.
- **G2.** Both bars are continuous across wire rows.
- **G3.** A preview that was cut ends its last content row with `...` in
  the same violet the bar wears.

## Non-goals

- **N1.** *No new column and no width change.* Every mark here goes in
  the fold column `fold_margin` already reserves, at the column
  `marker_column` already computes. Nothing about the geometry moves,
  which is what keeps the heat cue, the fold hit test (`mouse.rs`) and
  the caret arithmetic untouched.
- **N2.** *The preview bar's glyph does not change.* A dashed variant
  (`┆` U+2506, `╎` U+254E, `┊` U+250A) was considered to tell the two
  bars apart and rejected: they are never on screen in the same column
  at the same time — an overlay covers its node's committed rows — so
  the distinction would cost a glyph to buy nothing. The two bars are
  told apart by color and by which rows they run down.
- **N3.** *The current-node bar is not a selection and not a fold
  control.* It is not clickable, it is not in the mouse hit test, and it
  does not change what `z`, `t` or `w` act on. It is a readout of where
  the caret already is.
- **N4.** *Spec 0194's brace-pair highlight stays.* It answers a
  different question — *which* brace matches — and it answers it in the
  text, not in the margin.

## Specification

- **S1.** The current node's bar occupies the same column its own fold
  triangle does: `marker_column` of its header row, which is what
  `PreviewOverlay.tier_column` already stores for the preview. One rule,
  so the bar cannot drift from the triangle it hangs under.

- **S2.** Its rows are the node's subtree, minus its header:
  `absolute_start(cursor) + 1 .. absolute_start(cursor) + lines_total`.
  That is the range `cursor_brace_pair` already derives, and
  `lines_total` is the whole subtree's line count, so the bar reaches the
  closing brace without a per-row ancestor walk. The header keeps its
  triangle, exactly as overlay row 0 does.

  Consequences that need no code: a **folded** node draws its body as
  the one-row `{ ... }` collapse, so the range is empty and there is no
  bar — which is right, since a collapsed node's extent is the row you
  are on. A **leaf** has `lines_total == 1` and likewise gets none.

- **S3.** Its color is `margin_glyph_color(Some(cursor))` — the same
  call the triangle's own color comes from, so the bar is the triangle's
  color by construction rather than by a second lookup that could
  disagree.

- **S4.** **The row's own mark wins the cell.** A bar is drawn only where
  the row's margin is blank at that column. Under `--indent 2` the two
  never meet: a child's marker is at least two columns deeper. Under
  `--indent 0`/`1` `marker_column` floors at 0 and a child's marker
  lands in the same cell, and there the child's triangle — a control,
  and the row's own — outranks an ancestor's readout.

  This is the one property the preview bar has for free and the
  current-node bar cannot borrow: an overlay row has no owner, so it has
  no marker at any depth (0318 S7). Testing the margin string for a
  space is one byte compare and covers every `--indent`.

- **S5.** A wire row takes its left margin from the same function its
  document row does. `wire_row`/`preview_wire_row` are given the margin
  spans rather than an `indent` to pad with; the blank they built
  themselves is exactly `fold_margin(indent, None)`, so this is a
  substitution and not a new layout.

  The bar is drawn on a wire row, the triangle is not: a wire row is a
  continuation of the row above, and a second triangle in the column
  would read as a second foldable node.

- **S6.** A preview whose `tier` is not `Whole` appends `...` to its last
  **content** row — the row above the node's closing brace, or the only
  row when the preview is flat — in `theme::preview_bar_color`'s violet,
  the bar's own color. The row is decided once when the overlay is built
  and kept as a field, so the renderer compares an index.

  Above the brace rather than after it: `} ...` says something follows
  the node, and what was withheld is inside it.

- **S7.** The `...` is a **display insertion**, `spans_with_insertions`
  at the row's end, the same mechanism spec 0193's `" ... }"` collapse
  summary uses. Spec 0318 N4's rule that every row must be grammatical
  prototext is about `window_text`, which is the raw line the highlighter
  parses; insertions are applied downstream of it and the parser never
  sees them. `row_text_of` does carry the collapse summary, because the
  caret walks over it — the ellipsis is not caret-addressable and is not
  added there.

## Alternatives considered

**A dashed bar for the preview.** See N2. It was drawn and the two bars
turned out never to be visible in one column, so the dash distinguished
nothing.

**Deriving the current node's rows by walking each row's owner up to
`cursor`.** O(depth) per row of the viewport and a second definition of
"in this node" beside `lines_total`. The line range is O(1) and is the
one `restore_scroll_anchor` and `cursor_brace_pair` already use.

**Giving the bar its own column.** A column of text on every row for a
mark that is one cell wide, and it would put the bar somewhere other
than under the triangle it belongs to.

**Skipping the wire row rather than drawing through it.** That is
today's behavior and is the defect.

**Putting the `...` in `window_text`.** It would then be parsed, and
tree-sitter's error recovery in this grammar swallows following siblings
— a whole preview turning one color. This is exactly why the collapse
summary is an insertion.

## Test plan

1. `the_current_node_wears_a_bar` — caret inside a multi-row node: every
   row of its subtree below the header draws `TIER_BAR_GLYPH` at the
   header's `marker_column`, in the header triangle's color; the header
   keeps its triangle; the row after the closing brace has none.
2. `a_folded_node_has_no_bar` — `z` on the same node leaves one row and
   no bar, and a leaf likewise.
3. `a_child_marker_outranks_the_bar` — at `--indent 1`, a child row with
   its own triangle draws the triangle, not the bar.
4. `a_bar_survives_a_wire_row` — with `W` over the node, the terminal
   rows alternate document and hex and *every* one of them carries the
   bar. Same assertion over a preview overlay.
5. `a_cut_preview_ends_in_an_ellipsis` — a preview with
   `tier != Whole` draws `...` on the row above its closing brace in the
   bar's violet; a `Whole` one draws none; and `window_text` for that
   row is unchanged, which is what keeps the parser out of it.

## Measured outcome

Implemented as specified; five new tests, all of which passed on their
first run against the finished renderer.

Two things the writing anticipated and the code confirmed:

- **S5 was a substitution, not a layout.** Handing `wire_row` and
  `preview_wire_row` the margin spans changed no column: the blank they
  used to build themselves is `fold_margin(indent, None)` byte for byte,
  and every existing wire test passed unedited.
- **S4's collision could not be provoked at the default indent.**
  `a_child_marker_outranks_the_bar` has to re-indent its fixture to
  `--indent 1` before an ancestor's bar and a child's triangle ever want
  the same cell, which is the measurement behind the sentence "under
  `--indent 2` the two never meet".
