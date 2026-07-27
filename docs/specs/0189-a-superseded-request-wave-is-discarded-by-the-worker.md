<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0189 — a superseded request wave is discarded by the worker, not scored

Status: implemented
Implemented in: 2026-07-27
App: protolens
Refs: docs/specs/0152-protolens-heat-cue-background-scoring-thread.md,
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md

## Background

Spec 0164 gave `Tier::Prefetch` two bands instead of one —
`prefetch_current` and `prefetch_previous` — so that a walk restart could
set the in-progress read-ahead aside in a single O(1) pointer splice
(`start_new_wave`, `tiered.rs:221-245`) rather than walking it entry by
entry. The superseded entries stay reachable: `pop_highest` serves them
after the live wave and `evict_one` spends them first
(`tiered.rs:309-317`).

That design is applied uniformly to all three `TieredBounded`
instances — the request queue and both `HeatCaches` maps — and
`App::prefetch_step` calls `start_new_wave()` on all three together on
every restart (`mod.rs:1586-1593`).

Uniformity is wrong here, because the two kinds of structure hold
opposite things.

### The caches hold results; the queue holds debts

Membership in `HeatCaches` means *already computed* (spec 0164 G2's own
wording). Keeping a superseded cache entry costs one slot and nothing
else; a later hit on it is a full `score_all` avoided. Serving it later
is exactly right there.

Membership in `HeatRequestQueue` means *not yet computed*. A superseded
queue entry is not a saved result — it is an outstanding promise to
spend one `inferred_candidates`/`score_all` call, on a range that was
chosen for read-ahead from a cursor position the user has already left.
Spec 0164 justified serving it as "still cheap to serve if nothing
better shows up first" (line 176). For the queue that premise is
inverted: serving it is the single most expensive thing the worker
does.

### Consequence 1 — the worker spends whole sweeps on stale distances

`pop_highest` prefers `prefetch_current.head` over
`prefetch_previous.head`, so the worker only reaches demoted entries
when the live wave is momentarily empty. But `prefetch_step` pushes one
candidate per call, interleaved with the event loop, so
`prefetch_current` is empty *most of the time* — it refills one entry at
a time and the worker drains it immediately. Every time the worker wins
that race it falls through to `prefetch_previous` and starts a sweep for
a row whose distance was measured from a superseded origin.

During a scroll — a rapid succession of cursor moves, hence of restarts
— this is the common case, not the corner case: each restart deposits
another partial wave into `prefetch_previous`, and the one worker thread
spends its time on rows ranked by where the cursor used to be.

### Consequence 2 — stale entries carry stale type keys

`HeatRequest.current_key` is a snapshot of the node's assigned type at
push time. An override edit bumps `structural_version`, which is one of
the two restart triggers (`mod.rs:1577-1578`), so demoted entries
routinely carry a `current_key` the node no longer has. This is not a
correctness bug — `current_score` is keyed by `(range_start, key)`, so
the result lands under the old key and is simply never read — but it
means the worker computes a score, takes a cache slot, and evicts
something else, all for an answer nobody can ask for.

### Consequence 3 — the cap is backpressure, and it is spent on stale work

`HEAT_REQUEST_QUEUE_MAX_ENTRIES` is 512, and its comment calls the bound
"purely defensive… not expected in practice". That is wrong, and the
mistake matters. When `upsert` answers `Rejected`, `prefetch_step`
returns `PrefetchStep::Idle` (`mod.rs:1631-1632`), which parks the walk
until the worker frees a slot. The cap is therefore a *backpressure*
limit on how far the speculative walk may run ahead of the worker, and
reaching it under a fast walk is the designed steady state.

Because the walk radiates outward from the cursor's row and spans the
whole expanded document (`next_row(self.visible_rows.len())`,
`mod.rs:1605`), the cap truncates the *far* end of the walk and never
the near end. That is the right behavior — but every slot held by a
superseded wave is a slot the live walk cannot use, so today the
backpressure fires earlier than the useful work justifies.

## Where the discard should happen

There are two ways to stop paying for superseded requests, and they
differ only in which thread pays and when.

