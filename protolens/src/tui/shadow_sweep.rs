// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0343 B4: the structural pass — one DFS that inserts the arena
//! into a trie keyed by field-number path, building links between every
//! pair of slots that share a path.
//!
//! The trie is scaffolding: it is built here and dropped at the end of
//! the walk; only the links survive.  The filter (B5) visits links and
//! sets a bit per shadowed slot; the display reads the bits.
//!
//! Three properties that are load-bearing for correctness.
//!
//! **DFS, not level-order.** The arena is in level order (S16), so a
//! simple `0..arena.len()` loop visits siblings before children and
//! computes wrong ancestors on documents whose links are correct.  The
//! explicit stack below bounds depth by `MAX_WIRE_DEPTH`.
//!
//! **Kind-blind.** Every slot writes the value half of its trie cell;
//! a slot with children also descends into the child half — the two are
//! not alternatives.  Which half corresponds to "message or leaf" is a
//! schema answer, entering only in B5.
//!
//! **The clock gives common ancestors for free.** Each DFS stack frame
//! records the clock at which its slot was entered.  When slot A is
//! displaced by slot B, the common ancestor is the slot at the deepest
//! DFS frame whose entry clock is ≤ A's write stamp — the deepest
//! frame whose occupant has not changed since A was written.

use std::collections::HashMap;

use prototext_core::helpers::{parse_varint, MAX_WIRE_DEPTH};
use prototext_core::Arena;

// ── Trie ─────────────────────────────────────────────────────────────────────

/// One node in the merged-trie.  Children are looked up by field number;
/// the field set of a real message is small, so a linear scan over
/// `entries` costs nothing in practice.
#[derive(Default)]
struct TrieNode {
    entries: Vec<TrieEntry>,
}

impl TrieNode {
    /// Return a mutable reference to the entry for `field`, creating it
    /// (with a default cell) if absent.
    fn entry_mut(&mut self, field: u32) -> &mut TrieEntry {
        match self.entries.iter().position(|e| e.field == field) {
            Some(i) => &mut self.entries[i],
            None => {
                self.entries.push(TrieEntry {
                    field,
                    cell: TrieCell::default(),
                });
                self.entries.last_mut().unwrap()
            }
        }
    }
}

struct TrieEntry {
    field: u32,
    cell: TrieCell,
}

/// The two independent halves of a trie cell (spec 0343 B4 — they are
/// not alternatives).
#[derive(Default)]
struct TrieCell {
    /// Slab index of the child trie node, present once any slot with
    /// children has been seen at this field.
    child: Option<usize>,
    /// The last arena slot written to this field's value half.
    value: Option<u32>,
    /// Clock at which `value` was last written — used to identify the
    /// displaced occupant during a collision.
    value_stamp: u64,
}

// ── DFS stack frame ───────────────────────────────────────────────────────────

/// One frame on the explicit DFS stack.
struct Frame {
    /// Arena slot that opened this frame.
    slot: u32,
    /// Clock value at the moment `slot` was entered — used by
    /// `find_ancestor` to identify the deepest frame whose occupant has
    /// not changed since a displaced slot was written.
    entry_stamp: u64,
    /// Cursor into `first_child[slot]..first_child[slot+1]` — the next
    /// child arena slot to visit.
    child_cursor: u32,
    /// Slab index of the trie node this frame descends into for its
    /// children.
    child_trie: usize,
}

// ── Links ─────────────────────────────────────────────────────────────────────

/// One structural link: the displaced slot, the displacing one, and
/// their nearest common arena ancestor (spec 0343 B4).
#[derive(Clone, Copy, Debug)]
pub(super) struct Link {
    pub(super) shadowed: u32,
    pub(super) shadowing: u32,
    pub(super) ancestor: u32,
}

// ── ShadowSweep ──────────────────────────────────────────────────────────────

