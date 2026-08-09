// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Background scoring worker thread (spec 0152) — keeps every
//! `inferred_candidates` call (the heat-cue miss path and the override
//! pane's `t` key) off the render/input thread, on one dedicated
//! worker thread sharing a small piece of state under a single mutex.
//! See spec 0152's "The approach, in plain terms" for the design.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

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

/// What a worker is handed by [`HeatRequestQueue::next_task`] (spec
/// 0262 S2).
///
/// Two shapes rather than one because the cache re-check a popped
/// request needs takes the *caches* lock, and `next_task` is called
/// holding the *queue* lock. Admission is therefore its own turn: a
/// worker takes a request off the queue, checks it, and either answers
/// it outright or registers its parts in the pool for whoever asks
/// next — itself included.
enum Task {
    /// A request just off the queue, not yet checked against the cache.
    Admit { start: usize, req: HeatRequest },
    /// One part of a query already admitted and registered.
    Walk {
        start: usize,
        part: usize,
        req: HeatRequest,
        /// [`HeatRequestQueue::abort_epoch`] as it stood when this task
        /// was handed out — see there for what a change in it means.
        epoch: u64,
    },
}

/// A query the pool is walking (spec 0262 S2): the parts still to be
/// handed out, the parts handed out and not yet accounted for, and the
/// runs the finished ones produced.
///
/// The runs accumulate here rather than in any worker because no worker
/// owns the query. Whichever one accounts for the last part takes them
/// all and merges (S4).
struct ActiveQuery {
    start: usize,
    req: HeatRequest,
    /// Part indices not yet handed to a worker. A stack, so a part
    /// abandoned under S8 goes straight back to the top and is the
    /// first thing redone once the machine is free again.
    pending: Vec<usize>,
    /// Parts handed out and neither deposited nor abandoned. The query
    /// is complete when this is zero *and* `pending` is empty — either
    /// alone would call a query finished while a worker was still
    /// inside it.
    outstanding: usize,
    runs: Vec<crate::decode::RankedCandidates>,
}

/// A request as the *queue* holds it: what the caller asked for, plus
/// the window it was asked for (spec 0252 S1).
///
/// The generation is deliberately not a field of [`HeatRequest`]. No
/// caller supplies it — `push` stamps it from the queue's own counter —
/// and keeping it out of the request is what makes S1's merge rule
/// automatic rather than a rule somebody has to remember: a range
/// scrolled away and back is re-stamped with the current generation by
/// construction, instead of inheriting the stale stamp of the entry it
/// merged into.
#[derive(Clone)]
struct QueuedRequest {
    req: HeatRequest,
    generation: u64,
}

struct HeatRequestQueueState {
    mru: TieredBounded<usize, QueuedRequest>,
    stop: bool,
    /// Which window the main pane is drawing (spec 0252 S1) — bumped by
    /// [`new_window`](HeatRequestQueue::new_window) whenever the set of
    /// rows on screen changes.
    ///
    /// Plain state under the same `Mutex` as `mru` rather than an
    /// atomic: `push` reads it to stamp and `pop` reads it to compare,
    /// both already holding this lock, so keeping it here makes the two
    /// exact instead of merely close. Bumped outside the lock, a
    /// request pushed *for* the new window could be stamped with the
    /// old one and discarded, costing a frame's delay for nothing.
    generation: u64,
    /// Ranges (by `start`) somebody has asked the *whole* candidate
    /// list of and not been served yet — spec 0250 S8's gate on
    /// [`CompleteLists`].
    ///
    /// A set rather than a flag on the request because the ask and the
    /// sweep that answers it need not be the same request. Opening the
    /// override pane pushes the bounded first page and the unbounded
    /// list back to back (`open_override_on_default`); if the worker
    /// pops the first before the second is pushed they do not merge,
    /// and gating the write on the *popped* request's own `end` would
    /// make the pane pay a second full sweep for a list the first one
    /// had already computed and thrown away. The worker consults this
    /// *after* its walk instead, by which point the pane's ask —
    /// microseconds behind, against a walk of ~0.9 s — has landed.
    ///
    /// Entries are consumed on being answered, so this holds only
    /// outstanding asks: one `usize` per range whose pane is opening.
    complete_wanted: std::collections::HashSet<usize>,
    /// The ranges being swept right now, and at which tier — spec 0250
    /// S4 and S5's replacement for the single `in_flight` store.
    ///
    /// Lives here, under the same `Mutex` as `mru`, so that popping a
    /// request and registering it are one atomic act. Split across two
    /// locks there is a window in which a worker has taken a range off
    /// the queue but not yet announced it, and a second worker popping
    /// a re-push of the same range in that window sees an empty
    /// registry and sweeps it a second time — which is exactly the
    /// waste S4 exists to remove.
    ///
    /// A `Vec` because it holds at most one entry per live worker: a
    /// linear scan over a single-digit number of `usize` comparisons,
    /// on a path that already holds a lock.
    in_flight: Vec<(usize, Tier)>,
    /// The queries the pool is walking (spec 0262 S2), one entry per
    /// range in `in_flight` that got as far as needing a sweep.
    ///
    /// Same `Mutex` as `mru` and `in_flight` for the same reason they
    /// share one: handing a part out, accounting for it and declaring
    /// the query over are all decisions about the same fact, and split
    /// across two locks each seam is a window in which the pool
    /// disagrees with itself about whether a query is finished.
    active: Vec<ActiveQuery>,
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
    /// Lock-free mirror of `state.in_flight`: one bit per tier that
    /// some worker is sweeping at right now, 0 for "nothing in flight"
    /// (spec 0190 S3, amended by spec 0250 S5).
    ///
    /// This used to be a plain `store` of the *one* tier the *one*
    /// worker was on. With more than one worker that is a lie in the
    /// direction that matters: whichever worker finishes first stores
    /// "idle" while the other is still walking, so `activity()` reports
    /// an idle machine during live work — silently, since nothing
    /// crashes and no result is wrong. Mirroring the registry instead
    /// means the byte reads 0 only when the registry is empty.
    ///
    /// Kept separate from `queued` rather than packed into the same
    /// byte precisely because the two are republished at different
    /// moments: separated, each store publishes exactly the state its
    /// writer just established, and there is no window in which one
    /// writer's update is half-applied from the other's point of view.
    in_flight: AtomicU8,
    /// `HeatRequestQueueState::stop`, republished outside the `Mutex` so
    /// the walk can poll it (spec 0217's `score_subset` `cancel`).
    ///
    /// A duplicate rather than a replacement: `stop` is read under the
    /// same lock as the condvar wait in `next_task`, and moving it out
    /// would open the classic lost-wakeup window between the test and
    /// the wait. The walk, in contrast, holds no lock and must not take
    /// one per wire field.
    stop_flag: AtomicBool,
    /// Is the whole pool owed to `Tier::User` work right now (spec 0262
    /// S8)? Lock-free, because the walker polls it once per wire field.
    ///
    /// This is the `cancel` flag every sub-`User` part is walked under,
    /// so raising it stops those walks mid-field rather than at the next
    /// part boundary. A `User` request is the override pane's, with a
    /// human blocked on it, and the worst part of the document root's
    /// query is 1.5 s — too long to wait for politely.
    ///
    /// It is raised by a `User` request being *queued*, not by one being
    /// picked up: the pool has to be free before the request can be
    /// admitted, so waiting for the admission would be waiting for the
    /// thing this flag exists to arrange. Stop raises it too, so a
    /// shutdown unwinds every walk in the pool and not only the
    /// `User`-tier ones.
    stand_aside: AtomicBool,
    /// How many times `stand_aside` has been *raised* (spec 0262 S8).
    ///
    /// A worker cannot test `stand_aside` after its walk to learn
    /// whether it was cancelled: raised and lowered again while the walk
    /// ran, the flag reads false and the worker would cache a truncated
    /// — that is, wrong — ranking. A counter cannot be missed that way.
    /// It is read under the queue lock as a part is handed out, when
    /// `stand_aside` is known to be low, so any raise afterwards changes
    /// it. Unchanged therefore means "no raise happened", which is
    /// exactly "this run is whole".
    abort_epoch: AtomicU64,
    /// Test-only log of the full sweeps this queue's workers have run,
    /// in completion order (spec 0152/0154 test plans; spec 0250 S6) —
    /// proves the "no second `score_all` call" claim for a request the
    /// cache already covers by the time the worker re-checks it.
    ///
    /// A log rather than the counter it replaces, because with more
    /// than one worker a before/after *delta* no longer says **which**
    /// sweep ran: a test asserting "the pane's range was not re-scored"
    /// would pass or fail on whether some unrelated prefetch happened
    /// to land inside its window. Recording `(range.start, tier)` lets
    /// the assertion name the range it means, which is order- and
    /// concurrency-independent.
    ///
    /// It lives here, rather than in a `static`, precisely because this
    /// queue is the one structure both `HeatWorkerHandle` and its own
    /// `heat_worker_loop` already share. A process-global log would be
    /// read while other tests in the same binary spawn real workers of
    /// their own — green in isolation, flaky under the full suite.
    #[cfg(test)]
    sweeps_performed: Mutex<Vec<(usize, Tier)>>,
}

impl HeatRequestQueue {
    fn new() -> Self {
        HeatRequestQueue {
            state: Mutex::new(HeatRequestQueueState {
                mru: TieredBounded::new(HEAT_REQUEST_QUEUE_MAX_ENTRIES),
                stop: false,
                generation: 0,
                complete_wanted: std::collections::HashSet::new(),
                in_flight: Vec::new(),
                active: Vec::new(),
            }),
            condvar: Condvar::new(),
            queued: AtomicU8::new(0),
            in_flight: AtomicU8::new(0),
            stop_flag: AtomicBool::new(false),
            stand_aside: AtomicBool::new(false),
            abort_epoch: AtomicU64::new(0),
            #[cfg(test)]
            sweeps_performed: Mutex::new(Vec::new()),
        }
    }

