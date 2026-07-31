<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0217 — the sweep is divided among the cores

Status: implemented
Implemented in: 2026-07-31
App: protolens (with a new entry point in prototext-graph)
Refs: docs/specs/0048-multi-entry-score.md (`score_all`, the
        multi-entry walk),
      docs/specs/0114-protolens-range-type-override.md
        (§3.2, `inferred_candidates` and the ranking rule),
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (the heat worker and its request queue),
      docs/specs/0168-protolens-resolve-root-type-before-decode.md
        (G3, `seed_root_heat` — the startup sweep's result is handed to
        the caches instead of being recomputed),
      docs/specs/0180-own-the-scoring-graph-by-arc.md
        (S4, `SCORING_THREAD_STACK_SIZE`),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md
        (S10/step 9, moving the walk off the main thread — this spec
        subsumes its rationale)

## Background

### The sweep is most of startup

Measured 2026-07-31 on `googleapis.desc` (25.6 MB, 7 771 files,
58 777 types, 49 255 scoring roots), release build, batch `exit`, with
the lazy descriptor path live, pinned to `taskset -c 4-11`:

| run | wall time |
|---|---|
| `--raw`, no descriptor set | 1.61 s |
| `--raw` + `--descriptor-set` | 1.61 s |
| `--type FileDescriptorSet` | 2.67 s |
| inferred, single-threaded | 9.50 s |

Two things fall out. The descriptor set costs approximately nothing
now that the pool opens lazily against `index.rkyv`. And **the
inference sweep is 6.8 s, 72% of startup** — confirmed directly by
timing `score_all` in isolation against the same blob and graph
(6.78 s), so the subtraction is not carrying an unrelated cost.
Everything else put together is 2.7 s.

This invalidates the rationale spec 0216's S10 gave for moving the walk
off the main thread, which assumed a multi-second descriptor load to
hide the walk behind. There is no such load. The walk is the load.

### Who sweeps

Audited 2026-07-31. `score_all` has exactly three production call sites:

| site | thread | scope | when |
|---|---|---|---|
| `main.rs:362` → `decode::determine_root_type` | **main** | the whole payload | startup, once |
| `heat_worker.rs:519` → `override_pane::inferred_candidates` | **heat worker** | one node's range | every sweep in a normal session |
| `heat_cue.rs:386` → `override_pane::inferred_candidates` | **main** | one node's range | only when `heat_worker` is `None` |

(`score_one` — `heat_cue.rs:376` and the worker's fast path — is a
single-entry scoring call, not a sweep, and is out of scope.)

The third row is a fallback for the no-worker configuration; in an
interactive session `run` always spawns the worker (`tui/mod.rs:2384`).
So the operative fact is:

> **Beyond startup, the heat worker is the only sweeper.** The main
> thread enqueues requests and reads caches; it does not sweep.

And the worker consumes its queue one request at a time. Therefore at
most one sweep is ever in flight, and a pool of shard threads has
exactly one client at any moment. There is no sweep-versus-sweep
contention to design around — only sweep-versus-everything-else.

That is what makes a single divide-and-conquer form usable everywhere:
startup, worker, and the no-worker fallback can all call the same
sharded entry point.

### Why sharding by root is not sharding by work

`score_all` is not a loop over roots. It is **one** walk of the blob
carrying every candidate at once (`walk.rs:251`): it seeds an active
set by grouping the roots by their graph state
(`group_by_state(graph.roots …)`), then makes a single
`score_message_multi` descent. Hopcroft minimization has already
collapsed behaviorally equivalent candidates, so two roots in the same
state are indistinguishable to the walk and cost one traversal between
them.

The consequence for sharding is the whole design:

- Per-step cost is driven by the number of **active state groups**, not
  the number of roots. Halving the root list does not halve per-step
  work.
- If a partition splits a group across two shards, both shards carry
  that state and both walk it. The work is duplicated, not divided.

So the partition must be over **groups**, which are disjoint by
construction. This is the only correctness-relevant subtlety in the
whole spec, and it is invisible from the caller's side, which is one
reason the partitioning helper belongs in the library (S1).

The residual inefficiency is honest and should be stated: groups that
start distinct can **converge** on the same state deeper in the
message. One pass merges them and walks the shared subtree once; N
shards each walk it. Speedup is therefore strictly below N, by however
much convergence the corpus has. That is a measurement, not a
derivation (S8) — and the measurement, once taken, said the penalty is
small and the real limit is elsewhere. See "Measured outcome".

## Goals