/// B4's resumable structural pass (spec 0343 B4/B6).
///
/// Created once per document from `ShadowSweep::new`, driven by
/// `step()` one chunk at a time from the idle ladder, and complete when
/// `is_complete()` returns true.  The links it builds are the inputs to
/// B5's filter.
pub(super) struct ShadowSweep {
    /// Slab of all trie nodes.  Index 0 is the root (corresponds to the
    /// virtual wrapper slot 0's children).
    trie: Vec<TrieNode>,
    /// Monotone clock, incremented on every arena-slot entry.
    clock: u64,
    /// The explicit DFS stack.  Depth is bounded by `MAX_WIRE_DEPTH`.
    stack: Vec<Frame>,
    /// Cursor into the root-level child range.  The virtual wrapper is
    /// slot 0; its children span `first_child[0]..first_child[1]`.
    root_cursor: u32,
    /// `first_child[1]` — the exclusive end of the root child range,
    /// cached at construction.
    root_end: u32,
    /// Forward map: shadowed slot → (shadowing slot, ancestor slot).
    /// Populated during the walk; read by B5.
    pub(super) forward: HashMap<u32, (u32, u32)>,
    /// Backward map: shadowing slot → shadowed slot.
    /// Populated during the walk; read by B5.
    pub(super) backward: HashMap<u32, u32>,
}

/// How many arena slots `ShadowSweep::step` processes per call from the
/// idle ladder (spec 0343 B6).  Sized so that one chunk plus one bake
/// step fits the frame budget; 100 is conservative — each insertion is
/// a handful of instructions plus one or two hash writes.
pub(super) const SHADOW_CHUNK: usize = 100;

impl ShadowSweep {
    /// Start a new pass over the given arena.
    pub(super) fn new(arena: &Arena) -> Self {
        let fc = arena.first_child();
        ShadowSweep {
            trie: vec![TrieNode::default()],
            clock: 0,
            stack: Vec::new(),
            root_cursor: fc[0],
            root_end: fc[1],
            forward: HashMap::new(),
            backward: HashMap::new(),
        }
    }

    /// True when the DFS has visited the whole arena.
    pub(super) fn is_complete(&self) -> bool {
        self.stack.is_empty() && self.root_cursor >= self.root_end
    }