    /// Test-only: notes that a real sweep of `range_start` is about to
    /// begin, for [`sweeps_of`](HeatWorkerHandle::sweeps_of).
    ///
    /// Called once per sweep *started*, not per part: a query is one
    /// sweep however many workers walk it and however often a part of
    /// it is abandoned and redone (spec 0262 S2/S8).
    #[cfg(test)]
    fn note_sweep(&self, range_start: usize, tier: Tier) {
        self.sweeps_performed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((range_start, tier));
    }

    /// Republishes `queued` from the state the caller has just
    /// established (spec 0190 S2). Every mutating operation calls this
    /// as its last act *before* releasing the lock, which is what
    /// makes the `Relaxed` store exact rather than approximate.
    fn publish_occupancy(&self, state: &HeatRequestQueueState) {
        self.queued
            .store(state.mru.band_occupancy(), Ordering::Relaxed);
        self.refresh_stand_aside(state);
    }

    /// Republishes `in_flight` from the registry the caller has just
    /// established (spec 0190 S3, spec 0250 S5) — the same idiom as
    /// `publish_occupancy`, and exact for the same reason: every store
    /// happens under the `Mutex`, so the stores are totally ordered
    /// and each publishes the state its writer just established.
    ///
    /// Encoded as the same bitmask `band_occupancy` uses, so
    /// `activity` can simply `|` the two together; 0 means idle.
    fn publish_in_flight(&self, state: &HeatRequestQueueState) {
        let mask = state.in_flight.iter().fold(0, |m, (_, t)| m | t.bit());
        self.in_flight.store(mask, Ordering::Relaxed);
        self.refresh_stand_aside(state);
    }

