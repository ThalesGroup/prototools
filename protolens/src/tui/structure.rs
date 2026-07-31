// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0216: the tree's shape, read off the arena rather than stored.
//!
//! Every link a node used to carry — parent, first and last child, both
//! siblings, both document-order neighbors, and the sibling ordinal — is
//! arithmetic on the arena's `first_child` and `parent` arrays. That is
//! what level order buys: children occupy one contiguous block, blocks
//! run in parent order, so a sibling step is `+1` and a child step is an
//! add (S16, S17).
//!
//! The arena describes *all* the structure the bytes admit, and an
//! interpretation shows only part of it — a payload the greedy walk
//! descended into may well be printed as a string. So every accessor
//! here answers about the **rendered** tree, and the overlay's
//! `is_rendered` is what tells the two apart.
//!
//! The one property that makes this sound is checked against a real
//! corpus by `decode::the_arena_covers_a_real_corpus`: a rendered node
//! consumes either the whole of its slot's child block or none of it,
//! never a subset. Were that false, a step in a positional path would
//! count rendered children while `first_child[i] + step` counted arena
//! children, and the two would silently drift apart.

use super::*;

impl App {
    /// `idx`'s enclosing node, or `None` for a root.
    ///
    /// A root is its own arena parent, which is what terminates the climb
    /// without a sentinel (spec 0216 S8). A loaded document has exactly
    /// one — slot 0, the wrapper, being the whole blob seen as field 1 of
    /// a virtual encompassing message (S1) — but the arena is defined for
    /// any byte string, and a fixture that hands it an unwrapped blob of
    /// several top-level records gets one root per record.
    #[inline]
    pub(super) fn parent(&self, idx: usize) -> Option<usize> {
        let parent = self.arena.parent()[idx] as usize;
        (parent != idx).then_some(parent)
    }

    /// The block of slots `idx` shares with its siblings.
    ///
    /// Level order puts the roots first, so a root's block is the whole
    /// of level 0 — `0..` the first slot that has a parent.
    #[inline]
    fn sibling_block(&self, idx: usize) -> Range<usize> {
        match self.parent(idx) {
            Some(parent) => {
                let first_child = self.arena.first_child();
                first_child[parent] as usize..first_child[parent + 1] as usize
            }
            None => {
                let parent = self.arena.parent();
                let roots = (0..parent.len())
                    .position(|i| parent[i] != i as u32)
                    .unwrap_or(parent.len());
                0..roots
            }
        }
    }

    /// The half-open range of slots holding `idx`'s children, empty if
    /// this interpretation shows none.
    ///
    /// Empty covers two different situations that need no distinction
    /// here: a node with no children in the bytes at all, and one whose
    /// payload the walk decomposed but this rendering prints as a
    /// scalar. Either way there is nothing below it to navigate to.
    #[inline]
    fn child_slots(&self, idx: usize) -> Range<usize> {
        let first_child = self.arena.first_child();
        let block = first_child[idx] as usize..first_child[idx + 1] as usize;
        if block.is_empty() || !self.tree[block.start].is_rendered() {
            return 0..0;
        }
        block
    }

    /// How many children `idx` shows.
    #[inline]
    pub(super) fn child_count(&self, idx: usize) -> usize {
        self.child_slots(idx).len()
    }

    /// `idx`'s `k`-th child, counting from 0.
    ///
    /// The whole of spec 0216 S17's path descent: one add and a bounds
    /// check per level, no hash and no allocation.
    #[inline]
    pub(super) fn nth_child(&self, idx: usize, k: usize) -> Option<usize> {
        let block = self.child_slots(idx);
        (k < block.len()).then_some(block.start + k)
    }

    #[inline]
    pub(super) fn first_child(&self, idx: usize) -> Option<usize> {
        let block = self.child_slots(idx);
        (!block.is_empty()).then_some(block.start)
    }

    #[inline]
    pub(super) fn last_child(&self, idx: usize) -> Option<usize> {
        let block = self.child_slots(idx);
        (!block.is_empty()).then_some(block.end - 1)
    }

    /// The sibling after `idx`, which — level order being what it is —
    /// is the very next slot, so long as it is still inside the parent's
    /// block.
    #[inline]
    pub(super) fn next_sibling(&self, idx: usize) -> Option<usize> {
        let block = self.sibling_block(idx);
        (idx + 1 < block.end).then_some(idx + 1)
    }

    #[inline]
    pub(super) fn prev_sibling(&self, idx: usize) -> Option<usize> {
        let block = self.sibling_block(idx);
        (idx > block.start).then_some(idx - 1)
    }

    /// `idx`'s 1-based position among its siblings.
    ///
    /// Spec 0192 stored this in the node and had `splice_override`
    /// repair it. It is `idx - first_child[parent]`, and the reason it
    /// needs no packed-run special case any more is that a run *is* one
    /// slot now: the rule spec 0184 S2 had to state — a run's N spans
    /// share one ordinal — is no longer a rule, it is arithmetic.
    #[inline]
    pub(super) fn sibling_position(&self, idx: usize) -> usize {
        idx - self.sibling_block(idx).start + 1
    }

    /// The node after `idx` in document order (spec 0216 S27).
    ///
    /// Pre-order: descend if there is anywhere to descend to, else step
    /// sideways, else climb until something has a next sibling. Stored
    /// as two arrays until now, because the render emits post-order and
    /// document order could not be read off it; in level order it is
    /// three lines and no memory.
    pub(super) fn doc_next(&self, idx: usize) -> Option<usize> {
        if let Some(child) = self.first_child(idx) {
            return Some(child);
        }
        let mut cur = idx;
        loop {
            if let Some(sibling) = self.next_sibling(cur) {
                return Some(sibling);
            }
            cur = self.parent(cur)?;
        }
    }
}
