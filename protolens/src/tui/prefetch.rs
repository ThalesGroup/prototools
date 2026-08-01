// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Read-ahead: the zigzag walk outward from the cursor that keeps the
//! heat worker fed while the user is not moving (spec 0164 G7, spec
//! 0191).
//!
//! Its own file because it is a closed subsystem — one walk, one budget,
//! one trace, and the two `App` methods that drive them — behind a single
//! entry point (`prefetch_step`) that only `run_loop` calls.
//!
//! `PrefetchWalk` and its fields are `pub(super)` only because they were
//! private to `tui` before the move and the tests reach for them; nothing
//! outside `tui` can see them either way.

use super::*;

/// Spec 0191 G1: how many rows one read-ahead wave may visit before it
/// reports `Idle` and lets both threads park. Without it the walk's only
/// stopping condition is running off *both* ends of the document, so a
/// single cursor move sweeps every expanded row there is — keeping the
/// main thread in `prefetch_step`'s `Progressed` branch (never reaching
/// `recv_timeout`) and the worker in back-to-back `score_all` calls, on
/// a large `FileDescriptorSet` for tens of thousands of rows.
///
/// A budget across both ends, not a reach limit per side, so a cursor
/// near the top of the document still gets its full allowance downward.
///
/// Deliberately *not* derived from `HEAT_REQUEST_QUEUE_MAX_ENTRIES`,
/// which happens to hold the same number (spec 0191 N1): that one bounds
/// requests outstanding against the worker, this one bounds rows visited
/// per wave. Raising one to smooth out stalls must not silently double
/// the other's reach.
pub(super) const PREFETCH_WALK_MAX_ROWS: usize = 2048;

/// Spec 0191 G2. Past the result cache's capacity a prefetch result
/// evicts *itself* on insert — `evict_one` takes `prefetch_current`'s
/// tail, the same end `upsert` inserts at — so the worker would pay a
/// whole `score_all` for an answer nobody can ever read. This is the
/// one relation the walk budget genuinely has to another constant.
const _: () = assert!(PREFETCH_WALK_MAX_ROWS <= heat_cue::HEAT_CACHE_MAX_ENTRIES);

/// Spec 0164 G7: per-`App` zigzag-walk state, persisted across
/// `run_loop` iterations (not rebuilt on every call) — reset only when
/// the cursor's display row or the document's structural/reflow state
/// (`App::structural_version`) has changed since the walk began.
/// `origin_line`/`above`/`below` are visible-row numbers (rendered-line
/// space), not raw `App::lines` indices — folded/hidden content has no
/// row, so the walk naturally skips it.
pub(super) struct PrefetchWalk {
    pub(super) origin_line: usize,
    pub(super) above: usize,
    pub(super) below: usize,
    pub(super) above_done: bool,
    pub(super) below_done: bool,
    /// Spec 0210 S3: the two ends as *positions* rather than as row
    /// numbers, stepped one visible line at a time by `prefetch_step`.
    ///
    /// The row numbers `next_row` produces would each have to be turned
    /// back into a node by a `visible_row_pos` descent, and a wave
    /// visits up to `PREFETCH_WALK_MAX_ROWS` of them — on the reference
    /// corpus that is 2 048 crossings of the root's 7 771 children, on
    /// the UI thread. Carrying the positions makes the whole wave one
    /// descent (when it is seeded) plus O(1) per row.
    ///
    /// `None` before the first seeding, and on a document with no rows
    /// at the origin at all.
    pub(super) above_pos: Option<LinePos>,
    pub(super) below_pos: Option<LinePos>,
    /// `App::structural_version` as of this walk's start — part of
    /// the staleness signal `prefetch_step` checks on entry (the
    /// exact mechanism the spec left TBD during implementation).
    pub(super) structural_version: u64,
}

impl PrefetchWalk {
    /// Both ends already exhausted, and an `origin_line`/
    /// `structural_version` that can never coincide with a real walk's
    /// — guarantees the very first `prefetch_step` call starts a fresh
    /// walk.
    pub(super) fn exhausted() -> Self {
        PrefetchWalk {
            origin_line: usize::MAX,
            above: 0,
            below: 0,
            above_done: true,
            below_done: true,
            above_pos: None,
            below_pos: None,
            structural_version: u64::MAX,
        }
    }