    /// Process up to `SHADOW_CHUNK` slots and return.  Safe to call
    /// again after `is_complete()` — it becomes a no-op.
    pub(super) fn step(&mut self, arena: &Arena, blob: &[u8]) {
        let fc = arena.first_child();
        let rs = arena.raw_start();

        let mut budget = SHADOW_CHUNK;
        'outer: while budget > 0 {
            budget -= 1;

            // ── Continue an open frame ──────────────────────────────────────
            if let Some(frame) = self.stack.last_mut() {
                let end = fc[frame.slot as usize + 1];
                if frame.child_cursor < end {
                    let child_slot = frame.child_cursor;
                    let child_trie = frame.child_trie;
                    frame.child_cursor += 1;
                    self.visit(child_slot, child_trie, fc, rs, blob);
                    continue 'outer;
                }
                // Frame exhausted — pop and try the next budget tick.
                self.stack.pop();
                continue 'outer;
            }

            // ── Advance the root cursor ─────────────────────────────────────
            if self.root_cursor >= self.root_end {
                break;
            }
            let slot = self.root_cursor;
            self.root_cursor += 1;
            self.visit(slot, 0, fc, rs, blob);
        }
    }

    /// Process one arena slot: decode its field number, write the value
    /// half (linking if already occupied), and push a stack frame if it
    /// has children.
    fn visit(&mut self, slot: u32, parent_trie: usize, fc: &[u32], rs: &[u32], blob: &[u8]) {
        self.clock += 1;
        let stamp = self.clock;

        // Decode the field number from the tag bytes.
        let raw_start = rs[slot as usize] as usize;
        let field_number = match parse_varint(blob, raw_start).varint {
            Some(v) => (v >> 3) as u32,
            // Malformed — no readable field number: skip (spec 0343 B4).
            None => return,
        };

        let has_children = fc[slot as usize] < fc[slot as usize + 1];

        // ── Value half ──────────────────────────────────────────────────────
        // Read the collision data first, drop the mutable trie borrow,
        // then call `ancestor_for` (which takes `&self`).
        let collision = {
            let entry = self.trie[parent_trie].entry_mut(field_number);
            let cell = &mut entry.cell;
            let prev = cell.value.replace(slot);
            let prev_stamp = cell.value_stamp;
            cell.value_stamp = stamp;
            prev.map(|p| (p, prev_stamp))
        };
        if let Some((prev, prev_stamp)) = collision {
            let ancestor = self.ancestor_for(prev_stamp);
            self.forward.insert(prev, (slot, ancestor));
            self.backward.insert(slot, prev);
        }

        // ── Child half ──────────────────────────────────────────────────────
        if has_children && self.stack.len() < MAX_WIRE_DEPTH {
            let child_trie = {
                let entry = self.trie[parent_trie].entry_mut(field_number);
                let cell = &mut entry.cell;
                match cell.child {
                    Some(idx) => idx,
                    None => {
                        let idx = self.trie.len();
                        self.trie.push(TrieNode::default());
                        // Re-borrow after the push (trie may have
                        // reallocated).
                        self.trie[parent_trie].entry_mut(field_number).cell.child = Some(idx);
                        idx
                    }
                }
            };
            self.stack.push(Frame {
                slot,
                entry_stamp: stamp,
                child_cursor: fc[slot as usize],
                child_trie,
            });
        }
    }

    /// Climb the DFS stack to find the common ancestor of a displaced
    /// slot whose value half was written at clock `write_stamp`.
    ///
    /// The common ancestor is the slot at the deepest frame whose
    /// `entry_stamp ≤ write_stamp` — the deepest enclosing message
    /// instance that has not changed since the displaced slot was written
    /// (spec 0343 B4, clock/occupant/stamp).
    ///
    /// Returns slot 0 (the virtual root) if no frame qualifies, which
    /// is the correct answer for a collision at the top level.
    fn ancestor_for(&self, write_stamp: u64) -> u32 {
        for frame in self.stack.iter().rev() {
            if frame.entry_stamp <= write_stamp {
                return frame.slot;
            }
        }
        // No enclosing frame predates the write: the two slots are in
        // different top-level records, so the root is the ancestor.
        0
    }
}

// ── App integration (B6) ──────────────────────────────────────────────────────

// ── App integration (B5/B6/B7) ───────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use prototext_core::serialize::render_text::Label;

use super::App;

impl App {
    /// Spec 0343 B6 stage 2: do one trie chunk.
    ///
    /// Called from the idle ladder between `discard_step` and `bake_step`,
    /// and — critically — does **not** `continue` back to the top of the
    /// ladder.  One idle pass does one chunk *and* one bake step.
    ///
    /// Returns `true` while still in progress, `false` when done.
    pub(super) fn shadow_step(&mut self) -> bool {
        // Lazily allocate on the first idle pass (B6 stage 1 = no-op at
        // open; stage 2 = first call here).
        if self.shadow_sweep.is_none() {
            self.shadow_sweep = Some(ShadowSweep::new(&self.arena));
        }
        let sweep = self.shadow_sweep.as_mut().unwrap();
        if sweep.is_complete() {
            return false;
        }
        let blob = self.blob.as_ref();
        sweep.step(&self.arena, blob);
        !sweep.is_complete()
    }

