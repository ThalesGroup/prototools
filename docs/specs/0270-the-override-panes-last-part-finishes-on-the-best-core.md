<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0270 — the override pane's last part finishes on the best core

Status: implemented
Implemented in: 2026-08-09
App: protolens
Refs: docs/specs/0269-the-last-part-of-a-sweep-finishes-on-the-best-core.md
        (the seating plan, the chooser and the migration mechanism, which
        this lifts out of `Pull` into a type both callers drive — read it
        first; this spec is its N1),
        docs/specs/0262-the-machine-shares-a-screenful-not-a-query.md
        (the part pool, the `Tier::User` hand-out floor that makes the
        machine dedicated, and the measured cost of a root query),
        docs/specs/0264-the-thread-the-user-watches-takes-the-fast-core.md
        (`detect_fast`, and `widen()`, which a worker must go back to
        between queries),
        docs/specs/0265-the-drawing-core-belongs-to-the-drawing-thread.md
        (the core this spec does *not* lend to the pool, and the 1.8x
        that says why),
        docs/specs/0249-a-large-document-answers-the-user-first.md (its
        rejected "reserve a core for `User` work", which this is not)

## Background

Spec 0269 shortened the startup sweep's makespan by seating one worker
per physical core and moving the straggler onto the first core to go
idle. The override pane runs the same shape of job through a different
machine, and gets none of it.

A `Tier::User` query is one deliberate act — a reader asking what this
range could be — and it is answered by a complete scoring of that range.
It is commonly the whole document: overriding the root's type is the
ordinary way to open a blob whose schema protolens guessed wrong. On
googleapis that is the corpus's most expensive query, measured (spec
0262): **24 parts, 15.17 s of P-work, median 512 ms, worst 1.53 s** —
2.4x the 632 ms mean, the same straggler ratio as startup's 956/404 ms,
at 1.6x the scale.

It runs on a dedicated machine, which is the condition spec 0269 needed.
`next_task` raises the hand-out floor to `Tier::User` while one is live
(spec 0262 S8) and withholds every lower tier, so no `Visible` or
`Prefetch` work competes with it for a core or for memory bandwidth.

And the pool takes that job exactly as the startup sweep took its own
before spec 0269: `spawn_worker` calls `widen()`, every worker gets
`inherited − drawing core`, the kernel places them, both hyperthreads of
a physical core are fair game, and the last part finishes on whatever
core it happened to draw. On the reference host that core may be 2.85x
slower than the best one, or 1.92x slowed by a sibling walking another
part of the same query.

The waiting is more visible here than at startup. The pane is empty
until the answer lands, and somebody is watching it.

## Goals

- **G1.** A `Tier::User` query's makespan is shortened by the two
  mechanisms of spec 0269 — one worker per physical core, and a
  straggler moved onto a core that has gone idle.
- **G2.** On a machine whose kernel does not say which cores are fast,
  nothing changes at all. Same gate, same `affinity::seats()`.
- **G3.** The candidate list is bit-for-bit what it is today. Which core
  walked which part must not reach the answer.
- **G4.** The drawing thread keeps the whole physical core spec 0265
  gave it, for the entire query. A keystroke arriving mid-query is drawn
  at the frame cost 0265 bought, not at 1.8x it.
- **G5.** Between `User` queries the pool is what it is today: every
  worker on every CPU the process was given, which is what `Visible` and
  `Prefetch` throughput work wants.
- **G6.** One crew, one chooser, one migration path. Spec 0269's
  machinery is *driven* by the pool, not copied beside it — including
  the one thing here that is genuinely hard to get right twice, the
  published state a donor reads while its victim is inside a walk.

## Non-goals

- **N1. The drawing core is not lent to the pool.** This is spec 0269 S3
  and it must not carry. At startup the main thread is provably blocked
  in `join`; during a `User` query it is blocked in `poll(2)` and must
  draw on the next keystroke. Lending its core would buy roughly what it
  buys at startup — ~170 ms of a 1.6 s query — at the cost of the
  responsiveness the pane exists to preserve (G4). `affinity::seats()`
  already excludes that core, so this is the default and not an
  exclusion this spec has to make.

