<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0337 — the scale learns its top

Status: implemented
Implemented in: 2026-08-20
App: protolens
Refs: docs/specs/0336-the-square-is-the-scale.md (the continuous `t`
        this spec supplies, the three hues, and the negative-tail
        question it deferred here),
      docs/specs/0138-protolens-main-pane-inference-heat-cue.md (the
        Fibonacci `heat_level` this replaces),
      docs/specs/0335-a-question-not-asked-is-not-an-answer.md (the
        amber sentinel, which is outside the scale and stays outside),
      docs/specs/0287-the-chrome-beside-a-row-says-what-it-is.md (the
        hover box, which is where the anchor is disclosed),
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        (`prefetch_step`, which
        is what walks the whole arena and so what populates the
        histogram off-screen)

## Background

`heat_level` buckets a score onto twelve stops with a fixed Fibonacci
ladder: `[1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144]`, everything above 144
saturating at level 12. The ladder is absolute, and scores are not: they
scale with the size of the subtree scored.

On `googleapis.desc` the root scores 2 829 366, a `file` node 35, a
`message_type` 13, a tie at 5. Anything of any size is pinned at the
top; the values that actually distinguish one finding from another live
in the bottom four rungs, and the top eight are spent on a range the
document mostly does not occupy. On `bobshark` and `boblog` the whole
document lives in the bottom half and the top rungs are never reached at
all.

One absolute ladder cannot serve a document whose scores run to millions
and one whose scores run to tens. What the reader wants from brightness
is *large for this document*, and no constant knows what this document
is.

## Goals

- **G1.** Brightness is continuous in the score and logarithmic, so a
  factor-of-ten difference reads the same wherever on the range it
  falls.
- **G2.** The top of the range is discovered from the document, in the
  background, over the whole arena — not from the screenful in view, and
  not from a constant.
- **G3.** A square's brightness never oscillates. It may settle in one
  direction as the document is understood; it may not flicker.
- **G4.** Before calibration has anything to say, the scale is today's.

## Non-goals

- **N1.** *A manual override.* No `:heat-scale` command, no flag. The
  automatic scale is the thing to live with first, and a manual escape
  hatch shipped alongside it would be the thing that gets used instead
  of the thing that gets fixed. Revisit once there is experience.

- **N2.** *Histogram equalization proper.* Mapping the score through its
  own CDF is the information-theoretic optimum and is rejected: it is
  non-linear in the score, so two nodes a hair apart in a dense region
  get visibly different squares while two far apart in a sparse region
  get the same one. The reader loses "brighter means bigger" as a
  reliable reading and gains a precision that is not there. The
  percentile anchor below gets most of the entropy with a map that can
  be stated in one sentence.

- **N3.** *A per-document persisted scale.* The calibration is session
  state, discarded on exit, like every other way of looking.

- **N4.** *Resetting the anchor when a subtree is retyped.* An override
  invalidates the scores under it (`override_apply` clears their
  `HeatState`) and the new ones re-enter the histogram. The anchor is
  not rolled back: it only rises (S4), and a reset would re-brighten the
  whole column for a change that concerns one subtree.

- **N5.** *Correcting the double-count.* A node retyped and rescored
  contributes twice. It is a percentile over tens of thousands of
  samples; one extra sample does not move it, and the bookkeeping to
  avoid it would be per-node state on the hot path.

## Specification

- **S1.** `heat_level(score) -> u8` becomes `heat_fraction(score, anchor)
  -> f32`, returning the `t` spec 0336's `heat_color` takes:

  ```
  t = clamp(ln(score) / anchor, 0.0, 1.0)      for score >= 1
  t = 0.0                                       otherwise
  ```

  The low end is **pinned** at score 1, where `ln` is 0, and this is the
  whole of G3's mechanism: with only one end free, a square's brightness
  can move in one direction only. Both ends free and a square brightens
  and dims as unrelated nodes are scored, which is flicker carrying no
  information.

  Score 1 is the natural floor — the smallest positive score — and
  everything at or below it, negatives included, is `t = 0` and so is
  drawn at `L(0)` or, on ANSI-16, not drawn at all.

- **S2.** The anchor is discovered from a histogram of `ln(best_score)`
  over the nodes that **draw a graded square** — mismatches and ties.
  Not over all arena nodes: the population is the values the scale is
  applied to. That the root of `googleapis.desc` contributes nothing is
  a consequence of it being settled rather than a special case for the
  root, and the distinction matters — a root that *were* a mismatch
  would count like any other node.

  Sixty-four buckets of 0.25 nats over `[0, 16]`, which covers score 1
  to 8.9 million; anything beyond falls in the top bucket. 256 bytes,
  one increment per node settled.

