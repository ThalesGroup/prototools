// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0247: every node's status, rolled up from its subtree.
//!
//! `rolled(n) = max(own(n), max over children of rolled(c))`, where
//! `own` is the worst thing the node's *own* rows say (`node_status::
//! row_status`). Not leaves-only: a message header carries annotations
//! of its own (`len_ohb`, `tag_ohb`), so a node contributes as well as
//! aggregates.
//!
//! `own` is well defined because every rendered line belongs to exactly
//! one node — the partition `decode::overlay_spans` establishes and
//! relies on. A bracketed node's own rows are its header line alone
//! (the footer is derived, spec 0222); a flat or packed one's are all
//! of them.
//!
//! Nothing here reads the override table (S9). An override rewrites the
//! document — supplying `--field-name` makes `splice_override` render a
//! symbolic key, which is what clears `Unknown` — and the status simply
//! reads what the document now says.

use super::*;
use crate::node_status::row_status;

impl App {
    /// `idx`'s status including everything below it — what the fold
    /// toggle draws (S11).
    pub(super) fn status_of(&self, idx: usize) -> Status {
        self.status_rolled[idx]
    }

    /// The whole document's status, in one reverse linear pass — O(n)
    /// (S7).
    ///
    /// Level order is what makes this possible: a node's children are
    /// one contiguous block and a parent's index is always below any
    /// child's, so walking the arena backwards visits every child
    /// before its parent. No recursion, no stack, no queue, and each
    /// slot is touched once as a parent and once as a child.
    ///
    /// `auto_folded` is flattened to one bit per slot first. Asking the
    /// set directly is a random probe into a table of tens of thousands
    /// of entries, once per node — 4.74 M of them on googleapis, which
    /// measured **5.0% of a startup**, almost all of it hashing and
    /// missing. The bitset is filled in one pass over the set (which is
    /// three orders of magnitude smaller than the arena) and then read
    /// *in the order the loop already walks*, so it costs one cache line
    /// per 512 nodes and prefetches perfectly.
    ///
    /// A cheaper-looking fix — a faster hasher — was tried and measured
    /// **3.3% slower** on 3.7% fewer instructions: SipHash's arithmetic
    /// was overlapping the probe's cache miss, so removing it exposed the
    /// latency it had been hiding. The probe is the cost, not the hash.
    pub(super) fn rebuild_status(&mut self) {
        let mut stops = vec![0u64; self.tree.len().div_ceil(64)];
        for &idx in &self.auto_folded {
            stops[idx / 64] |= 1 << (idx % 64);
        }
        for idx in (0..self.tree.len()).rev() {
            let own = self.own_status(idx, stops[idx / 64] & (1 << (idx % 64)) != 0);
            self.status_own[idx] = own;
            self.status_rolled[idx] = own.max(self.children_status(idx));
        }
    }

    /// Recompute `idx`'s subtree after a splice — O(k) in the slots it
    /// renders (S8 step 1).
    ///
    /// Recursive rather than a slice of the arena: level order groups
    /// slots by *depth*, so a subtree is a union of one range per level
    /// rather than a single range, and the ranges would sweep in
    /// unrelated siblings. The recursion is bounded by the arena's own
    /// depth cap, and the deepest real document measured is 13.
    ///
    /// Returns `idx`'s rolled status, so a parent's `max` needs no
    /// second read.
    pub(super) fn refresh_status_subtree(&mut self, idx: usize) -> Status {
        let own = self.own_status(idx, self.auto_folded.contains(&idx));
        self.status_own[idx] = own;
        let mut rolled = own;
        for child in self.child_slots(idx) {
            rolled = rolled.max(self.refresh_status_subtree(child));
        }
        self.status_rolled[idx] = rolled;
        rolled
    }

