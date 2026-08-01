// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Background scoring worker thread (spec 0152) — keeps every
//! `inferred_candidates` call (the heat-cue miss path and the override
//! pane's `t` key) off the render/input thread, on one dedicated
//! worker thread sharing a small piece of state under a single mutex.
//! See spec 0152's "The approach, in plain terms" for the design.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use prototext_graph::score::load::LoadedGraph;

use super::event::AppEvent;
use super::heat_cue;
use super::tiered::{Tier, TieredBounded, UpsertOutcome};
use super::App;
use crate::blob::Blob;
use crate::override_pane;

/// Backpressure limit for `HeatRequestQueue` (spec 0189 G6) — not a
/// defensive bound that is never reached. When `upsert` answers
/// `Rejected`, `App::prefetch_step` returns `PrefetchStep::Idle`, which
/// parks the read-ahead walk until the worker frees a slot; under a
/// fast walk that is the designed steady state, not an anomaly. Because
/// the walk radiates outward from the cursor's row, the cap truncates
/// its *far* end and never the near end.
///
/// The useful depth is bounded by what the worker can drain before the
/// next cursor move — not by document size, since a restart re-ranks
/// everything from the new origin — so raising this buys speculation
/// that the next restart supersedes. 2048 is a judgment call; a
/// counter on `Rejected` is the prerequisite to changing it again.
pub(super) const HEAT_REQUEST_QUEUE_MAX_ENTRIES: usize = 2048;

/// One request for the worker thread (spec 0152 "plain terms"/G3):
/// which node's payload range, its currently-assigned type (if any),
/// and the `[start, end)` window of the ranked candidate list actually
/// wanted. `tier` (spec 0164 G4) tags what the worker should stamp
/// the eventual cache write with — the queue's own `TieredBounded`
/// tracks priority for ordering purposes separately, but that
/// bookkeeping doesn't survive `pop_highest`, so the request carries
/// its own copy.
#[derive(Clone)]
pub(super) struct HeatRequest {
    pub(super) range: Range<usize>,
    pub(super) current_key: Option<String>,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) tier: Tier,
}

struct HeatRequestQueueState {
    mru: TieredBounded<usize, HeatRequest>,
    stop: bool,
}

/// Merge-on-push, most-recently-touched-first request queue (spec
/// 0152 G3) — asking again for a range that's already queued merges
/// into the existing entry (union window, newest `current_key` wins)
/// and moves it to the front, rather than piling up a second entry.
pub(super) struct HeatRequestQueue {
    state: Mutex<HeatRequestQueueState>,
    condvar: Condvar,
    /// Lock-free mirror of `state.mru.band_occupancy()` (spec 0190
    /// G2/S2), so `render` can read what the queue is holding without
    /// taking the `Mutex` the worker thread holds across every pop.
    ///
    /// `Relaxed` is sufficient *and* exact: every store happens while
    /// the `Mutex` is held, so the stores are already totally ordered
    /// with respect to each other and each one publishes the state the
    /// storing thread just established. The reader tolerates a
    /// slightly old value by construction — it re-reads on the next
    /// tick (G3).
    queued: AtomicU8,
    /// The tier of the request the worker is executing right now, or 0
    /// for "idle" (spec 0190 S3). Written by the worker thread only,
    /// *outside* the `Mutex`.
    ///
    /// Kept separate from `queued` rather than packed into the same
    /// byte precisely because the two are written by different threads
    /// at different moments: separated, each writer does a plain
    /// `store` and never a read-modify-write, and there is no window
    /// in which one writer's update is half-applied from the other's
    /// point of view.
    in_flight: AtomicU8,
    /// `HeatRequestQueueState::stop`, republished outside the `Mutex` so
    /// the walk can poll it (spec 0217's `score_subset` `cancel`).
    ///
    /// A duplicate rather than a replacement: `stop` is read under the
    /// same lock as the condvar wait in `pop_blocking`, and moving it out
    /// would open the classic lost-wakeup window between the test and
    /// the wait. The walk, in contrast, holds no lock and must not take
    /// one per wire field.
    stop_flag: AtomicBool,
    /// Test-only full-sweep call counter (spec 0152/0154 test plans) —
    /// proves the "no second `score_all` call" claim for a request the
    /// cache already covers by the time the worker re-checks it.
    ///
    /// It lives here, rather than in a `static`, precisely because this
    /// queue is the one structure both `HeatWorkerHandle` and its own
    /// `heat_worker_loop` already share. A process-global counter would
    /// be read as a before/after delta while other tests in the same
    /// binary spawn real workers of their own, letting an unrelated
    /// test's sweep land inside the window — green in isolation, flaky
    /// under the full suite.
    #[cfg(test)]
    score_all_calls: AtomicUsize,
}

impl HeatRequestQueue {
    fn new() -> Self {
        HeatRequestQueue {
            state: Mutex::new(HeatRequestQueueState {
                mru: TieredBounded::new(HEAT_REQUEST_QUEUE_MAX_ENTRIES),
                stop: false,
            }),
            condvar: Condvar::new(),
            queued: AtomicU8::new(0),
            in_flight: AtomicU8::new(0),
            stop_flag: AtomicBool::new(false),
            #[cfg(test)]
            score_all_calls: AtomicUsize::new(0),
        }
    }

    /// Republishes `queued` from the state the caller has just
    /// established (spec 0190 S2). Every mutating operation calls this
    /// as its last act *before* releasing the lock, which is what
    /// makes the `Relaxed` store exact rather than approximate.
    fn publish_occupancy(&self, state: &HeatRequestQueueState) {
        self.queued
            .store(state.mru.band_occupancy(), Ordering::Relaxed);
    }

    /// Spec 0190 S3: called by `heat_worker_loop` around the scoring
    /// of one popped request. `Some(tier)` on entry, `None` once that
    /// request is done — including on the early-out path where the
    /// cache already covers it by the time the worker re-checks.
    /// Encoded as the same bitmask `band_occupancy` uses, so
    /// `activity` can simply `|` the two together; 0 means idle.
    fn set_in_flight(&self, tier: Option<Tier>) {
        let encoded = tier.map_or(0, Tier::bit);
        self.in_flight.store(encoded, Ordering::Relaxed);
    }

