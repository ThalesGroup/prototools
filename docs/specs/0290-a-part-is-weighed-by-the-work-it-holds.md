<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0290 — a part is weighed by the work it holds

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0217-the-sweep-is-divided-among-the-cores.md (introduced
        `partition_roots`, and the rule that a state group is the unit),
      docs/specs/0269-the-last-part-of-a-sweep-finishes-on-the-best-core.md
        (named this defect and deferred it here)

## Background

`partition_roots` splits on state-group boundaries — its doc comment
explains at length that two roots sharing a Hopcroft state cost one
traversal between them, so a group is the unit of work — and then
balanced the parts by counting **roots**.

The two halves disagree, and the disagreement is not academic. Replayed
on the googleapis graph (49 255 roots, 16 680 groups, `n = 24`):

| parts | groups each | roots each |
| --- | ---: | ---: |
| 0, 1, 2 | **1** | 4645, 3925, 3122 |
| 3 … 23 | 768-797 | 1788-1789 |

Three parts hold 23.7% of the roots and, timed one part at a time during
the 0269 work, do **0.82% of the work** — one group is one traversal
however many roots share it. Root counts were even to four digits while
that happened. The effective pool was **21 parts, not 24**.

## Goals

- **G1.** Balance on the quantity the doc comment already names as the
  unit of work: the group.
- **G2.** Keep every existing property of the partition — groups never
  split, every root exactly once, no empty parts, at most `n` parts.

## Non-goals

- **N1.** *Not a cost model.* Group count does not predict cost either:
  parts 3-23 above were within 3.8% on group count and varied **10x** in
  measured time. This makes the intent and the implementation agree; it
  does not make the partition balanced in time.
- **N2.** *Not a fix for the straggler.* The 2.4x-of-mean part remains,
  and remains handled by spec 0269's seat donation, which needs no
  foresight and got most of the available win.
- **N3.** No change to the part count. `SWEEP_PARTS = 24` was calibrated
  around the 21-effective pool, but the simulation behind 0269 found the
  part count barely matters (32 marginally best), so re-tuning it is a
  separate question with its own evidence.

## Specification

- **S1.** The hand-out weighs a part by the number of groups it holds.
- **S2.** It is written as `parts[i % k]` over groups sorted largest-first.
  With group count as the key that *is* round-robin — every part stays
  tied until the wheel comes round, and `min_by_key` returns the first of
  a tie — so the modulo is the same function, and drops the hand-out from
  O(G·n) to O(G). Sorting largest-first is retained: it spreads group
  *sizes* across parts, which is free once the wheel decides placement.

## Alternatives considered

### Balance on an estimated cost rather than a count

The honest fix, and out of reach: the only cost signal available before
the walk is the graph, and the 10x spread across parts with equal group
counts shows the graph does not carry it. Measuring costs requires
walking, which is the thing being scheduled. 0269's seat donation
deliberately buys the same outcome after the fact instead.

### Leave it alone, since the makespan barely moves

Offline simulation over 3000 hand-out orders put this at **1-3% of
makespan** — the reason 0269 declined to bundle it. But the code as
written stated one rule in prose and implemented another, and three
parts sat nearly idle for reasons no reader could infer from the doc
comment. The cost of the fix is smaller than the cost of explaining it
again.

## Test plan

1. `a_partition_is_balanced_by_group_count_not_root_count` — over a
   deliberately lopsided graph (one group of 40, forty singletons), group
   counts across parts differ by at most one. Simulated against the old
   algorithm, the same input gives a spread of 39 at `n = 2` and 5 at
   `n = 8`, so this is a real regression guard.
2. `a_partition_never_splits_a_state_group` — unchanged, and must stay
   passing: G2.
3. `a_sharded_sweep_matches_the_whole_sweep` — unchanged. A scheduling
   change is never a scoring change.

## Measured outcome

googleapis, `n = 24`, group counts per part:

| | min | max | spread |
| --- | ---: | ---: | ---: |
| before | 1 | 797 | 796 |
| after | 695 | 695 | **0** |

16 680 groups over 24 parts divides exactly. The three dead parts are
gone and the pool is genuinely 24 wide. Root counts per part now range
1424-6044, which is expected and is not a defect — root count was never
the thing being balanced.

**Wall clock contradicted the 1-3% prediction.** protolens startup on
googleapis, `taskset -c 0-7 … -j 8`, the two binaries interleaved,
medians of 5:

| | before | after |
| --- | ---: | ---: |
| startup | 2.299 s | **2.032 s** |
| sweep only (startup − 1.376 s serial floor) | 0.923 s | **0.656 s** |

**−11.6% of startup, −29% of the sweep**, and all five interleaved pairs
agree in sign with barely-overlapping ranges — well clear of this
machine's 1-2% noise.

The 1-3% figure came from simulating makespan over 3000 random hand-out
orders on the *measured cost vector*, and it understated this badly. The
simpler argument was the right one: three of twenty-four parts were
nearly empty of work, so the pool was 21 wide, and widening it to 24 is
worth about 24/21 = 14% before any scheduling effect. Measured 11.6%.
N1 still stands — the parts are not balanced *in time*, and the 2.4x
straggler is untouched — but "the intent fix is not a performance fix"
was wrong.