    /// Spec 0343 B6 stage 3: run the filter over all links.
    ///
    /// Called once, from the idle ladder, when both the structural pass
    /// *and* the bake are idle.  Allocates the shadow bitset (B7) and
    /// sets bits for every link that passes B5.  Schedules a redraw for
    /// each slot it marks (B7 third clause).
    ///
    /// Returns `true` if any bit was set (a redraw is owed).
    pub(super) fn shadow_filter(&mut self) -> bool {
        let sweep = match self.shadow_sweep.as_ref() {
            Some(s) if s.is_complete() => s,
            _ => return false,
        };

        // Lazily allocate the shadow bitset (B7: length = arena length,
        // never changes).
        let n = self.arena.len();
        let nwords = n.div_ceil(64);
        if self.shadowed.is_none() {
            let words = (0..nwords).map(|_| AtomicU64::new(0)).collect();
            self.shadowed = Some(Arc::new(words));
        }

        // Collect surviving links first (one pass, no double evaluation).
        let marked: Vec<u32> = {
            let sweep = self.shadow_sweep.as_ref().unwrap();
            sweep
                .forward
                .iter()
                .filter(|(&shadowed, &(shadowing, ancestor))| {
                    self.link_survives(shadowed, shadowing, ancestor)
                })
                .map(|(&shadowed, _)| shadowed)
                .collect()
        };

        if marked.is_empty() {
            return false;
        }

        let bitset = self.shadowed.as_ref().unwrap();
        for &slot in &marked {
            // Set the bit (Relaxed: sweep and display are the same
            // thread; the segment scan only reads, B7).
            let w = slot as usize / 64;
            let bit = 1u64 << (slot as usize % 64);
            bitset[w].fetch_or(bit, Ordering::Relaxed);
        }

        // B9: refresh each marked slot and its ancestors so the
        // roll-up reaches the margin color.
        for slot in marked {
            self.refresh_status_subtree(slot as usize);
            self.refresh_status_ancestors(slot as usize);
        }

        true
    }

    /// True iff the link from `shadowed` to `shadowing` under `ancestor`
    /// passes B5's four-clause filter.
    fn link_survives(&self, shadowed: u32, shadowing: u32, ancestor: u32) -> bool {
        let tree = &self.tree;

        // Drop if either end is unrendered (lines_total == 0).
        if !tree[shadowed as usize].is_rendered() || !tree[shadowing as usize].is_rendered() {
            return false;
        }

        // Drop if either end renders as a message.
        if tree[shadowed as usize].span.is_message || tree[shadowing as usize].span.is_message {
            return false;
        }

        // Walk from shadowing up to ancestor: drop if any crossed field
        // is Repeated or NoSchema.
        if !self.chain_is_singular(shadowing as usize, ancestor as usize) {
            return false;
        }
        // Same from shadowed.
        if !self.chain_is_singular(shadowed as usize, ancestor as usize) {
            return false;
        }

        true
    }

    /// Walk from `start` up to (but excluding) `ancestor`, returning
    /// `false` if any node's label is `Repeated` or `NoSchema`.
    fn chain_is_singular(&self, mut start: usize, ancestor: usize) -> bool {
        let tree = &self.tree;
        while start != ancestor {
            match tree[start].span.label() {
                Label::Repeated | Label::NoSchema => return false,
                Label::Optional | Label::Required => {}
            }
            match self.parent(start) {
                Some(p) => start = p,
                None => break, // reached a root without hitting ancestor
            }
        }
        true
    }

    /// Spec 0343 B7: read one slot's shadow bit, returning `false` when
    /// the bitset is not yet allocated (before B6 stage 3 runs).
    pub(super) fn is_shadowed(&self, idx: usize) -> bool {
        match &self.shadowed {
            None => false,
            Some(bitset) => {
                let w = idx / 64;
                bitset
                    .get(w)
                    .is_some_and(|a| a.load(Ordering::Relaxed) & (1u64 << (idx % 64)) != 0)
            }
        }
    }

    /// Read one shadow word (64 bits starting at slot `64*w`), returning
    /// `0` when the bitset is not yet allocated.  Used by `rebuild_status`
    /// fast path (spec 0343 B9).
    pub(super) fn shadow_word(&self, w: usize) -> u64 {
        match &self.shadowed {
            None => 0,
            Some(bitset) => bitset.get(w).map_or(0, |a| a.load(Ordering::Relaxed)),
        }
    }

    /// Clear the shadow bitset and reset the filter cursor so stage 3
    /// re-runs on the next idle pass.  Called by override handling on
    /// every splice (spec 0343 B6: "override invalidates the bits,
    /// never the links").
    pub(super) fn invalidate_shadow_bits(&mut self) {
        if let Some(bitset) = &self.shadowed {
            for word in bitset.iter() {
                word.store(0, Ordering::Relaxed);
            }
        }
        self.shadow_filter_done = false;
    }
}