    /// Spec 0190 S4: the highest-priority tier that is live — queued
    /// or in flight — or `None` when the worker has nothing to do at
    /// all. Two relaxed loads and some bit twiddling; no lock, which
    /// is the whole point (G2).
    ///
    /// The in-flight tier is folded in because reporting only what is
    /// *queued* would make `User` practically invisible: a `User` push
    /// wakes the worker through the condvar, which pops it
    /// immediately, so the bit is set and cleared within microseconds
    /// and almost never survives to a draw — yet a `User` sweep is
    /// exactly the one the user is waiting on.
    pub(super) fn activity(&self) -> Option<Tier> {
        let live = self.queued.load(Ordering::Relaxed) | self.in_flight.load(Ordering::Relaxed);
        Tier::highest_in(live)
    }

    /// `tier` (spec 0164 G3) governs both where a push lands (via
    /// `TieredBounded::upsert`'s own promotion/in-place-update rules,
    /// G5) and, tagged onto the merged request itself, what the
    /// eventual worker completion should be tagged with (G4).
    /// Merging by `range.start` (union window, newest `current_key`
    /// wins) happens regardless of tier — the promoting `peek` that
    /// looks up the existing entry already applies `tier`'s own
    /// promotion, so `upsert`'s subsequent `max` is a no-op on top of
    /// it.
    fn push(&self, req: HeatRequest, tier: Tier) -> UpsertOutcome<usize> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = req.range.start;
        let existing = state.mru.peek(&key, tier);
        let merged = match &existing {
            Some(existing) => HeatRequest {
                range: req.range.clone(),
                current_key: req.current_key.clone(),
                start: existing.start.min(req.start),
                end: existing.end.max(req.end),
                tier,
            },
            None => HeatRequest { tier, ..req },
        };
        let outcome = state.mru.upsert(key, merged, tier);
        self.publish_occupancy(&state);
        self.condvar.notify_one();
        outcome
    }

    /// Blocks until a request is available or `stop` is set; pops the
    /// highest-priority entry (spec 0164 G3: `TieredBounded::
    /// pop_highest`). `None` once `stop` is set — checked *before*
    /// popping, so a `shutdown()` mid-backlog abandons whatever is
    /// still queued instead of draining it first (each entry can be
    /// an expensive `inferred_candidates` call; the one request already
    /// popped and mid-flight when `stop` was set still finishes
    /// normally — unavoidable, and bounded to one item).
    ///
    /// Spec 0189 S3: with nothing live to do, the worker reclaims
    /// superseded requests instead of scoring them — `pop_highest`
    /// never serves `prefetch_previous`. The mutex is released and
    /// retaken around each reclaimed entry, so a whole superseded wave
    /// can never block a UI-thread `push` (G4); draining the band under
    /// one lock hold would put that whole batch back on the critical
    /// path, on the worker instead of the UI thread.
    fn pop_blocking(&self) -> Option<(usize, HeatRequest)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.stop {
                return None;
            }
            if let Some(entry) = state.mru.pop_highest() {
                self.publish_occupancy(&state);
                return Some(entry);
            }
            if state.mru.discard_one_superseded() {
                drop(state);
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                continue;
            }
            state = self.condvar.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn signal_stop(&self) {
        // Before the lock: a worker mid-sweep does not hold it and is not
        // waiting on the condvar, so raising the flag first is what lets
        // the walk start unwinding while this thread is still acquiring.
        self.stop_flag.store(true, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stop = true;
        self.condvar.notify_all();
    }

    /// Spec 0164 G7: sets the in-progress `Prefetch` wave aside in one
    /// O(1) splice — called by `App::prefetch_step` on a walk restart.
    ///
    /// Unlike the `HeatCaches` maps, where a superseded entry is a
    /// computed result worth serving later, a superseded entry here is
    /// an unpaid `score_all` on a range ranked from an origin the
    /// cursor has already left. The splice stays O(1) because it is on
    /// the UI thread's critical path; the *worker* discards the demoted
    /// entries rather than scoring them (spec 0189).
    fn start_new_wave(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.mru.start_new_wave();
        self.publish_occupancy(&state);
    }

    /// Test-only entry-count introspection (spec 0152 test plan).
    #[cfg(test)]
    fn len(&self) -> usize {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.mru.len()
    }
}

/// One range's cached scoring results (spec 0152 G4) — stats and
/// candidates in one entry, since both derive from the same
/// `inferred_candidates` call and splitting them would cost a second
/// lookup/insert for what is, underneath, one piece of data.
#[derive(Clone)]
pub(super) struct RangeHeatEntry {
    pub(super) best_score: Option<i64>,
    pub(super) best_count: usize,
    /// Ranked candidates `[0, top_n.len())`.
    pub(super) top_n: Vec<(String, i64)>,
}

impl RangeHeatEntry {
    /// The entry for a range that has just been scored: `stats` is
    /// `derive_stats` of the full ranking, `top_n` an already-capped
    /// prefix of it.
    ///
    /// Every writer into `by_range` arrives with exactly that pair —
    /// the entry stores the stats flattened only because it is one
    /// cache value, and flattening them at four `upsert` sites is four
    /// chances to pair `best_score` with the wrong `best_count`.
    pub(super) fn new(stats: heat_cue::RangeHeatStats, top_n: Vec<(String, i64)>) -> Self {
        Self {
            best_score: stats.best_score,
            best_count: stats.best_count,
            top_n,
        }
    }

    /// The stats back out, for a reader that wants them without the
    /// candidate list — the exact inverse of `new`'s flattening.
    pub(super) fn stats(&self) -> heat_cue::RangeHeatStats {
        heat_cue::RangeHeatStats {
            best_score: self.best_score,
            best_count: self.best_count,
        }
    }
}

/// The most recently fully-scored range and its complete candidate
/// list — factored into a named type to keep clippy's
/// `type_complexity` lint happy.
type CompleteSlot = (Range<usize>, Vec<(String, i64)>);

/// `heat_lookup_ex`'s return type (spec 0164 G7) — factored into a
/// named type for the same reason as `CompleteSlot` above.
type HeatLookupResult = (Option<Vec<(String, i64)>>, Option<UpsertOutcome<usize>>);