- **G1.** A sweep is divided across a bounded number of threads.
- **G2.** The result is identical to today's, entry for entry and in
  ranking order — sharding is a scheduling change, never a scoring
  change.
- **G3.** One global CPU budget for the process, settable from the
  command line, honored by every sweeper.
- **G4.** The startup sweep runs off the main thread, so the draw loop
  and signal handling stay responsive across it (spec 0216 S10's goal,
  with this spec's rationale rather than its own).
- **G5.** `--jobs 1` reproduces today's behavior exactly, on the
  calling thread, with no shard threads spawned.

## Non-goals

- **N1.** Changing any scoring rule. In particular the threshold
  between what penalizes and what vetoes is where it should be and is
  not to be moved in the name of pruning the candidate set.
- **N2.** Reducing the sweep's total work. This spec divides the work;
  it does not shrink it.
- **N3.** Progressive rendering — painting the raw document first and
  retyping when the sweep lands. That design existed and was
  deliberately removed (`tui/mod.rs:2378-2383`): the document changed
  underneath a reader who had already started browsing. Spec 0216 makes
  the retype cheaper but does not make it less startling.
- **N4.** A general work-stealing runtime (rayon or similar). The
  parallelism here is a single static fan-out with one client at a
  time; a scheduler would be more machinery than the problem has.
- **N5.** Any threading inside `prototext-graph`. See S1.
- **N6.** `prototext-core`. It is the codec/render side and is not on
  this path at all.

## Specification

### S1. The library exposes a partition; the application owns the threads

The sweep lives in **`prototext-graph`** (`src/score/walk.rs`), not
`prototext-core`. It gains two public items and no threading:

```rust
/// Partition the graph's roots into `n` balanced parts that never
/// split a state group. Returns at most `n` non-empty parts.
pub fn partition_roots(graph: &ArchivedCompiledGraph, n: usize)
    -> Vec<Vec<u32>>;

/// `score_all` restricted to `roots` (indices into `graph.roots`).
/// Single-threaded, allocation-local, no knowledge of sharding.
pub fn score_subset<'g>(
    pb: &[u8],
    graph: &'g ArchivedCompiledGraph,
    opts: &ScoringOpts,
    roots: &[u32],
) -> Vec<EntryScore<'g>>;
```

`score_all(pb, graph, opts)` becomes `score_subset` over every root and
keeps its signature, so no existing caller changes. `score_one` — the
single-entry call behind the `prototext score` CLI and the heat cue's
fast path — is `score_subset` over a one-element subset, and is
reimplemented as one: it had its own hand-written copy of the active-set
setup, which is the same duplication the group/root subtlety above
exists to prevent.

The active set indexes positions within `roots`, not within
`graph.roots`, so the result comes back in `roots` order and the
caller's merge needs no index remapping.

`partition_roots` is in the library because balancing requires
`graph.roots[i].state_id` — rkyv internals protolens has no business
reading, and the group-versus-root subtlety is exactly the kind of
thing that must not be re-derived by each caller. Deciding *how many*
parts, spawning them, and joining them is protolens's, in line with
the existing convention that kept `node_budget` out of the library.

Balancing is largest-group-first bin packing by group size, which is
adequate: it is O(G log G) on a few thousand groups and runs once per
sweep against seconds of walking.

### S2. The ranking rule moves into one place

Today the same comparator is written twice —
`decode.rs:363-` and `override_pane.rs:62-` — as two closures that
happen to agree (vetoed last; then score descending; then FQDN). It
becomes one function, `sweep::candidate_order`, because S3 depends on
shard and merge ordering being the same relation, and two hand-copied
closures are not a guarantee.

Root FQDNs are unique, so `candidate_order` is a **total** order on the
candidates. That is what makes a sharded ranking byte-identical to a
whole-sweep one rather than merely equivalent up to tie order, and so
what makes G2 provable rather than approximately true.

Vetoed entries are dropped **before** the sort rather than after it,
which both replaced call sites did the other way around. Same list — a
type the wire data already contradicts is not a plausible candidate at
any rank — over fewer sorted elements.

### S3. The merge is explicit, not rediscovered

Each shard sorts its own output under S2's comparator. protolens then
performs an **N-way merge over the N sorted runs** — a binary heap of N
cursors, O(M log N) — in `sweep::Merged`.

Concatenating and calling `sort_by` would also work, since Rust's
stable sort detects natural runs, but it would pay ~M comparisons to
rediscover boundaries we already know. That is about `1/log2(N)` of the
merge — 25% at N = 16 — of a merge that is itself microseconds against
a multi-second sweep. The reason to write the explicit merge is not
those microseconds; it is that an explicit merge can be **lazy**, and
several consumers do not want the full ranking:

- `decode::pick_winner` needs the winner plus the tie check — the first
  two elements.
- `heat_cue::derive_stats` needs `best_score` and `best_count`, both
  combinable per shard in O(N).
- `by_range`'s `top_n` is capped at
  `max(override_list_height, HEAT_CUE_PREVIEW)`.

So `Merged` is an `Iterator<Item = (String, i64)>` and `collect()` is
the eager case rather than the only one. It is also an
`ExactSizeIterator`: the shards partition the roots, so the total is
the sum of the run lengths and is known before the first element is
yielded — a caller that stops after the top few can still report how
many candidates there were, and `collect()` allocates exactly once.

The `complete` cache does want the unbounded list, and it is on the
path of every current caller, so today every one of them ends in
`.collect()`. The laziness is available, not yet exploited; see
Future work.

### S4. One global CPU budget

A new `--jobs N` (`-j`) caps the number of CPU-bound threads protolens
will create, defaulting to `std::thread::available_parallelism()`. It is
a process-wide budget, not a per-subsystem one.

**It is a ceiling, not a target.** Every sweeper clamps the request to
`available_parallelism()` before partitioning
(`sweep::effective_jobs`), so `--jobs 10` on a two-CPU machine fans out
two ways, not ten. Past the CPU count an extra shard buys nothing and
costs a partition, a stack reservation and another copy of the
convergence duplication described in Background. `available_parallelism`
is the right ceiling because it respects the cgroup quota and the
affinity mask, so under a container limit or a `taskset` it reports what
the process can really run on rather than what the machine has.

The clamp is applied inside `sweep::ranked_with` and not only at the
command line, so a path into the sweep is bounded by the machine whether
or not its caller remembered to ask.

The two claimants behave differently and must be treated differently:

- The **input thread** (`event.rs:42`) is blocked in `event::poll`
  essentially always. It consumes no CPU and needs no reserved core;
  what it needs is prompt scheduling on wakeup, and a thread that has
  just slept has low vruntime, so it preempts CPU-bound shards readily.
  Saturating every core costs it a scheduling quantum of latency, not
  starvation. It is **not** charged to the budget.
- The **heat worker** is a genuine CPU-bound competitor running the
  same scorer. It is charged one permit for as long as it is serving a
  request.

So: the startup sweep takes all N (the worker has nothing queued until
a rendering exists); a mid-session sweep takes N-1, leaving the draw
loop a core. Because at most one sweep is in flight (Background), this
needs no pool or scheduler — the sweeper reads the budget, subtracts
what is held, and fans out that wide.

`--jobs 1` means one *compute* thread, not a single-threaded process.
The input thread and the heat worker still exist; they are there for
latency and correctness, not throughput. Under `--jobs 1` the sweep is
`score_subset` over the whole root list on the calling thread —
bit-for-bit today's path, which is the known-good escape hatch for a
shared machine.

### S5. Stack sizing

`stack = depth × frame`, and both terms are already measured
(`MAX_WIRE_DEPTH`'s doc comment in `prototext-core/src/helpers/bounds.rs`
is the source of truth):

- **depth** is bounded only by `MAX_WIRE_DEPTH = 1000`. That cap is the
  only reason a worst case is finite at all, since wire depth otherwise
  scales with input length (a LEN level costs 2 bytes).
- **frame** is ≈ 590 B for `score_message_multi` in release, bisected
  by `stack_size`. Debug is ~8× (≈ 4.8 KiB/frame), which is why the
  deep-nesting tests abort under a debug `cargo test`.
- so the full-cap worst case is ≈ 576 KiB per walking thread, release.

**Sharding moves neither term.** Depth is a property of the blob, not
of the candidate set, so a shard carrying a tenth of the roots recurses
just as deep. The frame is candidate-count-independent because the
active set is on the heap and its `SmallVec` inline capacity is a
compile-time constant.

Each shard therefore gets `SCORING_THREAD_STACK_SIZE` (16 MiB), the
constant the heat worker already uses. At 16 shards that is 256 MiB of
**address space**, not resident pages: measured max depth on googleapis
is 13, so roughly 8 KiB per shard is actually touched.

The frame's independence from candidate count is the one claim here
that is argued rather than measured. It is verified as part of the test
plan before it is relied on.

### S6. The startup sequence

```
main thread                           shard pool (N)
-----------                           --------------
1. Blob::load (mmap where possible)
2. load the scoring graph
   (mmap hopcroft.rkyv, ~free)
3. partition_roots + spawn      ───>  score_subset × N over the payload
4. build the maximal arena            (spec 0216 — a function of the
   (spec 0216, ~70 ms; bytes           bytes alone, needs no root type)
   only, needs no root type)
5. open the descriptor pool
   lazily (index.rkyv, ~free)
6. draw the "inferring…" cue;
   stay responsive
7. join  <──────────────────────────  N sorted runs
8. N-way merge → winner + tie check
9. ctx.message(winner)
10. render_resolved into the arena
```

Steps 1 and 2 are strictly ordered before 3 and are both mmaps. Step 4
is the change spec 0216 bought: the arena no longer depends on the root
type, so it can be built while the sweep runs instead of after it.

Steps 3 and 4 are expressed as `sweep::ranked_with(pb, graph, jobs,
meanwhile)`: the shards are spawned, `meanwhile` runs **on the calling
thread**, and only then are the shards joined. A closure rather than a
returned join handle, so the shards can borrow `pb` and `graph` instead
of demanding `'static` copies of them — `thread::scope` +
`Builder::spawn_scoped` is what makes that sound. `meanwhile` runs
exactly once whether or not any shard was spawned, so the `--jobs 1`
and no-graph paths need no separate arrangement.

The overlap is not why startup gets faster: the arena walk is ~70 ms
against a sweep measured in seconds, so this hides the arena behind the
sweep and not the reverse. It is simply work that no longer has to queue
behind it.

`main` consequently no longer calls `decode::decode` at all — it needs
the resolution and the render apart, both to run the arena build between
them and to announce the two phases separately. Batch mode takes the
same split path with the announcements suppressed, rather than a second
whole-decode path that would have had to be kept in step with it.
`decode::decode` survives as the one-call shape the tests want.

Note what is *not* here: nothing renders before step 10. N3 stands.

### S7. The other two call sites

`heat_worker.rs:519` and `heat_cue.rs:386` call the same sharded entry
point, `sweep::ranked`. The worker's budget is N-1 per S4 — it sweeps
while the main thread is still drawing — floored at 1. The no-worker
fallback at `heat_cue.rs:386` runs on the main thread and takes N: that
arm is only reached when there is no worker, so nothing else in the
session is sweeping.

The budget itself lives on `App::sweep_jobs`, set from `--jobs` next to
`override_preview_byte_budget` and defaulting to 1 — which is what every
test wants, a sweep that runs where it was called.

### S8. What must be measured before this is believed

The convergence penalty (Background) means speedup is below N by an
unknown factor. Before committing to a shard count, run the sweep at
N = 1, 2, 4, 8, 16 and record wall time. If the curve flattens early,
the walk is dominated by shared subtrees and the useful N is small.
Also record the *total serial* work per part count and the per-part
times, since a flat curve has two possible causes — duplicated work and
imbalance — and they call for opposite remedies. (Done; see "Measured
outcome". The cause was imbalance.)

Pin the run: `taskset -c 4-11` is this machine's reproducible set.

Do **not** validate this with `cargo bench --bench score`: that target
has a measured +15.9% same-binary noise floor, which is larger than
several of the deltas in question. Wall-clock startup on the real
corpus, repeated, is the instrument.

## Test plan

1. `a_sharded_sweep_matches_the_whole_sweep` — for a fixture graph,
   `score_subset` over every partition of `partition_roots(graph, n)`,
   merged, equals `score_all` entry for entry and in order, for
   `n` in 1..=8. This is G2.
2. `a_partition_never_splits_a_state_group` — every root sharing a
   `state_id` lands in the same part; the parts are disjoint and cover
   every root exactly once.
3. `a_subset_reports_its_entries_in_subset_order` — the result of
   `score_subset` is indexed by position within `roots`, which is what
   lets the merge skip an index remapping.
4. `merging_sorted_runs_equals_sorting_the_concatenation` — the merge
   reproduces what sorting the concatenation gives. Stated over the
   merge alone, so a failure here cannot be confused with a scoring
   difference.
5. `a_score_tie_is_broken_by_fqdn_across_runs` — equal scores are
   ordered by FQDN *across* runs, not merely within one. This is the
   case a per-run sort cannot settle by itself, and the one that decides
   whether a sharded ranking is identical to a whole one or only
   equivalent.
6. `the_merge_yields_in_order_without_draining` — taking k elements does
   not drain the runs behind them, and the reported length is of what is
   left.
7. `a_shard_frame_does_not_depend_on_the_candidate_count` — bisect
   `stack_size` for `score_subset` against a full root list and against
   a one-root list; the two figures must agree within measurement
   noise. This is S5's unverified claim, and it must assert a match
   count too so an early stop cannot make the assertion vacuous
   (the trap `max_depth_walk_fits_in_a_default_thread_stack` already
   guards against). **Not yet written.**

Item 7 aside, a `--jobs 1` test is deliberately absent: the escape hatch
is exercised by the whole existing suite, which runs at `sweep_jobs = 1`
throughout.

## Future work

- **More parts than threads, taken dynamically.** The measurement above
  says the fan-out is limited by imbalance, not by convergence or
  overhead, and that per-group cost is not predictable in advance. The
  standard answer to an unpredictable cost is to stop predicting: cut
  k × N parts and let N threads pull the next unclaimed one from a
  shared `AtomicUsize` cursor. That bounds the tail at one part rather
  than at one N-th of the work, and since total serial work is nearly
  flat in the part count (7.03 s at 8 parts vs 6.76 s at 1), finer parts
  are close to free. It is a cursor, not a runtime, so N4 still stands.
- **Choose the sharding axis by tier.** The heat worker's sweeps are
  per-node ranges, and two ranges are independent inputs to a read-only
  computation — so serving k queued requests on k threads divides real
  work with none of the duplication sharding one request's roots pays.
  Which axis is right depends on what the queue looks like:
  - A **user-tier** query — an override at the root, the largest range
    there is — is one request that somebody is waiting on. Latency is
    the whole point, and there is nothing else to run in parallel with.
    Shard it by root group, as this spec does.
  - **Prefetch-tier** queries are many small ranges that nobody is
    waiting on. Each is cheap enough that sharding it by root would be
    mostly setup, and there are k of them. Shard by query.

  So the two forms compose rather than compete, and the budget should be
  spent along whichever axis the current queue actually offers. Wants a
  benchmark before it is believed.
- **Stop the merge early.** `Merged` is already a lazy
  `ExactSizeIterator`, but every current consumer ends in `.collect()`
  because the `complete` cache wants the whole ranking. Deciding what
  that cache actually owes its readers would let `pick_winner` take two
  elements and `by_range` take a screenful.

## Measured outcome

Measured 2026-07-31, googleapis, release, batch `exit`, pinned to
`taskset -c 4-11` (8 CPUs), two runs per point:

| `--jobs` | startup | sweep (startup − 2.67 s floor) | speedup |
|---|---|---|---|
| 1 | 9.50 s | 6.8 s | 1.0× |
| 2 | 6.02 s | 3.4 s | 2.0× |
| 4 | 4.65 s | 2.0 s | 3.4× |
| 6 | 4.77 s | 2.1 s | 3.2× |
| 8 | 4.52 s | 1.9 s | 3.6× |
| 12 | 4.49 s | 1.8 s | 3.7× |

**Startup halves.** `--jobs 12` on an 8-CPU affinity mask lands on
`--jobs 8`, which is S4's clamp working.

Two further measurements say *why* the curve flattens, and the answer is
not the one Background predicted.

**Convergence is not the limit.** Running every part of
`partition_roots(g, n)` serially, so that the total is all the work the
shards would do between them:

| parts | total serial work |
|---|---|
| 1 | 6.76 s |
| 2 | 7.39 s |
| 4 | 7.68 s |
| 8 | 7.03 s |

Splitting into eight parts costs ~4% more total work than not splitting
at all. The convergence penalty exists but is small enough to ignore.

**Load imbalance is the limit.** The same run, per part, at n = 8 —
every part within 0.02% of the same root count and within 1% of the same
group count:

```
6157 roots, 1482 groups, 0.263 s
6157 roots, 1842 groups, 0.600 s
6157 roots, 2168 groups, 1.107 s
6157 roots, 2415 groups, 0.673 s
6157 roots, 2416 groups, 1.329 s
6157 roots, 2416 groups, 1.865 s   <- critical path
6157 roots, 2417 groups, 0.648 s
6156 roots, 2416 groups, 0.633 s
```

A 7× spread. The longest part is 1.87 s of a 7.03 s serial total, so a
perfect scheduler over *this* partition finishes in 1.87 s — and the
sweep measured 1.9 s. **The fan-out is already extracting essentially
all the parallelism the partition contains.** What it does not contain
is balance: a group's cost is how much of the blob it stays alive
through, which is neither its root count nor knowable before the walk.

So the next gain is not a wider fan-out; it is a partition whose parts
are not required to be right first time. See Future work.