**Discard at the restart, on the UI thread.** `prefetch_step` walks
`prefetch_current` and frees every entry. Measured on the release build,
worst of 1000 trials at a full 512-entry band: **37.1 µs**, or ~72 ns
per entry. That is negligible at 512. It is not obviously negligible at
2048 (~150 µs), and it scales with a constant we want to be free to
raise.

**Discard at the pop, on the worker thread.** The restart keeps its O(1)
splice; the worker, when it finds nothing live to do, reclaims
superseded entries one at a time instead of scoring them. The same total
number of removals happens, spread across the worker's idle moments,
and the UI thread's restart path stays O(1) forever regardless of the
cap.

This spec takes the second. The UI thread is the one with a frame
deadline; the worker is the one that was going to waste a `score_all` on
these entries anyway.

### The trap this opens, and the rule that closes it

Discarding at the pop introduces a failure the current code does not
have. `upsert` refreshes a same-tier entry in place, and `tiered.rs:159
-165` states this explicitly for `Prefetch`: the payload is refreshed
"in place, not moved to `prefetch_current`", *even when the key lives in
`prefetch_previous`* (spec 0164 G5). `HeatRequestQueue::push` reaches
`upsert` through a `peek` that only relinks when the requested tier is
strictly higher (`tiered.rs:142`), so a `Prefetch` re-push of a
superseded key stays superseded.

Under the current code that is harmless: the entry is scored, merely
late. Under discard-at-the-pop it means a range the **new** wave is
actively asking for gets refreshed inside the doomed band and is then
thrown away unscored. This is not a corner case — the restarted walk
radiates outward from the new origin and re-asks for many of the rows
the old wave had already queued. It would self-heal on the following
wave, since by then the key is gone and a fresh push lands in
`prefetch_current`, but a row could go a whole wave without being
scored.

The rule that closes it: **re-asking revives**. A `Prefetch` upsert on
an existing key relinks it to `prefetch_current`'s tail rather than
updating it in place. No band detection is needed —
`link_at_insertion_end` already targets `prefetch_current` for
`Tier::Prefetch` (`tiered.rs:331`), and `unlink` already handles a slot
in either prefetch band (`tiered.rs:384-390`, `:407-413`).

This amends spec 0164 G5. Its cost is that a re-push of a key already in
`prefetch_current` also moves to that band's tail instead of holding its
FIFO position. That case does not arise: `prefetch_step` visits each row
once per wave, and the other push site, `heat_cue_for`, pushes at
`Tier::Visible`, which takes the promotion path instead.

### Why the two bands survive

The obvious simplification is to make the band *be* the tier — a fourth
`Tier` variant below `Prefetch` — at which point `cur_tier.max(tier)`
(`tiered.rs:179`) would implement "re-asking revives" for free, the
trap above could not have been written, and the `else if` fallbacks in
`fix_head_if_matches`/`fix_tail_if_matches` (`tiered.rs:384-390`,
`:407-413`) would disappear, since they exist solely because one tier
maps to two bands.

It is rejected because `Slot` stores the tier (`tiered.rs:346`). If the
band were the tier, demoting a wave would mean rewriting every slot in
it, and `start_new_wave` would stop being an O(1) splice — on the UI
thread, in `prefetch_step`, which is the exact cost this spec exists to
avoid. It would hit the caches harder still, at 8192 entries.

So the two-band arrangement buys one thing: an O(1) restart. The type
doc must say so, because that is the only justification for the
subtlety.

## Goals

- **G1**: The worker never spends a `score_all` on a request from a
  superseded wave. It discards such entries instead.
- **G2**: The restart path on the UI thread stays O(1) — the queue
  keeps `start_new_wave`'s pointer splice — regardless of how large
  `HEAT_REQUEST_QUEUE_MAX_ENTRIES` grows.
- **G3**: Re-asking for a superseded range revives it: a `Prefetch`
  upsert relinks the entry into `prefetch_current`, so a request the
  live wave wants is never discarded. (Amends spec 0164 G5.)
- **G4**: Superseded entries are reclaimed one at a time, with the
  queue's mutex released between entries, so a batch of them can never
  block a UI-thread `push`.
- **G5**: Both `HeatCaches` maps are unchanged. Their superseded entries
  remain servable and remain the preferred eviction victims.
