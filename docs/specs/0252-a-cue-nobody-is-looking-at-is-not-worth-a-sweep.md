<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0252 — a cue nobody is looking at is not worth a sweep

Status: implemented
Implemented in: 2026-08-07
App: protolens
Refs: docs/specs/0250-the-machine-works-on-what-the-user-waits-for.md
        (its S3 parked worker is what the stale band stalls, and its S4
        in-flight rule is what the queue already merges on);
        docs/specs/0189-a-superseded-request-wave-is-discarded-by-the-worker.md
        and docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        (the wave supersession this generalizes to the `Visible` band);
        docs/specs/0208-attention-follows-the-cursor.md (the band and
        eviction order it leaves alone)

## Background

Nothing supersedes the `visible` band. `start_new_wave` sets the
in-progress `Prefetch` wave aside on every walk restart, for the reason
`prefetch_step_inner` gives in so many words — "a queue entry is
*pending work* … an unpaid sweep on a range ranked from an origin the
cursor has already left — so the worker discards it rather than scoring
it". That argument is exactly as true of a `Visible` request for a row
that has scrolled off, and it has never been applied there.

Measured on 12 CPUs against `googleapis.desc` (25.6 MB), over a 40.1 s
window covering a burst of 400 `PgDn` and the settle after it: 8394
worker turns, of which 8369 were full sweeps and **2529 were `Visible`**
— against a window of about 40 rows. The queue sat at its 2048-entry
cap throughout.

**This is a throughput item, not a latency one.** The queue already
orders live work correctly and it is worth writing down why, so that
nobody re-derives S1 as a latency fix:

- `pop_highest` takes `visible.head` (`tiered.rs:386`);
- `upsert` on an existing key unlinks and re-links at the insertion end
  (`tiered.rs:277-289`), and `push` merges on `range.start` — so a row
  still on screen is re-asked every frame and promoted back to the head;
- `evict_one` takes `visible.tail` (`tiered.rs:405`), which under
  head-insertion is the oldest entry.

A worker therefore already serves the most-recently-asked row first, and
the 2048 cap already throws stale entries away first. What the stale
entries cost is not the user's place in the queue. It is:

1. **Worker turns.** Roughly two thousand sweeps for rows nobody will
   look at again, competing for cores with prefetch rows *adjacent to
   the cursor*, which are the ones most likely to be wanted next.

2. **A stalled prefetcher.** This is the part that is bug-shaped rather
   than merely wasteful. A queued `Visible` entry sets bit 1 in
   `band_occupancy`; `urgent_live()` is `(queued | in_flight) &
   URGENT_TIERS`; so stale entries make `walk_until_yield` yield and
   `await_quiet` block. Every speculative worker parks and stays parked
   until some worker has swept the whole stale band away. The
   prefetcher is not merely outranked — it is stopped, by urgency that
   no longer exists.

### What this spec is not about

An earlier draft blamed the same stale entries for a reported symptom:
`[?]` cues that did not resolve for around ten seconds after a fast
scroll, with the activity dot dark blue throughout. That diagnosis was
wrong, and the symptom is already fixed.

It was spec 0250 S3's original rule that a parked sweep stays registered
in flight. A parked entry is registered at `Tier::Prefetch`, so the dot
reads blue; a visible row inside that same range pushes a `Visible`
request every frame; `pop` sees the range in flight and drops it under
0250 S4 — every frame, for as long as the sweep stays parked, which
while the user keeps scrolling is indefinitely. Permanent `[?]`, blue
dot, and an empty visible band, which is precisely the combination
observed and precisely the combination the stale-band theory cannot
produce. `park_sweep`/`resume_sweep` reversed the rule and the cues now
resolve promptly.

Recorded here because the false trail is instructive: the stale band and
the parked range produce similar-looking waste and completely different
symptoms, and the activity dot's color distinguishes them.

## Goals

