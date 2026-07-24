<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0164 — protolens: tiered priority for heat-cue queue/caches, plus main-pane prefetch

Status: implemented
Implemented in: 2026-07-24
App: protolens
Refs: docs/specs/0151-protolens-heat-cue-cache-and-startup-progress.md
      (introduced `BoundedMru` and the `by_range`/`current_score`
      caches this spec makes tier-aware, superseding their plain
      insertion-order eviction), docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
      (introduced `HeatRequestQueue` and `upgrade_active_override_to_
      complete`'s full-candidate-list prefetch — this spec formalizes
      and replaces the ad hoc two-value `Priority` split added on top
      of it by "2026-07-20 feedback", which was never itself
      committed as a spec), `protolens/src/tui/heat_worker.rs`,
      `protolens/src/tui/heat_cue.rs`, `protolens/src/tui/override_
      select.rs`, `protolens/src/tui/render.rs`, `protolens/src/tui/
      mod.rs`

## Background

Investigating the 2026-07-24 main-pane `Down`-slowdown report (fixed
separately, unrelated root cause — an O(document-size) rescan on
every worker-progress event) surfaced two adjacent, previously
undocumented gaps in how the background heat-cue scoring system
(spec 0152) prioritizes work:

**The request queue only distinguishes two priorities.**
`HeatRequestQueue`'s `Priority` enum (`UserEvent`/`Background`,
`heat_worker.rs`) was added by "2026-07-20 feedback" — a code comment,
not a spec — to stop passive per-frame re-checks from preempting a
request a user action just queued. It conflates two genuinely
different kinds of background traffic: re-verifying something
*currently on screen* (e.g. `heat_cue_resolve`'s per-visible-line
check) versus genuinely speculative work for something *not yet
visible at all*. Today only the former exists in the codebase; the
latter (read-ahead prefetch) exists only for the override pane's
candidate list (`upgrade_active_override_to_complete`, which
deliberately over-fetches everything — see below), and not at all for
the main pane, which only ever scores what's already on screen
(`warm_up_heat_cues` primes just the initial viewport once at
startup; nothing primes content scrolled into view later ahead of
time).

**The result caches have no priority concept at all.** `HeatCaches::
by_range`/`current_score` (spec 0151) are plain `BoundedMru`
instances: every read goes through the non-promoting `peek` (the
promoting `get` is never called anywhere), so eviction is really
insertion-order FIFO. A background scan's result and a directly
user-requested result are stored and evicted identically — a fresh
prefetch entry can push out an older but still-relevant
directly-requested one.

**`BoundedMru` itself is O(n) in the structure's size for every
operation that matters here.** `insert` calls `entries.retain(...)`
(linear scan to dedupe by key) then, once over `max_entries`,
`entries.remove(0)` (linear shift). `peek`/`update_in_place` do
`.iter().find(...)` (linear scan). There is no key→position index —
a plain `Vec` requires scanning to find a key at all. This is
invisible today because both the queue (capped at 512 entries) and
the caches (capped at 8192) stay small in practice, since nothing
generates background traffic for off-screen content. It stops being
invisible once the main pane gains genuine read-ahead prefetch (this
spec's other half): on a large document, a prefetch tier could
plausibly hold thousands of entries, at which point O(n) inserts/
evictions start to matter.

**The desired model** (agreed in discussion): three explicit priority
tiers, applied uniformly to the request queue *and* both caches (not
`HeatCaches::complete` — a single always-overwritten slot with no
eviction pressure, exempt), regardless of which pane generated the
traffic:

1. **User** — directly follows a user action: cursor navigation, `t`/
   `i` opening or re-sorting the override pane, or `upgrade_active_
   override_to_complete`'s full-list fetch. That fetch is a single
   `inferred_candidates`/`score_all` call covering on-screen *and*
   far off-screen candidate rows at once — it stays tagged `User` in
   full, not split or demoted, because the expensive work is paid for
   exactly once regardless of tier, and tagging it `User` guarantees
   it's never evicted/redone. (A key's tier only ever moves *up*,
   never down — there is no mechanism to demote this fetch even if it
   were desirable.)
2. **Visible** — passive re-verification of something currently
   rendered in *some* pane (main or override), but not the thing the
   user just directly acted on this instant. Existing `heat_cue_
   resolve` and `poll_pending_override_work` traffic.
3. **Prefetch** — speculative read-ahead for content not currently
   visible in any pane. New: main-pane read-ahead beyond the
   rendered viewport (G7). Not needed for the override pane, which
   already covers its off-screen candidates via its `User`-tier full
   fetch (see point 1).