- **G6**: `HEAT_REQUEST_QUEUE_MAX_ENTRIES` becomes 2048, and its comment
  describes the bound as backpressure on the speculative walk rather
  than as a defensive limit that is never reached.

## Non-goals

- **N1**: Unifying `Tier` with the band layout. Rejected above; the
  reasoning is recorded in the type doc rather than left implicit.
  *Trigger for revisiting*: if a restart ever stops being on the UI
  thread's critical path, the O(1) splice stops being worth the
  subtlety.
- **N2**: Draining `prefetch_previous` eagerly. The worker reclaims it
  only when both higher bands are empty, so under a sustained scroll
  superseded entries linger and are reclaimed by `evict_one` instead.
  That is acceptable: `evict_one` already spends `prefetch_previous`
  first (`tiered.rs:309-317`), so the stale entries are exactly the
  right eviction victims, and reclaiming them eagerly would spend worker
  time that live requests want.
- **N3**: Changing when a restart is triggered (`mod.rs:1577-1578`).
- **N4**: Making the discard a per-instance flag or type parameter on
  `TieredBounded`. The behavior lives in `pop_blocking`, which only the
  queue has; the caches never pop at all.
- **N5**: Tuning the cap by measurement. G6's 2048 is a judgment call,
  not a measured optimum — see Open questions.

## Specification

- **S1 — `pop_highest` stops serving superseded work.** Its band order
  loses the `prefetch_previous` arm: `user.head`, `visible.head`,
  `prefetch_current.head`, and nothing else. Its doc states the
  invariant plainly — a superseded entry leaves this structure only by
  `discard_one_superseded` or by `evict_one`, never by being served.

- **S2 — `TieredBounded::discard_one_superseded`.** New method: removes
  the head of `prefetch_previous` and returns whether it removed
  anything. Deliberately one entry per call rather than a drain, so the
  caller controls how long it holds a lock (G4).

  ```rust
  pub(super) fn discard_one_superseded(&mut self) -> bool {
      let Some(idx) = self.prefetch_previous.head else {
          return false;
      };
      self.remove_by_idx(idx);
      true
  }
  ```

- **S3 — the worker discards between jobs.** `HeatRequestQueue::pop_
  blocking` gains a middle case, in this order: return the highest live
  request if there is one; otherwise, if `discard_one_superseded()`
  reclaims an entry, **release the mutex and re-acquire it** before
  looking again; otherwise wait on the condvar. Releasing between
  entries is what makes the reclamation "one at a time" in the sense
  that matters (G4) — a UI-thread `push` can interleave at any point.