- **G1. No sweep is paid for a row that is not on screen.** Speculation
  is the prefetcher's job; it has its own band and its own budget. A
  `Visible` request is a promise about *this* window.
- **G2. A stale request does not count as urgency.** The yield gate
  should report what someone is actually waiting for.

## Non-goals

- **N1. No latency claim.** See above — the queue's existing ordering
  already serves the newest ask first. If this measurably improves
  time-to-cue, that is a surprise worth investigating, not the aim.
- **N2. The `Visible` fan-out is left alone.** An earlier draft had
  `Visible` stop taking spec 0250's `fanout` mutex and walk
  single-threaded like a speculative sweep, on the grounds that a
  screenful of small independent sweeps is served better concurrently
  than through a funnel. The argument may well be right, but its
  evidence was a p50 of 15.4 ms pooled over all tiers — and `Prefetch`
  was 5523 of those 8369 samples, so that number describes the
  prefetcher and says nothing about a `Visible` sweep. Amending 0250 S2
  wants a per-tier measurement first.
- **N3. Not a cache-sizing change.** 24 cache hits in 8394 worker turns
  says this workload is almost entirely first-time asks, which no
  `HEAT_CACHE_MAX_ENTRIES` answers.
- **N4. Not a change to `HEAT_REQUEST_QUEUE_MAX_ENTRIES`.** The cap is
  doing useful work here — tail eviction is already approximate staleness
  eviction. If the queue still saturates after S1, that is new
  information wanting its own diagnosis, not a bigger number.
- **N5. Cancelling a sweep already walking is still out**, as spec 0250
  N1. S1 drops stale work at the queue, which is where it is free.

## Specification

- **S1. A `Visible` request is stamped with the window it was asked for,
  and discarded rather than served once that window is gone.**

  The queue holds a `generation: u64` in its state, bumped when the set
  of rows the main pane draws changes. `push` stamps the entry it stores
  with the current value; `pop` discards, without serving, any `Visible`
  entry stamped older, and keeps looking.

  `User` and `Prefetch` are exempt. `User` is the cursor's own row or
  the override pane and the user is waiting on it whatever the window
  did; `Prefetch` has `start_new_wave` already.

  **The stamp lives on the queue entry, not on `HeatRequest`.** No
  caller supplies it — `push` assigns it — and keeping it out of the
  request is what makes the merge rule automatic rather than a rule
  anyone has to remember: a range scrolled away and back is re-stamped
  with the current generation by construction, instead of inheriting the
  stale stamp of the entry it merged into.

  **The generation is the window, not the cursor.** Keying it on the
  cursor row — which is what `prefetch_step_inner` does, correctly, for
  the prefetch *origin* — would miss an `Alt` pan, which moves the
  viewport and leaves the cursor where it is.

  **The drain wakes the parked workers.** Discarding the last stale
  entry clears the `visible` bit, and that transition is a handover to
  workers asleep in `await_quiet` with nothing else due to wake them —
  the same obligation `end_sweep` has (0250 S3). One `notify_all` per
  drain, not per discarded entry.

## Alternatives considered

**A fifth `visible_previous` band, mirroring `prefetch_previous`.**
Reuses the existing O(1) splice and needs no counter. Rejected because
it answers a coarser question — "was this asked before the last
supersession" rather than "is this row on screen" — and costs a band, an
eviction-order entry and a `band_occupancy` bit. The generation is
exact for less.

**Supersede the whole `visible` band on every window move.** Cheapest of
all, and *not* as lossy as it first appears: `push`'s promoting `peek`
would pull a re-asked row back out of the superseded band next frame. It
is still rejected, because "next frame" is the problem — between the
supersession and the re-push there is a window in which a worker
discards rows that are still on screen, and a one-row scroll shares 39
of its 40 rows with the window before it.

**Bound the `visible` band to the window height.** The true working set
is ~40 entries and this needs no generation. Rejected: eviction is by
band tail, which is the oldest entry only because insertion is at the
head, so under any access pattern other than a monotonic scroll it would
evict rows still on screen. It enforces the symptom's shape rather than
the rule.

