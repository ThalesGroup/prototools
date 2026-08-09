<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0262 — the machine shares a screenful, not a query

Status: implemented
Implemented in: 2026-08-09
App: protolens
Refs: docs/specs/0217-the-sweep-is-divided-among-the-cores.md,
        docs/specs/0218-the-sweep-is-pulled-from-a-shared-cursor.md (the
        sharded sweep, its 24-part partition and the cursor that hands
        parts out),
        docs/specs/0250-the-machine-works-on-what-the-user-waits-for.md
        (the `jobs` workers, the `fanout` mutex, the parked speculative
        sweep — this spec dissolves all three),
        docs/specs/0252-a-cue-nobody-is-looking-at-is-not-worth-a-sweep.md
        (a `Visible` request is stamped with its window; unchanged here)

## Background

A screenful is not one heat query. `heat_cue.rs`'s `heat_tier_for`
returns `Tier::User` for the cursor row alone and `Tier::Visible` for
every other drawn row, so drawing a 50-row pane over cold rows asks for
**one `User` query and about forty `Visible` ones**.

Spec 0250 gave those queries the arrangement a *single* urgent query
wants: each one fans out over every core, and `HeatRequestQueue::fanout`
lets only one run at a time. Forty of them therefore run one after
another, each using the whole machine for a range of a few bytes.

Measured on googleapis.desc (25.6 MB, 49 255 scoring roots), release,
`taskset -c 4-11` — 8 CPUs, so `jobs` = 7, the same
`available_parallelism() - 1` a real session would use. Ranges are the
distinct `heat_scored_range`s of the rows a 50-row pane draws at four
places in the document:

| pane top row | distinct ranges | payload bytes (min / median / max) |
|---|---|---|
| 0 | 41 | 1 / 8 / **25 660 332** |
| 5 000 | 37 | 1 / 16 / 558 |
| 200 000 | 41 | 1 / 3 / 89 |
| 2 000 000 | 37 | 1 / 8 / 17 |

Three facts fall out of that table and the timings beside it.

**A `Visible` range is a few bytes, and its query still costs
milliseconds.** `sweep::ranked` on a 7-byte payload at `jobs = 1` is
**8.4 ms**; the median real `Visible` query is 5.4–10.4 ms. The cost is
per *root*, not per byte: the walk sets up and rejects 49 255 candidate
types whatever the payload is.

**Fan-out therefore buys nothing on a range that size.** Summed over a
screenful, one thread against seven:

| pane top row | S (fan-out speedup) |
|---|---|
| 0 | 4.21x |
| 5 000 | 1.31x |
| 200 000 | 1.07x |
| 2 000 000 | 1.05x |

