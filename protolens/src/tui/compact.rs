// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Incremental compaction of the node arena.
//!
//! `App::tree` is append-only: every override batch materializes fresh
//! nodes for whatever it retypes and abandons the ones they supersede,
//! so the arena grows by roughly a document per batch while the live
//! set stays flat. Measured on a doubled `googleapis.desc`: 4 501 014
//! nodes at startup, 9 000 349 after one batch, 13 499 684 after two,
//! of which only 4 499 336 were ever reachable.
//!
//! The reclamation here rests on one property of the arena, which is
//! what makes it incremental rather than stop-the-world:
//!
//! **Every reference to a node is discoverable from that node.**
//!
//! A node's parent, siblings and document-order neighbors are named by
//! its own link fields, and the reverse link in each of them is the one
//! that has to change. Its children are reachable by walking its own
//! child chain. Its two line-map entries are found through its own
//! `text_range`. The remaining holders — `folded`, `pending_heat_
//! recheck`, `cursor`, `first_node`, `override_target` — are keyed by
//! index, so a hash lookup or an equality test finds them.
//!
//! So a single node can be relocated in time proportional to its own
//! degree, with no arena-wide pass and no remap table. Summed over the
//! arena that is `O(edges)`, i.e. `O(nodes)` — the same total as a
//! mark-compact, but divisible at *arbitrary* granularity, because the
//! arena is fully consistent after every individual move.
//!
//! That is the property that matters. It means a pass can be sliced
//! into the event loop a few thousand nodes at a time (the same place
//! read-ahead already runs), abandoned halfway with no cleanup, and
//! restarted from scratch later, all without the document ever being
//! observably wrong. Unlike an override batch, which mutates the arena
//! in place with no undo, compaction has no unsafe intermediate state
//! to protect the user from.
//!
//! Liveness is not recomputed. `splice_override` is the only producer
//! of garbage and already knows precisely what it abandons, so it
//! records it in `App::dead` as it goes; this module only moves nodes
//! and never has to decide which ones are worth keeping.

use super::App;

/// Nodes relocated per slice. At roughly the cost of a `TreeNode` copy
/// plus a handful of pointer writes each, this is well under a frame,
/// and the loop re-checks for input between slices.
pub(super) const COMPACT_SLICE_NODES: usize = 4_096;

/// A pass ends in a `shrink_to_fit`, which reallocates and copies the
/// whole arena, so it is worth starting only when it reclaims a real
/// fraction of one. Expressed as a fraction rather than a node count
/// because that is what it actually buys: refusing to run below a
/// garbage share of `1/N` bounds the arena at `N/(N-1)` times its live
/// size, which is the guarantee wanted, whereas any absolute floor says
/// nothing about a 4.5-million-node document.
const COMPACT_MIN_DEAD_SHARE: usize = 8;

/// What a slice accomplished, mirroring `PrefetchStep` so the event
/// loop can treat the two the same way.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) enum CompactStep {
    /// Nodes were moved; there is more to do.
    Progressed,
    /// The pass finished and the arena was truncated.
    Finished,
    /// Nothing to do.
    Idle,
}

impl App {
    /// Record that the node at `idx` is no longer reachable. Called by
    /// `splice_override` at the two points where it abandons one.
    ///
    /// Idempotent, so that the count stays in step with the flags even
    /// if a caller reaches the same node twice.
    pub(super) fn mark_dead(&mut self, idx: usize) {
        if !self.dead[idx] {
            self.dead[idx] = true;
            self.dead_count += 1;
        }
    }

