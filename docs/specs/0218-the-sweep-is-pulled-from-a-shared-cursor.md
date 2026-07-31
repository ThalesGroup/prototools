<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0218 — the sweep is pulled from a shared cursor

Status: implemented
Implemented in: 2026-07-31
App: protolens
Refs: docs/specs/0217-the-sweep-is-divided-among-the-cores.md
        (the fan-out this replaces; its `partition_roots`, `Merged` and
        `SCORING_THREAD_STACK_SIZE` are all reused unchanged)
      docs/specs/0180-protolens-scoring-thread-stack-size.md
        (the stack reservation every walking thread needs)
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md
        (the `meanwhile` work overlapped with the sweep)

## Background

Spec 0217 cut the scoring roots into exactly one part per thread and
gave each thread its part. It predicted that convergence duplication
would cap the speedup; measurement said otherwise. The limit is **load
imbalance**: at 8 parts the per-part times ran 0.24 s to 1.83 s — a 7x
spread — despite equal root counts, and the longest part *is* the sweep.

A part's cost is how deep into the blob its candidates stay alive. That
is not its root count, not its group count, and not knowable before the
walk. So the partition cannot be balanced. It can only be made **fine
enough that imbalance stops mattering**, with threads taking a new part
whenever they finish one instead of being handed a fixed share up front.

### What was measured

Two corpora, each part timed alone on one core (`taskset -c 4`), then
the makespan simulated for W workers pulling parts in partition order.
The simulation is trustworthy: at 8 parts it predicts 1.83 s where the
real 8-thread run measured 1.89 s.

| | googleapis | pdb |
|---|---|---|
| roots / state groups | 49 255 / 17 572 | 1 900 / 1 166 |
| blob | 25.7 MB | 1.1 MB |
| unsplit sweep | 6.96 s | 0.072 s |
| overhead per extra part | 18.4 ms | 0.17 ms |

googleapis, 8 workers, by part count:

| parts | 8 (0217) | 16 | **24** | 32 | 48 | 64 | 128 |
|---|---|---|---|---|---|---|---|
| makespan | 1.862 | 1.091 | **0.957** | 1.132 | 1.048 | 1.279 | 1.288 |

24 was also the optimum at every blob size from 3.2 to 25.7 MB (cut at
top-level record boundaries, graph held fixed) and at 4, 8 and 12
workers alike. pdb's optimum is 32-48, but its entire sweep is 20 ms;
at 24 parts it costs 7 ms more than its best, which is not a cost.

The two corpora differ 15x in group count and 23x in blob size, yet the
optimal **part count** moves only 24 → 40 while the optimal
**groups-per-part** moves 630 → 29. Part count is the near-invariant.

### What is not understood

Stated plainly, because the constant below is empirical:

- **Why a part count rather than a work-proportional target.** A model
  that fits both corpora is `makespan ≈ max((base + c·k)/W,
  largest_part(k))`, minimized near `k ≈ sqrt(W · base/c)`. The ratio
  `base/c` is 375 for googleapis and 424 for pdb — near-equal by
  apparent coincidence, and that coincidence is the whole reason one
  constant serves both. Two points do not make a law.
- **Why per-part overhead varies as it does.** 18.4 ms against 0.17 ms
  is 108x for a 23x blob difference, so blob size alone does not
  explain it and the graph must contribute.
- **Why splitting sometimes reduces total work.** googleapis does less
  total work at 16 parts than at 1 (6.96 → 5.53 s, −21%); pdb only ever
  does more (0.072 → 0.108 s). The effect is real on one corpus and
  absent on the other, so nothing is built on it.

A third corpus may well move the constant. It is deliberately a single
named constant in one place so that it can be moved.

## Goals

- **G1.** No thread idles while another still has unstarted work.
- **G2.** The ranking stays byte-identical to a whole-sweep one, as
  under 0217 — same total order, no dependence on how the parts fell or
  on the order they completed in.
- **G3.** `--jobs 1` behaves exactly as it does today: one part, on the
  calling thread, nothing spawned.
- **G4.** The tuning constant is one named item with the measurement
  recorded next to it.

## Non-goals

- **Splitting a state group.** `group_by_state` collapses roots sharing
  a state into a single traversal, so cutting a group duplicates the
  walk rather than dividing it. The floor this leaves is real —
  measured ~0.60 s on googleapis from about four fat groups, visible as
  the plateau at 64/128/256 parts — and beating it needs a walk that
  can be suspended mid-descent. Not here.
- **Ordering parts by predicted cost.** Simulating an oracle that hands
  out parts heaviest-first gives 0.757 s against 0.957 s, so ~0.2 s is
  available. But cost is unknown before the walk, and the packer
  deliberately equalizes root counts, so no runtime proxy is to hand.
  Partition order already leads with the largest groups (the packer
  deals the k largest one to each bin), and that is all the ordering
  this spec claims.
- **Work stealing.** A cursor is not stealing: nothing is ever taken
  from a thread that holds it, because a thread never holds more than
  the part it is walking.