- **N2. `Tier::Visible` and `Tier::Prefetch` are untouched.** A
  `Visible` query can be large — a screenful at pane top 0 costs 3.29 s,
  dominated by the document root's own 25.6 MB range (spec 0262) — but
  `Visible` does not raise the floor. `Prefetch` keeps running beside it,
  so there is neither a dedicated machine nor a clean idle event, and
  the median such query is 5.4–10.4 ms, where a migration is not a
  rounding error. Seating is for the tier that owns the pool.

- **N3. The pool's thread count does not change.** Threads are spawned
  once and live for the process. Making the pool one thread per physical
  core would halve it for the `Visible` and `Prefetch` work it does the
  rest of the time — throughput work with many independent items and no
  straggler to rescue, which is exactly the case where SMT's 1.04x is
  worth having. Seating is therefore a state a worker enters while a
  `User` query is live, not a property of the pool.

- **N4. No attempt to predict a part's cost.** Inherited from spec 0269
  N2 unchanged: a part's cost is not knowable before the walk, every
  mechanism here reacts to a core going idle, and nothing estimates.

- **N5. `SWEEP_PARTS`, `target_parts` and `Partition` are untouched.**
  Spec 0262 S1 builds the partition once for the process and spec 0269
  N3 settled the part count. Migration absorbs the ragged last round.

- **N6. This is not spec 0249's rejected "reserve a core for `User`
  work".** Nothing is reserved and nothing is serialized; every seated
  worker still walks parts of the query. What 0249 rejected put the
  whole query on one core, at ≈5.5 s.

## Specification

- **S1. The crew becomes one type in `sweep.rs`, and both callers drive
  it.** Spec 0269's `Chair`, `Busy`, `VACANT` and `rescue` are private
  to `sweep.rs` and reachable only from inside `Pull`. They are lifted
  into a `Crew` that owns them, with this surface and no other:

  | | |
  |---|---|
  | `Crew::new(members)` | one chair per member, plus the `Instant` that `since` is measured from. **The only caller of `affinity::seats()` in the program.** |
  | `Crew::seated(i)` | is member `i` owed a seat? S2's whole verdict, answered once. |
  | `Crew::member(i)` | a `Member<'_>` handle — an index and a borrow, nothing else. |
  | `Member::barred()` | is this member kept off the work the seats exist for? S3's whole verdict. **Not `!seated()`** — where the crew seats nobody it bars nobody, or a query nobody may walk would never finish. |
  | `Member::sit()` | take `seats()[i]` and move onto it (spec 0269 S2). A no-op when unseated, so an unseated caller needs no branch. |
  | `Member::lend(seat)` | take a seat *without* narrowing to it (spec 0269 S3) — the drawing seat, and startup's main thread is its only caller. |
  | `Member::walking()` | an RAII guard: stamps `since` on creation, clears it on drop. |
  | `Member::leave()` | donate this chair's seat to the straggler and vacate it (spec 0269 S4-S7). |

  Spec 0269's shipped code is refactored onto it: `Pull` keeps its
  cursor and its `parts`, and loses `crew` and `epoch` in favor of one
  `Member`. That is a refactor with no behavior change, and the tests
  that already exist are what say so.

  **`Member::walking()` is a guard rather than a pair of calls** because
  the pool's walk arm leaves by four paths — a deposited run, a shutdown
  `break`, an abandoned part, a superseded epoch — where startup's loop
  leaves by one. A `since` left standing on an idle member makes it look
  like the oldest straggler and attracts every donation on the machine.

  What is deliberately **not** shared is the loop around it. Startup
  draws parts from a slice cursor, keeps its runs, and returns them; the
  pool blocks on a queue, serves admissions and cache writes between
  parts, and deposits each run for whoever finishes last. Those two
  share the word "loop" and nothing under it — see the alternatives.

