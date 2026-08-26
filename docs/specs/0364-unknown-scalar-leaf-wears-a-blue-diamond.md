<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0364 — an unknown scalar leaf wears a blue diamond

Status: implemented
Implemented in: 2026-08-26
App: protolens
Refs: docs/specs/0322-a-leaf-can-be-wrong-too.md (N1 — which this
        supersedes — the `ANOMALY_GLYPH`, and the `margin_glyph_of`
        decision tree),
      docs/specs/0247-a-fold-toggle-carries-the-worst-news-below-it.md
        (S3 — `Status::Unknown`; S10 — the blue color; S12 — the
        whole-tree-blue problem that 0322 N1 was written to avoid),
      docs/specs/0349-shadowed-status-tier.md (S5 — hollow `◇` for
        `Shadowed`; the hollow/filled distinction),
      docs/specs/0287-the-chrome-beside-a-row-says-what-it-is.md
        (S5 — `DocElement::AnomalyMark` and its hover box)

## Background

Spec 0322 gave every scalar leaf whose status is `>= Status::NonCanonical`
a filled diamond `◆` in the fold column, colored amber or red. N1
declined to extend this to `Status::Unknown` (undeclared field in a typed
parent):

> Both are *absence of information*, not a defect, and both are
> near-universal in the situations that produce them: spec 0247 S12
> records that with no descriptor loaded the whole tree is `Unknown`, so
> an `Unknown` mark would put a diamond on every leaf of every untyped
> document and say nothing.

That argument is sound for documents opened without a descriptor. It does
not hold when the parent node *has* a schema: in that case `Unknown`
means the field number is not declared in the schema — it is out-of-band
data, invisible to protoc, and potentially significant (a credential
exfiltration vector, a version tag hidden from the schema, etc.). The
reader already gets the blue fold-toggle color rolled up from the
ancestor; the signal is present but not pinned at the leaf where the
finding lives.

The same motivation that drove 0322 applies here: panning moves the
annotation off screen and `a` removes it outright. The blue diamond pins
the signal to the fold column regardless.

## Goals

- **G1.** A scalar leaf whose status is `Unknown` *and* whose parent
  has a schema wears a blue `◆` in the fold column.
- **G2.** A scalar leaf in an untyped parent, or in a document with no
  descriptor, is unchanged: no mark.
- **G3.** The mark explains itself on hover, consistent with spec 0287
  and 0322 S5.

## Non-goals

- **N1.** A mark for `Unknown` when the parent has no schema. Spec 0247
  S12's objection stands: marking every leaf of every untyped document
  floods the column with a signal that carries no information.
- **N2.** Extending this to bracketed (non-scalar) nodes. A bracketed
  unknown node already has a blue fold toggle. This spec closes the gap
  for leaves only.
- **N3.** A new status rung. `Status::Unknown` exists and already
  receives the blue color on fold toggles. No ordering change is needed.
- **N4.** A hollow diamond for `Unknown`. `◇` is already taken by
  `Status::Shadowed` (spec 0349); reusing it would require color alone
  to distinguish two statuses, which is exactly the problem the
  hollow/filled distinction was introduced to avoid.
- **N5.** Double-marking a leaf that is simultaneously `Unknown` and
  `NonCanonical` or `Invalid`. The higher-severity rung already fires
  its own `◆`; that is the dominant signal.

## Specification

- **S1.** A predicate `parent_has_schema(idx: usize) -> bool` in
  `render.rs`. It returns `true` when the parent of `idx` exists and
  has a resolved type fqdn (i.e. the parent is typed). The root node
  returns `false`.

- **S2.** `margin_glyph_of` gains one new arm, inserted between the
  `>= Status::NonCanonical` arm and the catch-all:

  ```rust
  None => match self.status_of(idx) {
      s if s >= Status::NonCanonical => Some(ANOMALY_GLYPH),
      Status::Shadowed => Some(HOLLOW_ANOMALY_GLYPH),
      Status::Unknown if self.parent_has_schema(idx) => Some(ANOMALY_GLYPH),
      _ => None,
  },
  ```

  The glyph is `ANOMALY_GLYPH` (`◆`), colored via
  `theme::status_color(Status::Unknown)` — the same blue as the blue
  fold toggle. No new palette entry.