/// The shared cache (spec 0152 G4) — both the render/input thread and
/// the worker thread read and write this same structure directly,
/// under `App::heat_caches`' single `Mutex`.
pub(super) struct HeatCaches {
    /// Keyed by a node's tag/length-stripped payload range's `start`
    /// offset.
    pub(super) by_range: TieredBounded<usize, RangeHeatEntry>,
    /// The current type's exact score — kept separate from `by_range`
    /// because it's keyed on an orthogonal axis (the currently-
    /// assigned type, which changes independently of a range's
    /// candidate list on every override edit) and because it may not
    /// be one of `top_n`'s entries at all.
    pub(super) current_score: TieredBounded<(usize, String), Option<i64>>,
    /// The most recently *fully* scored range's complete candidate
    /// list — a single slot, not a cache: only one override pane can
    /// be open at a time. Refreshed unconditionally by the worker
    /// every time it fully scores *any* range (G5).
    pub(super) complete: Option<CompleteSlot>,
    /// Reusable probe key for `current_score`, owned by the cache so
    /// that a lookup allocates nothing.
    ///
    /// `TieredBounded::peek` takes `&K`, and a `std` `HashMap` can only
    /// be probed by a *borrowed form of the whole key* (`K: Borrow<Q>`)
    /// — which `(usize, String)` has none of: the borrowed counterpart
    /// would be `(usize, &str)`, and `Borrow` can only hand back a
    /// reference to something the key already holds, not a rebuilt
    /// tuple. Without the probe, every lookup must present a fully
    /// owned key, allocating and copying the type name on both threads
    /// at each of `peek_current`'s call sites only to drop it again
    /// after hashing.
    ///
    /// Its contents are meaningless between calls — `peek_current`
    /// overwrites both halves before every read, and `peek` never
    /// retains the reference. Private for exactly that reason.
    probe: (usize, String),
}

impl HeatCaches {
    pub(super) fn new(max_entries: usize) -> Self {
        HeatCaches {
            by_range: TieredBounded::new(max_entries),
            current_score: TieredBounded::new(max_entries),
            complete: None,
            probe: (0, String::new()),
        }
    }

    /// The cached score of type `key` over the payload starting at
    /// `start`, promoting the entry to `tier` exactly as a direct
    /// `current_score.peek` would (spec 0164 G9).
    ///
    /// This is the only way `current_score` should be *read*: it is
    /// what keeps the probe key off the heap. Writes still go through
    /// `current_score.upsert` directly, because an insert genuinely has
    /// to own its key.
    pub(super) fn peek_current(
        &mut self,
        start: usize,
        key: &str,
        tier: Tier,
    ) -> Option<Option<i64>> {
        self.probe.0 = start;
        self.probe.1.clear();
        self.probe.1.push_str(key);
        self.current_score.peek(&self.probe, tier)
    }

    /// Read-only-in-spirit lookup, no access to the queue (spec 0152
    /// G4): `Some` (a clone of) the answer for `[start, end)` if
    /// either `by_range`'s `top_n` already covers it, or `complete`
    /// holds this exact range; `None` otherwise. `by_range`'s check
    /// goes through the promoting `peek(key, tier)` (spec 0164 G9) —
    /// a `by_range` hit bumps the entry's tracked tier up to `tier`
    /// if it was lower.
    ///
    /// `complete` is always the true, unbounded, fully-scored
    /// candidate list for whichever range it matches (spec 0152 G5),
    /// so unlike `top_n` — a growable prefix that can genuinely still
    /// be incomplete — a `complete` hit is clamped to its own length
    /// rather than requiring `candidates.len() >= end`. Without this,
    /// a node whose real candidate-type count is smaller than the
    /// window bound (e.g. `override_list_height`, seeded from the
    /// raw terminal height) could never report a hit even after the
    /// worker finished scoring it, leaving callers to busy-loop.
    pub(super) fn window(
        &mut self,
        range_start: usize,
        start: usize,
        end: usize,
        tier: Tier,
    ) -> Option<Vec<(String, i64)>> {
        // `peek_with`, not `peek`: what is wanted is `end - start`
        // candidates — a screenful — and `peek` would copy the whole
        // `top_n` to get at them, with this cache's `Mutex` held.
        let window = self.by_range.peek_with(&range_start, tier, |entry| {
            // `start` clamped exactly as in the `complete` arm below.
            // Both arms slice on the same caller-supplied pair, and
            // `len() >= end` constrains only where the window ends,
            // never that it starts at or before there.
            (entry.top_n.len() >= end).then(|| entry.top_n[start.min(end)..end].to_vec())
        });
        if let Some(window) = window.flatten() {
            return Some(window);
        }
        if let Some((range, candidates)) = &self.complete {
            if range.start == range_start {
                let end = end.min(candidates.len());
                let start = start.min(end);
                return Some(candidates[start..end].to_vec());
            }
        }
        None
    }
}

impl App {
    /// The one thing rendering code calls (spec 0152 G6) — used by
    /// both `heat_cue_for` and the override pane. Checks whether the
    /// cache already answers `[start, end)` for `range` — and, when
    /// `current_key` is given, whether that type's exact score is
    /// cached too (both must hold; `current_key: None` — the override
    /// pane's case, G7 — only requires the window itself). On a hit,
    /// returns the data. On a miss, pushes a `HeatRequest` (merging
    /// with the queue's own semantics, G3) and returns `None` —
    /// "pending". `tier` (spec 0164 G1) is forwarded unchanged to
    /// `HeatRequestQueue::push` and also used to promote a cache hit
    /// (G9) — see `Tier`'s own doc comment.
    pub(super) fn heat_lookup(
        &self,
        range: &Range<usize>,
        current_key: Option<&str>,
        start: usize,
        end: usize,
        tier: Tier,
    ) -> Option<Vec<(String, i64)>> {
        self.heat_lookup_ex(range, current_key, start, end, tier).0
    }

