<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0330 — each order keeps your place

Status: implemented
Implemented in: 2026-08-19
App: protolens
Refs: docs/specs/0114-protolens-range-type-override.md (the override
        pane, its two sort modes and the `i` that toggles them),
      docs/specs/0137-protolens-override-primitive-and-enum-candidates.md
        (the alphabetic mode's row 0, the `None` sentinel),
      docs/specs/0185-the-preview-is-an-overlay.md (the live preview the
        caret drives),
      docs/specs/0305-a-loading-list-is-not-an-empty-list.md (the cold
        cache, and the pending placeholder a toggle can land on)

## Background

The override pane has two orders over the same question — alphabetic and
by inferred score — and `i` switches between them. Both of the things
that make the pane usable stop working across that switch.

**The preview goes stale.** Every other way the caret moves calls
`preview_override_highlight`, and the main pane redraws as the candidate
under the caret. `i` does not: it sets `override_sort` and calls
`recompute_override_candidates`, and the overlay left on screen is the
one built for the *previous* mode's candidate. The reader is looking at a
preview of a type the pane is no longer pointing at, with nothing saying
so.

**The place is lost.** `recompute_override_candidates` resets
`override_highlight` to 0 unconditionally. That is right when the pane
opens, and wrong on a toggle: a reader who has scrolled forty rows into
the alphabetic list, glanced at the inferred ranking and pressed `i`
again is back at row 0 with forty rows to walk again. The two orders are
two views of one question and the reader is moving between them, not
starting over in each.

## Goals

- **G1.** Toggling the order previews the candidate the caret lands on,
  at once.
- **G2.** Each order remembers where its caret was, so returning to it
  returns to that row.

## Non-goals

- **N1.** *The caret is not mapped between the orders.* Landing in the
  inferred list on the same *type* the alphabetic caret was on would be a
  third behavior, and it is not what either list is for: the reader
  toggles to ask "what does the ranking say", which is a question about
  row 0, not about the type they came from. Two independent positions,
  each remembered.
- **N2.** *Opening the pane still starts at row 0 in both orders.* The
  memory is per open-pane session, not per document. `t` on a new node
  asks a new question.
- **N3.** *No change to what the two lists contain, or to their order.*
  This is about the caret and the preview only.
- **N4.** *The cold-cache paths are untouched.* Spec 0305's pending
  placeholder, the fall-back to alphabetic when the inferred list resolves
  empty, and `poll_pending_override_work`'s own preview refresh all keep
  their current behavior; a toggle onto a list that is not there yet has
  nothing to point at and nothing to preview, which is already what the
  placeholder says.

## Specification

- **S1.** The pane keeps one remembered row per order. On `i`: store the
  current highlight under the order being left, switch, recompute, then
  restore the highlight remembered for the order being entered — clamped
  to the new list's length, since the inferred list can have grown or
  shrunk between two visits.

- **S2.** `recompute_override_candidates` keeps resetting the highlight to
  0. It is called on open, on the alphabetic fall-back and on the toggle,
  and only the toggle wants a different row; the restore in S1 therefore
  happens in the `i` handler, *after* the recompute, rather than by
  giving the recompute a mode to branch on. One caller with the exception
  beats a parameter every caller has to answer.

- **S3.** Both remembered rows are cleared when the pane opens, which is
  N2 stated as code, and they live beside the pane's other per-session
  state (`override_scroll`, `override_pan_offset`) so that
  `close_override` disposes of them the same way.

- **S4.** The `i` handler ends with `preview_override_highlight`, the same
  call `move_override_highlight` ends with. A toggle is a caret move and
  is treated as one.

  When the restored row is the row the caret was already on — toggling
  twice with no movement between — the call still runs, and it still
  rebuilds the overlay for the same candidate. Rebuilding it is one
  `render_node_as` on one node and the alternative is a staleness check
  that has to know what the overlay was built from.

- **S5.** Row 0 of the alphabetic list is the `None` sentinel (0137 G1)
  and previews the node as raw, which is a real answer and the one the
  pane opens on. Nothing special-cases it here.

## Alternatives considered

**Seeking the same type in the other order.** N1. There is machinery for
it already — `seek_override_highlight`, used to reopen the pane on an
existing entry — so it would be cheap to build. It answers a question
the toggle is not asking, and it fails whenever the type is absent from
the other list, which is the normal case in one direction: the inferred
list is a ranking of message types and the alphabetic list also carries
the primitives and the `None` sentinel.

**One remembered row shared by both orders.** Half the state and the
wrong semantics: row 40 of a 600-entry alphabetic list and row 40 of a
12-entry inferred one have nothing to do with each other, and the clamp
would silently make the pair asymmetric.

**Clearing the overlay on toggle instead of rebuilding it.** The main
pane would flick back to the committed rendering and then to the new
preview on the next keypress — a blank frame in the middle of a
comparison, which is exactly when the reader is looking at it.

## Test plan

1. `toggling_the_order_previews_at_once` — with a candidate highlighted
   in one order, `i` leaves an overlay built for the *new* order's row 0.
   Compare the overlay's lines against `render_node_as` for that
   candidate, not against a copy of the expected text.
2. `each_order_remembers_its_row` — move to row `n` in alphabetic, `i`,
   move to row `m` in inferred, `i`, and the caret is back on `n`; one
   more `i` and it is back on `m`.
3. `a_remembered_row_is_clamped` — a remembered row past the end of a
   list that has since shortened lands on the last row, not out of
   bounds.
4. `opening_the_pane_forgets_both_rows` — close and reopen on another
   node: both orders start at row 0.
5. The existing `override_i_toggles_the_sort_mode` is unchanged and must
   keep passing; it pins the mode flip itself, which nothing here moves.

## Measured outcome

Implemented as specified: one `[usize; 2]` beside the pane's other
per-session state, indexed by the sort mode, with the restore in the `i`
handler after the recompute exactly as S2 argued for. Four new tests, and
`override_i_toggles_the_sort_mode` unchanged.

What the tests cost was the fixture, not the feature. A pane with *two*
usable orders needs the inferred list to be non-empty, and spec 0305's
two guards make that harder to arrange than it looks: the inferred order
is only taken when the sort mode asks for it **and** the heat cache has a
non-empty hit, and `by_range` counts an entry as a hit only when its
`top_n` covers the window the pane is about to draw. So the fixture has
to set `override_list_height` and seed `top_n` with at least that many
entries; seeded with fewer, the pane silently falls back to alphabetic
and every assertion about "the other order" compares a list with itself.