- **S2. A member's seat is its index.** Member `i` holds
  `affinity::seats()[i]`; a member with `i >= seats().len()` has no seat
  and never gets one. Static, deterministic, and it needs no claiming
  protocol and no notion of when a `User` regime starts or stops — which
  is what makes S3 a filter rather than a lifecycle.

  Startup already sizes its crew so that every member is seated. The
  pool cannot (N3), so on the reference host it is `sweep_jobs − 1 = 13`
  workers over 11 seats (one P core, eight E, two LP-E; the drawing core
  is not among them), and two workers are unseated. `Crew::seated` is
  the one place that difference is expressed.

  The main thread's chair is the exception and is why `Member::lend`
  exists: it is member `threads`, `Crew::seated` may well say yes of
  that index, and it must still take the drawing seat rather than
  `seats()[threads]`. It never calls `sit()`.

- **S3. Only a seated worker walks a `Tier::User` part.** `next_task`
  learns whether its caller is barred — it asks `Member::barred`, it
  does not re-derive it — and where the pool is seated at all it does not
  hand a `Task::Walk` of a `User` part to a worker that is not. The
  unseated worker falls through to the `condvar.wait` it already falls
  through to, and the wakeup it needs already exists: `end_sweep`
  notifies all, which spec 0262 S8 relies on as "the handover back to
  everyone who stood aside".

  This is spec 0269 S2's one-worker-per-physical-core, expressed against
  a thread count that cannot change. Where `affinity::seats()` is `None`
  nobody is barred, which is the whole of G2.

  `Task::Admit` is not withheld. It is not a walk — it re-checks the
  cache and activates the parts, microseconds against a 512 ms part —
  and withholding it would leave a query unadmitted while its admitter
  slept.

- **S4. A seated worker sits before a `User` part and widens after its
  last one.** `Member::sit()` before the walk, `Member::walking()` for
  its duration. When it finds no `User` part to take it calls
  `Member::leave()` and then `affinity::widen()`, so the next `Visible`
  or `Prefetch` task it serves runs on every CPU the process was given
  (spec 0264 S7).

  That `widen()` is G5, and it is the one thing the startup sweep never
  had to do, because its workers exit instead. It stays at the call site
  rather than inside `Member::leave()`: startup's main thread also
  leaves its chair, and widening *it* would hand away the drawing core
  spec 0265 reserved.

- **S5. The endgame is spec 0269 S4-S7, unchanged, on a better idle
  event.** `Member::leave()` gives this chair's seat to the member that
  has been walking its part the longest on a slower seat, claims the
  victim by compare-exchange, and re-pins it to a mask that excludes the
  CPU it is on. Not one line of that is written twice: startup infers "a
  core has gone idle" from an empty cursor and the pool from a worker
  about to block on a condvar, and past that point they are the same
  code.

- **S6. The crew outlives a query; the chairs do not.** The pool holds
  one `Crew` for its whole life, built in `HeatWorkerHandle::spawn`
  beside the `Partition` that is already built once per process (spec
  0262 S1). Every chair is vacated by its own `leave()` — the last
  worker to finish donates to nobody and vacates all the same — so a
  drained query leaves every chair vacant and the next `User` query
  re-seats from S2. No reset, and no code that has to notice a query
  beginning or ending.

- **S7. Everything here is best-effort and silent.** A failed
  `sched_setaffinity` leaves the thread where it is and the part
  finishes anyway. G3 follows from spec 0262 S4's merge, which restores
  one total order however the runs arrive.

- **S8. Two crews never sit at once.** Sharing the type makes this
  sharper, not softer: `sweep::ranked_with`'s crew and the pool's crew
  are now two instances of `Crew` over one set of seats, and nothing in
  `Crew` enforces that only one is live. They cannot coexist today — the
  startup sweep runs inside `decode::decode`, before `App` exists and
  before the pool is spawned, and the only other `sweep::ranked` caller
  is `heat_cue`'s synchronous arm, whose own precondition is that it is
  "only reached when there is no worker" (spec 0217 S6). Recorded as a
  standing invariant so that a third caller has to think about it.

