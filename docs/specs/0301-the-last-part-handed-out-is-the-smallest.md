<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0301 — the last part handed out is the smallest

Status: rejected — implemented, measured, reverted 2026-08-15
App: prototext-graph
Refs: docs/specs/0217-the-sweep-is-divided-among-the-cores.md (the
        partition and the shared cursor this re-shapes),
      docs/specs/0290-a-part-is-weighed-by-the-work-it-holds.md (the
        group-count key this keeps and now varies),
      docs/specs/0269-the-last-part-of-a-sweep-finishes-on-the-best-core.md
        (the straggler mechanism this complements)

## Background

The sweep's parts hold an equal number of state groups and are handed
out in index order from a shared cursor. Equal group counts do not
produce equal costs: on googleapis at 24 parts the measured range is
51-515 ms around a 157 ms mean, and the most expensive part is 3.3x the
mean.

That alone is survivable. What is not is *when* the expensive part is
picked up. Simulating the pull queue on measured per-part costs, an
order that happens to leave the 515 ms part until last gives a makespan
of 855 ms against a 515 ms lower bound — **1.65x**. A reverted
experiment (commit `452d953`, spec 0300, reverted in `b45561a`)
demonstrated the same effect on real startup: re-ordering the parts and
changing nothing else moved `-j 8` startup between 1.332 s and 1.031 s,
a 22.6% spread with identical total work.

Hand-out order is therefore worth more than any reduction in total work
attempted so far. The obvious remedy — estimate each part's cost and
run the expensive ones first — has failed twice:

- **No static graph statistic ranks parts.** Measured Spearman against
  time: closure states −0.08, closure edges +0.10, root count −0.06.
  The 515 ms part has dead-median values for all three.
- **The score counters the walk already returns do not either**
  (Spearman +0.41). `work/ms` spans 36x across parts, and the straggler
  has the *lowest* counted work per millisecond of all 24 — its cost is
  a large active set carried across many tokens, which those counters
  do not observe.

## Goals

- **G1.** Bound the end-of-sweep imbalance by the *smallest* part
  rather than the largest, without predicting any part's cost.
- **G2.** Add no work. The partition must not grow the total the sweep
  performs, and must not add a runtime measurement phase.
- **G3.** Change no result. The partition is an internal division of
  labor.
- **G4.** Establish a baseline that any future cost model must beat, so
  that the cost of building one can be judged against what is available
  without one.

## Non-goals

- **N1.** No cost model, no oracle, no runtime probe. Two have been
  built and measured and neither ranks parts (see Background). This
  spec deliberately uses the one signal already in hand.

- **N2.** No change to `sweep.rs`. The cursor hands parts out in index
  order, so emitting the parts largest-first *is* the scheduling
  change. A partition that must be paired with a scheduler change is a
  worse baseline, not a better one.

- **N3.** No attempt to shrink the intrinsic straggler. Grading stops a
  large part from *starting* late; it does not make it smaller. A part
  that is both expensive and large still bounds the makespan from
  below, and collecting that needs 0269's seat donation or a genuine
  cost model.

- **N4.** No FQDN clustering. Spec 0300 cut total work 15% by ordering
  groups by name, and was reverted because it made costs heterogeneous
  and then handed the straggler out last. It is orthogonal to this spec
  and may be revisited once ordering is fixed — but not in the same
  change, because the two were confounded once already.

## Specification

- **S1.** A part's quota is a number of state groups, and quotas
  decrease geometrically across the parts: quota `i` is proportional to
  `DECAY^i`, normalized so the quotas sum to the group count. Parts are
  emitted in decreasing-quota order.

- **S2.** Groups are dealt largest-first to the part with the most
  *unfilled quota* remaining. When every quota is equal this is
  identical to today's round-robin, so the few very large groups stay
  spread across parts instead of piling into part 0.

- **S3.** Every quota is at least one group, and the number of parts is
  unchanged (`SWEEP_PARTS`, 24). Holding the part count fixed keeps
  convergence duplication where it is, so this change is measured on
  scheduling alone.