    /// Move the live node at `from` into the free slot at `to`, fixing
    /// every reference to it.
    ///
    /// The arena is consistent on entry and on exit, which is the whole
    /// basis for slicing this work. `to` must be a dead slot and `from`
    /// a live one; neither can be a neighbor of the other, since a dead
    /// slot is by definition named by no live node.
    fn relocate_node(&mut self, from: usize, to: usize) {
        debug_assert!(self.dead[to], "compaction moves into a dead slot");
        debug_assert!(!self.dead[from], "compaction moves a live node");

        let parent = self.tree[from].parent;
        let prev_sibling = self.tree[from].prev_sibling;
        let next_sibling = self.tree[from].next_sibling;
        let doc_prev = self.tree[from].doc_prev;
        let doc_next = self.tree[from].doc_next;
        let first_child = self.tree[from].first_child;
        let text_start = self.tree[from].span.text_range.start;
        let text_end = self.tree[from].span.text_range.end;

        // A parent names its first and last child separately, and for an
        // only child both are this node.
        if let Some(p) = parent {
            if self.tree[p].first_child == Some(from) {
                self.tree[p].first_child = Some(to);
            }
            if self.tree[p].last_child == Some(from) {
                self.tree[p].last_child = Some(to);
            }
        }
        if let Some(s) = prev_sibling {
            self.tree[s].next_sibling = Some(to);
        }
        if let Some(s) = next_sibling {
            self.tree[s].prev_sibling = Some(to);
        }
        if let Some(d) = doc_prev {
            self.tree[d].doc_next = Some(to);
        }
        if let Some(d) = doc_next {
            self.tree[d].doc_prev = Some(to);
        }
        // The one unbounded step, and the reason the per-node cost is
        // "degree" rather than a constant. It sums to one visit per
        // edge across a whole pass.
        let mut child = first_child;
        while let Some(c) = child {
            self.tree[c].parent = Some(to);
            child = self.tree[c].next_sibling;
        }

        // The node itself, and the two arrays that are parallel to it.
        self.tree.swap(from, to);
        self.heat_states.swap(from, to);
        if from < self.descend.len() && to < self.descend.len() {
            self.descend.swap(from, to);
        }
        self.dead[to] = false;
        self.dead[from] = true;

        // Both line maps are indexed by line, so this node's own
        // `text_range` locates its entries directly. The equality test
        // is deliberate rather than defensive: a node whose header line
        // is currently attributed to some other node must not have that
        // attribution stolen by this move.
        if let Some(slot) = self.line_to_node.get_mut(text_start) {
            if *slot == Some(from as u32) {
                *slot = Some(to as u32);
            }
        }
        if text_end > 0 {
            if let Some(slot) = self.footer_line_to_node.get_mut(text_end - 1) {
                if *slot == Some(from as u32) {
                    *slot = Some(to as u32);
                }
            }
        }

        if self.folded.remove(&from) {
            self.folded.insert(to);
        }
        if self.pending_heat_recheck.remove(&from) {
            self.pending_heat_recheck.insert(to);
        }
        if self.cursor == from {
            self.cursor = to;
        }
        if self.first_node == from {
            self.first_node = to;
        }
        if self.override_target == Some(from) {
            self.override_target = Some(to);
        }
    }

    /// Advance the current compaction pass by at most `budget` node
    /// moves, starting one if none is in flight.
    ///
    /// Order-preserving: live nodes keep their relative order, so a
    /// subtree that occupied a contiguous extent still does afterwards,
    /// and `descend`'s length keeps its second meaning as spec 0188
    /// S4's already-examined watermark (every surviving node was
    /// examined before the pass).
    pub(super) fn compact_slice(&mut self, budget: usize) -> CompactStep {
        // A batch must never observe a half-packed arena through the
        // cursors, and `pending_line_patches` names nodes by index, so
        // a pass only ever runs between batches.
        if self.override_batch_depth > 0 || !self.pending_line_patches.is_empty() {
            return CompactStep::Idle;
        }
        // Deciding whether to start a pass. `dead_count` is maintained
        // incrementally, so this is a counter read rather than a walk of
        // the arena — which matters because the event loop asks on every
        // idle iteration.
        if self.compact_src == 0
            && self.compact_dst == 0
            && self.dead_count * COMPACT_MIN_DEAD_SHARE < self.tree.len()
        {
            return CompactStep::Idle;
        }

        let len = self.tree.len();
        let mut moved = 0;
        while moved < budget {
            // The next hole.
            while self.compact_dst < len && !self.dead[self.compact_dst] {
                self.compact_dst += 1;
            }
            if self.compact_dst >= len {
                break;
            }
            // The next live node above it.
            if self.compact_src <= self.compact_dst {
                self.compact_src = self.compact_dst + 1;
            }
            while self.compact_src < len && self.dead[self.compact_src] {
                self.compact_src += 1;
            }
            if self.compact_src >= len {
                break;
            }
            let (src, dst) = (self.compact_src, self.compact_dst);
            self.relocate_node(src, dst);
            moved += 1;
        }

        // The pass is over once either cursor runs off the end: no live
        // node remains above the lowest hole.
        if self.compact_dst >= len || self.compact_src >= len {
            let live = self.compact_dst.min(len);
            self.tree.truncate(live);
            self.heat_states.truncate(live);
            self.dead.truncate(live);
            if self.descend.len() > live {
                self.descend.truncate(live);
            }
            // Without this the freed slots stay reserved and nothing is
            // handed back to the allocator — the arena's `capacity` was
            // measured at 18 004 056 for 13 499 684 live entries.
            self.tree.shrink_to_fit();
            self.heat_states.shrink_to_fit();
            self.dead.shrink_to_fit();
            // Every dead slot was above `live` by construction — that is
            // what ends the pass — so truncation discards all of them.
            self.dead_count = 0;
            self.compact_dst = 0;
            self.compact_src = 0;
            debug_assert!(
                !self.dead.iter().any(|d| *d),
                "a finished pass leaves no dead slot behind"
            );
            return CompactStep::Finished;
        }
        if moved == 0 {
            CompactStep::Idle
        } else {
            CompactStep::Progressed
        }
    }