- **S9. Two corrections to spec 0269, found while lifting its code.**
  Both are in the startup sweep and neither can bite on a machine that
  seats nobody, which is why nothing caught them.

  **The seat count replaced `--jobs` instead of lowering it.** Spec 0269
  spelled the spawn count `seats.map_or(workers, len).min(parts.len())`,
  so on a hybrid host `protolens -j 2` would have spawned 11 threads.
  `--jobs` is a ceiling and not a target (spec 0217 S4, and
  `effective_jobs`' own contract), and seating a member per core is a
  reason to spawn *fewer* threads, never a licence to overrun what the
  caller allowed. It is now `.min(workers)` as well.

  **The startup line named the ceiling, not the threads.** `protolens:
  inferring root type (24 MB) on 14 threads...` printed
  `effective_jobs(--jobs)`, which since spec 0269 is the one number
  guaranteed *not* to be the answer: the seating can lower it and the
  calling thread can join it. `ranked_with` is where the count exists,
  so it is now where it is reported from — its `meanwhile` closure is
  handed the real count, and `determine_root_type_meanwhile` passes `0`
  on the three paths where no sweep runs, which subsumes the
  `Infer`-and-a-graph test `main` used to make for itself. No new
  parameter on the two inner functions; one on
  `resolve_root_type_and_arena`, which owns the closure.

## Alternatives considered

### Lend the drawing core, as spec 0269 S3 does

Simulated to be worth about the same ~170 ms it is worth at startup. It
is rejected on G4 rather than on value: the main thread is not idle
during a `User` query, it is waiting for input it must serve, and a busy
sibling costs that frame 1.8x (spec 0265). Startup can lend the core
because nothing is going to be drawn until the sweep is over.

### Claim seats from a shared pool instead of by worker index

A seat cursor reset at the start of each `User` regime. It buys nothing
— the assignment is the same set of seats either way — and it needs
something to notice when a regime begins and ends, which is the one
thing S2 and S6 are shaped to avoid. Index seating is a pure function of
the member's own number, which is also what lets startup and the pool
share `Crew::seated`.

### Share the pull loop as well as the crew

The obvious next step from S1, and it does not survive contact. Startup's
loop draws from a slice cursor, keeps every run it produces, and returns
them to a `join`; the pool's blocks on a `Mutex`+`Condvar`, may get an
admission rather than a part, chooses its cancel flag by tier, checks an
abort epoch, deposits its run for whichever worker finishes last, and
sends a progress event. A trait over those two would have one method
that means something different on each side and a parameter list that
reconstructed the difference. What they actually share is the *state a
donor reads*, and that is exactly what `Crew` is.

### Put the crew in `affinity` rather than `sweep`

`affinity` is where `Seat` lives, so it looks like the home. But a chair
is not a fact about the machine, it is a fact about a member of a
running crew — and `sweep.rs` already holds `Chair`, `rescue` and
`Partition`, and is already a dependency of both callers. `affinity`
keeps answering only what the kernel said.

### Let the surplus workers run lower-tier work instead of parking

Tempting, since they are idle. But the floor is what makes the machine
dedicated: `Prefetch` walking beside the query would compete for exactly
the memory bandwidth the query is bound by, and spec 0262 S8 raised the
floor to stop that. Parking is not a cost this spec adds — those workers
already park whenever a `User` query owns the pool.

### Spawn the pool one thread per physical core

Would make seating a spawn-time decision and delete S3 entirely. It also
halves the pool for the `Visible`/`Prefetch` work that is most of what
it does, where there is no straggler and 1.04x of throughput for free is
worth taking. See N3.

### Give a `User` query its own dedicated threads

A second pool, seated, spawned per query. It pays a thread-creation cost
inside the latency the pane is waiting on, duplicates the queue, and
solves nothing that S3 does not: the workers that would be idle are idle
either way.

## Test plan

The first three are `sweep` tests of the shared type, and they are what
makes the rest of the plan short — the pool's tests then only have to
show that it drives `Crew` at the right moments, not that `Crew` is
right.

1. `a_walk_mark_is_cleared_however_the_walk_ends` (sweep) — S1's guard:
   `since` reads non-zero inside `walking()` and zero after the guard
   drops, including on the early-return and panic paths. Without this
   an idle member looks like the oldest straggler and attracts every
   donation on the machine.
2. `a_crew_seats_its_first_members_and_no_others` (sweep) — S2 over a
   fabricated seating: `seated(i)` is true below the seat count and
   false at and above it, and false for every `i` when nobody is seated.
3. `a_rescue_takes_the_longest_running_slow_seat`,
   `a_rescue_is_claimed_once` (sweep, existing) — S5's decision and its
   race, already written for spec 0269 and now covering both callers.
4. `every_thread_count_produces_the_ranking_one_thread_produces` (sweep,
   existing) — G3, and the guard on S1's refactor of `Pull`: the startup
   sweep's answer does not move when its crew moves out from under it.
5. `an_unseated_worker_does_not_take_a_user_part` (heat_worker) — with a
   fabricated seating of one seat and a live `User` query, the unseated
   caller is offered no `Task::Walk` and the seated caller is offered
   the same part.
6. `an_unseated_worker_takes_a_user_part_when_nobody_is_seated`
   (heat_worker) — the same queue with `seats()` unset hands the part to
   anyone. G2, and the reason every existing pool test keeps passing on
   a VM.
7. `a_user_query_is_served_when_only_one_worker_is_seated` (heat_worker)
   — all 24 parts land with a single seated worker among several, i.e.
   S3 cannot strand a query.
8. `a_drained_query_leaves_every_chair_vacant` (heat_worker) — S6, so
   the next query re-seats from S2 rather than inheriting a rescue.
9. `a_worker_widens_after_its_last_user_part` (heat_worker) — S4 and G5.
   The mask itself turned out not to be observable: `affinity::widen()`
   is inert wherever `apply()` declined, which is everywhere this test
   can run, so asserting "the mask is not a single CPU" would assert
   that the test machine is a hybrid host. What is asserted instead is
   the decision that drives the widen — `Member::leave` reports a seat
   given back on the first call and not on the ones after, so a worker
   parking repeatedly does not re-widen.
10. The existing `heat_worker` query tests — G3 on the pool's side, and
    unchanged, since they run where `seats()` is `None`.
11. `every_thread_count_produces_the_ranking_one_thread_produces` and
    the `batch_export` suite cover S9's second half by construction: the
    startup line is printed from inside `ranked_with`'s `meanwhile`, and
    `tests/batch_export.rs` already asserts a successful `export` writes
    nothing at all to stderr.

## Measured outcome

**Unmeasured, and unmeasurable on the development VM** — the same
verdict specs 0265 and 0269 recorded, and for the same reason: no
`cpu_core/cpus`, no `cpu_capacity`, and every `thread_siblings_list`
names one CPU, so `detect_fast` declines, `affinity::seats()` is `None`,
`Member::barred` is false for every worker, and no chair is ever
occupied. All of this spec is inert here, which is G2 working. Seeing
the gain needs a host whose kernel declares a hybrid topology.

What *is* verified here is that the inert path is unchanged: the whole
suite passes, including the 38 `heat_worker` tests and the 14 `sweep`
tests, five of which fabricate a seating precisely because the machine
will not supply one.

When it is measured, the number to record is the query's own duration —
the interval from the `User` push to the candidate list landing in
`by_range` — and **not** a wall-clock total. The part cursor randomizes
which core draws which part, so differenced totals mix the effect with a
fresh scheduling lottery, and have produced a spurious "17.6% win" once
already (spec 0269, Measured outcome).
