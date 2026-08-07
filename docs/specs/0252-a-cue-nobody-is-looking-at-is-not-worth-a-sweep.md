<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0252 — a cue nobody is looking at is not worth a sweep

Status: draft
App: protolens
Refs: docs/specs/0250-the-machine-works-on-what-the-user-waits-for.md
        (amends its S2 — `Visible` stops fanning out);
        docs/specs/0189-a-superseded-request-wave-is-discarded-by-the-worker.md
        and docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        (the wave supersession this generalizes to the `Visible` band);
        docs/specs/0208-attention-follows-the-cursor.md (the band and
        eviction order it leaves alone)

## Background

Loading `googleapis.desc` and scrolling rapidly leaves `[?]` cues to the
right of the lines for around ten seconds after the scroll stops, with
the activity dot dark blue throughout.

The dot is honest and stays as it is: dark blue is `Tier::Prefetch`, and
after spec 0250 a speculative sweep really does take ≈5.5 s rather than
≈0.9 s, so the prefetcher is genuinely busy for a long time. That is the
design working. The cues are the defect.

**The measurement.** 12 CPUs, `googleapis.desc` (25.6 MB), a burst of
400 `PgDn` and then rest; a 40.1 s window instrumented at the worker
loop:

| | |
|---|---|
| worker turns | 8394 |
| of which full sweeps | 8369 |
| of which cache hits | **24** |
| tier mix | Prefetch 5523, **Visible 2529**, User 342 |
| sweep duration | p50 15.4 ms, p90 66.8 ms, p99 117 ms, max 340 ms |
| sum of sweep time | **245 s**, in a 40 s window |
| peak queue length | **2048 — the cap** |

The cue latency is not new and 0250 did not cause it: 14 s at `46a5db6`
(before 0250), 10 s at `c3b4b2a`, 11 s at `4fecd67`.

**What the numbers rule out.** Not queue ordering: `push` re-promotes a
merged request to the head of its band and `heat_lookup_ex` re-pushes on
every miss on every frame, so a row that is on screen *now* is already
first in line. Not cache staleness either — 24 hits in 8394 turns means
essentially nothing is being asked twice.

**What they leave.** 2871 urgent sweeps in 40 s, against a visible window
of about 40 rows. Two mechanisms produce that ratio, and both are
addressed here:

1. **A `Visible` request outlives the row that asked for it.** Spec 0189
   already states the principle, and `prefetch_step_inner` states it in
   so many words — "a queue entry is *pending work* … an unpaid sweep on
   a range ranked from an origin the cursor has left — so the worker
   discards it rather than scoring it". It was applied to the `Prefetch`
   band only. `start_new_wave` touches `prefetch_current`; nothing ever
   supersedes `visible`. So a fast scroll leaves the queue holding
   thousands of unpaid sweeps for rows the user passed hundreds of pages
   ago, and the workers pay every one of them.

2. **Urgent sweeps serialize against each other.** Spec 0250's `fanout`
   mutex admits one fanned-out sweep at a time, which is right — twelve
   workers each spreading `SWEEP_PARTS` over twelve cores is
   `workers × jobs` walkers. But it means 2871 urgent sweeps run one
   after another: at the p50 alone that is ~44 s of serialized work
   inside a 40 s window. The machine is not short of throughput; it is
   funnelling a screenful of independent small sweeps through a lock
   built for one large one.

## Goals

- **G1. A cue resolves while the user is looking at it.** Once scrolling
  stops, every `[?]` in the window resolves within a small number of
  frames, not seconds.
- **G2. No sweep is paid for a row that is not on screen.** Speculation
  is the prefetcher's job and it has its own band and its own budget. A
  `Visible` request is a promise about *this* window.

## Non-goals

- **N1. The activity dot is not changed.** It reports `Prefetch` while
  the prefetcher works, and after 0250 that is a long time. Making it
  quieter would hide the truth rather than fix the cues; the honest
  report is what let this be diagnosed at all.
- **N2. Not a cache-sizing change.** `HEAT_CACHE_MAX_ENTRIES` is 8192
  and a 400-page scroll exposes far more distinct ranges than that, so
  the cache does thrash — but 24 hits in 8394 turns says the workload is
  almost entirely first-time asks, which no cache size answers. Growing
  the cache buys nothing here.
- **N3. Not a change to `HEAT_REQUEST_QUEUE_MAX_ENTRIES`.** The queue
  hitting its 2048 cap is a symptom of S1's leak, not a capacity
  problem. If it still saturates after S1, that is new information and
  wants its own diagnosis, not a bigger number.
- **N4. Cancelling a sweep already walking is still out.** As spec 0250
  N1: a request that has been popped runs to completion. S1 drops stale
  work at the *queue*, which is where it is free to drop.

## Specification

