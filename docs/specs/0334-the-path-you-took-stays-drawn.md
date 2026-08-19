<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0334 — the path you took stays drawn

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0328-the-node-you-are-in-has-an-edge.md (the bar this
        one repeats, its column, its extent, and the rule that the
        row's own mark wins the cell),
      docs/specs/0318-a-preview-says-how-much-it-is-showing.md (the
        glyph, and the overlay margin this spec does not touch),
      docs/specs/0247-a-node-says-how-it-is-doing.md (the status color
        the triangle wears, and so the bar)

## Background

Spec 0328 draws one bar: down the fold column of the node the caret is
in, from under its triangle to its closing brace. It answers *where does
this node end* and nothing else.

The question a reader asks next is *where am I* — which is not about one
node but about the path from the root. In a document indented forty
columns deep the enclosing nodes' headers are scrolled off the top, and
the only thing on screen that says how deep the caret is, is the
indentation itself, which says a number and not a lineage.

The fold column already has the answer sitting in it: every ancestor's
triangle is in a column of its own, strictly left of the caret node's,
because indentation is monotone. Drawing each ancestor's bar in its own
column turns the margin into the path.

## Goals

- **G1.** Every ancestor of the caret node draws a bar in the fold
  column, on the same terms spec 0328 gives the caret node's own: down
  that ancestor's column, over that ancestor's rows, in that ancestor's
  triangle's color.
- **G2.** The caret node's own bar stays the one that stands out. An
  ancestor's is dimmer.

## Non-goals

- **N1.** *A different glyph, a different column, or a wider margin.*
  All the bars are `│` in the fold field spec 0193 already reserves.
  Spec 0328 N1's reason holds unchanged: the column exists, it is one
  cell wide, and adding to it costs the document nothing.

- **N2.** *A bar on an overlay row.* An override preview's rows draw
  their own bar, in their own color, saying how much of the node is
  shown (spec 0318 S7). Two bars in one column cannot both be read, and
  the preview's is the one the reader is deciding by. The committed rows
  above and below the overlay keep theirs.

- **N3.** *Descendant bars, or a bar for a sibling.* The margin says
  where the caret is. A subtree's shape is what the document body is
  for, and drawing every open child's bar would fill the field on any
  deep document and say nothing about the caret at all.

- **N4.** *A per-depth color ramp.* An ancestor's bar takes its color
  from its own triangle (G1), which already carries meaning — the
  subtree's worst status. A second scale in the same cells would fight
  it.

## Specification

- **S1.** The bars are the caret node's, then its parent's, then its
  parent's, up to and including the root. Each is computed exactly as
  spec 0328 S1/S2 computes the caret node's: column is `marker_column`
  of the node's header line, rows are the node's subtree minus that
  header. A node that spec 0328 gives no bar — a leaf, a folded node —
  contributes none here either, by the same `lines_total < 2` test, and
  the walk continues past it. So a caret on a leaf still draws its
  ancestors' bars.

- **S2.** A bar's color is `margin_glyph_color` of *its own* node —
  the same call that node's triangle takes its color from, so the two
  agree by construction rather than by a second lookup (spec 0328 S3
  applied per bar). It is `None`, i.e. the default foreground, for an
  `Ok` node, which is most of them.

- **S3.** An ancestor's bar adds `Modifier::DIM`; the caret node's does
  not. DIM and not a blended color, because `margin_glyph_color` returns
  no color at all for an `Ok` node and a blend has nothing to blend —
  see Alternatives.

- **S4.** Spec 0328 S4's rule stands and gains a tie-break. A bar is
  drawn only into a cell this row's own margin left blank, so the row's
  own triangle still wins outright. Where two bars want the same cell —
  possible only at `--indent 0`/`1`, where `marker_column` floors and
  two levels share a column — **the nearer node's bar wins**, for the
  same reason the row's own mark does: the more specific statement is
  the more useful one.

- **S5.** A wire row takes the same bars as its document row, spec 0328
  S5 unchanged, including the caret node's header's own wire row.

## Alternatives considered

**A leveled or blended color instead of `Modifier::DIM`.** The color to
dim is usually absent: `margin_glyph_color` returns `None` for an `Ok`
node, and an ancestor of the caret is `Ok` more often than not. A blend
would therefore need a second rule for the uncolored case — the common
one — and would have to be resolved against the theme, which is where
`theme.rs` already reaches for `Modifier::DIM` twice for exactly this
situation. One modifier covers both cases with one line.

**Dim by depth, so a grandparent is dimmer than a parent.** Terminals
have one DIM, not a scale, so this would mean the leveled color above
plus its uncolored-case problem, to encode a depth the columns already
encode by their position.

**Only the parent, not the whole path.** The parent is already the
easiest ancestor to find without help — it is the nearest header above
the caret. The ones worth drawing are the far ones.

## Test plan

1. `the_current_node_wears_a_bar` — spec 0328's, re-pointed at the
   undimmed bars, which is now what "the caret node's own" means.
2. `a_folded_node_has_no_bar` — likewise: the caret node contributes
   none, while its ancestors still do.
3. `every_ancestor_wears_a_dimmer_bar` — with the caret two levels
   down, one bar column per ancestor, each strictly left of the caret
   node's, each running that ancestor's rows and wearing DIM while the
   caret node's does not.
4. `a_bar_on_a_leaf_comes_from_its_ancestors` — S1's tail: a caret on a
   leaf draws no bar of its own and every ancestor's.
5. `the_nearer_bar_wins_a_shared_column` — S4 at `--indent 1`, where
   two ancestors share a column: one bar there, undimmed if it is the
   caret node's.

## Measured outcome

Implemented as specified. `CursorBar` gained an owner and a dim flag,
the memo holds a `Vec` of them instead of an `Option`, and the two
callers — the document margin and the wire margin — now share one
`bars_on_row`, which is where both S4 filters live. No new state on
`App`, no new column, no change to `row_content`.

**One latent assumption in the tests broke, and it was arithmetic, not
rendering.** `row_content_and_row_spans_agree_byte_for_byte` located
each bar by its byte offset in the *drawn* row and indexed `row_content`
with it. That holds for one bar, because everything before it is ASCII
blanks; it stops holding for the second, because the glyph is three
bytes standing in for one. The test now subtracts two per bar already
passed. Its other assertion — `drawn.replace(bar, " ") == content` — was
right all along and is what says the substitution is still cell for
cell.

Five other tests measured "the bar" when there was only one to measure.
Four are re-pointed at the undimmed cells, which is now what "the caret
node's own" or "the preview's" means; `a_folded_node_has_no_bar` keeps
its meaning under that reading and gains
`a_bar_on_a_leaf_comes_from_its_ancestors` beside it for the other half.
Distinguishing bars *by their dim* rather than by their column is
deliberate: the column is the thing under test in three of them.
