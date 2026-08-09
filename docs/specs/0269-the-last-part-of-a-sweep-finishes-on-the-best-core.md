<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0269 — the last part of a sweep finishes on the best core

Status: implemented
Implemented in: 2026-08-09
App: protolens
Refs: docs/specs/0217-the-inference-sweep-is-divided-among-the-cores.md
        (the cursor and `meanwhile` this changes),
        docs/specs/0218-the-roots-are-cut-finer-than-the-thread-count.md
        (`SWEEP_PARTS`, and its "a part's cost is not knowable before
        the walk" — now measured),
        docs/specs/0264-the-thread-the-user-watches-takes-the-fast-core.md
        (`detect_fast`, and `widen()`'s one-mask-for-every-worker),
        docs/specs/0265-the-drawing-core-belongs-to-the-drawing-thread.md
        (the reserved core this lends to the sweep once `meanwhile`
        is done)

## Background

Root-type inference is **87%** of protolens's startup on googleapis
(2848 ms of 3270 ms at `-j 14`). It is cut into `SWEEP_PARTS` parts that
threads pull from a shared cursor, and it ends when the *last* part
ends — so its cost is a makespan, and a makespan is decided by the
slowest core holding the biggest part.

Both terms of that are large, and both were measured (2026-08-09,
reference host, one part at a time under `taskset`, 24 parts):

| | |
|---|---|
| part cost, P-core | 0.1 to **956 ms**; mean 404; max = 2.4x mean |
| cost spread within equal-group parts | **10x** |
| E core / P core | **1.52** |
| LP-E core / P core | **2.85** |
| two SMT threads of one P core, each | **1.92** |

Two consequences the current code has no answer for.

**A part that lands late on a slow core defines the makespan.** One
956 ms part costs 2725 ms on an LP-E core — more than the entire
measured 14-way sweep — while every other core stands idle waiting for
it. Replaying the cursor over the measured costs, the last round costs
the sweep about a quarter of its wall time.

**Every worker gets the same mask, so nothing can be done about it.**
`widen()` (spec 0264 S7) hands every thread `inherited − drawing core`.
The kernel places them, both hyperthreads of a physical core are fair
game, and no thread knows or can change which CPU it is on.

Two smaller facts fall out of the same measurements and shape the fix:

- **SMT is worth nothing here.** Two threads on one P core deliver
  `2/1.92 = 1.04` cores of throughput for 1.92x the latency on each. A
  part on a shared core takes nearly twice as long for a 4% throughput
  gain.
- **The main thread is idle for most of the sweep.** `meanwhile` (the
  arena build, ~450 ms) finishes long before a ~2000 ms sweep, and the
  main thread then blocks in `join` holding the machine's best core,
  which spec 0265 reserved for it.

## Goals

- **G1.** The sweep's makespan is shortened on a machine whose kernel
  says which cores are fast — by putting one worker per physical core,
  by lending the sweep the drawing core once `meanwhile` is done, and
  by moving a straggler onto a core that has gone idle.
- **G2.** On every other machine — a VM, CI, anything under a
  `taskset`, anything whose kernel is silent — nothing changes at all,
  by the same rule spec 0264 already uses.
- **G3.** The ranking is bit-for-bit what it is today. Which core walked
  which part must not reach the answer.

## Non-goals

- **N1. The override pane is out of scope here — but it is the same
  problem, and it should get the same treatment in its own spec.** A
  `Tier::User` query over the document root is not a small job: 24
  parts, **15.17 s of P-work, median part 512 ms, worst 1.53 s** — 2.4x
  the mean, the identical straggler ratio to startup's 956/404, at 1.56x
  the scale. And it runs on a dedicated machine: spec 0262 S8's
  `next_task` sets `floor = User` while one is live and withholds every
  lower tier, so nothing else is running. Every measured premise of this
  spec holds there, and a migration is 0.23% of a median part rather
  than 0.29%.

  It is even better served than startup in one respect: **the idle event
  is explicit.** A worker with no `User` part left does not merely find
  an empty cursor, it falls through to `condvar.wait` — the core has
  provably gone idle, which is what S4 needs to observe.

  Two things do not carry over, and are why this is a separate spec
  rather than an extension of this one:

  - **The pool's thread count is fixed at `spawn`,** so S2's one worker
    per physical core cannot be a spawn-time decision. It has to be a
    seat held only while a `User` query is live, with the surplus
    workers parking on the condvar they already park on — a different
    mechanism for the same effect.
  - **S3 must not carry.** At startup the main thread is provably
    blocked in `join`; during a `User` query it is blocked in `poll(2)`
    but must draw on the next keystroke, and a busy sibling costs that
    frame 1.8x (spec 0265). Lending the drawing core would buy roughly
    what it buys here — the ~170 ms of the Measured outcome below — at
    the cost of the responsiveness the pane exists to preserve.

  And it must stay `User`-only. A `Tier::Visible` query can also be
  large — a screenful at pane top 0 costs 3.29 s, dominated by the
  document root's own 25.6 MB range (spec 0262) — but `Visible` does
  not raise the floor, `Prefetch` keeps running beside it, and there is
  then neither a dedicated machine nor a clean idle event.

  Not to be confused with spec 0249's rejected "reserve a core for
  `User` work", which serialized the query onto one core at ≈5.5 s.
  Nothing here reserves anything; every worker still serves the query.
- **N2. No attempt to predict a part's cost.** Spec 0218 says a part's
  cost is not knowable before the walk and the measurements above
  confirm it: parts within 3.8% on group count vary 10x in cost. Every
  mechanism here is reactive — it responds to a core going idle and
  never to an estimate.
- **N3. No re-partitioning, and `SWEEP_PARTS` does not move.**
  Simulated over the measured costs, 24, 32 and 48 parts differ by a
  few percent, and migration absorbs the ragged last round that a
  part count tuned to the worker count would have been for.
- **N4. Nothing is decided from cache topology.** See the alternatives.

## Specification

- **S1. `affinity` publishes a worker seating plan, or nothing.**
  A `Vec<Seat>` — one CPU per *physical core* of `inherited − drawing
  core`, each flagged fast or not by the same `detect_fast` spec 0264
  uses, ordered fast cores first and by CPU number within that. It is
  `Some` exactly when spec 0264 acted *and* the sibling lists are
  readable; `None` otherwise, which is G2. A core contributes its
  lowest-numbered CPU, arbitrary but deterministic. An unreadable
  sibling list declines this spec alone: spec 0264 needs no topology
  beyond the fast set and must keep working without one.

- **S2. A seated worker pins itself to its seat, instead of calling
  `widen()`.** One worker per physical core, so no worker shares a core
  with another. `widen()` remains for every other spawn site and for
  the unseated case.

  **Corrected 2026-08-09 (spec 0270 S9).** As shipped, the seat count
  *replaced* `--jobs` rather than lowering it, so on a hybrid host
  `protolens -j 2` would have spawned 11 threads. `--jobs` is a ceiling
  and not a target (spec 0217 S4), and seating a member per core is a
  reason to spawn fewer threads, never a licence to overrun the number
  the caller allowed. No test caught it: `affinity::seats()` is `None`
  on every machine the suite runs on, and there the expression was
  already right.

- **S3. The main thread takes a seat on the drawing core once
  `meanwhile` returns, and pulls parts like any worker.** It already
  owns that core and is otherwise blocked in `join` — spec 0265's
  reservation is for the *drawing* thread while it draws, and during
  startup it is not drawing. This adds the machine's best core to the
  pool for the part of the sweep that matters, and it cannot lose on a
  corpus whose sweep is shorter than its arena build, because
  `meanwhile` still runs first and still runs on the fast core.

  It does not narrow itself to that seat. It keeps the whole physical
  core spec 0265 gave it and then blocks in `join`, so the seat is a CPU
  it has to hand out, not one it holds against anybody — and nothing has
  to give the mask back afterwards.

- **S4. A worker that finds the cursor empty donates its seat before
  it exits.** It looks for the busy seat that will finish last, and if
  its own seat is faster, re-pins that worker's thread onto it. This is
  the whole of the endgame: there is no deadline, no estimate of when
  the tail begins, and no separate sentinel — a worker reaching the end
  of the cursor *is* the event that a core has gone idle.

- **S5. The straggler is chosen as: on a slower seat than the donor's,
  and among those, running its part the longest.** `detect_fast` is
  binary, so "slower" means the donor is fast and the victim is not.
  Longest-running is the only signal available — a part's remaining
  work is unknowable (N2), and the parts still running at the end are
  the expensive ones. The choice is a pure function of the crew's
  published state, so it is testable on a machine that seats nobody.

- **S6. A donation is claimed by a compare-exchange on the victim's
  published seat.** Two workers can go idle at once and pick the same
  victim; the loser re-reads and looks again. Without this, one thread
  is pinned twice and one donated core is wasted.

- **S7. The re-pin must exclude the victim's current CPU.** Measured:
  `sched_setaffinity` on a running, never-sleeping thread does **not**
  move it while its current CPU is still in the new mask — that is spec
  0265's observation, and this is its precise scope. Excluding the
  current CPU forces the move, and it completes in under a millisecond,
  which is 0.5% of a mean part. So a seat is always a single CPU, never
  a set.

- **S8. Everything here is best-effort and silent.** A failed
  `sched_setaffinity` leaves the thread where it is and the walk
  finishes anyway; the ranking cannot depend on it (G3).

## Alternatives considered

### Replicate the straggler on the idle core, first answer wins

The original proposal. Rejected once it was clear the thread can simply
be *moved*: a replica throws away everything the slow core has already
done, needs a cancellation path into `score_subset`, and only pays when
the remaining fraction exceeds `1 − 1/ratio`. Migration keeps the
progress, needs no cancellation, and is never a loss.

### Exclude the cores with no L3

Built and measured, and it does not hold. On the reference host cpus
12-13 have no L3 at all and the scoring graph is 4.63 MiB, which fits
the 12 MiB L3 and never a 2 MiB L2 — a clean causal story that predicted
a large penalty. Measured at `-j 1`, where one part keeps all 49 255
roots alive, it is there: 4.25x. Measured at 24 parts, which is what
protolens actually runs, it is **2.85x against a pure-clock prediction
of 2.75x** — no anomaly at all, because partitioning shrinks the live
root set ~28x and the working set drops under the L2. And with S4 in
place the slow cores are worth keeping: simulated, dropping them is
worse than keeping them in every configuration.

### Make the part count a multiple of the worker count

24 parts over 14 workers is 14 + 10, so four workers idle through the
second round. Simulated over the measured costs the effect is a few
percent and it is subsumed by S4, which is what the even rounds were
meant to protect against. Not worth a second tunable.

### Balance `partition_roots` by group count instead of root count

`partition_roots` balances on `p.len()`, the root count, though its own
doc comment explains that the state group is the unit of work. The
result is real and ugly: three of twenty-four parts hold **23.7% of the
roots and do 0.82% of the work**, because one group is one traversal
however many roots share it. But balancing by groups changes the
makespan by 1-3% — group count does not predict cost either (768-797
groups, 10x cost). It is a defect worth fixing on its own terms, in
`prototext-graph`, and not part of this spec.

### Put the main thread on a slow core for the whole sweep

Simulated to exactly the same makespan as S3 (1598 ms either way), and
strictly worse on a small corpus: it slows the arena build 1.52x when
the arena build, not the sweep, is the critical path. S3 gives the
sweep the same core without ever slowing `meanwhile`.

### Re-pin per unit of work rather than on a donation

This is spec 0265 S5, dropped there as unnecessary, and it is
unnecessary here for the same reason: seats do not change except at a
donation, and a donation already does the re-pin.

## Test plan

1. `a_seating_plan_is_one_cpu_per_physical_core` (affinity) — on the
   fabricated host root, the plan is one CPU from each of core {2,3}
   and cpus 4-13, with the drawing core absent and the fast core first.
2. `a_silent_kernel_seats_nobody` (affinity) — an empty root yields
   `None`, which is G2.
3. `a_rescue_takes_the_longest_running_slow_seat` (sweep) — the pure
   chooser, over a fabricated crew: prefers a slow seat to a fast one,
   the longest-running among the slow, and answers `None` when the
   donor is not faster or every seat is done.
4. `a_rescue_is_claimed_once` (sweep) — two donors racing for one
   victim; exactly one wins and the loser moves on.
5. `every_thread_count_produces_the_ranking_one_thread_produces`
   (sweep, existing) — G3, unchanged, and it already covers the seated
   path because seating never changes which parts exist.
6. `a_migrated_thread_leaves_its_cpu` (affinity, Linux) — a thread
   spinning on one CPU, re-pinned to another with the first excluded,
   is observed on the second. Guards S7, which is the one kernel
   behavior the design rests on. Skips where fewer than two CPUs are
   available.

## Measured outcome

**Unmeasured, and cannot be measured where it was written.** The
development VM's kernel publishes no `cpu_core/cpus` and no
`cpu_capacity`, and every `thread_siblings_list` names one CPU, so
`detect_fast` declines, `affinity::seats()` is `None`, and every line
of this spec is inert there — which is G2 working, and also why the
gain cannot be observed. Spec 0265 is unmeasured for the same reason.

What *is* measured is everything the design rests on, on the reference
host and reproducibly (one part at a time under `taskset`, 24 parts):
the per-part cost spread, the three tier factors, the SMT figure, and
that `sched_setaffinity` moves a running never-sleeping thread in under
a millisecond when and only when the new mask excludes its current CPU.
Simulated over those costs across 3000 random hand-out orders, the
makespan goes from 2148 ms (mean) / 2692 ms (p95) today to **1598 /
1793** — −26% and −33% — of which S4 is the large term and S3 is
about 170 ms. At 87% of startup, that is roughly −22% of startup.

The number to record when a host with a hybrid kernel is available is
the sweep's own duration, not `protolens ... quit` wall clock: the
shared cursor randomizes which core gets which part, so differenced
totals mix the effect with a fresh scheduling lottery and have already
produced a spurious "17.6% win" once.
