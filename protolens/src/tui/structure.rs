// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0216: the tree's shape, read off the arena rather than stored.
//!
//! Every link — parent, first and last child, both siblings, both
//! document-order neighbors, and the sibling ordinal — is arithmetic on
//! the arena's `first_child` and `parent` arrays. That is what level
//! order buys: children occupy one contiguous block, blocks run in
//! parent order, so a sibling step is `+1` and a child step is an add
//! (S16, S17).
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

/// The rendered tree's shape, as the two arrays that define it.
///
/// [`App`]'s accessors below are this view's, one line each. It exists
/// because spec 0274's document cursor walks the same links from a
/// worker thread, where there is no `&App` to borrow — only the pieces
/// the scan was handed.
#[derive(Clone, Copy)]
pub(super) struct Structure<'a> {
    pub(super) tree: &'a [TreeNode],
    pub(super) arena: &'a Arena,
}

impl<'a> Structure<'a> {
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
    pub(super) fn sibling_block(&self, idx: usize) -> Range<usize> {
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
    pub(super) fn child_slots(&self, idx: usize) -> Range<usize> {
        let first_child = self.arena.first_child();
        let block = first_child[idx] as usize..first_child[idx + 1] as usize;
        if block.is_empty() || !self.tree[block.start].is_rendered() {
            return 0..0;
        }
        block
    }

    #[inline]
    pub(super) fn first_child(&self, idx: usize) -> Option<usize> {
        let block = self.child_slots(idx);
        (!block.is_empty()).then_some(block.start)
    }

    #[inline]
    pub(super) fn last_child(&self, idx: usize) -> Option<usize> {
        let block = self.child_slots(idx);
        // `then`, not `then_some`: the latter evaluates its argument
        // whether or not the condition held, so an empty block — which
        // `child_slots` reports as `0..0` — would compute `0 - 1` before
        // discarding it. The result is unused either way, but it is an
        // overflow, and the release profile now traps those.
        (!block.is_empty()).then(|| block.end - 1)
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
        // `then` for the same reason as `last_child`: slot 0 is the root
        // (spec 0216), and `then_some` would evaluate `0 - 1` on it.
        (idx > block.start).then(|| idx - 1)
    }

    #[inline]
    pub(super) fn is_bracketed(&self, idx: usize) -> bool {
        self.tree[idx].is_bracketed()
    }
}

impl App {
    /// The shape accessors' one source, borrowed for as long as the
    /// caller needs it.
    #[inline]
    pub(super) fn structure(&self) -> Structure<'_> {
        Structure {
            tree: &self.tree,
            arena: &self.arena,
        }
    }

    #[inline]
    pub(super) fn parent(&self, idx: usize) -> Option<usize> {
        self.structure().parent(idx)
    }

    #[inline]
    pub(super) fn child_slots(&self, idx: usize) -> Range<usize> {
        self.structure().child_slots(idx)
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
        self.structure().first_child(idx)
    }

    #[inline]
    pub(super) fn last_child(&self, idx: usize) -> Option<usize> {
        self.structure().last_child(idx)
    }

    #[inline]
    pub(super) fn next_sibling(&self, idx: usize) -> Option<usize> {
        self.structure().next_sibling(idx)
    }

    #[inline]
    pub(super) fn prev_sibling(&self, idx: usize) -> Option<usize> {
        self.structure().prev_sibling(idx)
    }

    /// `parent`'s children bearing `field`, in document order.
    ///
    /// A filter over the child block rather than a `next_sibling` walk:
    /// level order makes the block contiguous, so the siblings are just
    /// a range. The four callers all want this — three collect it, one
    /// takes the first — and all four ask about `field` as a `u64`,
    /// which is how an `OverrideOrigin` carries a field number.
    pub(super) fn children_with_field(
        &self,
        parent: usize,
        field: u64,
    ) -> impl Iterator<Item = usize> + '_ {
        self.child_slots(parent)
            .filter(move |&c| u64::from(self.tree[c].span.field_number) == field)
    }

    /// `idx`'s 1-based position among its siblings.
    ///
    /// Just `idx - first_child[parent]`, with no packed-run special
    /// case: a run is one slot, so spec 0184 S2's rule that a run's N
    /// spans share one ordinal is arithmetic rather than a rule.
    #[inline]
    pub(super) fn sibling_position(&self, idx: usize) -> usize {
        idx - self.structure().sibling_block(idx).start + 1
    }

    /// The node after `idx` in document order (spec 0216 S27).
    ///
    /// Pre-order: descend if there is anywhere to descend to, else step
    /// sideways, else climb until something has a next sibling. Level
    /// order makes this derivable, so nothing is stored.
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