- **S4.** `DECAY` is set so that the first part's quota is `SPREAD`
  times the last's. The evidence for the chosen value lives in a doc
  comment beside the constant.

- **S5.** The only signal is the group count. Nothing is measured,
  sampled or estimated at runtime.

## Why it works without a cost model

Group count is useless as a *predictor* — at 24 equal parts it is
constant while costs range 51-515 ms. It is a good *control on the
mean*. Measured mean part cost against groups per part:

| groups/part | 1390 | 695 | 348 | 174 | 87 | 43 | 22 |
|---|---:|---:|---:|---:|---:|---:|---:|
| mean cost (ms) | 277 | 186 | 118 | 73 | 44 | 28 | 16 |

Each halving multiplies the mean by ~0.62, so cost grows as roughly
`groups^0.7`. A part holding a quarter of the groups costs about 40% of
the mean. That is enough to shape the tail even though it says nothing
about any individual part.

The scheduling consequence is the whole point: under a pull queue the
residual imbalance is bounded by the size of the last part started. If
the parts started last are the small ones, that bound is small. The
large parts are placed while every worker is still idle, and the small
ones are the sand that fills the gaps.

Grading is also free on the duplication axis. Closure size is concave
in group count, so for a fixed part count the *equal* split is the one
that maximizes total closure. Uneven quotas can only reduce it.

## Alternatives considered

**Estimate each part's cost and run the expensive first.** The direct
remedy, and the one this spec exists to substitute for. Two estimators
were built and measured; neither ranks parts (Background). A sampled
estimator reproduces the score counters almost perfectly (Spearman
+0.99 from 0.5% of the blob) and still schedules at 1.38x the bound,
because it estimates a quantity that is not cost.

**Equal parts, one per worker.** Theoretically optimal and practically
the worst case: with `k = W` a single slow worker — noise, thermal, a
neighbor — puts the makespan over by its full margin with no way to
recover, because no part remains to redistribute.

**More parts of equal size.** Finer parts dilute the straggler but
multiply total work, because splitting the sweep duplicates any state
two parts both reach: measured 1969 ms undivided, 4469 ms at 24 parts,
12 132 ms at 768. The marginal cost of one added part is 95 ms at 24
parts. Grading buys tail granularity at a fixed part count instead, and
so pays none of this.

**Guided self-scheduling proper** (next chunk = remaining/W). Does not
terminate at a bounded part count — for 16 680 groups and 8 workers it
places 96% in 24 chunks and needs ~30 more to finish. Capping it puts
the remainder in the last part, which is precisely the large-part-last
shape being removed. The geometric schedule of S1 is GSS with the part
count as the controlled parameter instead of the outcome.

## Test plan

1. `a_partition_grades_its_parts` — quotas decrease, the first is
   `SPREAD` times the last, and they sum to the group count.
2. `a_partition_emits_the_largest_part_first` — the order the cursor
   consumes is decreasing, since that is the entire scheduling change.
3. `a_partition_never_splits_a_state_group` — unchanged. Still a
   partition, still keeps a group whole.
4. `a_sharded_sweep_matches_the_whole_sweep` — unchanged. G3.
5. The existing `sweep.rs` part-count assertions, unchanged.
6. Full workspace suite; `protolens … export /` over googleapis
   byte-identical.

## Measured outcome — the mechanism does not do what S1 claims