- **S3.** The hover box for `AnomalyMark` gains a branch for `Unknown`.
  It says: *this field is not declared in the parent's schema — it is
  invisible to protoc; the annotation at the end of the line says what
  wire type it carries*, and (like 0322 S5) notes that the annotation
  is present whether or not `a` is currently showing it.

  The existing `invalid: bool` field on `AnomalyMark` is widened to a
  `Status` (or a three-way enum) so the box can distinguish all three
  tiers without structural duplication.

- **S4.** The mark is inert to the mouse. `in_fold_field` already
  returns `false` when `!has_children`, so no guard is needed (same
  argument as 0322 S4).

## Alternatives considered

### A hollow diamond (`◇`) for `Unknown`

`◇` is taken by `Status::Shadowed` (spec 0349 S5). Reusing it would
force the reader to distinguish two statuses by color alone — the exact
problem the hollow/filled separation was built to avoid.

### Extend to bracketed unknown nodes

A bracketed unknown node already has a blue fold toggle (spec 0247 S10).
Adding a second mark in the same column duplicates the signal. Scalar
leaves are the gap.

### A new `UnknownInTypedContext` status rung

The parent-has-schema condition is a rendering predicate, not a semantic
one: the same field is `Unknown` whether or not the parent has a schema.
Pushing it into the text-based status classifier would couple the
classifier to the schema, entangling two layers that are currently
independent.

## Test plan

1. `an_unknown_scalar_in_a_typed_parent_wears_a_blue_diamond` — a
   fixture with a schema and one undeclared field; assert `ANOMALY_GLYPH`
   in the blue hue at the fold column of the leaf, and that its declared
   sibling has no mark. Repeat with annotations off: the mark remains.
2. `an_unknown_scalar_without_a_schema_wears_no_diamond` — same field
   number, untyped parent; assert no mark. The N1/G2 boundary.
3. `hovering_an_unknown_leaf_diamond_names_the_tier` — the hover box
   says the field is undeclared in the parent schema and notes the
   annotation. S3.
4. `a_click_on_an_unknown_leaf_diamond_places_the_caret` — inert to
   the mouse, folds nothing. S4.

## Measured outcome

Four changes, no new state:

- `popup_wire.rs`: `parent_is_typed` promoted to `pub(super)` (already
  existed; used by the wire-box S7 line).
- `render.rs`: one new arm in `margin_glyph_of`.
- `popup_doc.rs`: `AnomalyMark { tier: Tier }` → `AnomalyMark { status:
  Status }`, covering all four statuses that produce a mark; `Tier` was
  replaced by `Status` directly (no new enum needed — `Status` is the
  existing four-rung type). `anomaly_mark_hit` gains the `Unknown` arm.
  `doc_lines` gains the `Unknown` and `Shadowed` box arms (the latter
  was previously silent — `AnomalyMark` could never carry `Shadowed`
  before, since `anomaly_mark_hit` only emitted `NonCanonical` /
  `Invalid`).
- Tests: `an_unknown_leaf_wears_no_diamond` superseded by
  `an_unknown_scalar_in_a_typed_parent_wears_a_blue_diamond` (typed
  parent → mark fires) and
  `an_unknown_scalar_without_a_schema_wears_no_diamond` (untyped →
  no mark). One new popup_doc test:
  `hovering_an_unknown_leaf_diamond_names_the_tier`.
  Pre-existing test `a_defect_tints_the_fold_marker_of_every_node_
  above_it` updated to exclude `ANOMALY_GLYPH` from the tinted-glyph
  filter (the unknown leaf's new diamond was counted among the blue
  fold-toggle cells). All 1261 tests pass.