    /// Abandon any pass in flight. Free, because a partially compacted
    /// arena is a fully consistent one — the cursors are the only state
    /// a pass carries, and dropping them loses nothing but the scanning
    /// already done.
    pub(super) fn reset_compaction(&mut self) {
        self.compact_dst = 0;
        self.compact_src = 0;
    }

    /// The four preconditions this module's safety rests on, checked
    /// rather than asserted in prose. `Err` names the first violation.
    ///
    /// Worth having as executable code because compaction changes the
    /// *consequence* of breaking them. Today a link into an abandoned
    /// subtree is survivable: the slot is never reused and never freed,
    /// so the reader sees a stale node and renders something wrong.
    /// Once slots are reused and the arena is truncated, the same link
    /// reads a live but unrelated node, or falls off the end. This
    /// converts an existing latent class of bug into a crash, so it is
    /// the arena's own well-formedness — not compaction's arithmetic —
    /// that most needs a witness.
    ///
    /// `O(nodes + lines)`, so it is a test instrument, not something the
    /// event loop can afford per slice. Test-only, which is a real
    /// limitation rather than a tidy one: the documents most likely to
    /// expose a violation are the multi-million-node ones no fixture
    /// reaches. A hook that let the shipping binary run this on demand
    /// would cover that, and is deliberately not built here on spec.
    #[cfg(test)]
    pub(super) fn verify_arena(&self) -> Result<(), String> {
        let len = self.tree.len();
        if self.dead.len() != len {
            return Err(format!(
                "dead has {} entries for {len} nodes",
                self.dead.len()
            ));
        }
        if self.heat_states.len() != len {
            return Err(format!(
                "heat_states has {} entries for {len} nodes",
                self.heat_states.len()
            ));
        }
        if self.descend.len() > len {
            return Err(format!(
                "descend has {} entries, past the {len}-node arena",
                self.descend.len()
            ));
        }

        // The live set, by definition: what the child chains reach from
        // the top level.
        //
        // The top level is a *forest*, not a single root — a document's
        // top-level fields are a sibling chain of nodes with no parent
        // (7 771 of them on the reported `FileDescriptorSet`). Climbing
        // `parent` from `first_node` therefore lands on one arbitrary
        // member of that chain, so the head has to be found by walking
        // `prev_sibling` afterwards and every sibling seeded. Getting
        // this wrong reports most of the document as unreachable.
        // `build_tree` is post-order, so none of this is index 0.
        let mut top = self.first_node;
        while let Some(p) = self.tree[top].parent {
            top = p;
        }
        while let Some(s) = self.tree[top].prev_sibling {
            top = s;
        }
        let mut live = vec![false; len];
        let mut order = Vec::new();
        let mut stack = Vec::new();
        let mut t = Some(top);
        while let Some(n) = t {
            stack.push(n);
            t = self.tree[n].next_sibling;
        }
        while let Some(n) = stack.pop() {
            if live[n] {
                return Err(format!(
                    "node {n} is reachable twice — the tree is not a tree"
                ));
            }
            live[n] = true;
            order.push(n);
            let mut c = self.tree[n].first_child;
            while let Some(ci) = c {
                stack.push(ci);
                c = self.tree[ci].next_sibling;
            }
        }

        // P1 — soundness of `dead`. Marking a live node is the one error
        // that loses data outright: its slot becomes a hole, gets
        // overwritten, and is truncated away.
        for n in 0..len {
            if live[n] && self.dead[n] {
                return Err(format!("live node {n} is marked dead"));
            }
        }
        let counted = self.dead.iter().filter(|d| **d).count();
        if counted != self.dead_count {
            return Err(format!(
                "dead_count says {} but {counted} slots are marked",
                self.dead_count
            ));
        }

        // P2 — closure, and the bidirectional consistency P3 relies on.
        for &n in &order {
            let node = &self.tree[n];
            for (what, link) in [
                ("parent", node.parent),
                ("first_child", node.first_child),
                ("last_child", node.last_child),
                ("prev_sibling", node.prev_sibling),
                ("next_sibling", node.next_sibling),
                ("doc_prev", node.doc_prev),
                ("doc_next", node.doc_next),
            ] {
                if let Some(i) = link {
                    if i >= len {
                        return Err(format!("node {n}'s {what} is {i}, past the arena"));
                    }
                    if !live[i] {
                        return Err(format!(
                            "live node {n}'s {what} names {i}, which is not \
                             reachable from the top level"
                        ));
                    }
                }
            }
            // Each inverse link, which is what makes every reference to
            // a node discoverable *from* that node.
            if let Some(s) = node.next_sibling {
                if self.tree[s].prev_sibling != Some(n) {
                    return Err(format!("{n}.next_sibling = {s}, but not conversely"));
                }
            }
            if let Some(s) = node.prev_sibling {
                if self.tree[s].next_sibling != Some(n) {
                    return Err(format!("{n}.prev_sibling = {s}, but not conversely"));
                }
            }
            if let Some(d) = node.doc_next {
                if self.tree[d].doc_prev != Some(n) {
                    return Err(format!("{n}.doc_next = {d}, but not conversely"));
                }
            }
            if let Some(d) = node.doc_prev {
                if self.tree[d].doc_next != Some(n) {
                    return Err(format!("{n}.doc_prev = {d}, but not conversely"));
                }
            }
            // The child chain must be exactly the nodes claiming `n` as
            // parent, and must end where `last_child` says it does —
            // together these are what let `relocate_node` repair every
            // `parent` pointer by walking `first_child` alone.
            let mut c = node.first_child;
            let mut last = None;
            while let Some(ci) = c {
                if self.tree[ci].parent != Some(n) {
                    return Err(format!(
                        "{ci} is on {n}'s child chain but its parent is {:?}",
                        self.tree[ci].parent
                    ));
                }
                last = Some(ci);
                c = self.tree[ci].next_sibling;
            }
            if last != node.last_child {
                return Err(format!(
                    "{n}.last_child = {:?} but its chain ends at {last:?}",
                    node.last_child
                ));
            }
        }

        // The document-order thread has to cover exactly the live set,
        // or `relocate_node`'s two-step repair of it is repairing a
        // chain that some other node is still hanging off.
        let mut doc_first = top;
        while let Some(d) = self.tree[doc_first].doc_prev {
            doc_first = d;
        }
        let mut seen = 0usize;
        let mut cur = Some(doc_first);
        while let Some(n) = cur {
            if !live[n] {
                return Err(format!("the document chain passes through dead node {n}"));
            }
            seen += 1;
            if seen > order.len() {
                return Err("the document chain is cyclic".to_string());
            }
            cur = self.tree[n].doc_next;
        }
        if seen != order.len() {
            return Err(format!(
                "the document chain covers {seen} of {} live nodes",
                order.len()
            ));
        }

        // P3's non-link holders. Every one of these is index-keyed, so
        // `relocate_node` finds it by lookup or equality — but only if
        // it is on this list, which no compiler enforces. A new field
        // holding a node index belongs here and in `relocate_node`.
        let holders: Vec<(&str, usize)> =
            [("cursor", self.cursor), ("first_node", self.first_node)]
                .into_iter()
                .chain(self.override_target.map(|t| ("override_target", t)))
                .chain(self.folded.iter().map(|f| ("folded", *f)))
                .chain(
                    self.pending_heat_recheck
                        .iter()
                        .map(|p| ("pending_heat_recheck", *p)),
                )
                .collect();
        for (what, i) in holders {
            if i >= len {
                return Err(format!("{what} names {i}, past the {len}-node arena"));
            }
            if !live[i] {
                return Err(format!("{what} names dead node {i}"));
            }
        }

        // The line maps, which `relocate_node` repairs through the moved
        // node's own `text_range` — so an entry filed anywhere else
        // would be missed.
        for (l, slot) in self.line_to_node.iter().enumerate() {
            if let Some(n) = *slot {
                let n = n as usize;
                if n >= len || !live[n] {
                    return Err(format!("line_to_node[{l}] names unusable node {n}"));
                }
                if self.tree[n].span.text_range.start != l {
                    return Err(format!(
                        "line_to_node[{l}] names node {n}, whose range starts at {}",
                        self.tree[n].span.text_range.start
                    ));
                }
            }
        }
        for (l, slot) in self.footer_line_to_node.iter().enumerate() {
            if let Some(n) = *slot {
                let n = n as usize;
                if n >= len || !live[n] {
                    return Err(format!("footer_line_to_node[{l}] names unusable node {n}"));
                }
                if self.tree[n].span.text_range.end != l + 1 {
                    return Err(format!(
                        "footer_line_to_node[{l}] names node {n}, whose range ends at {}",
                        self.tree[n].span.text_range.end
                    ));
                }
            }
        }
        Ok(())
    }
}