    /// Carry `idx`'s new status up to the root — O(depth · width), with
    /// an early stop (S8 step 2).
    ///
    /// Not O(depth): `max` is not invertible. An *increase* would need
    /// one comparison per level, but a *decrease* — which is what an
    /// override that resolves a type causes — leaves no choice but to
    /// re-`max` the siblings. Both scans are over a contiguous byte
    /// slice, and this runs once per splice, never per frame.
    pub(super) fn refresh_status_ancestors(&mut self, idx: usize) {
        let mut cur = idx;
        while let Some(parent) = self.parent(cur) {
            let rolled = self.status_own[parent].max(self.children_status(parent));
            if self.status_rolled[parent] == rolled {
                // Nothing above can have changed either.
                return;
            }
            self.status_rolled[parent] = rolled;
            cur = parent;
        }
    }

    /// Forget what a vacated slot used to say. Paired with the
    /// `heat_states` reset on the same path: a slot this rendering does
    /// not show must not contribute to anything, and must not be
    /// believed if a later override brings it back.
    pub(super) fn clear_status(&mut self, idx: usize) {
        self.status_own[idx] = Status::Ok;
        self.status_rolled[idx] = Status::Ok;
    }

    /// The worst thing `idx`'s own rows say (S2, S3), plus whether it
    /// has been looked at at all (spec 0249 S12).
    ///
    /// One exception, at the root. `row_status` reads `Unknown` off a
    /// numeric key, and a root's key is always numeric — the wrapper is
    /// field 1 of a *virtual* encompassing message (spec 0216 S1), so
    /// it renders `1 {` no matter what the blob is. Left alone that
    /// would tint the topmost fold toggle of every document ever
    /// opened, which is the one place the signal has to mean something.
    /// A root has no enclosing message, so there is no schema that
    /// could have declared it and nothing for the rung to say.
    ///
    /// The `Unbaked` rung is the node's *own*, not a roll-up: a stop's
    /// descendants are vacant slots that say `Ok`, so reading the fold
    /// off the children would find nothing. Rolling it up then happens
    /// by the ordinary `max`, which is the point — every ancestor of an
    /// unbaked region reads provisional until the bake reaches it.
    ///
    /// `is_stop` is `auto_folded.contains(&idx)`, taken as a parameter
    /// rather than asked here so that `rebuild_status` can answer it
    /// from a bitset. There is still one statement of the status rule.
    fn own_status(&self, idx: usize, is_stop: bool) -> Status {
        let mut worst = match self.node_text[idx].as_deref() {
            Some(text) => {
                let text_says = text.split('\n').map(row_status).max().unwrap_or_default();
                if text_says == Status::Unknown && self.parent(idx).is_none() {
                    Status::Ok
                } else {
                    text_says
                }
            }
            None => Status::Ok,
        };
        // A stop always has text — its header and its footer — so this
        // is never the reason a vacant slot gets a status.
        if is_stop {
            worst = worst.max(Status::Unbaked);
        }
        worst
    }

    /// The worst of `idx`'s children, `Ok` when it shows none.
    ///
    /// A contiguous `&[Status]` — one byte per child, read forwards —
    /// which is why `status_own` and `status_rolled` are two arrays
    /// rather than one array of pairs.
    fn children_status(&self, idx: usize) -> Status {
        self.status_rolled[self.child_slots(idx)]
            .iter()
            .copied()
            .max()
            .unwrap_or(Status::Ok)
    }

    /// Spec 0247's invariant, checked over the whole document: the
    /// incrementally maintained arrays are what a full rebuild would
    /// produce.
    ///
    /// Hung off `finalize_override_batch` rather than written as one
    /// dedicated test, so that *every* splice in the suite is a case —
    /// the same arrangement, and for the same reason, as spec 0186 G3's
    /// line-count check next to it. The case that most needs it is a
    /// status going *down*, where the ancestor walk's early stop is the
    /// thing most likely to be wrong.
    #[cfg(test)]
    pub(super) fn assert_status_is_exact(&mut self) {
        let (own, rolled) = (self.status_own.clone(), self.status_rolled.clone());
        self.rebuild_status();
        for idx in 0..self.tree.len() {
            assert_eq!(
                own[idx], self.status_own[idx],
                "node {idx}: own status drifted from a full rebuild"
            );
            assert_eq!(
                rolled[idx], self.status_rolled[idx],
                "node {idx}: rolled-up status drifted from a full rebuild"
            );
        }
    }
}
