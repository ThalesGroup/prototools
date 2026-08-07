<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0250 — the machine works on what the user waits for

Status: draft
App: protolens
Refs: docs/specs/0217-the-sweep-is-divided-among-the-cores.md (the
        sharded sweep, `--jobs` as a ceiling, the 16 MiB walker stack),
      docs/specs/0218-the-sweep-is-pulled-from-a-shared-cursor.md
        (`SWEEP_PARTS = 24`, the shared cursor, and its "what is not
        understood"),
      docs/specs/0224-the-frame-is-what-notices-a-scored-answer.md (the
        heat worker's current request/queue arrangement),
      docs/specs/0249-a-large-document-answers-the-user-first.md (split
        from the same draft; independent of this one),
      docs/specs/0251-a-cached-render-is-read-not-copied.md (S8 here is
        the same rule as its S9, for the other cache)

## Background

Work the user is waiting for queues behind work nobody asked for.

### The heat worker is a dispatcher, not the parallelism

There is one `heat-worker` thread, which reads as the bottleneck and is
not. `sweep::ranked` fans each request out itself: `target_parts` cuts
the roots into `SWEEP_PARTS = 24` parts and `min(jobs, parts)` scoped
threads pull them from a shared cursor. `terminal.rs` passes
`worker_jobs = sweep_jobs.saturating_sub(1).max(1)`, so on this 12-CPU
VM a single request already occupies ~11 threads. The scoring library
is concurrent; the dispatcher is serial *between* requests.

The real gap is **preemption**. Requests are tiered
`Prefetch < Visible < User` and the queue pops by tier, but a sweep
already in flight cannot be stopped: `stop_flag` is raised only on
shutdown. So a `User` request — the override pane's full ranked list,
the one the user is visibly waiting for — waits for a speculative
prefetch to finish before it gets a single core.

### One inference list is kept, and the wrong sweeps overwrite it

`HeatCaches::complete` is a single `Option<(Range<usize>, Vec<(String,
i64)>)>` slot, rewritten by *every* completed sweep
(`heat_worker.rs:595`, "always refreshed"). A user hunting overrides
alternates between a handful of fields; each return re-scores from
scratch, because a prefetch for somewhere else evicted the answer in
between.

### Measurements this spec is built on

| | corpus | figure |
|---|---|---|
| sweep, 24 parts, 12 workers | googleapis | **0.89 s** (was 1.89 s un-sharded across threads, spec 0218) |
| sweep total *work*, 1 part → 24 | googleapis | 6.96 s → **5.53 s** (`sweep.rs:144-147`) |
| sweep total *work*, 1 part → 24 | pdb | 0.072 s → **0.098 s** (same) |

**Partitioning is not more work on googleapis — it is 21% less, on one
thread.** Both sides of that row are single-threaded: one worker doing
1 part against one worker doing 24 parts in sequence. No parallel
overhead is involved, so the partition itself makes the walk faster,
and pdb goes the other way.

Why is not known — spec 0218's "what is not understood". The most
likely mechanism is locality: a part confined to fewer state groups
holds a smaller live candidate set, which may fit in cache where the
whole sweep does not. **If that is the mechanism, time is not a proxy
for energy** — the slower arrangement stalls more, drawing less core
power and more DRAM traffic, and which way that nets out is not
derivable from a stopwatch. So this row refutes "fewer, bigger queries
do less work"; it settles the energy question in neither direction, and
nobody has measured joules. See open question 3.

What it does settle is that keeping the partition costs nothing, which
is what S1 rests on.

## Goals

- **G1.** A `User` request gets the whole machine without waiting for a
  speculative *query* to finish — at worst for the *part* in hand.
- **G2.** Threads running the scoring walk never exceed
  `available_cpus()` in total, across every sweep in flight.
- **G3.** Returning to a recently visited range reopens the override
  pane without re-scoring.

## Non-goals

- **N1. No cancellation *inside* a walk.** Nothing is threaded through
  `score_subset` and no half-scored part is ever interpreted. S3 yields
  at part boundaries instead, which is a checkpoint the walk already
  has.
- **N2. `SWEEP_PARTS` is not retuned.** 24 is a fit measured across two
  corpora at three worker counts and three blob sizes; nothing here is
  evidence about it.
- **N3. No cache is merged with another.** `CandidateCache` was deleted
  because a shared MRU generic was not worth it at two call sites; a
  third would have to appear first.
- **N4. A new prefetch wave does not discard a parked sweep.**
  `start_new_wave` clears the *queue*; a parked sweep is worker-local
  state it cannot see. So a speculative sweep whose wave has been
  superseded still runs to completion. That is deliberate: it is already
  most of the way through, its result is not wrong — just no longer the
  most interesting range — and it lands in `by_range` where a later
  visit will find it. Discarding it would throw away work to save
  nothing anybody is waiting for.

## Specification

### How the work is arranged

- **S1. A speculative sweep keeps its 24 parts but walks them on one
  thread.** A `Prefetch` request is not spread across threads — it
  takes the parts one after another from its own cursor. Several such
  queries run at once, one per speculative worker, so the machine is
  filled by *whole queries in parallel* rather than by one query's
  shards.

  This requires decoupling the part count from the worker count:
  `target_parts(1)` currently returns 1, which is exactly the
  arrangement to avoid. Keeping the partition is free — it is the
  *faster* single-threaded arrangement on googleapis (5.53 s against
  6.96 s) — and it is what makes S3 possible at all.

  Estimated throughput: twelve whole queries in parallel finish roughly
  `12 / 5.53 ≈ 2.2` queries/s against roughly `1 / 0.89 ≈ 1.1`
  queries/s for sharded queries run one after another. *Derived from
  the table above, not measured as such.* Each individual query gets
  slower (≈5.5 s rather than ≈0.9 s), which is acceptable only because
  nobody is waiting for it — and only because S3 stops it being in the
  way.

  **IMPLEMENTED 2026-08-07** as `sweep::Resumable`: it partitions once
  in `new`, walks parts from a `next` cursor, keeps each part's ranking
  in `runs`, and hands the same lazy N-way `Merged` its parts in the
  same order the sharded path would have — so the ranking is not merely
  equivalent, it is the identical value.

  `target_parts` is **untouched**. The item above proposed changing what
  `target_parts(1)` returns, but that function answers a different
  question — how many parts a *sharded* sweep should cut for a given
  worker count — and its `1` is the `--jobs 1` escape hatch that makes a
  sweep single-part and so debuggable. The speculative caller names
  `SWEEP_PARTS` itself instead. Nothing else had to move.

  **Second half IMPLEMENTED 2026-08-07.** `HeatWorkerHandle` spawns
  `jobs` workers rather than one, each still fanning out to width `jobs`
  when it draws urgent work. `join: Option<JoinHandle>` becomes
  `joins: Vec<JoinHandle>` and shutdown joins them all.

  This needs one piece of mutual exclusion that S2 did not anticipate: a
  `fanout: Mutex<()>` held for the length of a fanned-out sweep. Without
  it G2 fails outright rather than marginally — a screenful of rows
  produces a `Visible` request each, every worker pops one, and every
  one of them spreads `SWEEP_PARTS` parts over the whole machine, giving
  `workers × jobs` walkers against `jobs` cores. Speculative sweeps do
  not take it: they are single-threaded by S1 and so compose freely.

- **S2. `User` and `Visible` requests take the whole machine.** They
  are latency-bound and someone is waiting, so they fan `SWEEP_PARTS`
  parts across every available core — not across whatever a reservation
  left over. Serving the override pane's full list in ≈0.9 s rather
  than ≈5.5 s is the entire point of the arrangement.

  **IMPLEMENTED 2026-08-07**, and it is the path that did not change:
  `User` and `Visible` keep calling `inferred_candidates` with the
  worker's `jobs`, which is what already fanned out over the machine.
  The new `(false, _) if req.tier == Tier::Prefetch` arm sits *above* it
  and takes only speculation away.

  The dividing line is named once, as `URGENT_TIERS = User | Visible`.
  It is a tier test rather than a "is this the override pane" test
  because `heat_tier_for` gives the cursor's own row `Tier::User` too —
  and that is correct here: the user is waiting on the cursor row's cue
  in exactly the sense this item means.

- **S3. A speculative worker yields at a part boundary.** When a `User`
  request arrives, speculative workers finish the part in hand, record
  their cursor, and park. The `User` sweep then has the machine.

  A part boundary is the only checkpoint needed: mean part ≈ 5.53/24 ≈
  **0.23 s**, and with the ~7x per-part imbalance the worst is order
  0.8 s, so a `User` request waits well under a second for the machine
  to clear and then runs at full width. Parts are independent, so a
  yielded prefetch **resumes from its cursor** rather than restarting —
  nothing is discarded and no partial result is ever interpreted (N1).

  This replaces reserving a core. A reservation would leave a `User`
  request one core and therefore ≈5.5 s, which is worse than today.

  **IMPLEMENTED 2026-08-07.** `walk_until_yield` polls a `should_yield`
  closure before each part and returns `false` if it fires, leaving the
  cursor where it stopped; the worker holds the half-walked sweep in a
  `ParkedSweep` and comes back to it.

  Three things this needed that were not obvious:

  1. **The gate counts the sweep that is *walking*, not just the one
     queued.** `urgent_live()` is `(queued | in_flight) & URGENT_TIERS`.
     On the queued half alone the handover would last only until the
     urgent request was *dequeued*: every parked worker would then see
     an empty queue and resume against the very sweep it stepped aside
     for. Counting it while it walks makes the handover last as long as
     the work — and S5's registry, built for an unrelated reason, is
     what makes that expressible with no new state at all.

  2. **A parked worker must not block on the queue.** It asks in a new
     `PopMode::UrgentOnly`, which serves urgent work and otherwise
     answers `None` immediately rather than waiting — it has work of its
     own to get back to. It also must not *spin*, which it would: the
     urgent request that made it yield may have gone to another worker,
     so `UrgentOnly` returns `None` while `urgent_live()` still holds.
     `await_quiet()` is the third state, sleeping on the condvar until
     the machine is handed back.

  3. **`end_sweep` has to `notify_all`.** By the time an urgent sweep
     finishes, the queue may be empty and no push is coming — so the end
     of that sweep is the *only* event that can release the workers
     parked behind it, and nothing else was going to wake them.

  4. **A parked sweep must *release* its range.** This spec first said
     the opposite — that a parked sweep stays registered in flight,
     because the range really is being swept and letting it look free
     would wave a duplicate straight through S4's rule. That reasoning
     is right about the duplicate and wrong about the priority.

     A sweep parks precisely because the user is asking for something,
     and while the user keeps asking it stays parked — which, during a
     scroll, is indefinitely. Registered, S4's rule would drop every
     request landing inside that range, every frame, for as long as the
     park lasts; the rows inside it would never resolve. The cost of
     releasing is at worst one duplicate sweep of a range nobody is
     waiting on. The cost of holding is a permanently unresolved cue.

     So `park_sweep` calls `end_sweep`, and `resume_sweep` takes the
     range back — or **refuses**, if another worker has picked it up
     meanwhile, in which case the parked sweep is abandoned rather than
     walked twice.

### What assumes a single worker

Four pieces of the queue do. None of them is unsafe — they are plain
atomics — but each degrades differently, and the differences decide how
much care each needs.

- **S4. Merge-on-push must extend to in-flight requests.** `push`
  merges on `range.start` only while a request is *queued*; once
  popped, a re-push of the same range starts a second live sweep of it.
  The results are identical, so this is pure waste — but the unit of
  waste is a whole query. Keep an in-flight set keyed on `range.start`
  and merge against it too.

  **IMPLEMENTED 2026-08-07.** The set lives in `HeatRequestQueueState`,
  under the same `Mutex` as the queue itself, and `pop_blocking`
  registers a request before releasing that lock. The two must be one
  atomic act: split across two locks there is a window in which a worker
  has taken a range off the queue but not yet announced it, and a second
  worker popping a re-push in that window sees an empty set and sweeps
  the range twice — the exact waste this item removes.

  A pop of a range already in flight **drops** the request rather than
  serving or requeueing it. Dropping is self-healing, because the reader
  re-checks the cache each frame and pushes again once the sweep in
  flight has written its answer, whereas requeueing would spin the
  worker on an entry it cannot act on. No ask is lost that way either:
  S8's `complete_wanted` is keyed on the range and consumed by whichever
  sweep answers it, not by the request that carried it.

  This changed nothing about the queue's ordering rules but did expose
  an assumption in five queue tests: they used `pop_blocking` as a plain
  dequeue and never declared the sweep over, so under the new rule a
  later push of a key they had already popped was correctly dropped and
  the test waited forever on a queue it had emptied itself. They now go
  through a `take_one` helper that models a whole worker turn — pop,
  then end. The deadlock was the test's, not the queue's, but it is the
  kind of thing worth stating: **a bare `pop_blocking` now leases a
  range, and every path out of one owes an `end_sweep`.**

- **S5. `in_flight` stops being a single-writer store.**
  `set_in_flight` *stores* one encoded tier (`heat_worker.rs:149-151`):
  worker B overwrites A, and whichever finishes first stores `None`
  while the other is still walking, so `activity()` reports **idle
  during live work**. That is a lying busy indicator and a wrong input
  to anything gated on activity — the worst of the four, because it is
  silent. It becomes a per-tier count, or a set the workers register
  in.

  **IMPLEMENTED 2026-08-07** as the set — the same one S4 needs, so the
  two items cost one structure between them. The `AtomicU8` survives as
  a *mirror* of it rather than as the state itself: `publish_in_flight`
  recomputes the tier mask under the lock and stores it, exactly the
  `publish_occupancy` idiom already in the file, which keeps
  `activity()` lock-free and makes the byte read 0 only when the set is
  empty. `one_worker_finishing_does_not_clear_anothers_in_flight_tier`
  pins it.

  Two existing activity tests had been written against the old
  single-store API and asserted its shape directly. Both were
  restatements of the same thing: that a request stops being reported
  when the worker *says* it is done, not when it leaves the queue. That
  claim survives — the moment it becomes true just moved earlier, from
  a `set_in_flight` call after the pop to the pop itself.

- **S6. `score_all_calls` becomes order-independent.** The
  `#[cfg(test)]` counter is read before/after in tests asserting that a
  sweep did or did not happen; with concurrent workers the delta no
  longer says *which* sweep ran. Test-only, but it makes those tests
  flaky rather than failing, so it must be fixed with the rest.

  **IMPLEMENTED 2026-08-07.** The counter becomes a
  `Mutex<Vec<(range.start, Tier)>>` log, and `score_all_calls()` its
  length — so every existing call site keeps working unchanged — plus
  `sweeps_of(range_start)`, which is what an assertion about one range
  should use, being independent of what any other range's sweeps did
  meanwhile. `a_prefetch_does_not_evict_a_user_list` was the one test
  whose assertion was genuinely of the flaky shape (a `+ 1` delta
  around a prefetch); it now names the prefetch's own range.

  (The fourth is `HeatCaches::complete`, S7's subject. Its failure is
  benign: the reader checks `range.start`, so a clobbered slot never
  serves the wrong list — it only misses.)

### The inference-list cache

- **S7.** `HeatCaches::complete` becomes a bounded MRU of **16** full
  ranked lists keyed on `range.start`, replacing the single slot. No
  invalidation: the key is a byte offset into an immutable blob.

  **IMPLEMENTED 2026-08-07** as `CompleteLists` in `heat_worker.rs`,
  holding **8** rather than 16 — see S9. A `Vec` in MRU order rather
  than a map, because at that cap a linear scan over 8 `usize`
  comparisons beats hashing and the MRU order is the storage order.
  A read promotes; that is what the cache is for, and
  `the_complete_cache_evicts_what_was_not_read` pins it.

- **S8. Only the `User` sweeps that serve the override pane write to
  it.** A prefetch must not evict the answer the user is alternating
  between — that is the whole defect. Prefetch results continue to land
  in `by_range`, which is capped and is what the cue reads.

  More generally: **a workload that scans the whole document writes to
  no cache.** A single pass visits every entry once, so under MRU
  eviction it gets a hit rate of approximately zero *and* leaves the
  cache holding the tail of its own scan for the interactive work that
  follows. Spec 0251 S9 states the same rule for the render cache.

  **IMPLEMENTED 2026-08-07**, with the rule sharpened twice by what
  implementation found.

  *The tier is the wrong test.* `Tier::User` is also the tier of the
  **cursor row's own heat cue** (`heat_tier_for`), so gating on it
  would record a 4 MB list for every row the cursor merely passed over
  — the same eviction defect from another source. What identifies the
  override pane is that it asks for the *whole* list:
  `upgrade_active_override_to_complete`'s `[0, usize::MAX)` is the only
  such request in the crate.

  *The asking request and the answering sweep need not be the same
  one.* Opening the pane pushes the bounded first page and then the
  unbounded list back to back (`open_override_on_default`); if the
  worker pops the first before the second is pushed, the two do not
  merge, and gating on the popped request's own `end` would make the
  pane pay a **second full sweep** for a list the first had already
  computed and thrown away. So the queue keeps a `complete_wanted` set
  of ranges somebody has asked the whole list of, and the worker
  consults it *after* its walk — by which point the pane's ask,
  microseconds behind against a walk of ~0.9 s, has landed. Entries are
  consumed on being answered.

  Three writers besides the worker exist and the spec named none of
  them; all three are the override pane's own list and all three keep
  writing: `seed_root_heat` (the startup ranking, so the root's pane
  opens free), `close_override` (the pane handing its list back), and
  `heat_cue_resolve`'s synchronous arm. That last one is a *cue* and so
  is an exception to the rule, stated at the call site: in the
  no-worker configuration `heat_lookup` has nothing to push a request
  to, so a whole-list ask can only ever be answered out of what a cue
  already computed — `by_range`'s `top_n` is a screenful and can never
  cover `usize::MAX`.

  What this costs, stated plainly: the *first* `t` on a range now
  sweeps, where before a cue's sweep for that same range might have
  left its list in the slot. In practice that saving was already
  illusory — the slot was clobbered by *every* completed sweep,
  including each of the prefetcher's, so by the time the user pressed
  `t` it almost never still held the cursor's range. That is the defect
  in this spec's Background. Second and later opens are now free, which
  is the trade.

- **S9. Size it before trusting 16.** Estimated at 49 255 roots ×
  ~90-100 B ≈ 4.5-5 MB per list, so 16 lists ≈ 80 MB — an estimate, and
  large enough that if it holds, the count is a decision and not a
  detail. Measure a real list first and record the figure in the
  constant's doc comment; adjust the count, not the rule.

  **MEASURED 2026-08-07.** The estimate held: on googleapis a real full
  list is 49 255 entries and **4.27 MB** — 1.58 MB of `(String, i64)`
  tuples plus 2.69 MB of FQDN bytes at a mean 54.7 B each.

  One guess in the estimate's neighborhood was wrong and worth
  recording: vetoing does *not* prune the list. Three real instance
  blobs left 100%, 99.6% and 19.9% of roots non-vetoed, so a full list
  is normally the whole root count and 4.27 MB is the ordinary case
  rather than a worst one.

  So the count is a decision, and it is **8**, not 16: ~34 MB on the
  largest corpus in hand, against ~68 MB for 16. The workload S7 exists
  for is a user alternating between *a handful* of fields; nobody has
  measured how many, and the smaller number serves that while halving
  the bill. It scales with the graph, not the document — twice the
  roots costs twice this.

  Recorded in the constant's doc comment along with the way out that is
  *not* a smaller count: two thirds of a list is FQDN bytes copied out
  of the archived graph, which `EntryScore` already borrows as
  `&'g str`. Borrowing them through would cost 1.18 MB a list rather
  than 4.27 MB — but it means threading the graph lifetime through
  `RankedCandidates` and every cache holding one, which is not this
  spec's business.

## Open questions

1. **How many speculative workers?** S1 fixes the shape but not the
   number, and with S3 in place the tension is no longer against `User`
   latency — a yield clears the machine either way — but against the
   yield's own cost: more workers mean more parts in hand when the
   request lands, and the wait is the *longest* of them, not the mean.
   Measure the thing that matters: time from opening the override pane
   to a full list, with the prefetcher saturated.

   **Answered 2026-08-07: `jobs`, one per core.** Not because the yield
   cost was measured — it was not — but because the tension the question
   assumes turned out not to be the binding one. A measured scroll over
   googleapis showed the machine is not short of speculative throughput;
   it is saturated by *urgent* work, which the `fanout` mutex serializes
   regardless of how many workers exist. So the worker count is chosen
   for the simple reason and left there until something measures against
   it. See spec 0252, which is about that saturation.

2. **What does a yielded prefetch cost in memory?** Each parked worker
   holds a partial ranked list and its cursor. With twelve of them and
   S9's ~5 MB per full list, the parked set is a real number that
   nobody has computed. It bounds the answer to question 1 from the
   other side.

3. **Is the partition cheaper in energy, or only in time?** On
   googleapis 24 parts are 21% faster on one thread and on pdb 36%
   slower, and nobody knows why (spec 0218's "what is not understood").
   If the mechanism is locality, the un-partitioned walk is *stalling*,
   and a stalled core spends less core power and more DRAM traffic, so
   the time ordering may not be the energy ordering.

   How to settle it: `LLC-load-misses` and `instructions` under `perf
   stat` identify the mechanism, and `power/energy-pkg/` (RAPL)
   measures the answer directly where the platform exposes it — a VM
   usually does not. This changes no decision here; it decides whether
   the partition is a general rule or a googleapis-shaped one.

## Alternatives considered

**Spawn more heat-worker threads and keep sharding everything.** The
first instinct, and it targets the wrong serialization: the dispatcher
is serial but the sweep is not, so two dispatchers each fanning out to
24 parts oversubscribe the machine and make every query slower.
`effective_jobs` already documents why — shards past the CPU count are
"pure loss".

**Reserve a core for `User` work instead of yielding.** Considered and
rejected: it bounds the *wait* but not the *work*. A `User` request
handed one core takes ≈5.5 s rather than ≈0.9 s — worse than today,
where it at least gets the machine once the queue drains. What the user
is waiting for is the list, not the start of the walk.

**Cancel a walk mid-part.** Rejected as N1. It needs a flag threaded
through `score_subset` and a decision about what a half-scored range
means, to shave a mean 0.23 s off a wait that S3 already bounds at
under a second.

**Run speculative queries with no partition at all.** The simplest
reading of "one whole query per core", and strictly worse: 26% slower
per query on googleapis (6.96 s against 5.53 s) *and* it removes the
only checkpoint S3 could yield at, leaving a 7 s uninterruptible block
in front of the user.

**Justify S1 on energy.** Withdrawn. The single-threaded row shows the
partition is *faster*, which refutes "fewer, bigger queries do less
work" — but time is not energy, and if the mechanism is locality then
the slower arrangement is stalling rather than computing. See open
question 3. S1 rests on throughput and on S3's checkpoint, not on cost.

## Test plan

1. `a_prefetch_walks_its_parts_on_one_thread` — a `Prefetch` request
   spawns nothing yet still cuts `SWEEP_PARTS` parts; a `User` request
   spreads them. S1, S2. **Written** as
   `a_speculative_sweep_records_what_the_fanned_out_one_would_have`,
   which is the claim that matters: the arm is a scheduling change, so
   what it writes into the cache must equal what the fanned-out call
   would have written. Counting threads was the weaker test — it would
   have restated the code.
2. `walking_threads_never_exceed_the_cpu_count` — with the prefetcher
   saturated and a `User` request issued, live walker count stays
   within `available_cpus()` throughout the handover. G2. **Not
   written** even once the workers exist, because with the `fanout`
   mutex the bound is structural rather than emergent: at most one sweep
   is fanned out at a time and the speculative ones are single-threaded,
   so the count cannot exceed `jobs + workers − 1` by construction. A
   test would have to observe live walker counts inside `score_subset`.
   `several_workers_behind_one_queue_serve_every_range_once` covers what
   is testable at this level — that N workers behind one queue still
   sweep each range once — and it pins the *outcome*, not the mechanism:
   at test scale `covers_window` catches the duplicate before S4's rule
   does.
3. `a_user_request_does_not_wait_for_a_whole_prefetch` — issued while
   speculative work is in flight, it starts within one part rather than
   one query. G1, S3. **Written as three tests of the gate rather than
   one of the race**, because the race is not schedulable: the test
   graph's sweep is a handful of microseconds, so a test that pushes a
   `User` request "while the prefetch walks" is a coin toss on a
   millisecond sleep. What is deterministic, and is what the claim
   actually rests on:
   `an_urgent_only_pop_refuses_speculative_work_instead_of_blocking`,
   `the_yield_gate_stays_shut_while_an_urgent_sweep_is_still_walking`
   (which pins the in-flight half specifically — the half whose absence
   would make the handover last only until the request was dequeued),
   and `await_quiet_returns_when_the_urgent_sweep_it_waits_on_ends`
   plus `await_quiet_returns_on_stop` for the wakeups.
4. `a_yielded_prefetch_resumes_where_it_stopped` — after the `User`
   sweep completes, the speculative query finishes without re-walking
   the parts it had already done, and its result equals an
   uninterrupted one. S3. **Written** at the sweep level, as
   `a_sweep_stopped_and_resumed_produces_the_uninterrupted_ranking`,
   under the most-interrupted schedule there is: it yields before
   *every* part, so the sweep is put down and picked up 24 times, and
   the result must still equal `ranked(&blob, graph, 1, None)` exactly.
   Its two siblings pin the endpoints —
   `a_sweep_told_to_yield_immediately_walks_no_part` and
   `an_uninterrupted_resumable_sweep_equals_the_ordinary_one`.
4b. The park-release rule of S3.4, added once the second half of S1
   made it reachable: `a_parked_sweep_does_not_hold_its_range_against_
   the_user` (a request inside a parked range is served, not dropped)
   and `a_parked_sweep_is_abandoned_if_its_range_is_taken_over`.
5. `the_same_range_is_swept_once` — pushing a range already in flight
   starts no second sweep. S4. **Written** as
   `a_range_already_in_flight_is_not_popped_a_second_time`, against the
   queue directly rather than against two live workers: with one worker
   the rule is dormant by construction, so a worker-level test could
   only ever pass vacuously. It pushes the duplicate *last*, so that a
   most-recently-touched-first pop reaches it first and has to step over
   it to answer at all — pushed first, the pop would have returned the
   other entry without ever examining it, which is how the assertion
   read on its first draft.
6. `activity_survives_several_workers` — `activity()` reports the
   highest live tier with more than one worker registered, and does not
   report idle while one is still walking. S5. **Written** as
   `one_worker_finishing_does_not_clear_anothers_in_flight_tier`.
6b. `sweeps_of` names a range rather than counting a delta — S6, folded
   into test 8 rather than given a test of its own, since the assertion
   it exists to make honest is exactly the one test 8 makes.
7. `the_pane_reopens_from_the_cache` — visiting 3 ranges then returning
   to the first re-scores nothing. S7, G3. **Written**, against a real
   worker thread; verified non-vacuous by cutting the cap to 1, which
   fails it.
8. `a_prefetch_does_not_evict_a_user_list` — a prefetch completing
   between two visits leaves the cached list intact. S8. **Written**,
   and it asserts the prefetch really swept before asserting the list
   survived, so it cannot pass by the prefetch not happening. That
   assertion is now `sweeps_of(2) == 1` — the prefetch's own range —
   rather than the `score_all_calls` delta it started as, which is S6's
   point made on the one test that needed it.
9. `the_complete_cache_evicts_what_was_not_read` — added: a read
   promotes, so the cap evicts the least recently *used* entry and not
   the least recently inserted. S7. Without it, a user alternating
   between two ranges loses one as soon as the cache fills with ranges
   nobody came back to.

## Measured outcome

Filled in at implementation. It must include: time from opening the
override pane to a full list, with and without the prefetcher
saturated, before and after; the observed yield latency against S3's
0.23 s mean and ~0.8 s worst part; queries per second of speculative
work, against the 2.2-vs-1.1 estimate in S1; the memory held by the
parked workers (open question 2); and the measured size of one full
ranked list and what that makes 16 of them (S9). State plainly anything
that did not improve.