- **S3.** **One writer.** `heat_states[idx] = state` has three
  production sites today — two in `heat_cue_resolve`, one in
  `prefetch_step` — and the histogram must see all three or it sees only
  what has been on screen. They collapse into
  `App::record_heat_state(idx, state)`, which writes the slot and, on a
  transition into `settled()` with a graded square, increments the
  bucket.

  This is what makes G2's "whole arena" true rather than aspirational:
  `prefetch_step` is what walks the arena off-screen, and it is one of
  the three.

- **S4.** The anchor is the **95th percentile** of the histogram,
  **ratcheted upward only**.

  A percentile, unlike a maximum, can fall as samples arrive; a falling
  anchor brightens every square at once. The ratchet keeps G3: early in
  a sweep the anchor is low and more squares clip bright, and it recedes
  as evidence lands. That is the benign direction — too dim early would
  hide findings during exactly the phase the reader is scanning.

  Clipping is the design and not a failure mode. At most 5% of squares
  sit at `t = 1` by construction, and a document's largest few finding
  being indistinguishable from each other costs nothing: they are all
  "at least this big", which is what the reader needs.

  The percentile is not consulted below 64 samples. With one sample the
  95th percentile is that sample, and a single large score would ratchet
  the anchor to it permanently.

- **S5.** The anchor starts at `ln(144) ≈ 4.97` — the top of the
  Fibonacci ladder it replaces. Before any calibration a small document
  therefore renders exactly as it does today, clipping at 144, which is
  G4; and a large one converges away from it. The default is drawn from
  the small documents on purpose: a small document is where calibration
  takes longest to have anything to say, so its uncalibrated rendering
  has to be right on its own.

- **S6.** The square's hover box states the anchor, in the score units
  the row shows: one line, "brightest at score N and above", where N is
  `round(exp(anchor))`. A document-relative brightness cannot be read
  without knowing what it is relative to, and a reader seeing three
  squares at maximum must be able to learn that they are three "at least
  N", not three equals.

- **S7.** The histogram answers spec 0336 N3's deferred question — is
  the negative tail populated, and by what. It is not a separate
  measurement and needs no throwaway harness: open each of
  `googleapis.desc`, `boblog` and `bobshark`, let the prefetch walk
  finish, and read the anchor off the hover box. If the negative tail
  turns out to matter, a hue for it is a further spec.

## Alternatives considered

**Anchoring on the root's score**, `t = ln(s) / ln(s_root)`. One number,
no histogram, and wrong on the document that motivates the spec: on
`googleapis.desc` the root is 2 829 366 (ln 14.9) while the cue-bearing
scores are 5 to 35 (ln 1.6 to 3.6), so every square in the document
would land between `t = 0.11` and `t = 0.24` and the top three quarters
of the ramp would go unused. The root sets a ceiling it is not itself
under.

**Anchoring on the maximum rather than a percentile.** Monotone for
free, no ratchet needed, and it hands the whole scale to one outlier.
The maximum over the graded population is by definition the least
typical value in it.

**Calibrating on the visible window.** Stable only while nothing
scrolls. The anchor would change with the viewport and the same node
would be a different brightness on two screens, which destroys the one
reading the square has.

**Freezing the squares until a sweep completes.** Removes the moving
anchor by removing the cue during the whole period the reader is
exploring. The sweep on `googleapis.desc` is long and the exploration
happens inside it.

**Keeping a bucketed ladder, recalibrated.** The twelve stops were a
consequence of the twelve-entry tables spec 0336 deleted. With a
generated color there is no reason to quantize before the terminal
does, and the ANSI-16 path quantizes anyway.

## Test plan

1. `the_fraction_is_logarithmic` — `heat_fraction` at a fixed anchor:
   equal ratios of score give equal steps in `t`, `t` is 0 at and below
   score 1, and 1 at and above `exp(anchor)`.
2. `a_negative_score_is_the_floor` — S1's other end, including the
   `SCORE_FLOOR` sentinel, which must not produce a NaN.
3. `the_anchor_only_rises` — feed the histogram a descending sequence
   and assert the anchor never decreases; then an ascending one and
   assert it does.
4. `a_thin_histogram_keeps_the_default` — S4's 64-sample guard: one
   enormous score does not move the anchor off `ln(144)`.
5. `an_uncalibrated_document_renders_as_it_did` — G4: with an empty
   histogram, the drawn colors for scores 1, 8, 55 and 200 are the ones
   the Fibonacci ladder gave, within the ramp's resolution.
6. `every_writer_feeds_the_histogram` — S3: a node settled through
   `prefetch_step` moves the histogram exactly as one settled through
   `heat_cue_resolve`. This is the test that says the calibration is
   over the arena and not over the screen.
7. `a_settled_node_does_not_feed_it` — S2's population rule: a
   `Settled { .. }` node, of either kind, leaves the histogram alone,
   the document root included.
8. `the_box_names_the_anchor` — S6, on the drawn hover box, with a
   known anchor seeded.

## Measured outcome

Filled in at implementation.
