// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Hopcroft DFA minimization over the raw scoring graph (spec 0062).
//!
//! Implements the textbook Hopcroft (1971) algorithm verbatim:
//!
//!   P  := initial partition
//!   W  := P   (worklist = every (block, symbol) pair)
//!
//!   while W not empty:
//!       pick and remove A from W
//!       for each symbol (f, l):
//!           X := predecessors of A via (f, l)
//!           for each block Y where Y∩X ≠ ∅ and Y\X ≠ ∅:
//!               split Y into Y₁ = Y∩X and Y₂ = Y\X
//!               for each symbol (f', l'):
//!                   if (Y, f', l') ∈ W:
//!                       replace (Y, f', l') with (Y₁, f', l') and (Y₂, f', l')
//!                   else:
//!                       add smaller(Y₁, Y₂) for (f', l') to W

use std::collections::{HashMap, HashSet, VecDeque};

use super::graph::{
    leaf_attrs, LeafRegistry, RawGraph, LEAF_I32, LEAF_I64, LEAF_LEN, LEAF_STRING, LEAF_VARINT,
    NUM_FIXED_LEAVES,
};

// ── Partition ─────────────────────────────────────────────────────────────────

pub struct Partition {
    /// block_of[node_index] = block ID.
    /// Indices 0..msg_count are message nodes; msg_count+i is leaf i.
    pub block_of: Vec<u32>,
    num_blocks: usize,
}

impl Partition {
    pub fn block_of(&self, node: u32) -> u32 {
        self.block_of[node as usize]
    }

    pub fn num_blocks(&self) -> usize {
        self.num_blocks
    }
}

// ── Minimization ──────────────────────────────────────────────────────────────