**Further discussion established that `Prefetch` is not simply "the
same mechanism at a lower priority" — it differs from `User`/
`Visible` in three load-bearing ways, each fully specified at its own
goal below rather than repeated here:**

- *Ordering within the tier* (G2): `Prefetch` alone gets a second,
  lower-priority band so a walk restart can demote a whole superseded
  wave in one O(1) splice, instead of a per-entry wave number.
- *Who enqueues it* (G7): pushed inline on the main thread, one
  candidate at a time between real events — no second thread; only
  the existing worker thread's *scoring* is what stays background.
- *Who gets notified when work completes* (G10): `Prefetch`
  completions never wake the main thread — `run_loop` is one event
  in, one unconditional redraw out, so notifying on every one of a
  large read-ahead burst would mean thousands of no-op redraws and
  risk delaying the next real keypress.

## Goals

- **G1**: New `Tier` enum (`Prefetch < Visible < User`, derived
  `Ord`), replacing `Priority` (`UserEvent`/`Background`) everywhere
  it's used. Existing call sites reclassified:
  - `heat_cue_resolve`'s per-visible-line lookup (`heat_cue.rs`) →
    `Tier::Visible`.
  - `poll_pending_override_work`'s override-pane visible-window
    recheck (`override_select.rs`) → `Tier::Visible`.
  - `recompute_override_candidates`'s `Inferred`-sort first-page
    fetch, directly following `t`/`i` (`override_select.rs`) →
    `Tier::User`.
  - `upgrade_active_override_to_complete`'s full-candidate-list fetch
    (`override_select.rs`) → `Tier::User`, unchanged in scope (see
    Background).