- **Changing `--jobs` semantics.** Still a ceiling, still clamped to
  `available_parallelism()`.

## Specification

- **S1. A constant part count.** `sweep::SWEEP_PARTS = 24`, and the
  sweep partitions into `max(SWEEP_PARTS, workers)` parts.
  `partition_roots` already returns at most one part per state group,
  so a small graph clamps itself and needs no lower bound here. The
  `max` matters only where `--jobs` exceeds 24, which is a real machine
  and not a hypothetical.

- **S2. Workers pull from an `AtomicUsize`.** One counter shared by the
  spawned threads. Each thread loops: `fetch_add(1, Relaxed)`, and if
  the index is in range, walk that part and keep the result; otherwise
  stop. `Relaxed` is sufficient — the counter's only job is to hand each
  index to exactly one thread, `fetch_add` is atomic regardless of
  ordering, and every other datum is either immutable and published
  before the scope opens (`pb`, `graph`, the parts) or returned through
  `join`, which already synchronizes.

- **S3. Threads, not parts, bound the spawn.** `min(effective_jobs,
  parts.len())` threads. Under 0217 these were the same number; they no
  longer are, and spawning 24 threads to run 24 parts on 8 CPUs would
  reintroduce exactly the fixed assignment this replaces.

- **S4. One run per part, not per thread.** A thread that walks three
  parts returns three separately-ranked runs. Concatenating them within
  a thread would break the sortedness `Merged` depends on, and
  re-sorting would throw away the property in G2. The runs are then
  flattened across threads and merged as before; `Merged` is order-
  insensitive, so the ranking does not depend on which thread drew what.

- **S5. `--jobs 1` takes one part.** When `effective_jobs` is 1 the
  sweep partitions into a single part and runs it on the calling
  thread, unchanged from 0217. Partitioning finer would be *slower* on
  pdb (0.072 → 0.098 s) even though it is faster on googleapis, and the
  single-threaded path is the escape hatch for a loaded machine — it
  must not become a place where a corpus can lose.

- **S6. The constant carries its evidence.** `SWEEP_PARTS`'s doc
  comment states the two corpora, the measured optimum on each, and
  that the value is a fit rather than a derivation.

## Test plan

1. `a_sharded_ranking_equals_a_whole_one` — unchanged from 0217, now
   exercising many parts per thread.
2. `every_part_is_walked_exactly_once` — a cursor over a known part
   count hands out each index once and only once, with more parts than
   threads.
3. `one_job_uses_one_part` — `effective_jobs(1)` partitions into a
   single part and spawns nothing.
4. `more_jobs_than_parts_spawns_only_as_many_threads_as_parts` — S3.
5. `a_graph_with_fewer_groups_than_the_target_clamps` — a graph with
   fewer state groups than `SWEEP_PARTS` yields one part per group.
6. `the_merge_is_insensitive_to_run_order` — shuffling the runs before
   merging gives the same ranking (G2 under S4's flattening).

## Measured outcome

googleapis (25.7 MB blob, 25.6 MB descriptor set), warm, opened as its
own blob with `exit`. `taskset -c 4-11` — eight uniform 3.8 GHz cores,
the workspace's reproducible set for a wall-clock measurement.

| | 0217 (8 fixed parts) | 0218 (24 parts, cursor) |
|---|---|---|
| inferred startup | 4.466 s | **3.548 s** (−20.6%) |
| `--type` floor | 2.574 s | 2.655 s |
| sweep, by difference | 1.89 s | **0.89 s** |
| `--jobs 1` | 9.50 s | 9.82 s |

**The sweep is 2.0x faster** and lands where the simulation said it
would — 0.957 s predicted from serial per-part timings, 0.89-0.95 s
measured, the residual being the ~3% spread on the two floor readings.
That the prediction held is the main reason to trust the part-count
constant rather than only the direction of the change.

Run-to-run under the pin is now **3.534 / 3.535 / 3.576 s**, a 1.2%
spread. Unpinned across ten runs it is 2.5-4.0 s: the P/E-core mix and
whatever else the machine is doing still dominate there, so the cursor
did not make an unpinned run reproducible. It made the *pinned* run
reproducible, which is the claim worth making.

`--jobs 1` is unchanged, as S5 requires — 9.82 s against 9.50 s, one
sample each, well inside the noise on a ten-second measurement.

Two things this did **not** get:

- **The oracle's 0.757 s.** Handing parts out heaviest-first is worth
  another ~0.2 s and remains out of reach for want of a cost proxy.
- **The ~0.60 s floor.** About four indivisible state groups set it. The
  sweep is now within ~50% of that floor rather than 3x it, so the
  remaining headroom for any scheduling change is small.

### Follow-on worth measuring

Chunking groups into parts by *descending size* instead of bin-packing
them into equal root counts. The packer's equalizing is what destroys
the cost signal; a descending chunk would produce parts that are ordered
heaviest-first by construction, which is exactly the ordering S3's
Non-goal says is unavailable. Whether size predicts cost well enough to
help is unknown — the 7x spread at equal root counts says probably not,
but it costs one harness to find out.
