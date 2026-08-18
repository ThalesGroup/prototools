<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0322 — a leaf can be wrong too

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0247-… (the five-rung `Status`, the colored fold
        toggle, and N1 — which this supersedes), docs/specs/0260-…
        (`Unbaked` inserted at rank 1, and the violet margin),
        docs/specs/0193-… (the two-column fold field and its geometry),
        docs/specs/0287-… (chrome explains itself on hover),
        docs/specs/0318-… (the preview's tier bar shares the column),
        docs/specs/0317-… (a packed record is one slot with many rows)

## Background

Spec 0247 colors a node's fold toggle with the worst status found
anywhere in its subtree, so a defect deep in a document announces itself
on every ancestor's triangle. A leaf has no toggle, and 0247 N1 declined
to give it a mark: *"a leaf's status is already the color of its own
annotation, so there is nothing left for a glyph to add."*

That justification only holds while the annotation is both drawn and on
screen, and it routinely is neither:

- **`a` removes it.** The key does not dim the annotation, it truncates
  the row at `annotation_start` (`render.rs`, `row_text_of` via
  `code_part`). With annotations off, a non-canonical leaf is
  indistinguishable from a clean one — the whole signal is gone.
- **Panning moves it off screen.** The annotation is the right-hand end
  of the row. The fold column is pinned to the left edge and never pans
  away; on a long row the reader is looking at the one part of the row
  that carries no status at all.

So the reader's own ancestors tell them a defect is somewhere below,
and then the row that has it looks exactly like its clean siblings.

## Goals

- **G1.** A leaf whose own status is an anomaly wears a mark in the
  fold column, colored by the same `theme::status_color` a toggle uses.
- **G2.** The mark cannot be mistaken for a fold toggle — neither by
  eye nor by hand: clicking it must not fold anything.
- **G3.** It explains itself on hover, like every other piece of chrome
  since spec 0287.

## Non-goals

- **N1.** A mark for `Unknown` or `Unbaked`. Both are *absence of
  information*, not a defect, and both are near-universal in the
  situations that produce them: spec 0247 S12 records that with no
  descriptor loaded the whole tree is `Unknown`, so an `Unknown` mark
  would put a diamond on every leaf of every untyped document and say
  nothing. `Unbaked` is transient by construction. The predicate is
  therefore `status >= Status::NonCanonical`, which is exactly the two
  tiers `crate::annotation::Tier` names.

- **N2.** A per-*row* status. The mark is per node, so a leaf slot that
  draws many rows — a packed record, spec 0317 — wears it on all of
  them. Fixing that would mean computing a status per visible row,
  which is precisely what spec 0247 S13 forbids ("one array index per
  visible row — no lookup, no walk, no allocation"). Repeating the mark
  down a packed run is also the honest rendering: the anomaly belongs to
  the record, not to one of its elements.

- **N3.** Marking a *bracketed* node's own status separately from its
  subtree's. A toggle already carries the roll-up, and splitting it into
  two marks in a one-column field is not available. For a leaf the two
  coincide — `rolled(n) = own(n).max(children…)` and a leaf has no
  children — which is why this spec needs no new state at all.

- **N4.** Anything in the wire row or the heat gutter. The wire row has
  its own vocabulary (spec 0225/0307) and the gutter is spoken for by
  the heat cue.

## Specification

- **S1.** `ANOMALY_GLYPH` is `◆` (U+25C6 BLACK DIAMOND). It is drawn in
  the fold column of any row whose owning node has no children and whose
  status is `>= Status::NonCanonical`, in the position `fold_margin`
  would have put a triangle.

- **S2.** `margin_glyph_of(owner)` is the one function that decides what
  goes in that column: the node's fold toggle when it has one, else the
  anomaly mark when the node warrants one, else nothing. Every renderer
  that reads the column goes through it. `fold_marker_of` stays, narrowed
  to the toggle alone, because `row_text_of` asks specifically whether
  the toggle is `FOLD_GLYPH_CLOSED` — the `{ ... }` collapse is a
  property of folding and not of the column.

- **S3.** The mark takes `theme::status_color(status_of(idx))`,
  unconditionally: unlike a toggle it is never drawn in the `Ok` color,
  because a node with an `Ok` status does not get one.

- **S4.** It is inert to the mouse. This needs no guard: `in_fold_field`
  — the single locator both `handle_click` and spec 0287's hover ask —
  already returns false when `!has_children`, so a click over the
  diamond places the caret exactly as a click over any other blank
  column does.

- **S5.** Hover: `DocElement::AnomalyMark { invalid }`, located by
  `anomaly_mark_hit`, which shares `in_fold_field`'s column geometry
  through a new `in_fold_column` and applies the complementary node
  test. The box says which tier and that it is the node's own, and — the
  one thing that cannot be discovered by looking — that the row's
  annotation says what is wrong, whether or not `a` is currently showing
  it.

- **S6.** In an override preview, `overlay_margin_spans`'s row 0 keeps
  whatever `margin_glyph_of` gives the previewed node, so a previewed
  leaf with an anomaly shows its diamond and the tier bar starts at row
  1, exactly as it does under a triangle. A previewed clean leaf is
  unchanged: no glyph, and the bar starts at the top.

## Alternatives considered

**`■` U+25A0 BLACK SQUARE**, as proposed. Three things against it. It is
semantically mute — a filled square is the universal *stop* mark, and
this is a warning. It is the heaviest mark that can occupy a cell, which
inverts the column's visual weight: a leaf anomaly would out-shout the
subtree roll-up on every ancestor above it. And a solid block is where
two adjacent hues are hardest to tell apart at a glance, which is
requirement (1). `◆` is the caution-sign silhouette, carries clearly less
ink than a toggle, and has an outline the eye reads a hue off.

**The other obvious candidates are unusable for mechanical reasons.**
The glyph must not carry the Emoji property — a terminal reaching for an
emoji font draws it in the font's own color, destroying the only thing
the mark exists for, and usually double-width besides. That rules out
`▪` U+25AA, `◼` U+25FC, `⬛` U+2B1B and `⏹` U+23F9. `⚠` U+26A0 is the
worst of the set: ideal semantics, emoji presentation. `◆` U+25C6 is
East Asian Ambiguous, which is the same width class as the heat cue's
`●` already at column 0 — a risk the app already carries in a gutter,
and one the narrow `◇`/`⋄` alternatives trade for illegibility.

**Dimming or re-coloring the leaf's whole row.** Rejected for spec 0247
S10's reason, restated: the margin is a column the reader can scan, and
a row-wide treatment competes with the syntax highlighting that carries
the row's meaning.

**Leaving 0247 N1 in force and instead pinning the annotation on screen
during a pan.** That is a much larger change to `pan_spans`, it would
cost the row's right-hand columns permanently, and it does nothing at
all for `a`.

## Test plan

1. `a_leaf_anomaly_wears_a_diamond` — a non-canonical leaf draws
   `ANOMALY_GLYPH` at the cell a toggle would occupy, in the
   non-canonical hue, and its clean sibling draws nothing there. Then
   again with `a` off, which is the case that motivates the spec: the
   annotation is verifiably gone from the row and the mark is not.
   S1, S3.
2. `an_unknown_leaf_wears_no_diamond` — the undeclared-field fixture,
   whose ancestors' toggles `a_defect_tints_the_fold_marker_of_every_
   node_above_it` already pins as tinted, has no marks at all. N1.
3. `a_click_on_a_leaf_diamond_places_the_caret` — `handle_click`
   reports the click unspent, folds nothing, and moves the caret. S4.
4. `hovering_a_leaf_diamond_names_the_tier` — the box's three lines,
   and that a clean leaf's fold column is not a target. S5.
5. `a_previewed_leaf_keeps_its_anomaly_mark` — one mark on overlay row
   0, in the status hue, with the tier bar contiguous below it. S6.

## Measured outcome

Behavioral; pinned by the tests above. The workspace suite passes with
and without `COLORTERM`, as do `cargo fmt --all --check`, `cargo clippy
--no-default-features --workspace -- -D warnings` and `reuse lint`.

Two things worth recording:

- **S4 cost nothing.** `in_fold_field`'s `!has_children` early return
  already made the whole column inert on a leaf, so the diamond was
  unclickable before it was drawn. The refactor for S5 splits that
  function's node test from its column geometry without changing either.
- **The roll-up needed no new state.** A leaf's `status_rolled` is its
  `status_own` by definition, so `status_of` was already the right
  answer and the change is confined to which glyph the renderer picks.