- **G2**: New `TieredBounded<K: Eq + Hash + Clone, V: Clone>` type
  backing the request queue and both result caches — a slab
  (`Vec<Option<Slot<K, V>>>` + free list) holding all entries and one
  `HashMap<K, usize>` index for O(1) key lookup regardless of tier,
  plus one intrusive doubly-linked `Band { head, tail }` per band.
  `User` and `Visible` get one `Band` each; `Prefetch` gets *two* —
  `prefetch_current` and `prefetch_previous` (four `Band`s total; see
  below for why `Prefetch` needs a second one). No distance field,
  wave counter, or `Ord` bound on `K` anywhere: since the prefetch
  walk (G7) always pushes candidates, within one continuous sweep, in
  strictly increasing distance-from-cursor order, plain arrival order
  already *is* distance order.

  **One uniform rule for every band: `pop_highest` reads a band's
  head, `evict_one` reads a band's tail.** Which end *new* entries
  insert at is the only thing that varies:
  - `Visible` inserts at the tail (oldest sits at head — FIFO,
    unchanged from before this spec).
  - `User` inserts at the *head* — an implementation-only change from
    before this spec, not a behavior change: `User` stays LIFO (most
    recent request served first), just achieved by physically
    flipping which end holds the newest entry, so `pop_highest` can
    read "head" here too instead of switching ends per tier. `evict_
    one` correspondingly reads `User`'s tail (where the oldest
    entries now accumulate), so "oldest `User` request evicted first
    under capacity pressure" is unchanged.
  - `Prefetch` inserts only into `prefetch_current`, at the tail
    (same as `Visible`). `prefetch_previous` never receives a direct
    insert — see below.

  `Visible` is deliberately **not** changed to evict-from-tail: since
  `evict_one` is only reached when `Visible` is the lowest occupied
  tier and the structure is over capacity, tail-eviction there would
  discard the entry *just pushed* first, which can thrash (evict on
  arrival) under sustained `Visible`-tier saturation. `User` and
  `Prefetch` don't have this problem — `User`'s head-insert already
  makes tail = oldest, and `Prefetch`'s two-band split (below) exists
  precisely to keep tail-eviction pointed at genuinely superseded
  entries.

  **Why `Prefetch` needs two bands.** A single band, tail-inserted,
  correctly orders *one* continuous walk (arrival order = distance
  order). But a walk restart (G7 — cursor moves, structure changes)
  starts a fresh walk whose pushes must be served *before* whatever
  the old walk left queued, without discarding that old work outright
  (still cheap to serve if nothing better shows up first) and without
  per-entry bookkeeping to tell old from new. Two bands do this with
  a single O(1) operation instead: `prefetch_current` holds the walk
  in progress; `prefetch_previous` holds every walk that's been
  superseded, oldest-superseded toward its tail. At walk restart,
  `start_new_wave()` splices `prefetch_current`'s whole list onto
  `prefetch_previous`'s *head* (an O(1) pointer relink — `prefetch_
  current`'s tail links to `prefetch_previous`'s old head, `prefetch_
  previous`'s head becomes `prefetch_current`'s old head) and empties
  `prefetch_current`. Nothing else ever writes into `prefetch_previous`
  directly — the only two ways a key leaves it are (a) promotion to
  `Visible`/`User` on a cache hit (G9) or (b) getting spliced one
  layer deeper by the *next* restart. This needs no wave number or
  per-entry tag: `pop_highest` simply prefers `prefetch_current`'s
  head over `prefetch_previous`'s (serve the live wave first), and
  `evict_one` prefers `prefetch_previous`'s tail over `prefetch_
  current`'s (discard the longest-superseded leftover first) — see
  below.

  Provides `upsert`, `peek` (promoting, G9), `remove`, `start_new_
  wave`, `pop_highest`, and `evict_one` (private) — exact per-band
  behavior for each (which end, which order, promotion/rejection
  handling) is documented once, as doc comments, in the Specification
  section below, not repeated here.

  Two distinct structures use this same generic type, but they are
  separate instances with separate storage — a key is never
  structurally "moved" from one to the other:
  - **`HeatRequestQueue`** (G3): holds *pending* work — membership
    means "not yet computed." Popping a key off it (`pop_highest`)
    removes it from the queue entirely; nothing links it to the
    cache.
  - **`HeatCaches`** (G4): holds *results* — membership means
    "already computed." The worker thread writes into it separately,
    via its own `upsert` call, after popping the corresponding
    request off the queue.

  A key can be present in the queue, the cache, both, or neither at
  any given time.

  Replaces `BoundedMru` in all three of its current use sites;
  `BoundedMru` itself is removed once nothing references it.
- **G3**: `HeatRequestQueue` migrates onto `TieredBounded<usize,
  HeatRequest>`; `push(req, tier)` becomes a single `mru.upsert(key,
  merged, tier)` call; `pop_blocking` calls `pop_highest()`. This is
  where the LIFO-vs-FIFO distinction (G2) actually matters: a `User`
  query should preempt older still-queued `User` queries (most recent
  cursor move wins over a stale one) — `User`'s head-insert plus
  head-pop is what makes this LIFO; `Visible`/`Prefetch` traffic is
  serviced in arrival order (tail-insert, head-pop) — nothing about
  background re-verification or read-ahead calls for reordering by
  recency of the push itself, beyond `Prefetch`'s current-vs-previous
  split (G2).
- **G4**: `HeatCaches::by_range`/`current_score` migrate onto
  `TieredBounded` too, tagging every write with the tier of the
  request that produced it (`HeatRequest` gains a `tier: Tier` field
  so the worker knows what to tag its result with). `HeatCaches::
  complete` is unchanged. Cache reads go through the promoting `peek`
  (G9), not a non-mutating one.
- **G5**: A key's *tier*, once tracked in either the queue or a
  cache, only ever moves up: `upsert` combines via `existing_tier.
  max(tier)`. This governs which structure a key lives in — it does
  not, by itself, say anything about position *within* a tier. Every
  tier shares one rule: a same-or-lower-tier upsert of an
  already-tracked key updates the payload in place without moving it
  (mirrors today's `Background`/`update_in_place` behavior); a
  genuine promotion relinks the entry to the new (higher) tier's
  `Band` tail (for a promotion *to* `Prefetch` — not reachable in
  practice, since nothing is tiered below it — this would mean
  `prefetch_current`'s tail; new `Prefetch` entries always land in
  `prefetch_current`, never `prefetch_previous`, per G2). `Prefetch` has
  one deliberate nuance: a re-push from the walk (G7) revisiting a
  key that's currently sitting in `prefetch_previous` (already
  superseded by an earlier restart) updates its payload in place
  *without* pulling it back into `prefetch_current` — per G2, the
  only ways out of `prefetch_previous` are promotion (G9) or a later
  `start_new_wave()` splice, never an in-place same-tier upsert.
- **G6**: `should_skip` (an earlier, separate pre-check) is folded
  into `upsert`'s own return value instead — one lock-protected
  operation instead of check-then-act:
  ```rust
  pub(super) enum UpsertOutcome<K> {
      Applied { evicted: Option<K> },
      Rejected,
  }
  ```
  `Rejected`: the structure is at capacity and nothing at or below
  the entry's tier exists to evict, so inserting it would just evict
  the entry itself — it is not inserted at all. For `Prefetch` this
  means: *both* `prefetch_current` and `prefetch_previous` are empty and
  the whole structure is saturated by `Visible`/`User` entries
  (genuinely no room for prefetch at all — `evict_one` already
  prefers spending `prefetch_previous` first, so `Rejected` only fires
  once there's nothing left in either `Prefetch` band). Since the
  prefetch walk (G7) proceeds outward with strictly increasing
  distance, the first `Rejected` implies every subsequent candidate
  in that walk would also be rejected — so `prefetch_step` stops the
  whole walk (returns `Idle`) on the first `Rejected`, not just that
  one candidate.
- **G7**: New main-pane read-ahead, done **inline on the main
  thread**, interleaved with its ordinary event loop — no second
  thread, no per-render sweep, no fixed margin constant:
  - `run_loop`'s idle wait (currently a blocking `rx.recv()`) becomes
    a loop that alternates one unit of prefetch work with a
    non-blocking channel check:
    ```rust
    let event = loop {
        match rx.try_recv() {
            // A bare mouse-move carries no user intent (`mouse.rs`'s
            // `handle_mouse` already discards it after dequeuing),
            // but `EnableMouseCapture` makes the terminal send one
            // on essentially every pixel the pointer crosses. If
            // treated as "a real event" here, it would starve
            // prefetching any time the mouse merely hovers over the
            // window — so it's discarded transparently at this
            // level too, without breaking out of the loop.
            Ok(AppEvent::Term(Event::Mouse(m)))
                if m.kind == MouseEventKind::Moved => continue,
            Ok(ev) => break ev,
            Err(TryRecvError::Empty) => match app.prefetch_step() {
                PrefetchStep::Progressed => continue,
                PrefetchStep::Idle => break rx.recv()?, // nothing left — block for real
            },
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
    };
    ```
    Checking the channel after *every single* prefetch push (a single
    O(1) `TieredBounded::upsert` call) keeps real-input latency
    negligible — prefetch never holds the thread for more than one
    push's worth of work before yielding to a pending event.
  - `App::prefetch_step(&mut self) -> PrefetchStep` advances a
    **zigzag walk outward from the cursor's current rendered
    (`app.lines`) line** (alternating `cursor - 1`, `cursor + 1`,
    `cursor - 2`, `cursor + 2`, ... — always the nearer of the two
    unexplored ends, and naturally skipping folded/hidden content
    since it never appears in rendered-line space), pushing one
    eligible (same "worth scoring" gating `heat_lookup`/`heat_cue_
    resolve` already apply), not-yet-settled candidate per call at
    `Tier::Prefetch`, then returns `Progressed`. It returns `Idle`
    once either both ends of the document have been fully walked, or
    the last push returned `UpsertOutcome::Rejected` (G6 — capacity
    reached, stop the whole walk immediately rather than continuing
    to the next candidate; note this bound tracks the *request
    queue's* capacity, which is typically much smaller than either
    result cache's — a queue-admitted push can still have its
    eventual result discarded by the cache's own, separately-governed
    capacity later, same as any other tier, not a new gap introduced
    here).
  - The walk's progress **persists across `run_loop` iterations** —
    it is *not* rebuilt from scratch on every event. `App` gains a
    small `prefetch_walk: PrefetchWalk` field (`origin_line`, the
    current `above`/`below` offsets, `above_done`/`below_done` flags)
    carrying state between `prefetch_step` calls. It resets to a
    fresh walk from the current cursor line only when, at the start
    of a `prefetch_step` call, either: `self.cursor`'s line differs
    from `origin_line`, or the document's structural/reflow state has
    changed since the walk began (fold/unfold, spec 0162's tree
    reclamation — anything that can shift rendered line numbers or
    invalidate eligibility; exact staleness signal TBD during
    implementation, e.g. a structural-version counter). Since this is
    all on the main thread, both checks are plain field comparisons —
    no atomics, polling thread, or synchronization primitive needed.
    Events that don't move the cursor or change document structure
    (e.g. `Term(Event::Resize)`, `RootTypeResolved`, a discarded
    `Moved` mouse event) leave the walk untouched, so it simply
    resumes on the next `prefetch_step` call instead of redoing
    already-settled near-cursor work. On an actual reset, before
    starting the fresh walk, `prefetch_step` calls `start_new_wave()`
    (G2) on all three `Prefetch`-bearing structures it can reach
    synchronously — the request queue and both of `HeatCaches`'
    per-range maps — splicing each one's `prefetch_current` onto its
    `prefetch_previous` (Background). This is exact, not approximated:
    the old walk's entries are demoted as a single O(1) operation per
    structure, not left to be inferred from push timing.
  - No `PREFETCH_MARGIN` constant: capacity saturation (`Rejected`)
    is the natural stopping condition, so total work per walk is
    bounded by roughly the structure's `max_entries`, not document
    size.
  - Because this is all on the main thread, it reads `tree`,
    `heat_states`, and `line_to_node` directly, the same way every
    other `App` method already does — no synchronization, snapshot,
    or shared/`Arc`-wrapped state is needed, and this has no
    interaction with spec 0162's tree-node reclamation beyond what
    already exists today (still exactly one thread touching `tree`).
- **G8**: This applies uniformly regardless of which pane generated
  the traffic — the same `Tier`/`TieredBounded` machinery backs main-
  pane and override-pane requests alike (see G1's reclassification
  list). The override pane gets no `Prefetch` traffic of its own
  (N5) — only the main pane's inline prefetch loop (G7) generates it.
- **G9**: Cache reads promote. `HeatCaches::by_range`/`current_score`
  reads go through `TieredBounded::peek(key, tier)` (G2), which bumps
  a hit's stored tier up to `tier` if it was lower. Without this, a
  node that was cached at `Prefetch` tier and then actually scrolled
  into view would keep looking low-priority forever, and could be
  evicted ahead of genuinely-still-relevant `Visible`/`User` data the
  next time the cache fills up, even though it's now on screen.
- **G10**: Tier-aware worker completion notification. `heat_worker_
  loop` only sends `HeatWorkerProgress` when it completes a `Visible`-
  or `User`-tier request; a `Prefetch`-tier completion writes its
  result into the cache (tagged with its tier, G4) without notifying.
  This composes with G5/G9's promotion: when a line scrolls into
  view, its new `Visible`-tier lookup either (a) hits an
  already-cached, now-`peek`-promoted (G9) `Prefetch` result directly
  — no wake-up needed, the value is available synchronously in the
  same render pass — or (b) if still queued (not yet processed), gets
  promoted in the queue (G5) to `Visible`, so the worker's *eventual*
  completion is now tagged `Visible` and *does* notify. Edge case: if
  the entry was already popped and mid-flight as `Prefetch` at the
  moment of promotion (no longer addressable in the queue, unchanged
  from spec 0152's N6), the promotion instead pushes a fresh
  `Visible`-tier request; the original in-flight `Prefetch`
  computation finishes separately and silently (per this goal), while
  the fresh `Visible` request is what ultimately notifies — a minor,
  already-accepted (N6) redundant computation, not a correctness gap.

## Non-goals

- **N1**: Tuning the zigzag step size, or any other rate-limiting
  knob beyond the natural capacity-based backpressure (`Rejected`,
  G6) and the per-push channel check (G7). Reasonable defaults to
  start; revisit if wrong in practice.
- **N2**: Any change to `HeatCaches::complete` — stays a single,
  un-prioritized, always-refreshed slot.
- **N3**: Demoting an existing higher-tier entry, ever. Tiers are
  monotonically non-decreasing per key, by design (G5).
- **N4**: Any numeric or timestamp-based sub-priority within a tier —
  every band orders purely by push/insertion recency via the same
  intrusive-linked-list mechanism (G2). `Prefetch`'s current-vs-
  previous split is the one structural exception to "one band per
  tier," and is itself a *structural* distinction, not a numbered
  sub-ordering — see N7.
- **N5**: A new `Prefetch`-tier call site for the override pane. Its
  off-screen candidates are already covered by `upgrade_active_
  override_to_complete`'s single `User`-tier full fetch (Background);
  splitting that into a `User` part plus a separate `Prefetch` part
  would mean either paying for `score_all` twice or plumbing a
  partial-result carry-over, neither justified by anything observed.
- **N6**: Cancelling a request already popped and mid-flight on the
  worker thread, even if its queued copy would otherwise have been
  evicted/skipped by G6 in the meantime — unchanged from spec 0152:
  the one in-flight computation always finishes and writes its
  result.
- **N7**: A per-entry wave counter, timestamp, or any other tag
  recording *which* walk a `Prefetch` entry came from. The current-
  vs-previous distinction (G2) is carried entirely by which of the two
  `Prefetch` bands an entry links into, updated only by the O(1)
  `start_new_wave()` splice at walk restart — no field on `Slot`, no
  comparison logic, no global rescan. A same-tier re-push of a key
  already sitting in `prefetch_previous` does not pull it back into
  `prefetch_current` (G5) — it stays exactly where it is, just with a
  refreshed payload, until promoted or spliced deeper by a later
  restart.

## Specification

### `protolens/src/tui/heat_cue.rs` (or a new `tiered.rs`, TBD during
implementation)

```rust
/// Spec 0164: replaces `Priority` (`UserEvent`/`Background`). A key's
/// tier only ever moves up — see `TieredBounded::upsert`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Tier {
    Prefetch,
    Visible,
    User,
}

