<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0254 — a changed count travels up as a difference

Status: draft
App: protolens
Refs: docs/specs/0210-the-nodes-hold-sizes-not-positions.md (the two
        counts, and `refresh_line_counts` itself),
        docs/specs/0249-a-large-document-answers-the-user-first.md (S11,
        whose bake this blocks; the measurement below is its simulation),
        docs/specs/0193-a-folded-node-is-one-row.md (`lines_visible`'s
        fold rule, which the difference has to respect)

## Background

`refresh_line_counts(idx)` (`tui/lines.rs:142`) tells a node's ancestors
that something below them changed size. It does so by recomputing each
ancestor's counts **from that ancestor's children**, level by level, up
to the root. Its own doc comment states the cost: "O(depth + the fanout
of each node on the way up)".

The fanout is the problem. On googleapis.desc the root has 7 771
children, so any splice anywhere in the document re-adds 7 771 numbers
to discover that exactly one of them moved — and the sum it computes is,
by construction, the sum it already had plus that one difference.

It does not show up on a single override, where it is 0.10 s of a 0.53 s
bounded confirm. It shows up the moment there are many splices. Spec
0249 S11's bake is exactly that: its simulation on googleapis.desc
drains 419 723 auto-folds, and this function is **50–63% of the whole
bake**, at a flat ≈33 µs per splice that does not move when the row
budget does:

| bake row budget | splices | line counts | whole bake |
|---|---|---|---|
| 50 | 419 723 | **13.66 s** | 21.8 s |
| 500 | 209 153 | **7.57 s** | 12.6 s |
| 5000 | 70 797 | **3.91 s** | 7.9 s |

Halving the splices halves the column, which is the signature of a
per-splice constant, not of work proportional to the document.

## Goals

- **G1.** The walk above the changed node is O(depth), independent of
  how many children any ancestor has.
- **G2.** Byte-identical counts. This function decides where every row
  is, so the change has to be provably neutral, not approximately so.

## Non-goals

- **N1.** No change to what the counts mean, to `lines_visible`'s fold
  rule, or to any caller's signature. Spec 0210 owns the representation;
  this is only how a change propagates through it.
- **N2.** No caching, no dirty set, no deferral. The walk stays
  immediate and synchronous — it is O(depth) afterwards, which on
  googleapis is 13 steps.
- **N3.** The *starting* node is still recomputed from its children,
  which is O(its own fanout). It has to be: nobody has told this
  function what changed, only that something did. That is one node's
  fanout once, not one per level.

## Specification

- **S1. The first node is summed; every node above it is adjusted.**
  `refresh_line_counts(idx)` computes `idx`'s two counts from its
  children exactly as today, and takes the two *differences* between the
  new values and the old. It then walks `parent[]`, adding each
  difference to the ancestor's stored count rather than recomputing it.

- **S2. An ancestor's own difference is what continues upward**, and it
  is not always the one that arrived. Two rules make them differ, and
  both are today's behavior restated:

  - a **folded** ancestor shows one row whatever is beneath it (spec
    0193), so its `lines_visible` does not move and the visible
    difference above it is zero — while its `lines_total` still does;
  - a node that is **not bracketed** has no children and owns its rows
    outright, so nothing below it can move them and the walk stops.

- **S3. The walk stops when both differences are zero.** This replaces
  today's "the recomputed values equal the stored ones" early exit and
  is the same condition, computed instead of compared. It is what keeps
  a fold toggle cheap: above a folded ancestor the visible difference is
  already zero, and if the total did not move either, nothing above can
  have.

- **S4. The counts are `u32` and the differences are signed.** A subtree
  that shrinks carries a negative difference, so the arithmetic is done
  in `i64` and stored back after a checked conversion. A difference that
  would take a count below zero is a corrupted tree, not a case to
  handle: it panics in debug through the conversion, and the existing
  whole-document assertion (below) is what would have caught it first.

- **S5. Correctness is established by the assertion that already runs.**
  `assert_line_counts_are_exact` (`tui/override_apply.rs:689`) walks the
  whole document after **every** splice in the test suite and checks each
  node's counts against its children and against where the text actually
  puts its braces. It is deliberately hung off `finalize_override_batch`
  rather than written as one test, so every fixture in the suite —
  nested patches, packed runs, repeated overrides of one node, folded
  targets, auto-expanded `Any` descendants — is already a case for this
  change. **No new fixture is needed for the equivalence; what is needed
  is that the assertion stays exactly as strict.**

  This matters more than usual here, because S1 trades a
  self-correcting computation for a trusting one: today a stale count
  anywhere up the chain is silently repaired by the next refresh that
  passes through it, and afterwards it is not. The assertion is what
  makes that trade safe, and it is why this spec does not also try to
  skip the starting node's sum.

## Alternatives considered

### Pass the difference in from the caller

`splice_override` knows the node it just rewrote, so it could hand the
difference down and skip N3's starting sum too. Rejected: the callers
that do *not* know are the fold toggles, and they are the ones that run
on a keypress. It would also put the invariant in six places instead of
one, each free to compute the difference slightly differently, for a
saving that is one node's fanout — the part that is not quadratic.

### Store a parent pointer to a running total, or a Fenwick tree over the rows

Both make the ancestor update O(1) or O(log n) instead of O(depth), and
both replace an exact, locally checkable field with a derived structure
that has to be rebuilt when the arena's shape changes under a splice.
O(depth) is 13 on googleapis against a cap of 1000 (spec 0216). There is
no problem left to solve.

### Leave it, and give the bake a large row budget

The measurement says a budget of 5000 already brings the column down to
3.91 s. Rejected: it is still half the bake, the budget is chosen for
other reasons (spec 0249 S11 — a large one amortizes the re-emitted
right frontier), and the cost is per *splice*, so a multi-site
`FqdnField` override pays it with no budget involved at all.

## Test plan

1. The whole existing suite, unchanged — S5. Every splice in it runs
   `assert_line_counts_are_exact` over the whole document.
2. `a_shrinking_subtree_carries_a_negative_difference` — override a node
   to a type that renders fewer rows than it had, and assert the
   ancestors' counts and the document's total. The growing direction is
   covered everywhere; the shrinking one is where a `u32` difference
   would wrap.
3. `a_folded_ancestor_absorbs_the_visible_difference` — a node's subtree
   changes size under a folded ancestor: the ancestor's `lines_total`
   moves, its `lines_visible` stays 1, and no node above it changes
   `lines_visible` at all.
4. `refresh_line_counts_stops_at_an_unchanged_ancestor` — the early exit
   still fires, observed through the counts of a node above the stop
   being untouched.

## Measured outcome

Filled in at implementation. The number to beat is spec 0249 S11's
simulation: line counts must fall from 7.57 s to a rounding error at
budget 500, taking the whole bake from 12.6 s to ≈5.1 s, with the
document still byte-identical to the unbounded render.
