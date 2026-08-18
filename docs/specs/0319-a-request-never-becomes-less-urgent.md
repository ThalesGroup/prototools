<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0319 — a request never becomes less urgent

Status: implemented
Implemented in: 2026-08-18
App: protolens
Refs: docs/specs/0164-… (tiers, and the promoting read),
        docs/specs/0190-… (the activity dot's two mirrors),
        docs/specs/0224-… (a completion reaches the screen only through
        `HeatWorkerProgress`), docs/specs/0250-… (a range already in
        flight is dropped, not swept twice), docs/specs/0252-… (a
        `Visible` ask is stamped with its window),
        docs/specs/0262-… (a query is a pool of parts)

## Background

Field report, on `googleapis.desc`: `Down`, `Ctrl-Left`, then hold
`PageDown` for four seconds. The rows show `[?]` and the activity dot
stays **blue** — `Tier::Prefetch` — for a long time. Pressing any key
clears every `[?]` at once.

Blue contradicts `[?]`. `[?]` means the reader has no `by_range` entry,
so it pushed a `Tier::Visible` request this frame; a queued or in-flight
`Visible` sets that band's bit and `activity()` takes the maximum, which
would paint the dot red. One of those steps is not happening.

The cause is in `HeatRequestQueue::push`. A merge onto an existing entry
rebuilds the request with **the new push's tier**, unconditionally:

```rust
Some(existing) => HeatRequest { …, tier },
```

`TieredBounded::upsert` is careful in exactly the place this is not: a
push below the entry's tier updates the payload in place and does not
retag or relink it, so the queue *slot* keeps its `Visible` rank. The
payload it now holds says `Prefetch`. Band and request disagree, and
every consumer downstream reads the request.

Nothing contrived is needed to reach it. `prefetch_step` walks outward
from the cursor's row and skips settled nodes, so the ranges it pushes
at `Prefetch` are precisely the unsettled on-screen rows the render pass
pushed at `Visible` a few milliseconds earlier in the same frame. Every
`[?]` on screen is a demotion candidate, every frame.

What the demotion costs, in order:

1. `pop_highest` still serves the entry from the `Visible` band, so
   hand-out order is right — which is why the symptom is so confusing.
2. `next_task` registers `(key, entry.req.tier)` in `in_flight`:
   **`Prefetch`**. The dot goes blue.
3. `activate` freezes `ActiveQuery.req.tier` at `Prefetch`, so all of
   the query's parts compete in `next_task`'s `max_by_key(tier)` as
   equals with genuine read-ahead instead of outranking it.
4. The demotion is now permanent: the next frame's `Visible` push for
   the same range hits the in-flight guard and is dropped. Spec 0250 S4
   calls that self-healing, and it is — but it heals into the same
   `Prefetch` query.
5. On completion, `if req.tier != Tier::Prefetch` is false, so no
   `AppEvent::HeatWorkerProgress` is sent. Since spec 0224 that event is
   the *whole* of how an answer reaches the screen: it is the only thing
   that sets `heat_dirty`, and the frame `heat_dirty` buys is what
   re-reads the cache for every unsettled row.

Step 5 is why any keystroke clears the whole screen at once: the answers
were already in the cache and only the repaint was missing. Step 3 is
why they took so long to get there. The two are separate faults with one
root, and the fix has to close both — closing only step 5 would leave an
on-screen node scheduled as read-ahead, and closing only step 3 would
compute the answer promptly and still not show it.

`git log -L` dates the merge block to spec 0252's commit. This is not a
recent regression; `Ctrl-Left` makes it florid, because folding all
siblings leaves a handful of enormous top-level nodes, so each demoted
query is seconds of work rather than milliseconds.

## Goals

- **G1.** A merge onto a queued request takes the **higher** of the two
  tiers, so the request agrees with the band the queue already put it
  in.
- **G2.** A query already in flight is **promotable**: an ask that
  arrives after the walk started raises the query's tier rather than
  being dropped and forgotten.
- **G3.** The completion event and the cache-write tier follow the
  query's tier **as it stands when the query ends**, not the snapshot
  the finishing part was handed.

## Non-goals

- **N1.** Demotion. Nothing lowers a tier, here or anywhere: `peek` has
  refused to since spec 0164 G9 and this makes `push` agree with it.
- **N2.** Merging the *window* or `current_key` of a later ask into a
  query already in flight. Only the tier is promoted. A wider window or
  a different current type still takes spec 0250 S4's drop-and-re-push
  path, which is correct and already self-healing; carrying them into a
  running query would mean parts already walked answered a different
  question from the ones still to come.
- **N3.** Cancelling and restarting a promoted query at its new tier.
  The parts already handed out keep the tier they were handed at — see
  S5 for what that costs.
- **N4.** The one-second `:q`. **The attribution here was wrong — see
  spec 0320, which measured it.** The second was glibc freeing the baked
  document after the terminal had already been restored; every worker is
  joined inside 17 ms, burst or idle.

  What is true, and remains out of scope: `override_pane::inferred_score`
  in the admit fast path takes no cancel flag, unlike the
  `partition.walk` beside it, so a worker inside it when the stop lands
  runs it to completion. Under 17 ms, so not a quit problem.

## Specification

- **S1.** `push` merges the tier as `existing.req.tier.max(tier)`. This
  restores the invariant that a queue entry's `req.tier` equals the band
  it is linked in — the two are set from the same value on every path,
  and `upsert`'s in-place branch is what makes taking the max necessary
  rather than merely tidy.

- **S2.** The generation stamp follows the tier. Spec 0252 stamps every
  push with the current generation; with S1 in place, a `Prefetch`
  re-ask of a stale `Visible` entry would refresh that stamp and keep an
  entry alive for a window nobody is looking at. So: a push **at
  `Visible` or above** stamps the current generation, and a push below
  it leaves an existing entry's stamp alone. A brand-new entry always
  takes the current generation — the staleness check only consults
  `Visible` entries, and a new `Prefetch` entry is not one.

  This is not a cost of S1. Today the same re-ask sets the entry's tier
  to `Prefetch`, which exempts it from the staleness check *entirely*
  while leaving it in the `Visible` band. S2 is strictly better.

- **S3.** `push` promotes the live query too, under the lock it already
  holds: the `in_flight` entry for the range, and the matching
  `ActiveQuery.req.tier`, each raised to the pushed tier if it is
  higher. `in_flight` is republished when either changed, which is what
  carries the promotion to the activity dot and, through
  `refresh_stand_aside`, to `user_live_locked`.

  In `push` rather than at the pop-time drop in `next_task`: the ask is
  being made now, and waiting for some worker to pop a duplicate before
  honoring it is the delay this spec exists to remove. The queue entry
  is still pushed and still dropped at pop — see N2.

- **S4.** `deposit_part` returns the query's own `HeatRequest` alongside
  its runs, and the worker uses **that** for `record_sweep` and for the
  `req.tier != Tier::Prefetch` notify, rather than the clone its
  `Task::Walk` carried. Without this, S3 is invisible at the only moment
  that matters: a query promoted after its last part was handed out
  would still write its cache entries at the old tier and still not
  notify. `end_sweep` likewise returns the tier it deregistered, which
  is what the `Task::Admit` early-outs use for the same decision.

- **S5.** Parts already handed out keep the tier they were handed at,
  and this is only visible for a promotion **to `User`**: that raises
  `stand_aside`, so those parts abort, are given back by `abandon_part`
  and are re-handed at the new tier. One part per busy worker is
  redone. Accepted — the alternative is reading the live tier under the
  lock once per wire field, and `stand_aside` is an atomic precisely so
  that the walk never takes a lock.

## Alternatives considered

**Promote at the pop-time drop instead of in `push`.** The guard in
`next_task` that discards a request for a range already in flight is
the natural place to notice the promotion. It costs a worker turn: the
tier does not rise, and the dot does not correct itself, until some
worker happens to pop the duplicate. With every worker inside a long
part — the reported situation exactly — that can be the whole of the
delay.

**Fix only the notify.** Making `HeatWorkerProgress` unconditional
would clear the `[?]` and would be one character. It reintroduces
exactly what spec 0164 G10 removed: a read-ahead burst becomes thousands
of no-op redraws. It also leaves an on-screen node scheduled as
speculation, which is the slower half of the fault.

**Make `in_flight` hold the ask rather than the walk.** Registering
intent instead of "a walk is happening right now" would let `activity()`
report the promoted tier without touching `ActiveQuery`. Rejected
before, and recorded: it stuck `activity()` at `Some(Prefetch)` and
dropped every request inside the registered range on every frame — the
same two symptoms this spec is fixing.

## Test plan

1. `a_prefetch_re_ask_does_not_demote_a_queued_visible_request` —
   push `Visible`, push `Prefetch` for the same range, pop: the admitted
   request is still `Visible`. The direct statement of S1.
2. `lower_tier_push_merging_an_existing_entry_does_not_reorder_it` —
   already asserts the ordering and says "nor change its tier" in its
   comment without checking it. Extended to check it.
3. `a_prefetch_re_ask_does_not_revive_a_stale_visible_request` — push
   `Visible`, `new_window`, push `Prefetch` for the same range: still
   discarded on pop. S2.
4. `a_visible_ask_promotes_a_query_already_in_flight` — admit a
   `Prefetch` request, push `Visible` for the same range: `activity()`
   is `Visible` and `in_flight_ranges()` says so. S3.
5. `a_promoted_query_records_and_notifies_at_its_new_tier` —
   `activate` at `Prefetch`, hand out both parts, promote, deposit both:
   the request handed back to the last part is `Visible`. S4.
6. `promoting_an_in_flight_query_to_user_takes_the_pool` —
   `stand_aside` rises and `abort_epoch` moves, so the parts in flight
   for the promoted query are given back and re-handed. S5.
7. Manual: the reported sequence on `googleapis.desc`. The dot must go
   red while the on-screen rows are unresolved, and the `[?]`s must
   clear without a keystroke.

## Measured outcome

Items 1-6 of the test plan are unit tests and pass. Item 7 is **not
verified**, and the honest record of why is worth more than a number.

A pty harness drove the reported sequence headlessly on
`googleapis.desc` at 50×200: settle, `Down`, `Ctrl-Left`, a four-second
`PageDown` burst at 33 Hz, then thirty seconds with no input at all,
then one `Up`. Counting `[?]` needs the *screen*, not the byte stream —
ratatui writes a diff, so a cue that stops being drawn leaves no trace
in the bytes — which took a small VT model over CUP/EL/text.

It does not reproduce, on **either** binary:

| settle | peak cues during the burst | cues when the burst ends | unprompted frames after |
|---|---|---|---|
| 45 s | 0 | 0 | 0 |
| 4 s | 13 | **0** | 8, all ≤ 133 B |

At a 45-second settle the document is finished before the fold and no
cue ever appears. At four seconds cues do appear — thirteen at once —
and the *baseline* resolves every one of them before the burst is over.
Two earlier readings of this harness were worthless and are recorded so
the mistakes are not repeated: `\[\?` matches `\x1b[?25l`, the
cursor-hide sequence every frame carries, and a `str`/`bytes` comparison
on the escape's final byte silently dropped every cursor move, piling
the whole screen onto row 0.

Why it does not bite here and does on the reporter's terminal is
unestablished. The plausible differences are the pane geometry, which
decides how many rows the fold leaves unsettled, and the core count,
which decides how long a demoted query stays demoted — this machine
finishes the thirteen inside four seconds and the report describes tens
of seconds. Neither is worth chasing: the fault is not in doubt. It is
visible in the source, the five unit tests pin each step of it, and the
reporter's own `Up`-clears-everything-at-once observation is the
signature of the lost `HeatWorkerProgress` and of nothing else.

So: **confirm at a real terminal.** The dot must go red while on-screen
rows are unresolved, and the `[?]`s must clear with no keystroke.