struct Slot<K, V> {
    key: K,
    value: V,
    tier: Tier,
    /// Intrusive recency-list links, shared by every band. To unlink
    /// a slot without a per-slot "which band am I in" tag: if `prev`
    /// is `None`, the slot is *some* band's head — check each
    /// plausible band's `head` pointer (at most two, for `Prefetch`)
    /// and fix up whichever one matches; symmetrically for `next`/
    /// `tail`. This is O(1) (a handful of pointer comparisons, not
    /// proportional to list length) and needs no extra state.
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Default)]
struct Band {
    head: Option<usize>, // pop end (see TieredBounded doc)
    tail: Option<usize>, // evict/insert end (see TieredBounded doc)
}

/// Spec 0164: bounded, tier-prioritized map — O(1) insert/promote/
/// pop/evict via a `HashMap` index plus one intrusive doubly-linked
/// `Band` per band. `User` and `Visible` get one `Band` each;
/// `Prefetch` gets two — `prefetch_current` and `prefetch_previous` —
/// so a walk restart (G7) can demote a whole superseded wave in one
/// O(1) splice instead of tracking per-entry wave numbers (G2).
/// `pop_highest` always reads a band's *head*; `evict_one` always
/// reads a band's *tail*, except `Visible` (evicts from its own
/// head too — see G2 for why). Which end *insertion* uses is the
/// only other per-band variation: `User` inserts at head (LIFO),
/// `Visible` and `prefetch_current` insert at tail (FIFO);
/// `prefetch_previous` is never inserted into directly. Shared backing
/// store for `HeatRequestQueue` and both of `HeatCaches`' per-range
/// maps — structurally-identical but independently-instantiated
/// structures, not one shared pool (G2).
pub(super) struct TieredBounded<K: Eq + Hash + Clone, V: Clone> {
    slots: Vec<Option<Slot<K, V>>>,
    free: Vec<usize>,
    index: HashMap<K, usize>,
    user: Band,
    visible: Band,
    prefetch_current: Band,
    prefetch_previous: Band,
    max_entries: usize,
}