- **S1. A `Visible` request is stamped with the window it was asked for,
  and is discarded rather than served once that window is gone.**

  `HeatRequest` carries a `generation: u64`, taken from an `AtomicU64`
  on the queue. `pop` discards, without serving, any `Visible` entry
  whose generation is older than the current one, and keeps looking.
  `User` and `Prefetch` entries are unaffected — `User` is the cursor's
  own row or the override pane, and the user is waiting on it whatever
  the window did; `Prefetch` keeps the wave mechanism it already has.

  `push`'s merge on `range.start` must adopt the *newer* generation, so
  that a range scrolled away and back is current again rather than
  inheriting the stale stamp it merged into.

  **The generation is the window, not the cursor.** It is bumped when
  the set of rows the main pane draws changes. Keying it on the cursor
  row — which is what `prefetch_step_inner` does, correctly, for the
  prefetch *origin* — would miss an `Alt` pan, which moves the viewport
  and leaves the cursor where it is.

  Discarding is a band-local unlink, so draining a full stale band is
  one O(n) pass of O(1) removals, paid once per generation and only by
  the worker that happens to pop next.

  This is deliberately an exact stamp rather than a fifth band in
  `TieredBounded`. A `visible_previous` band would mirror
  `prefetch_previous` and reuse its splice, but it answers a coarser
  question — "was this asked before the last supersession" rather than
  "is this row on screen" — and it costs a band, an eviction-order
  entry and a `band_occupancy` bit for the privilege.

- **S2. Only `User` requests fan out. `Visible` requests walk on one
  thread, like speculative ones.** This amends spec 0250 S2, which put
  `User` and `Visible` together on the grounds that both are latency-
  bound with somebody waiting. That is true of both and decisive for
  neither, because they differ in *number*: there is one `User` request
  — the cursor's row, or the override pane — and there is a screenful of
  `Visible` ones.

  A screenful of independent sweeps is served fastest by running them
  concurrently, one per worker, not by queueing each behind a lock so it
  can have all twelve cores to itself for 15 ms. With `jobs` workers the
  throughput argument is the same one spec 0250 S1 makes for
  speculation, and the arithmetic is the same: twelve rows at a p50 of
  15.4 ms cost 15 ms in parallel and 185 ms through the funnel.

  Per-row latency does get worse for the rare `Visible` row over a very
  large range. That is the right trade: what the user perceives is the
  window clearing, which is the *last* row, and the last row is decided
  by throughput.

  No threshold constant is introduced. A byte-size gate on the range
  was the obvious alternative and is rejected: it needs a number nobody
  can derive, and the tier already carries the distinction that matters.

  With this, `fanout` is contended by `User` alone, of which at most one
  is live — so it stops being a funnel and becomes what it was meant to
  be, a guard against oversubscription.

## Alternatives considered

**Supersede the `Visible` band wholesale on every window move, like
`start_new_wave`.** Cheaper to write — the splice already exists — but
wrong at the boundary: a scroll of one row shares 39 of its 40 rows with
the window before it, and wholesale supersession throws away 39 live
requests that are about to be re-pushed, re-merged and re-walked. The
per-request stamp keeps them.

**Bound the `Visible` band to the window height.** Attractive because
the true working set is ~40 entries, and it needs no generation. But
eviction is by band tail and the band is LIFO, so under a fast scroll it
would evict correctly and under any *other* access pattern it would
evict rows still on screen. A cap enforces the symptom's shape rather
than the rule.

**Make `heat_lookup_ex` push less often.** It pushes on every miss on
every frame, which looks profligate. It is not the problem: merge-on-
push collapses the repeats into one entry and re-promotes it, which is
exactly the behavior wanted, and spec 0250 S4 extended the same merge to
in-flight requests. Suppressing the re-push would break the self-healing
that S4's drop-on-duplicate rule depends on.

**Give `Visible` its own worker pool.** Isolates the two workloads
without any of the above, and is rejected as premature: with S1 the
`Visible` arrival rate falls to about a window per settle, and with S2
the existing pool serves those concurrently. Partitioning the workers
would fix a contention that no longer exists.

## Test plan

1. `a_visible_request_for_a_departed_window_is_dropped_not_served` — push
   a `Visible` request, bump the generation, and assert `pop` skips it
   and reports the next entry. S1.
2. `a_user_request_survives_a_window_change` — the same sequence at
   `Tier::User` is served. S1. Pins the exemption, which is the half a
   naive implementation gets wrong.
3. `a_re_pushed_range_adopts_the_current_generation` — push at
   generation 0, bump, push the same `range.start` again, and assert the
   merged entry is served rather than dropped. S1's merge rule; without
   it a row scrolled away and back never resolves.
4. `a_stale_band_is_drained_in_one_pop` — with many stale entries ahead
   of one live one, a single `pop` returns the live one and the queue is
   left holding only it. S1. Establishes that the drain is not one
   discard per call, which would starve the caller.
5. `a_visible_sweep_does_not_take_the_fanout_lock` — two `Visible`
   requests are served concurrently; a `User` request excludes. S2,
   observed through the sweep log rather than through thread counting.
6. Live: the reproduction from Background — 400 `PgDn` into
   `googleapis.desc`, then measure seconds to the last `[?]` clearing.
   G1. This is the only test that speaks to the reported symptom; the
   five above pin the mechanisms it depends on.

## Measured outcome

Filled in at implementation. Record at minimum: the urgent sweep count
over the same 40 s window (2871 before), the peak queue length (2048
before), and seconds-to-last-cue (10 s before). State plainly if the
cues still take seconds — the two mechanisms above account for the
ratio, but the decomposition of the 10 s between them was never
measured, and if S1 and S2 together do not close it then a third
mechanism exists and this spec named the wrong two.