    /// `heat_lookup`'s full-fidelity core (spec 0164 G7): additionally
    /// returns the `UpsertOutcome` of the queue push a miss triggers
    /// (`None` on a hit, or when no worker is present) — `prefetch_
    /// step` needs this to detect `UpsertOutcome::Rejected` (G6) and
    /// stop the walk.
    pub(super) fn heat_lookup_ex(
        &self,
        range: &Range<usize>,
        current_key: Option<&str>,
        start: usize,
        end: usize,
        tier: Tier,
    ) -> HeatLookupResult {
        let ready = {
            let mut c = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
            let window = c.window(range.start, start, end, tier);
            let current_ready =
                current_key.is_none_or(|k| c.peek_current(range.start, k, tier).is_some());
            window.filter(|_| current_ready)
        };
        if ready.is_some() {
            return (ready, None);
        }
        let outcome = self.heat_worker.as_ref().map(|worker| {
            worker.push(
                HeatRequest {
                    range: range.clone(),
                    current_key: current_key.map(str::to_string),
                    start,
                    end,
                    tier,
                },
                tier,
            )
        });
        (None, outcome)
    }
}

/// Worker loop body (spec 0152 G5): block until a request is
/// available, pop the most-recently-touched one, lock the cache
/// briefly to double-check it's still actually missing (cheap
/// insurance against a request satisfied by something else between
/// being queued and being popped — not the primary dedup mechanism,
/// G3's merge-on-push is), then, if still missing, run the one real
/// expensive call with no lock held, then re-lock briefly to write
/// everything just learned into the shared cache, then notify the
/// main thread before looping again.
pub(super) fn heat_worker_loop(
    queue: Arc<HeatRequestQueue>,
    caches: Arc<Mutex<HeatCaches>>,
    graph: Arc<LoadedGraph>,
    blob: Arc<Blob>,
    progress: mpsc::Sender<AppEvent>,
    jobs: usize,
) {
    // Spec 0180 S2: the worker owns a handle to the mapping rather than a
    // `&'static` copied out of one, so the borrow below cannot outlive it.
    let graph = graph.graph();
    while let Some((start, req)) = queue.pop_blocking() {
        // Spec 0190 S3: bracket the whole handling of one request, so
        // the activity dot shows a `User` sweep the user is actually
        // waiting on. The `(true, true)` "already done" arm is inside
        // the bracket too — it is short, but leaving it out would mean
        // a stretch of work reported as idle.
        queue.set_in_flight(Some(req.tier));
        let (covers_window, covers_current) = {
            let mut c = caches.lock().unwrap_or_else(|e| e.into_inner());
            // The same predicate the readers use, rather than a
            // restatement of half of it. This check used to probe only
            // `by_range`'s `top_n` and ignore the `complete` slot, so a
            // second request for the same range with a larger `end` —
            // exactly what `upgrade_active_override_to_complete` issues
            // after `recompute_override_candidates` — reported "not
            // covered" and paid a whole second `score_all` for an
            // answer `complete` already held in full.
            let covers_window = c.window(start, req.start, req.end, req.tier).is_some();
            let covers_current = req
                .current_key
                .as_deref()
                .is_none_or(|k| c.peek_current(start, k, req.tier).is_some());
            (covers_window, covers_current)
        };
        match (covers_window, covers_current) {
            (true, true) => {} // already done
            (true, false) => {
                // Spec 0154 G2: the window is already cached — only the
                // current type's exact score is missing. Fill just that,
                // via the cheap `score_one`-backed fast path, instead of
                // re-running a full `score_all` sweep over every root.
                let range_bytes = &blob[req.range.clone()];
                let key = req
                    .current_key
                    .as_deref()
                    .expect("covers_current false implies current_key is Some");
                let score = override_pane::inferred_score(range_bytes, key, graph);
                let mut c = caches.lock().unwrap_or_else(|e| e.into_inner());
                c.current_score
                    .upsert((start, key.to_string()), score, req.tier);
            }
            (false, _) => {
                let range_bytes = &blob[req.range.clone()];
                #[cfg(test)]
                queue.score_all_calls.fetch_add(1, Ordering::SeqCst);
                let candidates = override_pane::inferred_candidates(
                    range_bytes,
                    graph,
                    jobs,
                    Some(&queue.stop_flag),
                );
                // A cancelled sweep returns a partial ranking, which would
                // be indistinguishable from a real one once written into
                // the cache — and the cache outlives this thread. Nothing
                // is waiting for it either: the only thing that raises the
                // flag is a shutdown.
                if queue.stop_flag.load(Ordering::Relaxed) {
                    queue.set_in_flight(None);
                    break;
                }
                let stats = heat_cue::derive_stats(&candidates);
                let current_score = req
                    .current_key
                    .as_deref()
                    .and_then(|k| heat_cue::score_of(&candidates, k));
                let mut c = caches.lock().unwrap_or_else(|e| e.into_inner());
                let top_n_len = c
                    .by_range
                    .peek(&start, req.tier)
                    .map_or(0, |e| e.top_n.len())
                    .max(req.end);
                c.by_range.upsert(
                    start,
                    RangeHeatEntry::new(
                        stats,
                        candidates.iter().take(top_n_len.max(1)).cloned().collect(),
                    ),
                    req.tier,
                );
                if let Some(key) = &req.current_key {
                    c.current_score
                        .upsert((start, key.clone()), current_score, req.tier);
                }
                c.complete = Some((req.range.clone(), candidates)); // always refreshed
            }
        }
        // Spec 0164 G10: a `Prefetch`-tier completion writes its cache
        // entry but never wakes the main thread — a large read-ahead
        // burst would otherwise mean thousands of no-op redraws.
        queue.set_in_flight(None);
        if req.tier != Tier::Prefetch {
            let _ = progress.send(AppEvent::HeatWorkerProgress);
        }
    }
}

/// Owns the worker thread's join handle and its request queue (spec
/// 0152 Specification). `Drop` covers the one shutdown path an
/// explicit `shutdown()` call can't reach — a panic unwinding through
/// `run_loop` before that call — see "Shutdown and safety" in spec
/// 0152.
pub(super) struct HeatWorkerHandle {
    queue: Arc<HeatRequestQueue>,
    join: Option<JoinHandle<()>>,
}