pub(super) enum UpsertOutcome<K> {
    Applied { evicted: Option<K> },
    Rejected,
}

impl<K: Eq + Hash + Clone, V: Clone> TieredBounded<K, V> {
    pub(super) fn new(max_entries: usize) -> Self { .. }

    /// Promoting read (G9): if `key` is tracked at a tier lower than
    /// `tier`, bumps it to `tier` (relinking to the new tier's
    /// insertion end — `prefetch_current`'s tail if promoting *to*
    /// `Prefetch`, which cannot happen in practice) before returning
    /// the value. No-op reorder if already at `tier` or higher.
    pub(super) fn peek(&mut self, key: &K, tier: Tier) -> Option<V> { .. }

    /// `existing_tier.max(tier)` decides the resulting tier (G5).
    /// - Promotion (tier increases): relinks to the new tier's
    ///   insertion end.
    /// - Same-tier update (any tier): payload updates in place, no
    ///   reordering — for `Prefetch` this holds even if the key
    ///   currently lives in `prefetch_previous` (G5): it is refreshed
    ///   in place, not moved to `prefetch_current`.
    /// - Brand-new key: links at its tier's insertion end (new
    ///   `Prefetch` keys always go to `prefetch_current`).
    /// - Whenever this pushes the structure over `max_entries`,
    ///   evicts once via `evict_one`. If the *new* entry itself is
    ///   what gets evicted (only possible when nothing at or below
    ///   its tier exists to evict instead), returns `Rejected` and
    ///   links nothing.
    pub(super) fn upsert(
        &mut self,
        key: K,
        value: V,
        tier: Tier,
    ) -> UpsertOutcome<K> { .. }