    /// Advances the walk to the next unexplored row, alternating above/
    /// below (always the nearer of the two unexplored ends), and
    /// returns its visible-row number. `None` once both ends
    /// are exhausted, or once the wave has spent its
    /// `PREFETCH_WALK_MAX_ROWS` budget (spec 0191 S1).
    ///
    /// The budget bounds rows *visited*, not requests *pushed*:
    /// `prefetch_step` skips non-header, non-overridable and
    /// already-settled rows without pushing, and it is the visiting
    /// loop — not the pushing — that holds the main thread.
    pub(super) fn next_row(&mut self, visible_len: usize) -> Option<usize> {
        loop {
            if self.above_done && self.below_done {
                return None;
            }
            // `above`/`below` count steps taken on each end, so their
            // sum is exactly the number of rows returned so far. Both
            // `_done` flags are set rather than returning `None`
            // directly, so the walk has a single "exhausted" state and
            // the next call takes the early-out above without
            // re-deriving the budget.
            if self.above + self.below >= PREFETCH_WALK_MAX_ROWS {
                self.above_done = true;
                self.below_done = true;
                return None;
            }
            let try_above = if self.above_done {
                false
            } else if self.below_done {
                true
            } else {
                self.above <= self.below
            };
            if try_above {
                let next = self.above + 1;
                if next > self.origin_line {
                    self.above_done = true;
                    continue;
                }
                self.above = next;
                return Some(self.origin_line - next);
            } else {
                let next = self.below + 1;
                let row = self.origin_line + next;
                if row >= visible_len {
                    self.below_done = true;
                    continue;
                }
                self.below = next;
                return Some(row);
            }
        }
    }
}

pub(super) enum PrefetchStep {
    Progressed,
    Idle,
}

/// What one read-ahead wave did, for `PROTOLENS_TRACE`. A wave is the
/// span between two `prefetch_walk` resets, which is exactly the span
/// between two cursor moves — so one line per wave answers "how much did
/// that keystroke actually cost the read-ahead".
///
/// `rows` counts candidates the zigzag visited; `skipped` those it
/// dismissed without a lookup (no node on the line, not overridable, or
/// already settled — the last of which is the "already known" case);
/// `hits` those already in the result cache; `pushes` those it had to
/// queue for the worker. `hits` + `skipped` versus `pushes` is the
/// number the "most of it should already be cached" intuition is about.
///
/// `busy` is the sum of the time actually spent inside `prefetch_step`,
/// not the wall time the wave spanned: the walk is interleaved with
/// drawing and event handling, so wall time mostly measures the rest of
/// the loop and says nothing about what read-ahead costs.
#[derive(Default)]
pub(super) struct PrefetchTrace {
    busy: Duration,
    rows: u32,
    skipped: u32,
    hits: u32,
    pushes: u32,
    live: bool,
    reported: bool,
}

impl PrefetchTrace {
    fn restart(&mut self) {
        *self = Self {
            live: true,
            ..Self::default()
        };
    }

    fn report(&mut self, why: &str) {
        if self.reported || !self.live {
            return;
        }
        self.reported = true;
        trace::trace!(
            "wave {why} rows={} skipped={} hits={} pushes={} busy_ms={:.1}",
            self.rows,
            self.skipped,
            self.hits,
            self.pushes,
            self.busy.as_secs_f64() * 1000.0,
        );
    }
}

impl App {
    /// Advances the zigzag prefetch walk by one candidate, pushing it
    /// at `Tier::Prefetch` if it isn't already settled/cached (spec
    /// 0164 G7). First resets `self.prefetch_walk` to a fresh walk
    /// from the cursor's current display row if either the cursor's
    /// row or `self.structural_version` has changed since the walk
    /// began — superseding the in-progress wave in the request queue
    /// and both `HeatCaches` maps before the reset; otherwise resumes
    /// exactly where the previous call left off. Returns `Idle` once
    /// the document is fully walked, the last push returned
    /// `UpsertOutcome::Rejected` (G6), or no worker is running at all
    /// (nothing to prefetch into).
    /// Spec 0190 S4: what the activity dot reports — the highest-
    /// priority tier the heat-cue subsystem is working for, or `None`
    /// when it is idle or no worker is running at all. Lock-free: two
    /// relaxed atomic loads.
    pub(in crate::tui) fn heat_activity(&self) -> Option<tiered::Tier> {
        self.heat_worker.as_ref().and_then(|w| w.activity())
    }

    pub(super) fn prefetch_step(&mut self) -> PrefetchStep {
        let entered_at = Instant::now();
        let step = self.prefetch_step_inner();
        self.prefetch_trace.busy += entered_at.elapsed();
        step
    }