impl HeatWorkerHandle {
    pub(super) fn spawn(
        caches: Arc<Mutex<HeatCaches>>,
        graph: Arc<LoadedGraph>,
        blob: Arc<Blob>,
        progress: mpsc::Sender<AppEvent>,
        jobs: usize,
    ) -> Self {
        let queue = Arc::new(HeatRequestQueue::new());
        let worker_queue = Arc::clone(&queue);
        let join = thread::Builder::new()
            .name("heat-worker".to_string())
            .stack_size(crate::sweep::SCORING_THREAD_STACK_SIZE)
            .spawn(move || heat_worker_loop(worker_queue, caches, graph, blob, progress, jobs))
            .expect("spawn heat worker thread");
        HeatWorkerHandle {
            queue,
            join: Some(join),
        }
    }

    pub(super) fn push(&self, req: HeatRequest, tier: Tier) -> UpsertOutcome<usize> {
        self.queue.push(req, tier)
    }

    /// Spec 0164 G7: passthrough to
    /// `HeatRequestQueue::start_new_wave`.
    pub(super) fn start_new_wave(&self) {
        self.queue.start_new_wave();
    }

    /// Spec 0190 S4: passthrough to `HeatRequestQueue::activity` —
    /// what the activity dot renders. Lock-free on both sides.
    pub(super) fn activity(&self) -> Option<Tier> {
        self.queue.activity()
    }

    /// Signal stop, then block until the worker exits. Shared body
    /// with `Drop` below.
    fn shutdown_inner(&mut self) {
        self.queue.signal_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    pub(super) fn shutdown(mut self) {
        self.shutdown_inner();
    }

    /// Test-only queue-length introspection (spec 0152 test plan).
    #[cfg(test)]
    pub(super) fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Test-only full-sweep count for *this* worker (see
    /// `HeatRequestQueue::score_all_calls`).
    #[cfg(test)]
    pub(super) fn score_all_calls(&self) -> usize {
        self.queue.score_all_calls.load(Ordering::SeqCst)
    }

    /// Test-only construction (spec 0152 test plan) — a live queue
    /// with no spawned thread, so App-level "exactly one request
    /// pushed" tests can inspect the queue deterministically instead
    /// of racing a real worker thread that drains it near-instantly.
    #[cfg(test)]
    pub(super) fn stub_for_test() -> Self {
        HeatWorkerHandle {
            queue: Arc::new(HeatRequestQueue::new()),
            join: None,
        }
    }
}

impl Drop for HeatWorkerHandle {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use prototext_graph::build_scoring_graph::build_from_strings;
    use prototext_graph::score::load::LoadedGraph;

    use super::*;

    fn req(range_start: usize, start: usize, end: usize) -> HeatRequest {
        HeatRequest {
            range: range_start..range_start + 1,
            current_key: None,
            start,
            end,
            tier: Tier::User,
        }
    }

    // ── HeatRequestQueue (spec 0152/0164 test plan) ─────────────────

    /// Pushing the same `range.start` twice with different `[start,
    /// end)` windows yields one entry whose window is the union, not
    /// two entries (G3's merge-on-push behavior); the later push's
    /// `current_key` wins.
    #[test]
    fn push_merges_same_range_start_into_union_window() {
        let queue = HeatRequestQueue::new();
        queue.push(
            HeatRequest {
                range: 5..10,
                current_key: None,
                start: 0,
                end: 2,
                tier: Tier::User,
            },
            Tier::User,
        );
        queue.push(
            HeatRequest {
                range: 5..10,
                current_key: Some("x".to_string()),
                start: 1,
                end: 5,
                tier: Tier::User,
            },
            Tier::User,
        );
        assert_eq!(queue.len(), 1, "same range.start must merge into one entry");
        let (key, merged) = queue.pop_blocking().unwrap();
        assert_eq!(key, 5);
        assert_eq!(merged.start, 0);
        assert_eq!(merged.end, 5);
        assert_eq!(merged.current_key.as_deref(), Some("x"));
    }

    /// Pushing distinct ranges pops the most-recently-pushed one first
    /// (LIFO across distinct keys, head-insert/head-pop); and a later
    /// *same-tier* merging push for an already-queued key counts as a
    /// fresh query, moving it back to the head (spec 0208 S4c).
    #[test]
    fn pop_returns_most_recently_pushed_first_and_a_reask_moves_to_the_head() {
        let queue = HeatRequestQueue::new();
        queue.push(req(1, 0, 1), Tier::User);
        queue.push(req(2, 0, 1), Tier::User);
        queue.push(req(3, 0, 1), Tier::User);
        assert_eq!(queue.pop_blocking().unwrap().0, 3);
        assert_eq!(queue.pop_blocking().unwrap().0, 2);
        assert_eq!(queue.pop_blocking().unwrap().0, 1);

        queue.push(req(1, 0, 1), Tier::User);
        queue.push(req(2, 0, 1), Tier::User);
        queue.push(req(1, 1, 3), Tier::User);
        let (key, merged) = queue.pop_blocking().unwrap();
        assert_eq!(key, 1, "the re-asked key jumps back ahead of key 2");
        assert_eq!(
            (merged.start, merged.end),
            (0, 3),
            "and its window is still the union of both pushes"
        );
        assert_eq!(queue.pop_blocking().unwrap().0, 2);
    }

    // ── Activity reporting (spec 0190 test plan) ────────────────────

    fn req_at(range_start: usize, tier: Tier) -> HeatRequest {
        HeatRequest {
            tier,
            ..req(range_start, 0, 1)
        }
    }

    /// Spec 0190 S2: every mutating operation republishes `queued`
    /// before releasing the lock, so a lock-free reader sees the exact
    /// state — not an approximation that drifts.
    #[test]
    fn queued_occupancy_tracks_push_pop_and_supersede() {
        let queue = HeatRequestQueue::new();
        assert_eq!(queue.activity(), None);

        queue.push(req_at(1, Tier::Prefetch), Tier::Prefetch);
        assert_eq!(queue.activity(), Some(Tier::Prefetch));

        queue.push(req_at(2, Tier::Visible), Tier::Visible);
        assert_eq!(
            queue.activity(),
            Some(Tier::Visible),
            "the highest live tier wins, not the most recent push"
        );

        queue.push(req_at(3, Tier::User), Tier::User);
        assert_eq!(queue.activity(), Some(Tier::User));

        queue.pop_blocking(); // User
        assert_eq!(queue.activity(), Some(Tier::Visible));
        queue.pop_blocking(); // Visible
        assert_eq!(queue.activity(), Some(Tier::Prefetch));

        queue.push(req_at(4, Tier::Prefetch), Tier::Prefetch);
        queue.start_new_wave();
        assert_eq!(
            queue.activity(),
            None,
            "superseding the wave must republish it as no longer live \
             (spec 0189 S6), even though the entries are still queued"
        );
    }

    /// Spec 0189 S3: the worker reclaims superseded requests instead
    /// of returning them to be scored. With nothing live queued,
    /// `pop_blocking` must drain the superseded wave and then block —
    /// not hand any of it back — and must still serve a live request
    /// pushed afterwards.
    #[test]
    fn pop_blocking_discards_a_superseded_wave_instead_of_serving_it() {
        let queue = Arc::new(HeatRequestQueue::new());
        queue.push(req_at(1, Tier::Prefetch), Tier::Prefetch);
        queue.push(req_at(2, Tier::Prefetch), Tier::Prefetch);
        queue.start_new_wave();
        assert_eq!(queue.len(), 2, "superseded, but still occupying slots");

        let worker_queue = Arc::clone(&queue);
        let join = thread::spawn(move || worker_queue.pop_blocking());

        // The worker must consume the superseded wave without ever
        // returning it, then block on the condvar.
        let deadline = Instant::now() + Duration::from_secs(2);
        while queue.len() > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(queue.len(), 0, "the superseded wave must be reclaimed");

        queue.push(req_at(3, Tier::User), Tier::User);
        let served = join.join().expect("worker thread must not panic");
        assert_eq!(
            served.map(|(key, _)| key),
            Some(3),
            "only the live request may be served"
        );
    }

    /// Spec 0190 S3: the in-flight tier is reported even though the
    /// entry is no longer *queued*. Without this a `User` sweep — the
    /// one the user is actually waiting on — would be invisible,
    /// because the condvar wakes the worker to pop it within
    /// microseconds of the push.
    #[test]
    fn a_popped_request_is_still_reported_as_activity_while_in_flight() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(1, Tier::User), Tier::User);
        let (_, popped) = queue.pop_blocking().unwrap();
        assert_eq!(
            queue.activity(),
            None,
            "nothing queued and nothing bracketed yet"
        );

        queue.set_in_flight(Some(popped.tier));
        assert_eq!(queue.activity(), Some(Tier::User));

        queue.set_in_flight(None);
        assert_eq!(queue.activity(), None);
    }