    /// O(1) splice: prepends `prefetch_current`'s whole list onto
    /// `prefetch_previous`'s head, then empties `prefetch_current`. Does
    /// not touch or walk individual slots. No-op if `prefetch_
    /// current` is already empty.
    pub(super) fn start_new_wave(&mut self) { .. }

    /// Unlinks and returns an entry from the highest-priority
    /// non-empty band, checked in this order: `user`'s head,
    /// `visible`'s head, `prefetch_current`'s head, `prefetch_
    /// previous`'s head.
    pub(super) fn pop_highest(&mut self) -> Option<(K, V)> { .. }

    /// Unlinks and returns an entry from the lowest-priority
    /// non-empty band, checked in this order: `prefetch_previous`'s
    /// tail, `prefetch_current`'s tail, `visible`'s *head* (the one
    /// exception — see `TieredBounded`'s doc comment), `user`'s
    /// tail.
    fn evict_one(&mut self) -> Option<(K, V)> { .. }

    pub(super) fn remove(&mut self, key: &K) { .. }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize { .. }
}
```

`App::heat_lookup`'s `priority: Priority` parameter becomes `tier:
Tier`, forwarded unchanged into `HeatRequestQueue::push`.
`heat_cue_resolve`'s cache check calls the promoting `peek(key,
Tier::Visible)` (G9) instead of a non-mutating one; its queue-miss
push stays `Tier::Visible`.

### `protolens/src/tui/heat_worker.rs`

`HeatRequestQueueState`'s backing store becomes `TieredBounded<usize,
HeatRequest>`. `HeatRequest` gains a `tier: Tier` field. `push(req,
tier)` becomes a single `state.mru.upsert(key, merged, tier)` call
(the old three-way `match` on `(Priority, existing.is_some())` is no
longer needed — tier promotion/in-place-merge is now `upsert`'s own
job, G5). `pop_blocking` calls `state.mru.pop_highest()`.

`heat_worker_loop`'s cache writes become `c.by_range.upsert(start,
..., req.tier)` / `c.current_score.upsert((start, key), ...,
req.tier)` (G4). Notification (G10): the loop only sends
`HeatWorkerProgress` when `req.tier != Tier::Prefetch`; a `Prefetch`
completion writes its cache entry and continues silently.

### `protolens/src/tui/mod.rs`

`HeatCaches::by_range`/`current_score` field types become
`TieredBounded<usize, RangeHeatEntry>` / `TieredBounded<(usize,
String), Option<i64>>`.

New (G7):
```rust
/// Spec 0164 G7: per-`App` zigzag-walk state, persisted across
/// `run_loop` iterations (not rebuilt on every call) — reset only
/// when the cursor's line or the document's structural/reflow state
/// has changed since the walk began.
struct PrefetchWalk {
    origin_line: usize,
    above: usize,
    below: usize,
    above_done: bool,
    below_done: bool,
}