    fn prefetch_step_inner(&mut self) -> PrefetchStep {
        if self.heat_worker.is_none() {
            return PrefetchStep::Idle;
        }
        let origin_row = self.cursor_display_row();
        if self.prefetch_walk.origin_line != origin_row
            || self.prefetch_walk.structural_version != self.structural_version
        {
            // All three supersede with the same O(1) splice, and spec
            // 0189 keeps it that way deliberately: this runs on the UI
            // thread, so the restart path must not walk a wave. What
            // differs is the fate of the demoted entries. A cache entry
            // is a *result* — a later hit on it saves a whole sweep, so
            // it stays servable. A queue entry is *pending work* — an
            // unpaid sweep on a range ranked from an origin the cursor
            // has left — so the worker discards it rather than scoring
            // it (`pop_highest` never reaches `prefetch_previous`).
            self.prefetch_trace.report("superseded");
            self.prefetch_trace.restart();
            if let Some(worker) = &self.heat_worker {
                worker.start_new_wave();
            }
            {
                let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
                caches.by_range.start_new_wave();
                caches.current_score.start_new_wave();
            }
            // Spec 0210 S3: the one descent this wave pays. Both ends
            // start on the origin's own row and step outward from it,
            // so nothing below has to resolve a row number again.
            let origin_pos = self.visible_row_pos(origin_row).map(|(pos, _)| pos);
            self.prefetch_walk = PrefetchWalk {
                origin_line: origin_row,
                above: 0,
                below: 0,
                above_done: false,
                below_done: false,
                above_pos: origin_pos,
                below_pos: origin_pos,
                structural_version: self.structural_version,
            };
        }

        loop {
            let Some(row) = self.prefetch_walk.next_row(self.visible_row_count()) else {
                self.prefetch_trace.report("exhausted");
                return PrefetchStep::Idle;
            };
            self.prefetch_trace.rows += 1;
            // `next_row` advances exactly one end per call, and always
            // by one row, so which end it just moved is readable from
            // the row alone — and stepping that end's position by one
            // visible line reproduces the row it named.
            let going_up = row < self.prefetch_walk.origin_line;
            let from = if going_up {
                self.prefetch_walk.above_pos
            } else {
                self.prefetch_walk.below_pos
            };
            let stepped = from.and_then(|pos| {
                if going_up {
                    self.prev_visible(pos)
                } else {
                    self.next_visible(pos)
                }
            });
            let Some((pos, _)) = stepped else {
                // The walk's own bounds should have stopped it first;
                // if they somehow did not, close that end rather than
                // spinning on it.
                if going_up {
                    self.prefetch_walk.above_done = true;
                } else {
                    self.prefetch_walk.below_done = true;
                }
                self.prefetch_trace.skipped += 1;
                continue;
            };
            if going_up {
                self.prefetch_walk.above_pos = Some(pos);
            } else {
                self.prefetch_walk.below_pos = Some(pos);
            }
            // Heat is a property of the node, so only its header row
            // asks for it: a closing brace has none of its own, and the
            // later rows of a packed run would each re-ask the same
            // question (spec 0216 S7).
            if pos.line_in_node != 0 {
                self.prefetch_trace.skipped += 1;
                continue;
            }
            let idx = pos.node;
            if !self.can_override(idx) || self.heat_states[idx].settled() {
                self.prefetch_trace.skipped += 1;
                continue;
            }
            let range = self.heat_scored_range(idx);
            let current_key = self.current_type_key(idx);
            let (_, outcome) = self.heat_lookup_ex(
                &range,
                current_key.as_deref(),
                0,
                heat_cue::HEAT_CUE_PREVIEW,
                tiered::Tier::Prefetch,
            );
            match outcome {
                None => {
                    // Spec 0224 S3: a hit means the window *and* the
                    // current type's score are both cached (with
                    // `current_key: None`, the window alone is the whole
                    // question) — which is exactly what `settled()`
                    // asks. Record it, or the skip above never fires for
                    // this node and every later wave spends one of its
                    // guaranteed steps re-proving the same hit instead
                    // of stepping past it: the walk would slow down the
                    // more the worker has answered.
                    let state = self.read_heat_state(
                        range.start,
                        current_key.as_deref(),
                        tiered::Tier::Prefetch,
                    );
                    self.heat_states[idx] = state;
                    self.prefetch_trace.hits += 1;
                }
                Some(_) => self.prefetch_trace.pushes += 1,
            }
            return match outcome {
                Some(tiered::UpsertOutcome::Rejected) => {
                    self.prefetch_trace.report("rejected");
                    PrefetchStep::Idle
                }
                _ => PrefetchStep::Progressed,
            };
        }
    }
}