- **S4 — re-asking revives.** `upsert`'s existing-key branch relinks
  whenever the resulting tier is `Prefetch`, not only when the tier
  strictly increases:

  ```rust
  if new_tier > cur_tier || new_tier == Tier::Prefetch {
  ```

  The doc records that this amends spec 0164 G5 and why (see "The trap
  this opens" above).

- **S5 — the cap.** `HEAT_REQUEST_QUEUE_MAX_ENTRIES` becomes 2048 and
  its comment is rewritten per G6.

- **S6 — the activity dot ignores doomed work.** `band_occupancy`
  (spec 0190 S1) currently sets bit 2 when *either* prefetch band is
  non-empty. It is narrowed to `prefetch_current` alone: entries the
  worker is about to discard are not work being done for the user, and
  lighting the dot for them would misreport an idle subsystem as busy.
  The method is only called on the queue, so the caches are unaffected.

- **S7 — the restart is uniform again.** `App::prefetch_step` calls
  `start_new_wave()` on the worker queue and on both caches, as before.
  Its comment states that uniformity here is not an accident of history:
  all three demote in O(1), and the queue's superseded entries are then
  discarded by the worker rather than served.

- **S8 — the type doc earns its subtlety.** `TieredBounded`'s doc says
  why `Prefetch` has two bands where every other tier has one — the O(1)
  restart — and why the alternative (a fourth `Tier`) is rejected, so a
  reader does not have to reconstruct N1's argument.

## Feasibility

Every operation this spec adds or changes is O(1). The restart path is
unchanged in cost. The worker's added work is one `remove_by_idx` plus
one mutex round-trip per superseded entry, paid only when it has nothing
live to do, and paid instead of an `inferred_candidates`/`score_all`
call that is orders of magnitude more expensive.

No public surface changes: `TieredBounded`, `HeatRequestQueue` and
`HeatWorkerHandle` are all `pub(super)` within `protolens::tui`.

The one behavioral risk is S4's interaction with `evict_one`: reviving
an entry moves it out of the preferred eviction band, so a workload that
re-asks for many superseded ranges makes the queue harder to evict from.
This is correct — those entries are live requests again — and bounded,
because reviving does not grow the structure.

## Test plan

1. `tiered.rs` — `pop_highest` never returns a superseded entry: seed
   both prefetch bands, assert `pop_highest` yields the current wave and
   then `None` while `len()` still counts the superseded entries.
2. `tiered.rs` — `discard_one_superseded` removes exactly one entry,
   from `prefetch_previous`'s head, returns `false` on an empty band,
   and genuinely reclaims the slot (`len()` drops; a previously
   `Rejected` upsert becomes `Applied`).
3. `tiered.rs` — S4: a `Prefetch` upsert on a key in `prefetch_previous`
   moves it to `prefetch_current`, so `pop_highest` serves it.
4. `tiered.rs` — the existing in-place test
   (`prefetch_repush_updates_in_place_without_moving`) is rewritten to
   assert the new rule rather than deleted, so the amendment to 0164 G5
   is pinned rather than merely dropped.
5. `heat_worker.rs` — S3: with only superseded entries queued,
   `pop_blocking` on a worker thread does not return them and instead
   blocks; the queue drains to empty; a live request pushed afterwards
   is returned promptly.
6. `heat_worker.rs` — S6: `band_occupancy`'s prefetch bit clears once
   `start_new_wave` moves the wave aside, and `activity()` reports
   `None` for a queue holding only superseded entries.
7. `tests/prefetch.rs` — after a cursor move between two `prefetch_step`
   calls, the queue still holds both entries (the old one demoted, not
   dropped), which is what distinguishes this design from discarding on
   the UI thread.
8. Interactive check on `/tmp/pdb.desc`: scroll continuously through a
   large document and confirm heat cues still resolve outward from the
   cursor, with no regression in how quickly the rows around the cursor
   settle.

## Open questions

- G6's 2048 is not measured. The useful queue depth is bounded by what
  the worker can drain before the next cursor move, not by document
  size, so a bigger cap mostly buys speculation that the next restart
  discards. A counter on `UpsertOutcome::Rejected` would show whether
  the walk ever actually parks, and is the right prerequisite to any
  further change.

## Implementation notes

The 37.1 µs figure in "Where the discard should happen" was measured
with a throwaway `#[test]` in `tiered.rs` (removed afterwards): 1000
trials, each filling a 512-entry `prefetch_current` and timing the
band walk, release build, worst trial reported. Per-entry cost ~72 ns.

Test plan item 4 turned out to be the load-bearing one. The existing
`prefetch_repush_updates_in_place_without_moving` was the only thing
pinning spec 0164 G5's in-place rule, so it was rewritten in place as
`prefetch_repush_relinks_to_the_current_tail` rather than deleted —
the amendment is now asserted, not merely absent.

Two tests had to change their *observation method* rather than their
intent, because `pop_highest` no longer reaches `prefetch_previous`:

- `start_new_wave_two_resets_layer_without_disturbing_older_order`
  checks the layering through `evict_one` instead. The ordering it
  pins still matters — it is what makes the oldest superseded wave the
  first eviction victim — it is just no longer observable by popping.
- `pop_highest_never_serves_a_superseded_wave` (replacing
  `start_new_wave_preserves_order_and_layers_correctly`) asserts both
  halves of the new invariant: `pop_highest` returns `None` while
  `len()` still counts the superseded entries.

`publish_occupancy` is deliberately *not* called after
`discard_one_superseded` in `pop_blocking`. Per S6 the prefetch bit
tracks `prefetch_current` alone, so reclaiming a superseded entry
cannot change the published occupancy — calling it would be dead work
that implies a coupling that does not exist.
