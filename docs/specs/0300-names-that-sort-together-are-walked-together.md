<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0300 — names that sort together are walked together

Status: implemented
Implemented in: 2026-08-15
App: prototext-graph
Refs: docs/specs/0217-the-sweep-is-divided-among-the-cores.md (the
        partition this re-orders),
      docs/specs/0290-a-part-is-weighed-by-the-work-it-holds.md (the
        balancing key this keeps),
      docs/specs/0269-the-last-part-of-a-sweep-finishes-on-the-best-core.md
        (the straggler mechanism this does *not* replace)

## Background

`partition_roots` deals state groups out largest-first, one per part in
turn. Group size is uncorrelated with what a group *reaches*, so the
hand-out scatters structurally related groups across every part.

That matters because splitting the sweep does not divide its work, it
multiplies it. The walk carries one `ActiveEntry` per distinct state, so
a subtree is traversed once however many candidates converge on it —
and a state reachable from k different parts is walked k times. On
googleapis (16 696 states, 85 806 transitions, 49 255 roots) the roots
reach essentially the whole graph, `|closure(all roots)| = 16 695`, so
there is a great deal available to share and the current hand-out shares
none of it deliberately.

Measured, one part at a time, single-threaded, `taskset -c 4`:

| parts | total work | vs undivided |
| ---: | ---: | ---: |
| 1 | 1 969 ms | 1.00x |
| 24 | 4 469 ms | **2.27x** |
| 768 | 12 132 ms | 6.16x |

Parallel efficiency is capped near 44% by that multiplier, and at eight
workers the sweep is **work-bound, not straggler-bound**: at 24 parts
`sum/8` was 559 ms against a largest part of 532 ms.

## Goals

- **G1.** Cut the duplication the partition creates, not the way it is
  balanced — 0290's key stays.
- **G2.** Cost the startup critical path no more than a few
  milliseconds. `partition_roots` runs at 7.3 ms today.
- **G3.** Change no result. The partition is an internal division of
  labor; every root is still scored against the whole blob.

## Non-goals

- **N1.** No change to the part count. `SWEEP_PARTS = 24` was
  re-measured against this partitioner and sits on the crossover: 96
  parts cost a 770 ms bound against 24 parts' 518 ms. Splitting finer to
  dilute the straggler is the one remedy that cannot work here, because
  finer parts multiply the total.

- **N2.** No straggler rule. This leaves the largest part where it found
  it (517.6 ms against round-robin's 523.4 ms) and so turns the sweep
  tail-bound. Handling that is spec 0269's seat donation, and a further
  change of its own.

- **N3.** No cost model, and specifically no closure computation at
  runtime. A greedy partitioner that places each group by measured
  closure overlap was built and measured: it cuts *less* work than this
  (−10.5% against −14%) and costs **310 ms** to build against this
  spec's 14.9 ms. Reachability is the explanation, not the instrument.

## Specification

- **S1.** Groups are ordered by a representative FQDN rather than by
  size, and the sorted list is cut into `k` contiguous blocks of equal
  group count.

- **S2.** The representative is the lexicographically smallest FQDN
  among the roots sharing that state. Chosen for determinism: several
  roots may share a state, and picking the first in root order would
  make the partition depend on root ordering.

- **S3.** 0290's balancing key is unchanged — a block holds
  `groups/k` groups, so parts are still weighed by group count, never by
  root count.

- **S4.** The ordering key is the whole FQDN, not a package prefix.
  Prefix depths 1, 2 and 3 were measured and scored *identically*, so
  the clustering comes from lexicographic order over the full name, and
  a prefix parameter would be a knob with no effect behind it.

## Why it works

A protobuf FQDN encodes its package hierarchy, and types in one package
refer overwhelmingly to each other. Sorting by name therefore places
structurally related states adjacently for free, and a contiguous cut
keeps them in one part — which is exactly the property that lets their
shared closure be traversed once rather than once per part.

## Alternatives considered

**Greedy placement by closure overlap.** Built and measured. Cuts 10.5%
of the work for 310 ms of build time, against 14% for 14.9 ms. It is
also myopic: largest-closure-first with a capacity cap makes early
irreversible choices, which is why a plain sort beats it.

**A finer quantum, parts sized rather than counted.** Built and
measured, at 12/24/48/96/192/384/768 parts and under both partitioners.
Worse at every count above 24 — see N1.

**Closure overlap as the cost key.** Rejected as an instrument, not just
as too slow. It tracks total work usefully at a fixed part count, but it
inverted three separate timed comparisons and is worthless for
predicting the largest part: it scored this spec's partition best of all
on maximum closure while the measured straggler did not move.

## Test plan

1. `a_partition_cuts_the_name_order_into_contiguous_blocks` — new, and
   the only test S1 and S2 need: the parts' representatives concatenated
   in part order must reproduce the globally sorted list, which asserts
   both that the order is by name and that no part reaches outside its
   block.
2. `a_partition_never_splits_a_state_group` — unchanged. The partition
   is still a partition and still keeps a group whole.
3. `a_partition_is_balanced_by_group_count_not_root_count` — unchanged.
   0290's key survives S1.
4. `a_sharded_sweep_matches_the_whole_sweep` — unchanged. G3.
5. The existing `sweep.rs` part-count assertions, unchanged: this
   changes the *contents* of the parts, not how many there are.
6. Full workspace suite; `protolens … export /` over googleapis
   byte-identical to 0298's.

## Measured outcome

Dev VM (8 E-cores), googleapis (25.6 MB descriptor set, 49 255 roots),
`taskset -c 4`, two interleaved rounds — this box drifts ~6% between
runs, so only arms inside one run are comparable.

| | sum | max | bound at W=8 |
|---|---:|---:|---:|
| round-robin, round 1 | 4 589.7 ms | 523.4 ms | 573.7 ms |
| **this spec, round 1** | **3 877.3 ms** | 517.6 ms | **517.6 ms** |
| round-robin, round 2 | 4 373.5 ms | 525.6 ms | 546.7 ms |
| **this spec, round 2** | **3 825.6 ms** | 518.2 ms | **518.2 ms** |

**−15.5% and −12.5% of the sweep's total work**, for a partition that
builds in 14.9 ms against 7.3 ms.

Closure overlap falls from 81 314 to 21 793 against an ideal of 16 695
— from 4.87x duplication to **1.31x**.

**Not achieved: the straggler.** It moves about 1%, so the sweep is now
tail-bound (`sum/8` = 484.7 against a 517.6 ms largest part) and the
makespan bound improves ~7%, not ~14%. The remaining headroom is in one
part at 3.2x the mean, and collecting it needs N2's separate change.