pub fn minimize(
    raw: &RawGraph,
    reg: &LeafRegistry,
    node_wire_types: &HashMap<u32, u8>,
) -> Partition {
    let num_leaves = reg.num_leaves();
    let n = raw.num_nodes as usize;
    let msg_count = n - num_leaves;

    // ── node_index: map raw node ID to contiguous index 0..n ─────────────────
    let enum_sentinel_min = if num_leaves > NUM_FIXED_LEAVES {
        LEAF_VARINT - (num_leaves - NUM_FIXED_LEAVES) as u32
    } else {
        LEAF_VARINT
    };

    let node_index = |node: u32| -> usize {
        match node {
            x if x == LEAF_VARINT => msg_count,
            x if x == LEAF_I64 => msg_count + 1,
            x if x == LEAF_LEN => msg_count + 2,
            x if x == LEAF_STRING => msg_count + 3,
            x if x == LEAF_I32 => msg_count + 4,
            x if x >= enum_sentinel_min && x < LEAF_VARINT => {
                let i = (u32::MAX - 5).wrapping_sub(x) as usize;
                msg_count + NUM_FIXED_LEAVES + i
            }
            x => x as usize,
        }
    };

    // ── Alphabet Σ: all (field_number, label) pairs in the graph ─────────────
    let alphabet: Vec<(u32, u8)> = {
        let set: HashSet<(u32, u8)> = raw
            .edges
            .iter()
            .map(|e| (e.field_number, e.label))
            .collect();
        let mut v: Vec<(u32, u8)> = set.into_iter().collect();
        v.sort_unstable();
        v
    };

    // ── Reverse adjacency: rev[di] = list of (si, field_number, label) ────────
    let mut rev: Vec<Vec<(usize, u32, u8)>> = vec![Vec::new(); n];
    for edge in &raw.edges {
        let si = node_index(edge.src);
        let di = node_index(edge.dst);
        rev[di].push((si, edge.field_number, edge.label));
    }

    // ── Outgoing signature per node ───────────────────────────────────────────
    let mut sig: Vec<Vec<(u32, u8)>> = vec![Vec::new(); n];
    for edge in &raw.edges {
        let si = node_index(edge.src);
        sig[si].push((edge.field_number, edge.label));
    }
    for s in sig.iter_mut() {
        s.sort_unstable();
        s.dedup();
    }

    // ── Initial partition P₀ ─────────────────────────────────────────────────
    let mut sig_to_block: HashMap<(u8, Vec<(u32, u8)>), usize> = HashMap::new();
    let mut leaf_attr_to_block: HashMap<(u8, bool, u16), usize> = HashMap::new();
    let mut block_of: Vec<usize> = vec![0usize; n];
    let mut num_blocks: usize = 0;

    for i in 0..msg_count {
        let wt = node_wire_types.get(&(i as u32)).copied().unwrap_or(2);
        let s = sig[i].clone();
        let b = sig_to_block.entry((wt, s)).or_insert_with(|| {
            let b = num_blocks;
            num_blocks += 1;
            b
        });
        block_of[i] = *b;
    }
    for li in 0..num_leaves {
        let sentinel = leaf_sentinel_for_index(li, reg);
        let attrs = leaf_attrs(sentinel, reg);
        let key = (attrs.wire_type, attrs.is_string, attrs.enum_range_idx);
        let b = leaf_attr_to_block.entry(key).or_insert_with(|| {
            let b = num_blocks;
            num_blocks += 1;
            b
        });
        block_of[msg_count + li] = *b;
    }

    // ── Block membership: blocks[id] = set of node indices ───────────────────
    let mut blocks: Vec<HashSet<usize>> = vec![HashSet::new(); num_blocks];
    for i in 0..n {
        blocks[block_of[i]].insert(i);
    }

    // ── Worklist W: (block_id, field, label) ─────────────────────────────────
    // in_worklist mirrors the deque contents for O(1) membership queries.
    let mut worklist: VecDeque<(usize, u32, u8)> = VecDeque::new();
    let mut in_worklist: HashSet<(usize, u32, u8)> = HashSet::new();

    for b in 0..num_blocks {
        for &(f, l) in &alphabet {
            worklist.push_back((b, f, l));
            in_worklist.insert((b, f, l));
        }
    }

    // ── Refinement loop ───────────────────────────────────────────────────────
    while let Some((a_id, field, label)) = worklist.pop_front() {
        in_worklist.remove(&(a_id, field, label));

        if a_id >= blocks.len() || blocks[a_id].is_empty() {
            continue;
        }

        // X = predecessors of block A via (field, label).
        let x: HashSet<usize> = blocks[a_id]
            .iter()
            .flat_map(|&di| {
                rev[di]
                    .iter()
                    .filter(|(_, f, l)| *f == field && *l == label)
                    .map(|(si, _, _)| *si)
            })
            .collect();

        if x.is_empty() {
            continue;
        }

        // Group X members by their current block.
        let mut x_in_block: HashMap<usize, Vec<usize>> = HashMap::new();
        for &node in &x {
            x_in_block.entry(block_of[node]).or_default().push(node);
        }

        for (y_id, y1_nodes) in x_in_block {
            // Y₁ = Y ∩ X = y1_nodes; Y₂ = Y \ X = blocks[y_id] after removal.
            if y1_nodes.len() == blocks[y_id].len() {
                // Y ⊆ X: no split needed.
                continue;
            }

            // Perform the split: Y₁ gets a new block ID; Y₂ keeps y_id.
            let y1_id = blocks.len();
            let mut y1_block: HashSet<usize> = HashSet::new();
            for &node in &y1_nodes {
                blocks[y_id].remove(&node);
                y1_block.insert(node);
                block_of[node] = y1_id;
            }
            blocks.push(y1_block);
            // blocks[y_id] now contains Y₂.

            let y2_id = y_id;
            let y1_len = y1_nodes.len();
            let y2_len = blocks[y2_id].len();

            // Update worklist for every symbol (f', l') per textbook rule.
            for &(fp, lp) in &alphabet {
                if in_worklist.contains(&(y_id, fp, lp)) {
                    // (Y, f', l') ∈ W: replace Y with Y₁ and Y₂.
                    // Y₂ keeps the original y_id slot — its entry is already
                    // (y_id=y2_id, fp, lp) which remains in in_worklist.
                    // Just add Y₁.
                    if in_worklist.insert((y1_id, fp, lp)) {
                        worklist.push_back((y1_id, fp, lp));
                    }
                } else {
                    // (Y, f', l') ∉ W: add only the smaller of Y₁ and Y₂.
                    let smaller_id = if y1_len <= y2_len { y1_id } else { y2_id };
                    if in_worklist.insert((smaller_id, fp, lp)) {
                        worklist.push_back((smaller_id, fp, lp));
                    }
                }
            }
        }
    }

    // ── Renumber: non-leaf blocks first, then leaf blocks ─────────────────────
    let leaf_block_ids: Vec<usize> = (0..num_leaves).map(|li| block_of[msg_count + li]).collect();
    let leaf_block_set: HashSet<usize> = leaf_block_ids.iter().copied().collect();

    let mut old_to_new: HashMap<usize, u32> = HashMap::new();
    let mut next_msg_id: u32 = 0;
    for &b in &block_of {
        if !leaf_block_set.contains(&b) && !old_to_new.contains_key(&b) {
            old_to_new.insert(b, next_msg_id);
            next_msg_id += 1;
        }
    }
    let num_msg_blocks = next_msg_id as usize;
    for (slot, &old_leaf_id) in leaf_block_ids.iter().enumerate() {
        old_to_new
            .entry(old_leaf_id)
            .or_insert(num_msg_blocks as u32 + slot as u32);
    }

    let block_of_remapped: Vec<u32> = block_of.iter().map(|b| old_to_new[b]).collect();

    Partition {
        block_of: block_of_remapped,
        num_blocks: num_msg_blocks + num_leaves,
    }
}

fn leaf_sentinel_for_index(li: usize, _reg: &LeafRegistry) -> u32 {
    match li {
        0 => LEAF_VARINT,
        1 => LEAF_I64,
        2 => LEAF_LEN,
        3 => LEAF_STRING,
        4 => LEAF_I32,
        i => u32::MAX - 4 - 1 - (i - NUM_FIXED_LEAVES) as u32,
    }
}