**Do nothing; let the 2048 cap handle it.** The honest null option, and
it already gets the ordering approximately right (see Background). It is
rejected on G2 alone: eviction happens at the cap, so up to 2048 stale
entries sit in the band holding the `visible` bit high, and no eviction
policy fixes a yield gate that is reading the wrong question.

## Test plan

1. `a_visible_request_for_a_departed_window_is_dropped_not_served` —
   push at `Tier::Visible`, bump the generation, assert `pop` skips it
   and reports the next entry. S1.
2. `a_user_request_survives_a_window_change` — the same sequence at
   `Tier::User` is served. S1's exemption, which is the half a naive
   implementation gets wrong.
3. `a_re_pushed_range_adopts_the_current_generation` — push, bump, push
   the same `range.start` again, assert it is served. Without this a row
   scrolled away and back never resolves.
4. `a_stale_band_is_drained_in_one_pop` — with many stale entries ahead
   of one live one, a single `pop` returns the live one and leaves the
   queue holding only it. Establishes that the drain is not one discard
   per call, which would starve the caller.
5. `a_drained_stale_band_stops_counting_as_urgent` — with only stale
   `Visible` entries queued, `urgent_live()` is true; after a `pop`
   drains them it is false. G2, and the reason S1 is worth doing at all.
6. `a_stale_drain_wakes_a_parked_worker` — a thread in `await_quiet`
   returns once the drain clears the band. S1's `notify_all`.

## Measured outcome

Measured 2026-08-07 on 12 CPUs against `googleapis.desc` (25.6 MB) in a
200x50 pty: 45 s settle, a burst of 400 `PgDn` at 20 ms spacing, then 30
one-second samples. A/B on the same binary, S1 switched off by making
`new_window` return early. Two runs of each.

This is not the window Background's 2529 came from — that one was taken
before spec 0250's step 4 put a worker on every core, so its absolute
counts are not comparable with these. The A/B pair below is, since both
arms are the same binary minutes apart.

| | S1 off | S1 on |
|---|---|---|
| `Visible` sweeps | 1256 / 906 | **436 / 457** |
| `Prefetch` sweeps | 3090 / 2975 | **23819 / 7287** |
| `User` sweeps | 345 / 342 | 334 / 345 |
| stale entries discarded | 0 | 1557 / 1513 |
| peak queue length | 2048 | 2048 |
| generations bumped | 0 | 402 / 402 |
| `[?]` on screen, settled | 20 | **3** |

**G1 holds, and by more than the sweep count alone says.** `Visible`
sweeps fall roughly in half, but the 1500-odd discards are the truer
number: two thirds of the `Visible` work this window asked for was for
rows that had scrolled away before a worker reached them.

**G2 holds, and it is the large effect.** Prefetch throughput is between
2.4x and 8.0x. The spread is real — a prefetch sweep is a part of a
resumable walk, so its count depends on where the walk had got to — but
both S1-on runs are well clear of both S1-off runs, and S1-off is stable
at ~3000 across runs. This is the parked-worker stall predicted from
`urgent_live`'s definition, now observed: with the stale band drained,
the speculative workers stop being held in `await_quiet` by urgency that
no longer exists.

**The peak queue length did not move**, which is what N4 expected: the
2048 cap is reached during the burst either way. Tail eviction and the
generation discard are answering different questions, and S1 does not
relieve the cap.

**N1 was wrong, and pleasantly so.** The spec claimed no latency effect
and asked for a surprise to be reported. The count of unresolved `[?]`
cues on the settled screen drops from 20 to 3 — stable across both runs
of each arm. The mechanism is not the queue's ordering, which S1 does
not touch; it is that an unparked prefetcher reaches the rows *adjacent
to the cursor* while the user is still scrolling, so the row the burst
lands on is already in the cache. The latency win is a second-order
effect of G2, not of G1.
