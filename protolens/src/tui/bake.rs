// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0255: finishing a bounded confirm's document in the background.
//!
//! A confirm renders a screenful and folds everything it did not reach
//! (spec 0249 S1/S3). Those folds are a debt: until they are paid the
//! line-count footer, the scrollbar, `G` and every search describe a
//! document that is shorter than the one the user asked for. The bake
//! pays it, one subtree per idle iteration, drawing nothing.

use super::App;

/// What one call to [`App::bake_step`] did — the same two-valued answer
/// `prefetch_step` and `search_sweep_step` give, so `run_loop`'s idle
/// chain treats all three alike.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BakeStep {
    Progressed,
    Idle,
}

impl App {
    /// Spec 0255 S1: how many rows one bake step renders.
    ///
    /// Larger than a confirm's screenful, deliberately. A bounded render
    /// re-emits its whole right frontier as folded rows (spec 0249 S1:
    /// output size = budget + breadth) and the step that expands one of
    /// them renders those rows again, so a small budget pays for the
    /// frontier over and over. Measured over a full drain of
    /// googleapis.desc, the bake's own render is 3.3x the unbounded
    /// render at a budget of 50 and 1.6x at 5000, over 419 723 steps
    /// against 70 797.
    ///
    /// The ceiling is the event loop: at 5000 the worst single step is
    /// ~22 ms, with two steps over 8 ms out of 70 797 and none over 50.
    pub(super) const BAKE_ROW_BUDGET: usize = 5000;

    /// Render at most one subtree a bounded confirm stopped at.
    ///
    /// Spec 0255 S3: `auto_folded` is the queue and `bake_queue` is only
    /// the order to walk it in, so an entry that has left the set — a
    /// duplicate, a node the user opened by hand (spec 0249 S8), a node
    /// whose ancestor was re-overridden underneath it — is discarded
    /// here rather than guarded against everywhere else.
    ///
    /// One `expand_auto_fold` per call, never a loop: the caller
    /// re-checks its channel between steps, and that is what bounds a
    /// keystroke's wait to a single step.
    pub(super) fn bake_step(&mut self) -> BakeStep {
        while let Some(idx) = self.bake_queue.pop_front() {
            if !self.auto_folded.contains(&idx) {
                continue;
            }
            self.expand_auto_fold(idx, Self::BAKE_ROW_BUDGET);
            return BakeStep::Progressed;
        }
        BakeStep::Idle
    }
}