Row 0's 4.21x is the *document root*, whose range is the whole 25.6 MB
file; strip it out and fan-out is worth 5–30% on real visible rows.
Worse, it is billed at a fixed price: `partition_roots(graph, 24)` costs
**7.3 ms** and is rebuilt on **every** query, so the fanned-out path
spends 7.3 ms of serial setup to save a fraction of a millisecond.
(`partition_roots(graph, 1)`, the un-sharded path's, is 12 µs.)

**And a `User` request can wait for a whole `Visible` query.** The urgent
arm takes `fanout` and holds it across the entire `inferred_candidates`
call, with nothing consulted inside. An override-pane ask made while the
root row's `Visible` query is in flight blocks for that query's full
**3.4 s**.

**`Tier::User` is also too broad to be a priority.** It has three
producers: the override pane (`override_select.rs:356`, `:420`) and the
cursor row's own cue (`heat_cue.rs:356`), evaluated every frame. The
first is a deliberate act a human is blocked on; the second happens on
every arrow key that lands on a cold row. Anything the top tier is
allowed to cost has to be affordable at keystroke rate, which is why the
tier as it stands cannot be given the one power it needs.

The net effect is that on the first screenful of a large file the heat
subsystem uses one core's worth of the machine to do forty cores' worth
of asking, and the one query the user is actually waiting on queues
behind the rest.

## Goals

- **G1.** A screenful of `Visible` queries settles in the time the
  machine needs for the work, not in the time one query at a time needs.
- **G2.** A `User` request starts immediately — not after one part, and
  not after one query — whatever the size of what is running.
- **G3.** Work that depends only on the graph is done once for the
  process, not once per query.
- **G4.** No arrangement is catastrophic for any mix of query sizes. The
  screenful containing the document root and the screenful containing
  forty three-byte fields are both ordinary cases.

## Non-goals

- **N1.** The scoring walk, the ranking and its total order, and the
  value of `SWEEP_PARTS` are unchanged. This spec redistributes existing
  work; it does not make any of it cheaper.
- **N2.** A part is not made resumable. `score_subset`'s partial scores
  are meaningless, so an interrupted part is discarded and redone in
  full — S8 spends work to buy latency, and does not pretend otherwise.
- **N3.** The per-root fixed cost — 8.4 ms to reject 49 255 roots
  against seven bytes — is not addressed. It is the real ceiling on
  everything here and it is a prototext-graph question, not a scheduling
  one.
- **N4.** The startup root-type sweep (`sweep::ranked_with`, called from
  `main`) is untouched. It is one query over the whole blob with nothing
  else pending, which is precisely the case fan-out was designed for.
- **N5.** Nothing about *which* rows are asked for changes. Spec 0252's
  window stamping, the tier assignment, the read-ahead wave and the
  queue's merge-on-push all stay as they are.

## Specification

- **S1. One partition, built once.** `partition_roots(graph, n)` is a
  pure function of the graph and the part count, and the part count is
  fixed for the process (it comes from the worker budget, which is fixed
  at spawn). It is built once, next to the worker pool, and every query
  borrows it.

  This removes 7.3 ms from every fanned-out query and from every
  speculative one — `Resumable::new` pays it too, and a speculative
  query's whole walk is only about 5 ms.

  The part count keeps coming from `target_parts(workers)`, so the
  one-worker escape hatch survives: cutting a single-threaded sweep into
  24 parts is faster on googleapis and *slower* on pdb, and `--jobs 1`
  is the fallback for a loaded machine, which must not be where a corpus
  loses.

- **S2. The unit of work is a part of a query, not a query.** A popped
  request becomes `parts.len()` tasks in a shared pool; a worker takes
  one task, walks it, and comes back for whatever is highest priority
  next. There is no per-query owner and no per-query thread count.

  This is what makes the arrangement adapt with no policy to tune. One
  25.6 MB query is 24 tasks and every worker gets some; forty 3-byte
  queries are 960 tasks and every worker gets some; a mixture balances
  itself. The two ends of that range are exactly the two screenfuls
  measured above, and today no single fixed fan-out width is right for
  both.

- **S3. Task order is the owning query's tier, then the queue's order.**
  The queue already ranks requests (spec 0252's LIFO head-insert, merge,
  tail eviction); a task inherits its query's place. A `User` query's
  parts therefore go to the head, and every worker picks them up as it
  finishes its current part.

- **S4. A query is complete when its last part is walked, and the worker
  that walks it merges and records.** The runs accumulate per query and
  are merged with the same `Merged` the sharded path uses, so the
  ranking is bit-for-bit the one a single-threaded sweep produces —
  `candidate_order` is a total order and that is what makes a sharded
  ranking identical to a whole one (spec 0217).

  Charging the merge to a worker, not to whoever collects afterwards, is
  not bookkeeping: it is 2.3x on a screenful. Merged serially the
  arrangement below costs 244 ms at pane top 200 000; merged in the
  workers it costs 96 ms.

- **S5. `fanout`, `ParkedSweep` and the yield protocol are deleted.**
  `HeatRequestQueue::fanout`, `park_sweep`, `resume_sweep`,
  `await_quiet`, `PopMode::UrgentOnly`, `urgent_live`/`urgent_live_locked`
  and `walk_until_yield`'s `should_yield` all exist for one reason: a
  query is owned by one worker for its whole duration and must therefore
  be *put down* when something more urgent arrives. Under S2 nothing is
  ever put down — a worker finishes a part and then simply takes the
  most urgent task there is. Preemption becomes the scheduler's default
  behavior rather than a protocol.

  This also settles spec 0250's standing hazard, that `in_flight` means
  "a walk is happening right now" and not "somebody intends to finish
  this". With no parked sweeps there is no second meaning to confuse it
  with.

- **S6. `jobs` has one reading.** `HeatWorkerHandle::spawn`'s doc
  comment currently explains why the same number is both a thread count
  and a fan-out width and why the two never add up. There is no fan-out
  width any more; `jobs` is the number of workers.

- **S7. `Tier::User` means the override pane, and nothing else.** The
  cursor row's cue (`heat_cue.rs:356`) drops to `Tier::Visible`. `User`
  becomes what the tier was always described as — a request that
  directly follows a deliberate act, with a human watching a pane that
  is empty until it lands.

  The cursor row keeps its *ordering* advantage without the tier: it is
  pushed at the head of the `Visible` band, so it is still the first cue
  a screenful settles. What it loses is the power S8 grants, which it
  cannot be trusted with — a held arrow key issues one `User` request
  per keystroke, and S8 at keystroke rate is a machine that discards
  work continuously and finishes nothing.

  This also removes a wart: `record_sweep`'s "the tier cannot be the
  test" gate (`heat_worker.rs:1014`) exists only because `Tier::User`
  currently includes the cursor row, so gating `CompleteLists` on the
  tier would record a 4 MB list for every row an arrow key passed over.
  Once `User` means the override pane, the tier *is* the test.

  The activity dot reads the tier too (`render.rs:1845`): a cursor cue
  moves from `(Red, 12)` to `(Red, 5)`. That is the honest reading —
  level 12 is for something the user is blocked on.

- **S8. A `User` arrival aborts in-flight parts below `User`.** The
  discarded parts go back in the pool and are redone later; nothing is
  cached from them, since a truncated `score_subset` yields a wrong
  score rather than a partial one.

  The mechanism exists — `score_subset` already takes
  `cancel: Option<&AtomicBool>` and polls it per wire field — and today
  only shutdown raises it. What is new is a flag the queue maintains
  meaning "a `User` task is live", handed to every task walking below
  that tier.

  **The flag alone is not enough: it needs an epoch beside it.** A part
  handed out, aborted, and then finding the flag *low* again — because
  the user's work finished while it was unwinding — would look like a
  completed walk and be cached, and a truncated `score_subset` is
  wrong, not partial. So the queue also counts raises of the flag; a
  task carries the count it was handed out under, and a part whose
  epoch has moved is discarded whatever the flag says now.

  **A worker that abandons a part must not be handed it straight back**,
  or the pool spins instead of standing aside. So `next_task` withholds
  every sub-`User` task while `User` work is live — queued *or*
  walking — and parks the worker on the condvar; `end_sweep`'s
  `notify_all` is what releases it. This is a consequence, not an extra
  rule: the flag and the hand-out floor are the same predicate read at
  two moments.

  **Abort is confined to this one boundary.** `Visible` over `Prefetch`
  must *not* abort: `Visible` requests arrive continuously throughout a
  scroll, so aborting on them would destroy read-ahead during exactly
  the scroll spec 0252 fixed. Below `User` the pool's ordering is
  preemption enough, and it discards nothing — a worker finishes its
  part and takes the most urgent task next, while the parts a deferred
  query has already walked simply stay walked.

  The waste is bounded by `jobs` × one part, and paid only when the
  override pane is opened or its selection moved. On this corpus that is
  at most 7 × 574 ms.

## Alternatives considered

### Finer parts instead of abort (built and measured; it does not work)

If a part were short enough, S8's abort would be unnecessary: waiting
one part would be imperceptible, and no work would ever be discarded.
S1 makes finer parts affordable — the partition is no longer rebuilt per
query — so this looked like the free version of G2. It is not.

Partitioning googleapis' 49 255 roots, walking the document root
(25.6 MB, the corpus's worst query) and a 7-byte payload:

| `n` | parts | biggest part (roots) | root walk | root part median / worst | small walk |
|---|---|---|---|---|---|
| 24 | 24 | 4 645 | 14.82 s | 518 ms / **1.51 s** | 9–10 ms |
| 64 | 64 | 4 645 | 16.75 s | 185 ms / **935 ms** | 9.9 ms |
| 256 | 256 | 4 645 | 19.32 s | 27 ms / **690 ms** | 9.1 ms |
| 1024 | 1024 | 4 645 | 22.98 s | 12.8 ms / **583 ms** | 9.9 ms |
| 4096 | 4096 | 4 645 | 24.42 s | 0.44 ms / **574 ms** | 10.0 ms |

Two things stop it. The walk gets **65% slower** on the root query
between 24 and 4096 parts, because a part carries fixed setup that finer
cutting multiplies — and the root query is the one case that matters
here. And the worst part **plateaus at ~574 ms** however fine the cut:
`partition_roots` keeps a Hopcroft state group whole, and the biggest
group is 4 645 roots at every `n`. There is a structural floor, and it
is three orders of magnitude above "no wait at all".

`SWEEP_PARTS` therefore stays at **24**. Moving to 64 would buy a 38%
shorter worst part for 13% more root-query walk, which is a real trade
but not one worth making once S8 makes the worst part something a
`User` request never waits for.

### One query per worker, single-threaded (the obvious fix)

`Visible` stops fanning out, stops taking `fanout`, and runs
single-threaded; up to `jobs` of them run at once. This is the direct
reading of the measurements above and it was built and measured.

It is 2.5–3.8x on an ordinary screenful and **4.6x worse than today on
the screenful that contains the document root**: 14.83 s against 3.57 s,
because the root's query is 25.6 MB and one core takes 14.5 s over it.
The first screenful of every session contains the document root. Ruled
out by G4.

### Cache the partition and change nothing else

Worth 1.8–2.4x on an ordinary screenful (409 → 225 ms at pane top
200 000) for a change that touches one function, and it is a strict
subset of this spec — S1 is exactly this.

Rejected as the *whole* answer because it leaves both structural
problems standing: the queries still run one at a time, so six of seven
cores are idle for the whole screenful, and a `User` request still
blocks behind an in-flight query for up to 3.4 s.

### A size threshold: fan out only above N bytes

Recovers G4's root case and keeps the simple one-query-per-worker
scheduler. Rejected because N is underivable: cost tracks neither bytes
nor any other property of the request that is known before the walk
(the root is 3 000 000x the median range but only 1 700x the median
query cost), and a threshold set from one corpus is a number the next
corpus falsifies silently — the failure mode is a slow pane, which
nobody reports.

### Keep the owner, let idle workers steal parts from it

The same distribution as S2, reached by a harder route: a query is still
owned, `ParkedSweep` and the yield protocol still exist, and stealing
adds a second way for two workers to be inside one query. S2 gets the
same balancing by *removing* the owner rather than by adding an
exception to it.

## Measured outcome

The four arrangements were measured against each other before
implementation, on googleapis.desc, release, `taskset -c 4-11`, `jobs`
= 7 — wall time for one screenful of distinct ranges. The last column
is the shipped code, re-measured the same way: a real `App`, a real
`HeatWorkerHandle::spawn`, one `Visible` push per distinct
`heat_scored_range` of a 50-row pane, timed until every one of them has
landed in `by_range`.

| pane top row | A: today | A': S1 only | B: one query per worker | C: predicted | C: shipped |
|---|---|---|---|---|---|
| 0 | 3.57 s | 3.34 s | 14.83 s | 3.20 s | **3.29 s** |
| 5 000 | 412 ms | 196 ms | 163 ms | 98 ms | **91 ms** |
| 200 000 | 408 ms | 225 ms | 111 ms | 96 ms | **93 ms** |
| 2 000 000 | 259 ms | 109 ms | 69 ms | 48 ms | **59 ms** |

**4.4–4.5x on an ordinary screenful and never worse than today**, and
within 20% of the projection at every pane top — the simulation
harness modelled the arrangement, not just the walk.

Preemption latency, G2: the corpus's most expensive query is the
document root, whose 24 parts take 15.17 s in total, median 512 ms and
**worst 1.53 s** (a 3.0x imbalance). Under S2 alone that worst part
would be the bound on how long a `User` request waits — better than the
3.4 s whole query it waits for today, and still far too long, which is
what S8 exists for. It is also the bound on how much work one abort
discards.

What is *not* improved: the 8.4 ms a query costs before any payload is
considered (N3). A screenful is about 350 ms of real work on this
corpus and this spec is about spreading it, not shrinking it. Cutting
the root count, or memoizing across queries with the same range, is
where the next factor is.