pub(super) enum PrefetchStep {
    Progressed,
    Idle,
}

impl App {
    /// Advances the zigzag prefetch walk by exactly one candidate,
    /// pushing it at `Tier::Prefetch`. First resets `self.
    /// prefetch_walk` to a fresh walk from the current cursor line
    /// if `self.cursor`'s line or the document's structural state has
    /// changed since the walk began (G7) — calling `start_new_wave()`
    /// (G2) on the request queue and both `HeatCaches` maps before
    /// the reset; otherwise resumes exactly where the previous call
    /// left off. Returns `Idle` once the document is fully walked or
    /// the last push returned `UpsertOutcome::Rejected` (G6).
    pub(super) fn prefetch_step(&mut self) -> PrefetchStep { .. }
}
```

`App` gains `prefetch_walk: PrefetchWalk`, initialized to an empty/
exhausted state so the first call naturally starts a fresh walk.

On a genuine reset, `prefetch_step` calls `start_new_wave()` on all
three `Prefetch`-bearing structures it can reach synchronously from
the main thread: `app.heat_worker`'s queue (`HeatRequestQueue`
already owns its own lock internally) and, after locking `app.
heat_caches` once (already done elsewhere, e.g. `heat_lookup`),
`by_range` and `current_score`. Three calls, each O(1) — no rescan of
either structure's contents.

`run_loop`'s idle wait is restructured per G7 to interleave
`prefetch_step` calls with non-blocking `rx.try_recv()` polls,
falling back to a real blocking `rx.recv()` only once `prefetch_step`
returns `Idle`.

### `protolens/src/tui/render.rs`

No changes. G7's read-ahead is handled entirely by `run_loop`'s
idle-wait restructuring above, not by the render pass — there is no
per-frame sweep or margin constant to add here.

### `protolens/src/tui/override_select.rs`

`recompute_override_candidates`'s and `upgrade_active_override_to_
complete`'s `Priority::UserEvent` become `Tier::User`;
`poll_pending_override_work`'s `Priority::Background` becomes
`Tier::Visible`.

## Test plan

- **`TieredBounded` unit tests** (new, `heat_cue.rs` or `tiered.rs`
  test module):
  - `User`/`Visible`: `upsert` a key at `Visible` then again at `User`
    → tier becomes `User`, entry moves to `user`'s head; `upsert` at
    `User` then again at `Visible` for the same key → tier stays
    `User` (no demotion), payload updates, position unchanged. `pop_
    highest` drains `User` entries before `Visible` before `Prefetch`;
    two `User` pushes for different keys pop most-recent-first (LIFO,
    via head-insert/head-pop); two `Visible` pushes pop oldest-first
    (FIFO, via tail-insert/head-pop). `evict_one` under `Visible`-only
    saturation removes the oldest `Visible` entry (head), not the
    newest — confirms `Visible` keeps evicting from its pop end,
    unlike `User`/`Prefetch`.
  - `Prefetch`, single wave: pushes pop in arrival order via `pop_
    highest` (oldest/nearest-pushed first, `prefetch_current`'s head)
    and evict farthest-pushed (most recently pushed) first via
    `evict_one` (`prefetch_current`'s tail, since `prefetch_previous` is
    empty). Re-pushing an already-tracked `Prefetch` key updates the
    payload in place without moving it. Promoting a `Prefetch` entry
    to `Visible`/`User` relinks it into the target tier's head.
  - `Prefetch`, `start_new_wave`: after pushing several keys and
    calling `start_new_wave`, `prefetch_current` is empty and `pop_
    highest` serves those same keys from `prefetch_previous` in their
    original relative order (i.e. the splice preserves order, doesn't
    reverse it). Pushing fresh keys after the reset makes `pop_
    highest` serve *all* of them before falling through to any
    `prefetch_previous` leftover. `evict_one` under saturation removes
    from `prefetch_previous` before ever touching `prefetch_current`. Two
    successive resets layer correctly: a third `start_new_wave` call
    prepends the (by-then-drained-or-not) `prefetch_current` ahead of
    the existing `prefetch_previous` without disturbing relative order
    within the older layer. Re-pushing a key currently in `prefetch_
    previous` updates it in place *without* moving it back into
    `prefetch_current` (G5) — confirmed by checking it's still served
    only after `prefetch_current` drains.
  - `UpsertOutcome`: `Rejected` only when the structure is at capacity
    with nothing at or below the target tier to evict — for
    `Prefetch`, only when *both* `prefetch_current` and `prefetch_
    previous` are empty and the whole structure is saturated by
    `Visible`/`User` entries; `Applied { evicted: Some(_) }` for
    ordinary churn otherwise.
- **`HeatRequestQueue`**: existing spec 0152 tests (front-of-queue
  promotion, background merge-in-place, background-new-appends-
  behind) ported to 3 tiers.
- **`HeatCaches`**: a `Prefetch`-tier `by_range` entry is evicted
  before a `Visible`/`User` one at capacity; among multiple `Prefetch`
  entries in the same wave, the most-recently-pushed one is evicted
  first (tail), while the oldest/nearest-pushed one is popped/served
  first (head); entries superseded by `start_new_wave` are evicted
  ahead of the current wave's own entries.
- **G9**: a `Prefetch`-tier cache entry, once `peek`'d at `Visible`
  tier, is retagged `Visible` and survives eviction pressure that
  would otherwise have removed it as a `Prefetch` entry.
- **G10**: completing a `Prefetch`-tier worker request does not send
  `HeatWorkerProgress`; completing a `Visible`/`User`-tier request
  does.
- **G7 (`prefetch_step`)**: successive calls push nearest-to-cursor-
  first regardless of direction (zigzag); a `Rejected` push makes the
  *next* call return `Idle` immediately with no further push
  (bounding total work independent of document size); calling
  `prefetch_step` with a changed cursor position restarts the walk
  from the new line and calls `start_new_wave()` (G2) on the queue
  and both caches before doing so — the old walk's entries survive
  (now in `prefetch_previous`, served only after the new walk's fresh
  `prefetch_current` pushes are exhausted), not discarded, not left
  ambiguous. Separately, a `run_loop`-level test (or a
  direct unit test of the restructured idle-wait) confirms a pending
  channel event is always handled before the next `prefetch_step`
  call, and that a discarded `Moved` mouse event does not count as
  such an event — prefetch never delays a real event already in the
  channel, and never starves because of pointer-tracking noise.
- **Regression**: full existing `heat_cue`/`heat_worker`/`override_
  select` suites pass unchanged in behavior for `User`/`Visible`
  traffic — only `Prefetch` is genuinely new.
- **Manual/perf validation** (`tests/profiling.rs` against `/tmp/
  db3.desc`): confirm main-pane scrolling shows evidence of read-
  ahead (subsequent `Down` presses into freshly-prefetched territory
  settle faster than a cold miss would); confirm a burst of `Prefetch`
  completions produces no visible redraw storm and never delays a
  `User`-tier request (open the override pane immediately after a
  long idle period and confirm it isn't stuck behind queued prefetch
  work); confirm CPU usage while the cursor is stationary eventually
  quiesces (`prefetch_step` returns `Idle` once it exhausts the
  document or hits `Rejected`, at which point `run_loop` falls back
  to a real blocking `recv()`), rather than spinning indefinitely.