    /// Spec 0190 S4: the two sources are combined by priority, not by
    /// recency — a queued `User` outranks an in-flight `Prefetch` and
    /// vice versa.
    #[test]
    fn activity_takes_the_highest_tier_across_queued_and_in_flight() {
        let queue = HeatRequestQueue::new();
        queue.set_in_flight(Some(Tier::Prefetch));
        queue.push(req_at(1, Tier::User), Tier::User);
        assert_eq!(queue.activity(), Some(Tier::User));

        queue.pop_blocking();
        assert_eq!(
            queue.activity(),
            Some(Tier::Prefetch),
            "the in-flight prefetch must remain visible"
        );

        queue.set_in_flight(Some(Tier::User));
        queue.push(req_at(2, Tier::Prefetch), Tier::Prefetch);
        assert_eq!(queue.activity(), Some(Tier::User));
    }

    /// Spec 0164 G1/G3: a `Tier::Visible` push for a brand-new key
    /// must not preempt a `Tier::User` request already queued — bands
    /// alone guarantee `User` drains before `Visible`, no merge logic
    /// needed.
    #[test]
    fn visible_push_of_a_new_key_does_not_preempt_a_queued_user_request() {
        let queue = HeatRequestQueue::new();
        queue.push(req(1, 0, 1), Tier::User);
        queue.push(req(2, 0, 1), Tier::Visible);
        assert_eq!(
            queue.pop_blocking().unwrap().0,
            1,
            "the User-tier request must still pop first"
        );
        assert_eq!(queue.pop_blocking().unwrap().0, 2);
    }

    /// Spec 0164 G5: a lower-tier push that merges into an
    /// already-`User`-tracked entry updates its window in place
    /// without promoting/reordering it — a `User` push for a
    /// different key stays ahead of it.
    #[test]
    fn lower_tier_push_merging_an_existing_entry_does_not_reorder_it() {
        let queue = HeatRequestQueue::new();
        queue.push(req(1, 0, 1), Tier::User);
        queue.push(req(2, 0, 1), Tier::User);
        // Re-touch key 1 at a lower tier — merges its window but must
        // not re-promote it ahead of key 2, nor change its tier.
        queue.push(req(1, 1, 3), Tier::Visible);
        assert_eq!(
            queue.pop_blocking().unwrap().0,
            2,
            "key 2 must still pop first — the Visible push must not reorder key 1"
        );
        let (key, merged) = queue.pop_blocking().unwrap();
        assert_eq!(key, 1);
        assert_eq!(
            (merged.start, merged.end),
            (0, 3),
            "the lower-tier push's window must still be merged in"
        );
    }

    /// Pushing past `HEAT_REQUEST_QUEUE_MAX_ENTRIES` caps the queue
    /// length, dropping the least-recently-touched entry first.
    #[test]
    fn push_past_capacity_evicts_least_recently_touched() {
        let queue = HeatRequestQueue::new();
        for start in 0..HEAT_REQUEST_QUEUE_MAX_ENTRIES {
            queue.push(req(start, 0, 1), Tier::User);
        }
        assert_eq!(queue.len(), HEAT_REQUEST_QUEUE_MAX_ENTRIES);
        queue.push(req(HEAT_REQUEST_QUEUE_MAX_ENTRIES, 0, 1), Tier::User);
        assert_eq!(
            queue.len(),
            HEAT_REQUEST_QUEUE_MAX_ENTRIES,
            "must stay capped"
        );
        let mut state = queue.state.lock().unwrap();
        assert!(
            state.mru.peek(&0, Tier::User).is_none(),
            "the least-recently-touched entry must be evicted"
        );
        assert!(state
            .mru
            .peek(&HEAT_REQUEST_QUEUE_MAX_ENTRIES, Tier::User)
            .is_some());
    }