    /// Republishes [`stand_aside`](Self::stand_aside) from the state the
    /// caller has just established, counting every *raise* in
    /// [`abort_epoch`](Self::abort_epoch) (spec 0262 S8).
    ///
    /// Called from both `publish_*` above rather than from the handful
    /// of sites that can change the answer, because those two are
    /// already the "last act before releasing the lock" idiom this
    /// depends on. Calling it twice under one lock hold is free: only a
    /// transition bumps the counter, and the second call sees no
    /// transition.
    ///
    /// A stop is folded in from the atomic rather than from
    /// `state.stop`, because `signal_stop` raises the atomic *before*
    /// taking the lock — read off the state alone, a refresh landing in
    /// that window would lower the flag again and a walk already
    /// unwinding would carry on.
    fn refresh_stand_aside(&self, state: &HeatRequestQueueState) {
        let now =
            self.stop_flag.load(Ordering::Relaxed) || state.stop || Self::user_live_locked(state);
        if !self.stand_aside.swap(now, Ordering::Relaxed) && now {
            self.abort_epoch.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Is `Tier::User` work live — queued or being walked (spec 0262
    /// S7/S8)?
    ///
    /// Read off the state itself rather than off the atomic mirrors,
    /// which is what a condvar predicate must do: a mirror can be
    /// republished between the test and the wait, and the wakeup is then
    /// lost.
    ///
    /// Both halves are needed, and the in-flight half is the one that is
    /// easy to miss. Standing aside for the queued half alone would
    /// clear the pool only until the request was admitted: the moment
    /// some worker took it, everyone else would see an empty `User` band
    /// and go straight back to competing with the very query they had
    /// stepped aside for.
    fn user_live_locked(state: &HeatRequestQueueState) -> bool {
        state.mru.band_occupancy() & Tier::User.bit() != 0
            || state.in_flight.iter().any(|(_, t)| *t == Tier::User)
    }

    /// Spec 0250 S4/S5: the query over the range starting at `start` is
    /// over — deregister it and republish the mirror. Called by
    /// `heat_worker_loop` on every path out of an admitted request,
    /// including the early-outs where no sweep actually ran, and by the
    /// worker that accounts for a query's last part.
    ///
    /// `in_flight` means *a walk is happening right now*, not *somebody
    /// intends to finish this* — which is also the reading its other
    /// consumer, the activity dot, wants.
    fn end_sweep(&self, start: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.in_flight.retain(|(s, _)| *s != start);
        state.active.retain(|q| q.start != start);
        self.publish_in_flight(&state);
        // Spec 0262 S8: the end of a `User` query is the handover back
        // to everyone who stood aside for it, and this is the only thing
        // that will tell them.
        self.condvar.notify_all();
    }

    /// Registers the query over `start` as a set of `parts` tasks (spec
    /// 0262 S2) and wakes the pool to come and take them.
    ///
    /// Called by the worker that admitted the request, after its cache
    /// re-check found the answer really is missing — so a query that
    /// never needed a sweep never becomes tasks at all.
    fn activate(&self, start: usize, req: HeatRequest, parts: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active.push(ActiveQuery {
            start,
            req,
            // Reversed, so that popping this stack hands the parts out
            // in index order. The order is cosmetic — `merge` restores
            // one total order whatever sequence the runs arrive in —
            // but a `top -H` reading 0, 1, 2 is easier to believe.
            pending: (0..parts).rev().collect(),
            outstanding: 0,
            runs: Vec::new(),
        });
        self.condvar.notify_all();
    }

    /// Accounts for a finished part (spec 0262 S4). `Some` — carrying
    /// every run the query produced — exactly for the worker that walked
    /// its **last** part; that worker merges and records.
    ///
    /// The merge is charged to a worker rather than to a collector on
    /// purpose, and it is not a detail: merged serially it costs 244 ms
    /// per screenful on googleapis against 96 ms merged in the pool.
    fn deposit_part(
        &self,
        start: usize,
        run: crate::decode::RankedCandidates,
    ) -> Option<Vec<crate::decode::RankedCandidates>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let i = state.active.iter().position(|q| q.start == start)?;
        let query = &mut state.active[i];
        query.runs.push(run);
        query.outstanding -= 1;
        // Both halves: `outstanding` alone would call the query finished
        // while parts abandoned under S8 were still waiting to be redone,
        // and `pending` alone while another worker was still inside one.
        if query.outstanding > 0 || !query.pending.is_empty() {
            return None;
        }
        Some(state.active.remove(i).runs)
    }

    /// Gives a part back unwalked (spec 0262 S8) — its run was cancelled
    /// by a `User` arrival and is therefore partial, which is to say
    /// wrong rather than incomplete.
    ///
    /// It goes back on the pending stack rather than being dropped: the
    /// query is still owed an answer, and nothing else would ever ask
    /// for this part again.
    fn abandon_part(&self, start: usize, part: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(query) = state.active.iter_mut().find(|q| q.start == start) {
            query.pending.push(part);
            query.outstanding -= 1;
        }
        self.condvar.notify_all();
    }

    /// Test-only view of what is being swept right now (spec 0250 S5).
    #[cfg(test)]
    fn in_flight_ranges(&self) -> Vec<(usize, Tier)> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.in_flight.clone()
    }

    /// Test-only: the next task, which in a test with no query active
    /// is an admission, unwrapped to the request it admits.
    #[cfg(test)]
    fn admit_one(&self) -> Option<(usize, HeatRequest)> {
        match self.next_task(None)? {
            Task::Admit { start, req } => Some((start, req)),
            Task::Walk { .. } => panic!("this test activated no query"),
        }
    }

    /// Test-only: one whole worker turn — admit a request, then declare
    /// its query over, as `heat_worker_loop` does around every request
    /// whose answer the cache already holds.
    ///
    /// Tests about *ordering* must use this rather than a bare
    /// `admit_one`, which leaves the range registered as in flight:
    /// under spec 0250 S4 a later push of that same range is then
    /// correctly dropped, and a test that meant only "take the next
    /// entry" would wait on a queue it had itself emptied.
    #[cfg(test)]
    fn take_one(&self) -> Option<(usize, HeatRequest)> {
        let admitted = self.admit_one()?;
        self.end_sweep(admitted.0);
        Some(admitted)
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
        // Spec 0250 S8: recorded before the merge, since the merge is
        // what would otherwise hide an unbounded ask inside a bounded
        // request's `end`.
        if req.end == usize::MAX {
            state.complete_wanted.insert(key);
        }
        let existing = state.mru.peek(&key, tier);
        let merged = match &existing {
            Some(existing) => HeatRequest {
                range: req.range.clone(),
                current_key: req.current_key.clone(),
                start: existing.req.start.min(req.start),
                end: existing.req.end.max(req.end),
                tier,
            },
            None => HeatRequest { tier, ..req },
        };
        // Spec 0252 S1: always the *current* generation, including on a
        // merge — the ask being recorded is this one, whatever window
        // the entry it merged into belonged to.
        let generation = state.generation;
        let outcome = state.mru.upsert(
            key,
            QueuedRequest {
                req: merged,
                generation,
            },
            tier,
        );
        self.publish_occupancy(&state);
        self.condvar.notify_one();
        outcome
    }

    /// Blocks until there is something for a worker to do, or `stop` is
    /// set (spec 0262 S2/S3).
    ///
    /// Two shapes of work compete here, and they are ranked on the one
    /// axis: a part of a query already under way, and a request still on
    /// the queue. The highest tier wins; a tie goes to the part, because
    /// finishing a query is what produces an answer and starting another
    /// only spreads the pool thinner.
    ///
    /// `None` once `stop` is set — checked *before* serving anything, so
    /// a `shutdown()` mid-backlog abandons whatever is still queued
    /// instead of draining it first (each entry can be an expensive
    /// sweep; the parts already handed out when `stop` was set unwind
    /// through their `cancel` flag).
    ///
    /// Spec 0262 S8: while `User` work is live nothing below `User` is
    /// handed out at all. That is the same decision as the abort — a
    /// worker that gave a part back and were handed it straight back
    /// would abort it again immediately, and spin on the core the
    /// override pane is waiting for.
    ///
    /// Spec 0189 S3: with nothing live to do, the worker reclaims
    /// superseded requests instead of scoring them — `pop_highest`
    /// never serves `prefetch_previous`. The mutex is released and
    /// retaken around each reclaimed entry, so a whole superseded wave
    /// can never block a UI-thread `push` (G4); draining the band under
    /// one lock hold would put that whole batch back on the critical
    /// path, on the worker instead of the UI thread.
    ///
    /// Spec 0250 S4: `push` merges on `range.start` only while a
    /// request is still *queued*, so once admitted, a re-push of the
    /// same range would start a second live query over it. The results
    /// are identical, so that is pure waste — and the unit of waste is a
    /// whole query. An admission of a range some other worker is already
    /// sweeping is therefore **dropped**, not served and not requeued:
    /// dropping is self-healing, because the reader re-checks the cache
    /// each frame and pushes again once the query in flight has
    /// finished and written its answer, whereas requeueing would spin
    /// this worker on an entry it cannot act on. Nor can an ask be lost
    /// this way — `complete_wanted` is keyed on the range and consumed
    /// by whichever sweep answers it, not by this request.
    ///
    /// The admitted request is registered in `state.in_flight` before
    /// the lock is released — see that field for why the two must be
    /// one atomic act.
    /// `me` is the caller's chair (spec 0270). `None` — the tests — is a
    /// caller that is neither barred from `User` parts nor holds a seat
    /// to give back.
    fn next_task(&self, me: Option<&crate::sweep::Member<'_>>) -> Option<Task> {
        self.next_task_inner(me, true)
    }

    /// [`next_task`](Self::next_task), with the option of answering
    /// `None` instead of blocking.
    ///
    /// The non-blocking reading exists for tests, which need to ask
    /// "and is there anything else?" of a queue no worker is draining —
    /// a question that has no answer if asking it blocks forever.
    fn next_task_inner(
        &self,
        me: Option<&crate::sweep::Member<'_>>,
        blocking: bool,
    ) -> Option<Task> {
        // Spec 0270 S3: a worker with no seat is not one of the members
        // the crew is sized for, so it does not take a `User` part —
        // that part is worth more on a core of its own than on this
        // thread's share of a contended one.
        let barred = me.is_some_and(crate::sweep::Member::barred);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.stop {
                release(me);
                return None;
            }
            // Recomputed each turn rather than carried across the wait:
            // whatever woke this worker up may well be exactly this
            // changing.
            self.refresh_stand_aside(&state);
            let floor = if Self::user_live_locked(&state) {
                Tier::User
            } else {
                Tier::Prefetch
            };
            // The best part on offer. `max_by_key` keeps the *last*
            // maximum, and queries are appended as they are activated,
            // so a tie goes to the most recently started — the same
            // most-recently-touched-first rule the queue itself follows.
            let best_part = state
                .active
                .iter()
                .enumerate()
                .filter(|(_, q)| !q.pending.is_empty() && q.req.tier >= floor)
                .filter(|(_, q)| !(barred && q.req.tier == Tier::User))
                .max_by_key(|(_, q)| q.req.tier)
                .map(|(i, q)| (i, q.req.tier));
            let best_queued = Tier::highest_in(state.mru.band_occupancy()).filter(|t| *t >= floor);
            let take_part = match (best_part, best_queued) {
                (Some((_, part)), Some(queued)) => part >= queued,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if let (true, Some((i, _))) = (take_part, best_part) {
                let epoch = self.abort_epoch.load(Ordering::Relaxed);
                let query = &mut state.active[i];
                let part = query.pending.pop().expect("filtered on a non-empty stack");
                query.outstanding += 1;
                let task = Task::Walk {
                    start: query.start,
                    part,
                    req: query.req.clone(),
                    epoch,
                };
                // Spec 0270 S5: the seat is kept for a `User` part and
                // only for one. Giving it back *here* would be a gift
                // this thread immediately takes back — `sit()` returns
                // to the very CPU it just donated, on top of the
                // straggler now moving onto it.
                if query.req.tier != Tier::User {
                    release(me);
                }
                return Some(task);
            }
            if best_queued.is_some() {
                if let Some((key, entry)) = state.mru.pop_highest() {
                    self.publish_occupancy(&state);
                    // Spec 0252 S1: asked for a window that is gone, so
                    // nobody is looking at the row it would answer.
                    // `User` is exempt — the override pane is waited on
                    // whatever the window did — and `Prefetch` has
                    // `start_new_wave`.
                    //
                    // No wakeup is owed for a discard, however many it
                    // drains: nothing any worker waits on can be made
                    // *more* servable by the queue getting shorter.
                    if entry.req.tier == Tier::Visible && entry.generation != state.generation {
                        continue;
                    }
                    if state.in_flight.iter().any(|(s, _)| *s == key) {
                        continue;
                    }
                    state.in_flight.push((key, entry.req.tier));
                    self.publish_in_flight(&state);
                    if entry.req.tier != Tier::User {
                        release(me);
                    }
                    return Some(Task::Admit {
                        start: key,
                        req: entry.req,
                    });
                }
            }
            if !blocking {
                release(me);
                return None;
            }
            if state.mru.discard_one_superseded() {
                drop(state);
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                continue;
            }
            // About to sleep, so whatever this thread was sitting on is
            // better spent on a member still walking.
            release(me);
            state = self.condvar.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Test-only: the next task if there is one, without blocking.
    #[cfg(test)]
    fn try_next_task(&self) -> Option<Task> {
        self.next_task_inner(None, false)
    }

    /// Spec 0250 S8: has anyone asked for the whole candidate list of
    /// the range starting at `start`? Consumes the ask, so the sweep
    /// that answers it is the only one that records a list.
    fn take_complete_wanted(&self, start: usize) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.complete_wanted.remove(&start)
    }

    fn signal_stop(&self) {
        // Before the lock: a worker mid-sweep does not hold it and is not
        // waiting on the condvar, so raising the flags first is what lets
        // the walk start unwinding while this thread is still acquiring.
        // Both of them, because a part below `User` tier is walked under
        // `stand_aside` and would not see `stop_flag` at all.
        self.stop_flag.store(true, Ordering::Relaxed);
        self.stand_aside.store(true, Ordering::Relaxed);
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

    /// Spec 0252 S1: the main pane is drawing a different set of rows
    /// than it was, so every `Visible` request queued for the old set is
    /// an unpaid sweep on a row nobody is looking at.
    ///
    /// This is `start_new_wave`'s argument applied to the band it was
    /// never applied to. It differs in mechanism for a reason: the
    /// prefetch splice discards a *whole* wave, which is right when the
    /// origin has moved and every ranking is re-derived, whereas a
    /// one-row scroll shares all but one of its rows with the window
    /// before it. Stamping and comparing keeps those; a splice would
    /// throw them away and wait for the next frame to ask again.
    ///
    /// O(1) and nothing is walked here — the discarding happens in
    /// `pop`, on the worker, where it is free. That keeps the UI
    /// thread's critical path clear, which is the same rule spec 0164 G4
    /// imposes on the splice.
    fn new_window(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.generation += 1;
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

/// One fully-scored range and its complete candidate list — factored
/// into a named type to keep clippy's `type_complexity` lint happy.
type CompleteSlot = (Range<usize>, Vec<(String, i64)>);

/// How many complete candidate lists [`CompleteLists`] holds (spec 0250
/// S7/S9).
///
/// **A list is big, and its size follows the graph rather than the
/// document.** Measured on googleapis (49 255 roots): a real full list
/// is 49 255 entries and **4.27 MB** — 1.58 MB of `(String, i64)`
/// tuples plus 2.69 MB of FQDN bytes at a mean 54.7 B each. Vetoing
/// prunes almost nothing on a typical blob (100%, 99.6% and 19.9% of
/// roots survived on three real instance blobs), so that figure is the
/// normal case and not a worst case.
///
/// So this is a decision and not a detail: 8 lists is ~34 MB on the
/// largest corpus in hand, and a graph with twice the roots costs twice
/// that. 8 rather than 16 because the workload S7 exists for is a user
/// alternating between *a handful* of fields; nobody has measured how
/// many, and the smaller number still serves that while halving the
/// bill. Raise it only against a measurement of that alternation.
///
/// The obvious way to make this cheap is not a smaller count: two
/// thirds of a list is FQDN bytes copied out of the archived graph,
/// which `EntryScore` already borrows as `&'g str`. Borrowing them
/// through would cost 1.18 MB a list rather than 4.27 MB, but it means
/// threading the graph lifetime through `RankedCandidates` and every
/// cache that holds one — out of scope here, and recorded so the next
/// person weighing this count knows the alternative exists.
pub(super) const COMPLETE_LIST_ENTRIES: usize = 8;

/// The complete candidate lists of recently fully-scored ranges (spec
/// 0250 S7), most-recently-used first.
///
/// This replaces the single slot that preceded it, whose defect was
/// that *every* completed sweep overwrote it: a user alternating
/// between a handful of fields re-scored each one from scratch, because
/// a sweep for somewhere else had evicted the answer in between.
///
/// Keyed on the range's `start` offset alone, and needing no
/// invalidation, because a range is a byte offset into a blob that is
/// immutable for the whole session (spec 0216).
#[derive(Default)]
pub(super) struct CompleteLists {
    /// MRU order, front first. A `Vec` rather than a map because the
    /// cap is single digits: a linear scan over 8 `usize` comparisons
    /// beats hashing, and the MRU order is the storage order.
    entries: Vec<CompleteSlot>,
}

impl CompleteLists {
    /// The list for the range starting at `range_start`, promoted to
    /// most-recently-used.
    fn get(&mut self, range_start: usize) -> Option<&[(String, i64)]> {
        let i = self
            .entries
            .iter()
            .position(|(r, _)| r.start == range_start)?;
        if i != 0 {
            self.entries[..=i].rotate_right(1);
        }
        Some(&self.entries[0].1)
    }

    /// Records `candidates` as the complete list for `range`, evicting
    /// the least-recently-used entry once the cap is exceeded.
    ///
    /// A re-insert on a range already held replaces it in place rather
    /// than piling up a second entry: the key is a byte offset into an
    /// immutable blob, so the two lists are the same list.
    pub(super) fn insert(&mut self, range: Range<usize>, candidates: Vec<(String, i64)>) {
        self.entries.retain(|(r, _)| r.start != range.start);
        self.entries.insert(0, (range, candidates));
        self.entries.truncate(COMPLETE_LIST_ENTRIES);
    }

    /// Test-only entry-count introspection.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Test-only: the most recently used entry, which for a test that
    /// has inserted once is the one it inserted.
    #[cfg(test)]
    pub(super) fn newest(&self) -> Option<&CompleteSlot> {
        self.entries.first()
    }
}

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
    /// The complete candidate lists of recently fully-scored ranges
    /// (spec 0250 S7) — what the override pane's unbounded
    /// `[0, usize::MAX)` request reads, and the only cache here that
    /// holds a whole ranking rather than a screenful of one.
    ///
    /// Written only by the paths that produce a list *for the override
    /// pane* (S8): a whole-list request the worker serves,
    /// `seed_root_heat`'s startup ranking, and the pane handing its own
    /// list back on close. A prefetch never writes here — it would
    /// evict the answer the user is alternating between, which is the
    /// defect S7 exists to fix.
    pub(super) complete: CompleteLists,
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
            complete: CompleteLists::default(),
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
    /// still holds this range; `None` otherwise. `by_range`'s check
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
        if let Some(candidates) = self.complete.get(range_start) {
            let end = end.min(candidates.len());
            let start = start.min(end);
            return Some(candidates[start..end].to_vec());
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

/// Everything learned from one completed sweep, written into the shared
/// cache under one lock hold (spec 0152 G5).
///
/// Factored out because the same writes follow a sweep however its parts
/// were shared out — and a second copy of them is a second chance to
/// pair `best_score` with the wrong `best_count`.
fn record_sweep(
    queue: &HeatRequestQueue,
    caches: &Mutex<HeatCaches>,
    start: usize,
    req: &HeatRequest,
    candidates: Vec<(String, i64)>,
) {
    let wants_complete = queue.take_complete_wanted(start);
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
    // Spec 0250 S8: a list is recorded only where somebody asked for a
    // whole list of this range — which is exactly
    // `upgrade_active_override_to_complete`, the override pane's
    // `[0, usize::MAX)` request. Every other caller asks for a screenful
    // and is served from `by_range`, and a prefetch that wrote here
    // would evict the answer the user is alternating between.
    //
    // Asked *after* the walk, so this thread's own sweep can serve an
    // ask that arrived while it was walking.
    if wants_complete {
        c.complete.insert(req.range.clone(), candidates);
    }
}

/// Spec 0270 S4: give the seat back, if this caller was holding one.
///
/// The widening is here rather than inside `Member::leave` because the
/// startup sweep's main thread also leaves a chair, and it must keep
/// the drawing core spec 0265 gave it. Only a pool worker wants every
/// CPU back.
fn release(me: Option<&crate::sweep::Member<'_>>) {
    if me.is_some_and(crate::sweep::Member::leave) {
        crate::affinity::widen();
    }
}

/// Worker loop body (spec 0152 G5, spec 0262 S2): take whatever the
/// pool has for this thread and do it, until stop.
///
/// A task is one of two things. An **admission** is a request just off
/// the queue: lock the cache briefly to double-check the answer is still
/// actually missing (cheap insurance against a request satisfied by
/// something else between being queued and being taken — not the primary
/// dedup mechanism, G3's merge-on-push is), and if it is missing,
/// register the query's parts for the whole pool to walk. A **part** is
/// one of those: walk it with no lock held, hand the run back, and — if
/// this was the query's last part — merge the runs, write everything
/// learned into the shared cache and notify the main thread.
///
/// Nothing here owns a query, which is the point. A screenful is ~40
/// queries of wildly unequal size, and one worker per query leaves the
/// pool idle behind whichever query is the document root's.
#[allow(clippy::too_many_arguments)]
pub(super) fn heat_worker_loop(
    me: usize,
    queue: Arc<HeatRequestQueue>,
    caches: Arc<Mutex<HeatCaches>>,
    graph: Arc<LoadedGraph>,
    blob: Arc<Blob>,
    progress: mpsc::Sender<AppEvent>,
    partition: Arc<crate::sweep::Partition>,
    crew: Arc<crate::sweep::Crew>,
) {
    // Spec 0180 S2: the worker owns a handle to the mapping rather than a
    // `&'static` copied out of one, so the borrow below cannot outlive it.
    let graph = graph.graph();
    // Spec 0270 S2: the seat is the worker index. Nothing is claimed
    // here — the chair is only occupied while a `User` part is in hand.
    let me = crew.member(me);
    while let Some(task) = queue.next_task(Some(&me)) {
        match task {
            Task::Admit { start, req } => {
                // Spec 0190 S3: `next_task` has already registered this
                // request as in flight, so the activity dot shows a query
                // the user is actually waiting on. The "already done" arm
                // is inside that bracket too — it is short, but leaving
                // it out would mean a stretch of work reported as idle.
                let (covers_window, covers_current) = {
                    let mut c = caches.lock().unwrap_or_else(|e| e.into_inner());
                    // The same predicate the readers use, rather than a
                    // restatement of half of it. This check used to probe
                    // only `by_range`'s `top_n` and ignore the `complete`
                    // slot, so a second request for the same range with a
                    // larger `end` — exactly what
                    // `upgrade_active_override_to_complete` issues after
                    // `recompute_override_candidates` — reported "not
                    // covered" and paid a whole second sweep for an
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
                        // Spec 0154 G2: the window is already cached —
                        // only the current type's exact score is missing.
                        // Fill just that, via the cheap `score_one`-backed
                        // fast path, instead of a full sweep over every
                        // root.
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
                        // The one path that becomes work for the pool.
                        // The range stays registered in flight until the
                        // last of its parts is accounted for, so no
                        // `end_sweep` here.
                        #[cfg(test)]
                        queue.note_sweep(start, req.tier);
                        queue.activate(start, req, partition.parts());
                        continue;
                    }
                }
                queue.end_sweep(start);
                // Spec 0164 G10: a `Prefetch`-tier completion writes its
                // cache entry but never wakes the main thread — a large
                // read-ahead burst would otherwise mean thousands of
                // no-op redraws.
                if req.tier != Tier::Prefetch {
                    let _ = progress.send(AppEvent::HeatWorkerProgress);
                }
            }
            Task::Walk {
                start,
                part,
                req,
                epoch,
            } => {
                // Spec 0262 S8: a part below `User` tier is walked under
                // `stand_aside`, so the override pane's arrival stops it
                // mid-field rather than at the end of a part that can
                // take 1.5 s. A `User` part steps aside for nobody and
                // watches only the shutdown flag.
                let cancel = if req.tier == Tier::User {
                    &queue.stop_flag
                } else {
                    &queue.stand_aside
                };
                // Spec 0270 S4: take the seat for the length of this
                // part and no longer. The guard stamps the chair so a
                // member that runs out of parts can find the longest
                // runner and hand its own core over; every way out of
                // the walk below drops it.
                let user = req.tier == Tier::User;
                let walking = user.then(|| {
                    me.sit();
                    me.walking()
                });
                let run = partition.walk(part, &blob[req.range.clone()], graph, Some(cancel));
                drop(walking);
                if queue.stop_flag.load(Ordering::Relaxed) {
                    queue.end_sweep(start);
                    break;
                }
                // A cancelled walk returns a partial ranking, which would
                // be indistinguishable from a real one once written into
                // the cache — and the cache outlives this thread. The
                // epoch is what says whether this one was cancelled; see
                // `HeatRequestQueue::abort_epoch` for why the flag itself
                // cannot answer that.
                if req.tier != Tier::User && queue.abort_epoch.load(Ordering::Relaxed) != epoch {
                    queue.abandon_part(start, part);
                    continue;
                }
                let Some(runs) = queue.deposit_part(start, run) else {
                    continue;
                };
                // Spec 0262 S4: the merge is the last part's own work.
                record_sweep(&queue, &caches, start, &req, crate::sweep::merge(runs));
                queue.end_sweep(start);
                if req.tier != Tier::Prefetch {
                    let _ = progress.send(AppEvent::HeatWorkerProgress);
                }
            }
        }
    }
}

/// Owns the worker threads' join handles and their request queue (spec
/// 0152 Specification). `Drop` covers the one shutdown path an
/// explicit `shutdown()` call can't reach — a panic unwinding through
/// `run_loop` before that call — see "Shutdown and safety" in spec
/// 0152.
pub(super) struct HeatWorkerHandle {
    queue: Arc<HeatRequestQueue>,
    joins: Vec<JoinHandle<()>>,
}

impl HeatWorkerHandle {
    /// Spawns `jobs` workers behind one queue and one partition.
    ///
    /// `jobs` now has a single reading (spec 0262 S6): the number of
    /// threads. It reaches the partition too, but only as
    /// [`target_parts`](crate::sweep::target_parts)' input — how finely
    /// a query is cut, not how widely any one of them spreads. Nothing
    /// fans out any more, so nothing can multiply.
    pub(super) fn spawn(
        caches: Arc<Mutex<HeatCaches>>,
        graph: Arc<LoadedGraph>,
        blob: Arc<Blob>,
        progress: mpsc::Sender<AppEvent>,
        jobs: usize,
    ) -> Self {
        let queue = Arc::new(HeatRequestQueue::new());
        // Spec 0262 S1: once for the process, not once per query —
        // `partition_roots` is 7.3 ms on googleapis against a median
        // visible query of 5.4–10.4 ms.
        let partition = Arc::new(crate::sweep::Partition::new(graph.graph(), jobs));
        // Spec 0270 S6: one crew for the pool's whole life, holding one
        // chair per worker. A chair is only occupied while its worker
        // walks a `User` part; between queries every one of them is
        // vacant and the crew is inert.
        let crew = Arc::new(crate::sweep::Crew::new(jobs.max(1)));
        let joins = (0..jobs.max(1))
            .map(|n| {
                spawn_worker(
                    n,
                    Arc::clone(&queue),
                    Arc::clone(&caches),
                    Arc::clone(&graph),
                    Arc::clone(&blob),
                    progress.clone(),
                    Arc::clone(&partition),
                    Arc::clone(&crew),
                )
            })
            .collect();
        HeatWorkerHandle { queue, joins }
    }

    pub(super) fn push(&self, req: HeatRequest, tier: Tier) -> UpsertOutcome<usize> {
        self.queue.push(req, tier)
    }

    /// Spec 0164 G7: passthrough to
    /// `HeatRequestQueue::start_new_wave`.
    pub(super) fn start_new_wave(&self) {
        self.queue.start_new_wave();
    }

    /// Spec 0252 S1: passthrough to [`HeatRequestQueue::new_window`].
    pub(super) fn new_window(&self) {
        self.queue.new_window();
    }

    /// Spec 0190 S4: passthrough to `HeatRequestQueue::activity` —
    /// what the activity dot renders. Lock-free on both sides.
    pub(super) fn activity(&self) -> Option<Tier> {
        self.queue.activity()
    }

    /// Signal stop, then block until every worker exits. Shared body
    /// with `Drop` below.
    fn shutdown_inner(&mut self) {
        self.queue.signal_stop();
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }

    pub(super) fn shutdown(mut self) {
        self.shutdown_inner();
    }

    /// Test-only: `shutdown` without consuming the handle, so that the
    /// sweep log can be read once every worker has stopped writing to
    /// it. An assertion about how many sweeps happened is otherwise
    /// racing the workers it is counting.
    #[cfg(test)]
    pub(super) fn stop_and_join(&mut self) {
        self.shutdown_inner();
    }

    /// Test-only queue-length introspection (spec 0152 test plan).
    #[cfg(test)]
    pub(super) fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Test-only: the range the queue would hand out next, taken off
    /// it — which is how a test asks "and which of these is first?".
    #[cfg(test)]
    pub(super) fn take_next_range(&self) -> Option<usize> {
        self.queue.take_one().map(|(start, _)| start)
    }

    /// Test-only: take the next range off the queue and leave it
    /// *registered as in flight* — the state a worker is in for the
    /// whole of a walk, and the one spec 0263 S3 is about. `activity()`
    /// still reports it; a "the queue is empty" predicate would not.
    #[cfg(test)]
    pub(super) fn admit_next_range(&self) -> Option<usize> {
        self.queue.admit_one().map(|(start, _)| start)
    }

    /// Test-only full-sweep count for *this* worker (see
    /// `HeatRequestQueue::sweeps_performed`).
    #[cfg(test)]
    pub(super) fn score_all_calls(&self) -> usize {
        self.queue
            .sweeps_performed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Test-only: how many full sweeps of the range starting at
    /// `range_start` this worker has run (spec 0250 S6).
    ///
    /// This, not the count above, is what an assertion about one
    /// range should use: it is independent of what any other range's
    /// sweeps did in the meantime.
    #[cfg(test)]
    pub(super) fn sweeps_of(&self, range_start: usize) -> usize {
        self.queue
            .sweeps_performed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(s, _)| *s == range_start)
            .count()
    }

    /// Test-only construction (spec 0152 test plan) — a live queue
    /// with no spawned thread, so App-level "exactly one request
    /// pushed" tests can inspect the queue deterministically instead
    /// of racing a real worker thread that drains it near-instantly.
    #[cfg(test)]
    pub(super) fn stub_for_test() -> Self {
        HeatWorkerHandle {
            queue: Arc::new(HeatRequestQueue::new()),
            joins: Vec::new(),
        }
    }

    /// Test-only: `spawn`, but over the queue this stub already holds
    /// rather than a fresh one, so requests pushed *before* any thread
    /// existed are the ones the worker goes on to serve.
    ///
    /// This is what lets a test assert on the pending state without
    /// asserting on the scheduler. `heat_cue_resolve` pushes a request
    /// and reads the cache back as two separate lock acquisitions, so a
    /// worker running at the time can legally settle the node inside
    /// that window — and if it does, the very first call returns a
    /// settled cue and the path under test is never reached. Queueing
    /// against a stub and only then starting the thread removes the
    /// window instead of racing it.
    #[cfg(test)]
    pub(super) fn start_for_test(
        mut self,
        caches: Arc<Mutex<HeatCaches>>,
        graph: Arc<LoadedGraph>,
        blob: Arc<Blob>,
        progress: mpsc::Sender<AppEvent>,
        jobs: usize,
    ) -> Self {
        let partition = Arc::new(crate::sweep::Partition::new(graph.graph(), jobs));
        let crew = Arc::new(crate::sweep::Crew::new(jobs.max(1)));
        self.joins = (0..jobs.max(1))
            .map(|n| {
                spawn_worker(
                    n,
                    Arc::clone(&self.queue),
                    Arc::clone(&caches),
                    Arc::clone(&graph),
                    Arc::clone(&blob),
                    progress.clone(),
                    Arc::clone(&partition),
                    Arc::clone(&crew),
                )
            })
            .collect();
        self
    }
}

/// One worker thread. Numbered in its name so that a stack trace or a
/// `top -H` says which of them is walking.
#[allow(clippy::too_many_arguments)]
fn spawn_worker(
    n: usize,
    queue: Arc<HeatRequestQueue>,
    caches: Arc<Mutex<HeatCaches>>,
    graph: Arc<LoadedGraph>,
    blob: Arc<Blob>,
    progress: mpsc::Sender<AppEvent>,
    partition: Arc<crate::sweep::Partition>,
    crew: Arc<crate::sweep::Crew>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("heat-worker-{n}"))
        .stack_size(crate::sweep::SCORING_THREAD_STACK_SIZE)
        .spawn(move || {
            // Spec 0264 S7: undo the main thread's narrowing, inherited
            // across `clone(2)`. A heat worker is throughput work and
            // wants every CPU the process was given.
            crate::affinity::widen();
            heat_worker_loop(n, queue, caches, graph, blob, progress, partition, crew)
        })
        .expect("spawn heat worker thread")
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
    use crate::affinity::Seat;

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
        let (key, merged) = queue.take_one().unwrap();
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
        assert_eq!(queue.take_one().unwrap().0, 3);
        assert_eq!(queue.take_one().unwrap().0, 2);
        assert_eq!(queue.take_one().unwrap().0, 1);

        queue.push(req(1, 0, 1), Tier::User);
        queue.push(req(2, 0, 1), Tier::User);
        queue.push(req(1, 1, 3), Tier::User);
        let (key, merged) = queue.take_one().unwrap();
        assert_eq!(key, 1, "the re-asked key jumps back ahead of key 2");
        assert_eq!(
            (merged.start, merged.end),
            (0, 3),
            "and its window is still the union of both pushes"
        );
        assert_eq!(queue.take_one().unwrap().0, 2);
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

        queue.take_one(); // User
        assert_eq!(queue.activity(), Some(Tier::Visible));
        queue.take_one(); // Visible
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
    /// `next_task` must drain the superseded wave and then block —
    /// not hand any of it back — and must still serve a live request
    /// pushed afterwards.
    #[test]
    fn next_task_discards_a_superseded_wave_instead_of_serving_it() {
        let queue = Arc::new(HeatRequestQueue::new());
        queue.push(req_at(1, Tier::Prefetch), Tier::Prefetch);
        queue.push(req_at(2, Tier::Prefetch), Tier::Prefetch);
        queue.start_new_wave();
        assert_eq!(queue.len(), 2, "superseded, but still occupying slots");

        let worker_queue = Arc::clone(&queue);
        let join = thread::spawn(move || worker_queue.admit_one());

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

    // ── Stale visible requests (spec 0252 S1) ───────────────────────

    /// The rule itself: a `Visible` request asked for a window that has
    /// since scrolled away answers a row nobody is looking at, so the
    /// worker discards it rather than paying a sweep for it.
    #[test]
    fn a_visible_request_for_a_departed_window_is_dropped_not_served() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(1, Tier::Visible), Tier::Visible);
        queue.new_window();
        queue.push(req_at(2, Tier::Visible), Tier::Visible);

        assert_eq!(
            queue.take_one().map(|(key, _)| key),
            Some(2),
            "the entry asked for the current window is served"
        );
        assert!(
            queue.try_next_task().is_none(),
            "and the one asked for the window before it is discarded"
        );
        assert_eq!(queue.len(), 0, "discarded, not merely skipped over");
    }

    /// The exemption, which is the half a naive implementation gets
    /// wrong. `User` is the cursor's own row or the override pane, and
    /// the user is waiting on it whatever the window did.
    #[test]
    fn a_user_request_survives_a_window_change() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(1, Tier::User), Tier::User);
        queue.new_window();
        assert_eq!(queue.take_one().map(|(key, _)| key), Some(1));
    }

    /// The merge rule. Without it a row scrolled away and back inherits
    /// the stale stamp of the entry it merged into and is discarded
    /// every time it is asked for — so it would never resolve at all.
    #[test]
    fn a_re_pushed_range_adopts_the_current_generation() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(7, Tier::Visible), Tier::Visible);
        queue.new_window();
        queue.push(req_at(7, Tier::Visible), Tier::Visible);
        assert_eq!(queue.len(), 1, "the same range.start still merges");
        assert_eq!(
            queue.take_one().map(|(key, _)| key),
            Some(7),
            "re-asking re-stamps: the merged entry belongs to this window"
        );
    }

    /// The drain happens inside one `next_task`, not one discard per
    /// call. A caller that had to ask 2048 times to reach the live entry
    /// would be starved by exactly the backlog this rule exists to
    /// clear.
    #[test]
    fn a_stale_band_is_drained_in_one_pop() {
        let queue = HeatRequestQueue::new();
        for start in 0..64 {
            queue.push(req_at(start, Tier::Visible), Tier::Visible);
        }
        queue.new_window();
        queue.push(req_at(100, Tier::Visible), Tier::Visible);

        // The band is LIFO, so the live entry is at its head and is
        // served without the stale ones being looked at — which is why
        // they cost throughput rather than the user's place in the
        // queue.
        assert_eq!(queue.take_one().map(|(key, _)| key), Some(100));
        assert_eq!(queue.len(), 64, "the stale band is still standing");

        assert!(queue.try_next_task().is_none());
        assert_eq!(queue.len(), 0, "and one pop clears all of it");
    }

    /// G2, and the reason S1 is worth doing at all. A stale entry holds
    /// `band_occupancy`'s visible bit high, so until it is drained the
    /// machine reports itself busy on behalf of a row nobody is looking
    /// at — and outranks the read-ahead that could be using the pool.
    #[test]
    fn a_drained_stale_band_stops_counting_as_live() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(1, Tier::Visible), Tier::Visible);
        queue.new_window();
        assert_eq!(
            queue.activity(),
            Some(Tier::Visible),
            "a queued Visible entry counts as live however stale it is"
        );

        assert!(queue.try_next_task().is_none());
        assert_eq!(
            queue.activity(),
            None,
            "draining it must republish the band as empty"
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
        let (start, _) = queue.admit_one().unwrap();
        assert_eq!(
            queue.activity(),
            Some(Tier::User),
            "the pop itself registers the sweep (spec 0250 S5)"
        );

        queue.end_sweep(start);
        assert_eq!(queue.activity(), None);
    }

    /// Spec 0250 S5: two workers sweeping at once, and the first to
    /// finish must not report the machine idle while the second is
    /// still walking. That was the single `store`'s silent failure —
    /// nothing crashes, no result is wrong, the busy indicator simply
    /// lies.
    ///
    /// `Visible` rather than `User` for the first of the two: spec 0262
    /// S8 withholds every lower-tier task while a `User` sweep is in
    /// flight, so a `User` request here would be the one thing that
    /// stops the second sweep from starting at all.
    #[test]
    fn one_worker_finishing_does_not_clear_anothers_in_flight_tier() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(1, Tier::Visible), Tier::Visible);
        queue.push(req_at(2, Tier::Prefetch), Tier::Prefetch);
        let (first, _) = queue.admit_one().unwrap();
        let (second, _) = queue.admit_one().unwrap();
        assert_eq!(first, 1, "the Visible band drains first");
        assert_eq!(second, 2);
        assert_eq!(
            queue.in_flight_ranges(),
            vec![(1, Tier::Visible), (2, Tier::Prefetch)],
            "both sweeps are live"
        );

        queue.end_sweep(first);
        assert_eq!(
            queue.activity(),
            Some(Tier::Prefetch),
            "the surviving sweep must still be reported after the other ends"
        );

        queue.end_sweep(second);
        assert_eq!(queue.activity(), None, "now, and only now, idle");
    }

    /// Spec 0250 S4: a range some worker is already sweeping is
    /// dropped when popped rather than swept a second time. The two
    /// sweeps would produce identical results, so the duplicate is
    /// pure waste — and the unit of waste is a whole query.
    #[test]
    fn a_range_already_in_flight_is_not_popped_a_second_time() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(1, Tier::User), Tier::User);
        let (start, _) = queue.admit_one().unwrap();
        assert_eq!(start, 1);

        // Same range again, now that it is no longer *queued* and so
        // has nothing left to merge with. Pushed *last*, so that a
        // most-recently-touched-first pop reaches the duplicate first
        // and has to step over it in order to answer at all.
        queue.push(req_at(7, Tier::User), Tier::User);
        queue.push(req_at(1, Tier::User), Tier::User);
        assert_eq!(
            queue.admit_one().unwrap().0,
            7,
            "the duplicate must be skipped over, not served"
        );
        assert_eq!(queue.len(), 0, "and dropped rather than requeued");

        // Self-healing: once the sweep in flight is over, the same
        // range is servable again.
        queue.end_sweep(1);
        queue.push(req_at(1, Tier::User), Tier::User);
        assert_eq!(queue.admit_one().unwrap().0, 1);
    }

    /// Spec 0190 S4: the two sources are combined by priority, not by
    /// recency — a queued `User` outranks an in-flight `Prefetch` and
    /// vice versa.
    #[test]
    fn activity_takes_the_highest_tier_across_queued_and_in_flight() {
        let queue = HeatRequestQueue::new();
        queue.push(req_at(9, Tier::Prefetch), Tier::Prefetch);
        queue.admit_one();
        queue.push(req_at(1, Tier::User), Tier::User);
        assert_eq!(
            queue.activity(),
            Some(Tier::User),
            "a queued User outranks an in-flight Prefetch"
        );

        let (user, _) = queue.admit_one().unwrap();
        queue.end_sweep(user);
        assert_eq!(
            queue.activity(),
            Some(Tier::Prefetch),
            "the in-flight prefetch must remain visible"
        );

        queue.push(req_at(2, Tier::User), Tier::User);
        assert_eq!(
            queue.activity(),
            Some(Tier::User),
            "and an in-flight Prefetch does not mask a queued User"
        );
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
            queue.take_one().unwrap().0,
            1,
            "the User-tier request must still pop first"
        );
        assert_eq!(queue.take_one().unwrap().0, 2);
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
            queue.take_one().unwrap().0,
            2,
            "key 2 must still pop first — the Visible push must not reorder key 1"
        );
        let (key, merged) = queue.take_one().unwrap();
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

    /// `next_task` on a spawned thread against an empty queue
    /// blocks until `signal_stop()` (called from this test thread)
    /// wakes it, at which point it returns `None` and the thread joins
    /// promptly.
    #[test]
    fn next_task_returns_none_after_signal_stop() {
        let queue = Arc::new(HeatRequestQueue::new());
        let worker_queue = Arc::clone(&queue);
        let join = thread::spawn(move || worker_queue.admit_one());
        thread::sleep(Duration::from_millis(20)); // let the thread start blocking
        queue.signal_stop();
        let result = join.join().expect("worker thread must not panic");
        assert!(result.is_none());
    }

    // ── Spec 0262: a shared pool of parts ───────────────────────────

    /// S2/S4: a query is as many tasks as the partition has parts, and
    /// the last one back is handed every run to merge. Merging in a
    /// worker rather than in a collector is what keeps it off the
    /// critical path — serially it cost 2.5x the whole screenful.
    #[test]
    fn the_last_part_of_a_query_is_handed_every_run() {
        let queue = HeatRequestQueue::new();
        queue.activate(3, req_at(3, Tier::Visible), 2);

        let mut parts = Vec::new();
        while let Some(Task::Walk { start, part, .. }) = queue.try_next_task() {
            assert_eq!(start, 3);
            parts.push(part);
        }
        parts.sort_unstable();
        assert_eq!(parts, vec![0, 1], "a two-part partition is two tasks");

        assert!(
            queue
                .deposit_part(3, vec![("a.T".to_string(), 1)])
                .is_none(),
            "the first part back has nothing to merge yet"
        );
        let runs = queue
            .deposit_part(3, vec![("a.U".to_string(), 2)])
            .expect("the last part back is handed the whole query");
        assert_eq!(runs.len(), 2);
    }

    /// S8, the whole cycle: a `User` arrival takes the pool off
    /// speculation, keeps it while the user's own sweep walks, and
    /// hands it back at the end.
    #[test]
    fn a_user_arrival_takes_the_pool_off_speculation_and_gives_it_back() {
        let queue = HeatRequestQueue::new();
        queue.activate(5, req_at(5, Tier::Prefetch), 2);
        let Some(Task::Walk { part, epoch, .. }) = queue.try_next_task() else {
            panic!("an active query's parts are what is on offer");
        };
        assert!(!queue.stand_aside.load(Ordering::Relaxed));

        queue.push(req_at(9, Tier::User), Tier::User);
        assert!(
            queue.stand_aside.load(Ordering::Relaxed),
            "the walk in progress must be told to put its part down"
        );
        assert_ne!(
            queue.abort_epoch.load(Ordering::Relaxed),
            epoch,
            "and the epoch must move with it: a flag raised and lowered \
             again during the walk would otherwise let a truncated — and \
             so simply wrong — ranking be cached as the answer"
        );

        queue.abandon_part(5, part);
        let Some(Task::Admit { start, .. }) = queue.try_next_task() else {
            panic!("the user's request is the only thing servable now");
        };
        assert_eq!(start, 9);
        assert!(
            queue.stand_aside.load(Ordering::Relaxed),
            "dequeued is not done — the pool stays the user's while it walks"
        );
        assert!(
            queue.try_next_task().is_none(),
            "so the abandoned part must not be handed straight back, or \
             the worker that dropped it would pick it up and spin"
        );

        queue.end_sweep(9);
        assert!(!queue.stand_aside.load(Ordering::Relaxed));
        assert!(
            matches!(queue.try_next_task(), Some(Task::Walk { start: 5, .. })),
            "and the speculative query resumes from where it stood aside"
        );
    }

    /// S8 is confined to one tier boundary. A scroll is a stream of
    /// `Visible` requests, so aborting on those too would leave the
    /// pool throwing away every part it had started — read-ahead would
    /// make no progress at all for as long as the user kept moving.
    #[test]
    fn a_visible_arrival_does_not_abort_read_ahead() {
        let queue = HeatRequestQueue::new();
        queue.activate(5, req_at(5, Tier::Prefetch), 2);
        let epoch = queue.abort_epoch.load(Ordering::Relaxed);

        queue.push(req_at(9, Tier::Visible), Tier::Visible);
        assert!(!queue.stand_aside.load(Ordering::Relaxed));
        assert_eq!(queue.abort_epoch.load(Ordering::Relaxed), epoch);
        assert!(
            matches!(queue.try_next_task(), Some(Task::Admit { start: 9, .. })),
            "outranking the speculative parts for the next free worker is \
             all a Visible request is owed"
        );
    }

    /// The wakeup S8 rests on. A worker with nothing but speculation on
    /// offer blocks rather than spinning, so the end of the user's own
    /// sweep is the only event that can release it.
    #[test]
    fn ending_the_user_sweep_wakes_a_worker_holding_back_for_it() {
        let queue = Arc::new(HeatRequestQueue::new());
        queue.activate(5, req_at(5, Tier::Prefetch), 1);
        queue.push(req_at(7, Tier::User), Tier::User);
        let (start, _) = queue.admit_one().unwrap();
        assert_eq!(start, 7);

        let waiting = Arc::clone(&queue);
        let join = thread::spawn(move || waiting.next_task(None).is_some());
        thread::sleep(Duration::from_millis(20)); // let it reach the condvar
        assert!(!join.is_finished(), "it must not be handed the prefetch");

        queue.end_sweep(start);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !join.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            join.join().expect("the worker must not panic"),
            "end_sweep owes a notify_all: nothing else was going to wake it"
        );
    }

    // ── Seating the pool (spec 0270 test plan) ──────────────────────

    /// A seating the machine running these tests does not have. Every
    /// one of them fabricates its crew, because `affinity::seats()` is
    /// `None` here — which is also why every *other* test in this file
    /// still passes unchanged.
    static SEATS: [Seat; 2] = [
        Seat { cpu: 0, fast: true },
        Seat {
            cpu: 1,
            fast: false,
        },
    ];

    fn seated_crew(members: usize) -> crate::sweep::Crew {
        crate::sweep::Crew::with_seats(Some(&SEATS), members)
    }

    /// S3. A `User` part is worth more on a core of its own than on a
    /// third worker's share of a contended one, so the pool hands it
    /// only to the workers it has seats for.
    #[test]
    fn an_unseated_worker_does_not_take_a_user_part() {
        let queue = HeatRequestQueue::new();
        queue.activate(5, req_at(5, Tier::User), 2);
        let crew = seated_crew(3);

        let unseated = crew.member(2);
        assert!(
            queue.next_task_inner(Some(&unseated), false).is_none(),
            "a worker past the last seat is offered nothing, and parks"
        );
        assert!(
            matches!(
                queue.next_task_inner(Some(&crew.member(1)), false),
                Some(Task::Walk { start: 5, .. })
            ),
            "and the very same part is there for a seated one — the part \
             was withheld, not consumed"
        );
    }

    /// G2. The kernel says nothing about core speeds on a VM, in CI, or
    /// under a `taskset`, and then there are no seats to be off. Barring
    /// every worker of an unseated crew would strand the query forever.
    #[test]
    fn an_unseated_worker_takes_a_user_part_when_nobody_is_seated() {
        let queue = HeatRequestQueue::new();
        queue.activate(5, req_at(5, Tier::User), 2);
        let crew = crate::sweep::Crew::with_seats(None, 3);

        assert!(
            matches!(
                queue.next_task_inner(Some(&crew.member(2)), false),
                Some(Task::Walk { start: 5, .. })
            ),
            "the same index that is barred above is free here"
        );
    }

    /// S3 cannot strand a query, however few seats there are: one
    /// seated worker walks all of it, slowly, rather than the pool
    /// walking none of it.
    #[test]
    fn a_user_query_is_served_when_only_one_worker_is_seated() {
        static ONE: [Seat; 1] = [Seat { cpu: 0, fast: true }];
        let queue = HeatRequestQueue::new();
        queue.activate(5, req_at(5, Tier::User), crate::sweep::SWEEP_PARTS);
        let crew = crate::sweep::Crew::with_seats(Some(&ONE), 4);

        let me = crew.member(0);
        let mut parts = Vec::new();
        while let Some(Task::Walk { part, .. }) = queue.next_task_inner(Some(&me), false) {
            parts.push(part);
        }
        parts.sort_unstable();
        assert_eq!(
            parts,
            (0..crate::sweep::SWEEP_PARTS).collect::<Vec<_>>(),
            "every part of the query must reach the one worker allowed it"
        );
    }

    /// S6. Nothing resets the crew between queries, so the invariant
    /// has to be that draining one vacates every chair by itself —
    /// otherwise the next query inherits a rescue and re-seats onto
    /// seats that are still taken.
    #[test]
    fn a_drained_query_leaves_every_chair_vacant() {
        let queue = HeatRequestQueue::new();
        queue.activate(5, req_at(5, Tier::User), 2);
        let crew = seated_crew(2);
        // Stands in for the `sit()` the walk arm does. `lend` publishes
        // the chair without narrowing this thread, so the test states
        // the invariant without moving itself onto a CPU.
        for (i, seat) in SEATS.iter().enumerate() {
            crew.member(i).lend(*seat);
        }
        assert!(crew.occupied(0) && crew.occupied(1));

        for i in 0..2 {
            let me = crew.member(i);
            while queue.next_task_inner(Some(&me), false).is_some() {}
        }
        assert!(
            !crew.occupied(0) && !crew.occupied(1),
            "the last worker to run out donates to nobody and vacates all \
             the same"
        );
    }

    /// S4 and G5: the widen that follows the last `User` part is owed
    /// exactly once. The mask itself is not observable here — `widen()`
    /// is inert wherever `apply()` declined, which is everywhere this
    /// test runs — so what is asserted is the decision that drives it:
    /// `leave` reports a seat given back on the first call and not on
    /// the ones after, so a worker parking repeatedly does not re-widen.
    #[test]
    fn a_worker_widens_after_its_last_user_part() {
        let crew = seated_crew(2);
        let me = crew.member(0);
        me.lend(SEATS[0]);

        assert!(me.leave(), "the seat it held is the widen it owes");
        assert!(
            !me.leave() && !me.leave(),
            "and a worker that parks again owes nothing further"
        );
        assert!(
            !crew.member(1).leave(),
            "a worker that never sat never widens either"
        );
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
    /// produces. A second, cache-covered request for the same range is
    /// answered without a second `score_all` call (proven via the
    /// test-only call counter) — and so is a repeat of the unbounded
    /// request whose result `complete` records (item D1 of the quality
    /// audit, restated for spec 0250 S8's write rule).
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

        // Spec 0250 S8: the priming request above asked for a bounded
        // window, so nothing recorded a complete list — `by_range` is
        // the whole of what a screenful-sized ask leaves behind.
        assert_eq!(caches.lock().unwrap().complete.len(), 0);

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

        // D1: `upgrade_active_override_to_complete`'s own request — the
        // unbounded `[0, usize::MAX)` one. `top_n` cannot cover it by
        // construction and no complete list has been recorded, so this
        // one does sweep, and recording its result is what spec 0250 S8
        // reserves `complete` for.
        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: usize::MAX,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the unbounded request");
        assert_eq!(
            worker.score_all_calls(),
            calls_before + 1,
            "an unbounded request no cache covers must sweep exactly once"
        );
        assert_eq!(
            caches.lock().unwrap().complete.newest(),
            Some(&(0..2, expected_candidates.clone())),
            "the whole list the user asked for must be recorded"
        );

        // And the point of recording it: asking again is free. This is
        // the pre-flight check `heat_worker_loop` makes against the
        // reader's own predicate — testing only `by_range`'s `top_n`
        // would report "not covered" and pay for a second full sweep.
        worker.push(
            HeatRequest {
                range: 0..2,
                current_key: None,
                start: 0,
                end: usize::MAX,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire for the repeated request");
        assert_eq!(
            worker.score_all_calls(),
            calls_before + 1,
            "a request the recorded list already answers must not re-score"
        );

        worker.shutdown();
    }

    /// Spec 0250 S7: the cap evicts the least *recently used* entry,
    /// not the least recently inserted — a read has to promote, or a
    /// user alternating between two ranges loses one of them as soon as
    /// the cache fills with ranges nobody came back to.
    #[test]
    fn the_complete_cache_evicts_what_was_not_read() {
        let mut lists = CompleteLists::default();
        for i in 0..COMPLETE_LIST_ENTRIES {
            lists.insert(i..i + 1, vec![(format!("a.T{i}"), i as i64)]);
        }
        assert_eq!(lists.len(), COMPLETE_LIST_ENTRIES);

        // Read the oldest one back, which makes it the newest.
        assert!(lists.get(0).is_some());

        // One more insert therefore evicts entry 1, not entry 0.
        lists.insert(900..901, vec![("a.New".to_string(), 9)]);
        assert_eq!(lists.len(), COMPLETE_LIST_ENTRIES, "the cap must hold");
        assert!(lists.get(0).is_some(), "a read entry must survive");
        assert!(lists.get(1).is_none(), "the least recently used goes");

        // Re-inserting a range already held replaces it rather than
        // piling up a second entry for the same key.
        lists.insert(900..901, vec![("a.Again".to_string(), 1)]);
        assert_eq!(lists.len(), COMPLETE_LIST_ENTRIES);
        assert_eq!(
            lists.get(900).map(<[(String, i64)]>::len),
            Some(1),
            "the same range must be held once"
        );
    }

    /// Pushes one request and waits for the worker to finish it.
    fn serve(worker: &HeatWorkerHandle, rx: &mpsc::Receiver<AppEvent>, range: Range<usize>) {
        let end = usize::MAX;
        worker.push(
            HeatRequest {
                range,
                current_key: None,
                start: 0,
                end,
                tier: Tier::User,
            },
            Tier::User,
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("progress must fire");
    }

    /// Spec 0250 S7/G3: the override pane reopens on a range it has
    /// visited before without re-scoring it, even after the pane has
    /// been elsewhere in between.
    ///
    /// This is the whole point of replacing the single slot: the old
    /// one held the *most recent* full sweep, so visiting B and C
    /// between two visits to A guaranteed A had been overwritten and
    /// the second visit paid for a second walk.
    #[test]
    fn the_pane_reopens_from_the_cache() {
        let graph = Arc::new(test_scoring_graph());
        // Three distinct, individually valid ranges: field 1 = 5, thrice.
        let blob = Arc::new(Blob::unwrapped(vec![0x08, 0x05, 0x08, 0x05, 0x08, 0x05]));
        let caches = Arc::new(Mutex::new(HeatCaches::new(8)));
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let worker = HeatWorkerHandle::spawn(
            Arc::clone(&caches),
            Arc::clone(&graph),
            Arc::clone(&blob),
            tx,
            1,
        );

        serve(&worker, &rx, 0..2);
        serve(&worker, &rx, 2..4);
        serve(&worker, &rx, 4..6);
        let calls = worker.score_all_calls();
        assert_eq!(calls, 3, "three distinct ranges must cost three sweeps");
        assert_eq!(
            caches.lock().unwrap().complete.len(),
            3,
            "each of the three must have recorded its list"
        );

        serve(&worker, &rx, 0..2);
        assert_eq!(
            worker.score_all_calls(),
            calls,
            "returning to the first range must not re-score it"
        );

        worker.shutdown();
    }

    /// Spec 0250 S8: a prefetch completing between two visits leaves
    /// the recorded list intact. A prefetch is a whole sweep, and under
    /// the old single slot its completion was what evicted the answer
    /// the user was about to come back to.
    #[test]
    fn a_prefetch_does_not_evict_a_user_list() {
        let graph = Arc::new(test_scoring_graph());
        let blob = Arc::new(Blob::unwrapped(vec![0x08, 0x05, 0x08, 0x05]));
        let caches = Arc::new(Mutex::new(HeatCaches::new(8)));
        let (tx, rx) = mpsc::channel::<AppEvent>();
        let worker = HeatWorkerHandle::spawn(
            Arc::clone(&caches),
            Arc::clone(&graph),
            Arc::clone(&blob),
            tx,
            1,
        );

        serve(&worker, &rx, 0..2);
        let recorded = caches
            .lock()
            .unwrap()
            .complete
            .newest()
            .cloned()
            .expect("the user's whole-list request must record one");
        // A speculative sweep of somewhere else. `Tier::Prefetch` sends
        // no progress event (spec 0164 G10), so poll `by_range` for its
        // arrival instead of waiting on the channel.
        worker.push(
            HeatRequest {
                range: 2..4,
                current_key: None,
                start: 0,
                end: 1,
                tier: Tier::Prefetch,
            },
            Tier::Prefetch,
        );
        let mut landed = false;
        for _ in 0..200 {
            if caches
                .lock()
                .unwrap()
                .by_range
                .peek(&2, Tier::Prefetch)
                .is_some()
            {
                landed = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            landed,
            "the prefetch must have swept, or this proves nothing"
        );
        // Spec 0250 S6: named by range rather than as a before/after
        // delta on one counter — with more than one worker a delta no
        // longer says *which* sweep ran, and this assertion means the
        // prefetch's.
        assert_eq!(
            worker.sweeps_of(2),
            1,
            "the prefetch must really have been a full sweep"
        );

        let c = caches.lock().unwrap();
        assert_eq!(c.complete.len(), 1, "the prefetch must record no list");
        assert_eq!(
            c.complete.newest(),
            Some(&recorded),
            "the user's list must survive a prefetch completing behind it"
        );
        drop(c);

        worker.shutdown();
    }

    /// Spec 0262 S2: a sweep's parts are walked by whichever workers
    /// happen to be free, in whatever order the pool hands them out —
    /// and that is a scheduling change only. What it writes into the
    /// cache is what a single whole-query call would have written.
    ///
    /// The part-by-part ranking is pinned against the whole-query one in
    /// `sweep::tests::a_query_walked_part_by_part_produces_the_whole_sweeps_ranking`;
    /// this is the same claim made about the worker that uses it.
    #[test]
    fn a_speculative_sweep_records_what_a_whole_query_would_have() {
        let graph = Arc::new(test_scoring_graph());
        let range_bytes = vec![0x08, 0x05];
        let blob = Arc::new(Blob::unwrapped(range_bytes.clone()));
        let caches = Arc::new(Mutex::new(HeatCaches::new(8)));
        let (tx, _rx) = mpsc::channel::<AppEvent>();
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
        // No progress event at this tier (spec 0164 G10), so poll.
        let mut entry = None;
        for _ in 0..200 {
            if let Some(e) = caches.lock().unwrap().by_range.peek(&0, Tier::Prefetch) {
                entry = Some(e);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let entry = entry.expect("the speculative sweep must populate by_range");

        let expected = override_pane::inferred_candidates(&range_bytes, graph.graph(), 1, None);
        let expected_stats = heat_cue::derive_stats(&expected);
        assert_eq!(entry.best_score, expected_stats.best_score);
        assert_eq!(entry.best_count, expected_stats.best_count);
        assert_eq!(
            entry.top_n,
            expected.iter().take(1).cloned().collect::<Vec<_>>()
        );

        worker.shutdown();
    }

    /// Spec 0250 S1: several workers behind one queue serve every range
    /// asked for, and no range is swept twice — eight workers racing for
    /// sixteen ranges, each pushed twice.
    ///
    /// This pins the *outcome*, not the mechanism, and the distinction
    /// was checked rather than assumed: disabling S4's in-flight rule
    /// leaves the test green, because at this scale a sweep takes
    /// microseconds and the worker's own `covers_window` re-check
    /// catches the duplicate first. S4 is what covers the case the
    /// re-check cannot — a duplicate arriving during a sweep that has
    /// not written its answer yet — and that window is only wide enough
    /// to hit deterministically at the queue level, which is where
    /// `a_range_already_in_flight_is_not_popped_a_second_time` tests it.
    #[test]
    fn several_workers_behind_one_queue_serve_every_range_once() {
        const RANGES: usize = 16;
        let graph = Arc::new(test_scoring_graph());
        let blob = Arc::new(Blob::unwrapped([0x08u8, 0x05].repeat(RANGES)));
        let caches = Arc::new(Mutex::new(HeatCaches::new(RANGES * 2)));
        let (tx, _rx) = mpsc::channel::<AppEvent>();
        let worker = HeatWorkerHandle::spawn(
            Arc::clone(&caches),
            Arc::clone(&graph),
            Arc::clone(&blob),
            tx,
            8,
        );

        // Each range twice, so a pop that ignored the in-flight registry
        // would have a second copy waiting for it.
        for _ in 0..2 {
            for i in 0..RANGES {
                worker.push(
                    HeatRequest {
                        range: i * 2..i * 2 + 2,
                        current_key: None,
                        start: 0,
                        end: 1,
                        tier: Tier::Prefetch,
                    },
                    Tier::Prefetch,
                );
            }
        }

        let mut landed = 0;
        for _ in 0..400 {
            let mut c = caches.lock().unwrap();
            landed = (0..RANGES)
                .filter(|i| c.by_range.peek(&(i * 2), Tier::Prefetch).is_some())
                .count();
            drop(c);
            if landed == RANGES {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(landed, RANGES, "every range asked for must be swept");

        // Shut down first: a range still in flight could otherwise be
        // counted before its duplicate had a chance to be dropped, which
        // would make this pass for the wrong reason.
        let mut worker = worker;
        worker.stop_and_join();
        for i in 0..RANGES {
            assert_eq!(
                worker.sweeps_of(i * 2),
                1,
                "range {} was swept more than once",
                i * 2
            );
        }
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
                c.complete.len(),
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
            c.complete.len(),
            complete_before,
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