Implemented in full on 2026-08-15, four gates clean, `export /` over
googleapis byte-identical (5 278 322 lines). The code was correct and
**was not committed**; the implementation is preserved as a patch beside
this campaign's notes. `SPREAD = 1` reproduces today's partition exactly
(1.391 s against pre-0300's 1.393 s), which validates the control arm.

**Real `-j 8` startup**, `taskset -c 0-7`, paired rounds against the
`SPREAD = 1` control:

| SPREAD | campaign 1 (22 rounds) | campaign 2 (16 rounds) |
|---:|---|---|
| 1 (control) | 1.359 s | 1.302 s |
| 2 | — | **1.744 s (+34.0%)** |
| 3 | — | 1.332 s (+2.3%) |
| 4 | 1.277 s (−6.1%) | 1.237 s (−5.0%) |
| 6 | — | 1.437 s (+10.4%) |
| 8 | 1.314 s (−3.4%) | 1.330 s (+2.2%) |
| 16 | 1.407 s (+3.5%) | — |

Mean over spreads is −2% in one campaign and +9% in the other. That is
an expected value of about zero with a ±30% tail — shipping a constant
tuned to one blob's noise, which is the spec-0300 mistake in a new
costume.

**Why.** The quota shape does not tame the straggler; it only relocates
it, and the makespan is then decided by where it lands. Pull-queue
simulation over per-part costs, 24 parts, 8 workers:

| SPREAD | straggler index | makespan | vs bound |
|---:|---:|---:|---:|
| 1 | 10 | 694 ms | 1.30x |
| 2 | **23, last** | **914 ms** | 1.68x |
| 4 | 0 | 562 ms | 1.00x |
| 16 | 1 | 636 ms | 1.00x |

The landing is arbitrary by construction. The expensive groups are 1-3
roots each, so S2's largest-first dealing hands them out **last**, when
the quota remainders are in an uncontrolled state. The single decision
that sets the makespan is made by the least-controlled step in the
algorithm — so G1 is not met, and G4's baseline is a lottery ticket
rather than a floor.

Two effects the campaign *did* confirm, and which any successor should
inherit:

- The concavity argument holds. Total single-threaded cost **falls** as
  spread rises (4282 ms at spread 1 → 3972 ms at spread 16), so grading
  is free — indeed slightly profitable — on the duplication axis.
- `max_part` rises with spread (520 → 636 ms) until the straggler alone
  is the bound.

**Verdict: grading is a real gain once the straggler is placed, and a
lottery before it.** The mechanism is sound and the missing input is
placement, which S5 forbids and N1 rules out of scope.

## What replaced it, and why that is also not committed

The campaign continued past this spec and found the placement signal S5
forbids: **a runtime probe at state-group granularity**. Scoring every
one of the 16 680 groups against a record-aligned 0.2% prefix of the
blob costs 46 ms and ranks them at Spearman **+0.817** — against +0.211
for the best static signal ever measured here — and places
`google.protobuf.FileDescriptorSet`, the group whose position decides
the makespan, **3rd of 16 680**. The ranking is flat from 0.2% to 5% of
the blob, so the cheapest setting is also the best.

Granularity is the whole trick, and it has two axes that pull opposite
ways: coarse in the *blob*, fine in the *schema unit*. The part-level
version of the same probe schedules at 1.38x precisely because it
averages 695 bimodal groups into one number.

Re-run with the quota measured in *estimated work units* rather than
group counts, and groups dealt most-expensive-first, the same partition
reaches **1.00x of the oracle bound** (makespan 559 ms against 641 for
round-robin, −12.7%) and — the point — the champion lands at part index
3 for `SPREAD` 2, 4 and 8 alike. Placement stops depending on the
constant, because it happens in the first four deals before any quota
binds.

It is not committed either, for three reasons:

1. Every number above is one blob. No second large `FileDescriptorSet`
   was available to test transferability, which is the exact question
   that sank 0300 and this spec.
2. The estimate's **rank is good and its magnitude is garbage** — part 0
   believed it carried 49% of the work and measured 3.7%. It is usable
   as an order, not as a quantity, and the quota formulation uses it as
   a quantity.
3. A probe needs a payload, so `Partition` would stop being a pure
   function of the graph and the part count — that is spec 0262 S1,
   whose justification is 7.3 ms of rebuild against a 5.4-10.4 ms
   visible query. The workable shape is to probe once on the root
   payload at startup, where `ranked_with` already holds `pb` at the
   `partition_roots` call, but that is a spec of its own.