    /// `pop_blocking` on a spawned thread against an empty queue
    /// blocks until `signal_stop()` (called from this test thread)
    /// wakes it, at which point it returns `None` and the thread joins
    /// promptly.
    #[test]
    fn pop_blocking_returns_none_after_signal_stop() {
        let queue = Arc::new(HeatRequestQueue::new());
        let worker_queue = Arc::clone(&queue);
        let join = thread::spawn(move || worker_queue.pop_blocking());
        thread::sleep(Duration::from_millis(20)); // let the thread start blocking
        queue.signal_stop();
        let result = join.join().expect("worker thread must not panic");
        assert!(result.is_none());
    }

    // ── HeatCaches / worker round trip (spec 0152 test plan) ────────

    /// A minimal, real, in-memory scoring graph — one message with a
    /// single `uint64` field — built with zero file I/O via
    /// `build_from_strings` + `Box::leak` + `LoadedGraph::
    /// from_static_bytes`.
    fn test_scoring_graph() -> LoadedGraph {
        let yaml = "\
entries:
- Msg
messages:
  Msg:
    fields:
    - number: 1
      type: uint64
"
        .to_string();
        let (bytes, _, _) =
            build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        LoadedGraph::from_static_bytes(bytes).expect("test graph must load")
    }

    /// Real worker thread, real tiny in-memory graph, no file I/O:
    /// pushing a request populates `by_range` with the same answer a
    /// direct, synchronous `inferred_candidates`/`derive_stats` call
    /// produces, and refreshes `complete` unconditionally (G5). A
    /// second, cache-covered request for the same range is answered
    /// without a second `score_all` call (proven via the test-only
    /// call counter) — and so is a third whose window is *wider* than
    /// anything `top_n` holds but which the `complete` slot answers in
    /// full (item D1 of the quality audit).
    #[test]
    fn heat_caches_worker_round_trip() {
        let graph = Arc::new(test_scoring_graph());
        // A valid encoding of field 1 (varint) = 5: tag 0x08, value 0x05.
        let range_bytes = vec![0x08, 0x05];
        let blob = Arc::new(Blob::unwrapped(range_bytes.clone()));
        let caches = Arc::new(Mutex::new(HeatCaches::new(8)));
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let worker = HeatWorkerHandle::spawn(
            Arc::clone(&caches),
            Arc::clone(&graph),
            Arc::clone(&blob),
            tx,
            1,
        );

        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: 1,
                tier: Tier::User,
            },
            Tier::User,
        );

        // Bounded poll, not `recv` — this isn't exercising the
        // event-driven wiring, just the worker/cache contract.
        let mut entry = None;
        for _ in 0..200 {
            if let Some(e) = caches.lock().unwrap().by_range.peek(&0, Tier::User) {
                entry = Some(e);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let entry = entry.expect("worker must populate by_range within the bounded poll");
        // Drain the progress event the first request's completion sent.
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the first request");

        let expected_candidates =
            override_pane::inferred_candidates(&range_bytes, graph.graph(), 1, None);
        let expected_stats = heat_cue::derive_stats(&expected_candidates);
        assert_eq!(entry.best_score, expected_stats.best_score);
        assert_eq!(entry.best_count, expected_stats.best_count);
        let want_top_n: Vec<_> = expected_candidates.iter().take(1).cloned().collect();
        assert_eq!(entry.top_n, want_top_n);

        let complete = caches.lock().unwrap().complete.clone();
        assert_eq!(complete, Some((0..2, expected_candidates.clone())));

        let calls_before = worker.score_all_calls();
        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: 1,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the second request");
        let calls_after = worker.score_all_calls();
        assert_eq!(
            calls_after, calls_before,
            "a cache-covered request must not re-score"
        );

        // D1: a third request for the same range with a *larger* `end`
        // than the first — what `upgrade_active_override_to_complete`
        // issues right after `recompute_override_candidates`. `top_n`
        // holds one candidate and so does not cover `end: 2`, but
        // `complete` holds the whole list and does. The worker's
        // pre-flight check used to test only the `top_n` half and pay
        // for a second full sweep here.
        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: 2,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the widened request");
        assert_eq!(
            worker.score_all_calls(),
            calls_before,
            "a request the complete slot already answers must not re-score"
        );

        worker.shutdown();
    }

    /// W-01 (spec 0154 test plan): once a range's window is already
    /// cached, a request for that same range whose `current_key` isn't
    /// cached yet is served by the cheap `score_one`-backed fast path —
    /// no additional `inferred_candidates` sweep (the existing
    /// `score_all_calls` counter stays flat), and
    /// `by_range`/`complete` are left untouched, only `current_score`
    /// gains the new entry. (W-02 — the full-sweep path itself — is
    /// covered by `heat_caches_worker_round_trip` above.)
    #[test]
    fn worker_uses_cheap_fast_path_when_only_current_is_missing() {
        let graph = Arc::new(test_scoring_graph());
        let range_bytes = vec![0x08, 0x05];
        let blob = Arc::new(Blob::unwrapped(range_bytes.clone()));
        let caches = Arc::new(Mutex::new(HeatCaches::new(8)));
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let worker = HeatWorkerHandle::spawn(
            Arc::clone(&caches),
            Arc::clone(&graph),
            Arc::clone(&blob),
            tx,
            1,
        );

        // Prime the window via a full sweep first.
        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: 1,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the priming request");
        let (by_range_before, complete_before) = {
            let mut c = caches.lock().unwrap();
            let entry = c
                .by_range
                .peek(&0, Tier::User)
                .expect("window must be primed");
            (
                (entry.best_score, entry.best_count, entry.top_n.clone()),
                c.complete.clone(),
            )
        };
        let calls_before = worker.score_all_calls();

        // Ask again for the same window, now with a current_key that
        // isn't cached yet.
        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: Some("Msg".to_string()),
                start: 0,
                end: 1,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the cheap-path request");

        let calls_after = worker.score_all_calls();
        assert_eq!(
            calls_after, calls_before,
            "the cheap fast path must not re-run a full score_all sweep"
        );
        let mut c = caches.lock().unwrap();
        assert!(
            c.peek_current(0, "Msg", Tier::User).is_some(),
            "current_score must be filled by the fast path"
        );
        let entry = c.by_range.peek(&0, Tier::User).unwrap();
        assert_eq!(
            (entry.best_score, entry.best_count, entry.top_n.clone()),
            by_range_before,
            "by_range must be untouched by the cheap path"
        );
        assert_eq!(
            c.complete, complete_before,
            "complete must be untouched by the cheap path"
        );
        drop(c);
        worker.shutdown();
    }

    // ── Spec 0164 tier-aware caches/queue integration ───────────────

    fn range_entry(marker: i64) -> RangeHeatEntry {
        RangeHeatEntry {
            best_score: Some(marker),
            best_count: 1,
            top_n: vec![(format!("T{marker}"), marker)],
        }
    }

    /// G4/G2: at capacity, a `Prefetch`-tier `by_range` entry is
    /// evicted ahead of a `Visible`-tier one.
    #[test]
    fn heat_caches_prefetch_entry_evicted_before_visible_entry() {
        let mut caches = HeatCaches::new(2);
        caches.by_range.upsert(1, range_entry(1), Tier::Visible);
        caches.by_range.upsert(2, range_entry(2), Tier::Prefetch);
        let outcome = caches.by_range.upsert(3, range_entry(3), Tier::Visible);
        assert_eq!(
            outcome,
            UpsertOutcome::Applied { evicted: Some(2) },
            "the Prefetch-tier entry must be evicted before the Visible one"
        );
    }

    /// The reused probe key must carry nothing from the previous
    /// lookup. A shorter name after a longer one is the case that
    /// catches a missing `clear()`, and a name that is a *prefix* of
    /// the previous one is the case that catches a truncating reset
    /// (`Msg` after `MsgLonger` would still hash as `MsgLonger`, and a
    /// hit would be reported for a type that was never cached).
    #[test]
    fn peek_current_does_not_carry_the_previous_probe_key() {
        let mut caches = HeatCaches::new(8);
        caches
            .current_score
            .upsert((0, "MsgLonger".to_string()), Some(7), Tier::Visible);
        assert_eq!(
            caches.peek_current(0, "MsgLonger", Tier::Visible),
            Some(Some(7))
        );
        assert_eq!(
            caches.peek_current(0, "Msg", Tier::Visible),
            None,
            "a prefix of the previous key must miss"
        );
        assert_eq!(
            caches.peek_current(0, "MsgLonger", Tier::Visible),
            Some(Some(7)),
            "and the probe must still be usable afterwards"
        );
    }

    /// The probe's `start` half must be overwritten too — a stale
    /// offset would answer for the wrong node's payload.
    #[test]
    fn peek_current_does_not_carry_the_previous_probe_offset() {
        let mut caches = HeatCaches::new(8);
        caches
            .current_score
            .upsert((10, "Msg".to_string()), Some(3), Tier::Visible);
        assert_eq!(caches.peek_current(10, "Msg", Tier::Visible), Some(Some(3)));
        assert_eq!(caches.peek_current(11, "Msg", Tier::Visible), None);
    }

    /// G9: a `by_range` entry cached at `Prefetch` tier, once `peek`'d
    /// at `Visible` tier, is retagged and survives eviction pressure
    /// that would otherwise have removed it as a `Prefetch` entry.
    #[test]
    fn heat_caches_window_promotes_a_prefetch_entry_and_it_survives_eviction() {
        let mut caches = HeatCaches::new(2);
        caches.by_range.upsert(1, range_entry(1), Tier::Prefetch);
        caches.by_range.upsert(2, range_entry(2), Tier::User);
        // Promote key 1 to Visible via the same path `heat_lookup`
        // uses (`HeatCaches::window`).
        assert!(caches.window(1, 0, 1, Tier::Visible).is_some());
        // A new Prefetch-tier push now has nothing at or below
        // Prefetch tier left to evict (key 1 is Visible, key 2 is
        // User) — it must be rejected rather than displacing the
        // promoted entry.
        let outcome = caches.by_range.upsert(3, range_entry(3), Tier::Prefetch);
        assert_eq!(
            outcome,
            UpsertOutcome::Rejected,
            "no Prefetch-or-lower entry remains to evict, so the new Prefetch push is rejected"
        );
        assert!(
            caches.by_range.peek(&1, Tier::Visible).is_some(),
            "the promoted entry must have survived"
        );
    }

    /// G10: completing a `Prefetch`-tier worker request writes its
    /// cache entry but does not send `HeatWorkerProgress`; a
    /// subsequent `Visible`-tier request for a different range does.
    #[test]
    fn worker_does_not_notify_on_prefetch_tier_completion() {
        let graph = Arc::new(test_scoring_graph());
        let range_bytes = vec![0x08, 0x05, 0x08, 0x05];
        let blob = Arc::new(Blob::unwrapped(range_bytes.clone()));
        let caches = Arc::new(Mutex::new(HeatCaches::new(8)));
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let worker = HeatWorkerHandle::spawn(
            Arc::clone(&caches),
            Arc::clone(&graph),
            Arc::clone(&blob),
            tx,
            1,
        );

        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: 1,
                tier: Tier::Prefetch,
            },
            Tier::Prefetch,
        );

        // Bounded poll for the cache write itself (no progress event to
        // wait on for a Prefetch-tier completion, by design).
        let mut saw_entry = false;
        for _ in 0..200 {
            if caches
                .lock()
                .unwrap()
                .by_range
                .peek(&0, Tier::Prefetch)
                .is_some()
            {
                saw_entry = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            saw_entry,
            "the Prefetch-tier completion must still write its cache entry"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a Prefetch-tier completion must not send HeatWorkerProgress"
        );

        worker.push(
            HeatRequest {
                range: 2..4,
                current_key: None,
                start: 0,
                end: 1,
                tier: Tier::Visible,
            },
            Tier::Visible,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("a Visible-tier completion must send HeatWorkerProgress");

        worker.shutdown();
    }
}
