// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Hopcroft DFA minimization over the raw scoring graph.

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

pub fn minimize(raw: &RawGraph, reg: &LeafRegistry) -> Partition {
    let num_leaves = reg.num_leaves();
    let n = raw.num_nodes as usize;
    let msg_count = n - num_leaves;

    // Map a node ID (message or leaf sentinel) to a contiguous index in 0..n.
    // Smallest possible ENUM sentinel: u32::MAX - 5 - (num_enum_leaves - 1).
    // All values >= this threshold and < LEAF_VARINT are ENUM sentinels.
    let enum_sentinel_min = if num_leaves > NUM_FIXED_LEAVES {
        LEAF_VARINT - 1 - (num_leaves - NUM_FIXED_LEAVES) as u32 + 1
    } else {
        LEAF_VARINT // no ENUM leaves — sentinel range is empty
    };

    let node_index = |node: u32| -> usize {
        match node {
            x if x == LEAF_VARINT => msg_count,
            x if x == LEAF_I64 => msg_count + 1,
            x if x == LEAF_LEN => msg_count + 2,
            x if x == LEAF_STRING => msg_count + 3,
            x if x == LEAF_I32 => msg_count + 4,
            x if x >= enum_sentinel_min && x < LEAF_VARINT => {
                // Dynamic ENUM sentinel: u32::MAX - 5 - i → msg_count + 5 + i
                let i = (u32::MAX - 5).wrapping_sub(x) as usize;
                msg_count + NUM_FIXED_LEAVES + i
            }
            x => x as usize,
        }
    };

    // ── Reverse adjacency: dst_index → Vec<(src_index, field_number, label)> ──
    // Label is part of the edge identity: two edges with the same field_number
    // but different labels (optional/required/repeated) are distinct splitters.
    let mut rev: Vec<Vec<(usize, u32, u8)>> = vec![Vec::new(); n];
    for edge in &raw.edges {
        let si = node_index(edge.src);
        let di = node_index(edge.dst);
        rev[di].push((si, edge.field_number, edge.label));
    }

    // ── Outgoing signature per node: sorted Vec<(field_number, label)> ────────
    let mut sig: Vec<Vec<(u32, u8)>> = vec![Vec::new(); n];
    for edge in &raw.edges {
        let si = node_index(edge.src);
        sig[si].push((edge.field_number, edge.label));
    }
    for s in sig.iter_mut() {
        s.sort_unstable();
        s.dedup();
    }

    // ── Initial partition ─────────────────────────────────────────────────────
    // Message nodes: grouped by outgoing (field_number, label) signature.
    // Leaf nodes: grouped by (wire_type, is_string, enum_range_idx) — nodes
    // with identical attributes are equivalent and may share a block.
    let mut sig_to_block: HashMap<Vec<(u32, u8)>, u32> = HashMap::new();
    let mut leaf_attr_to_block: HashMap<(u8, bool, u16), u32> = HashMap::new();
    let mut block_of: Vec<u32> = vec![0u32; n];
    let mut num_blocks: u32 = 0;

    for i in 0..msg_count {
        let s = sig[i].clone();
        let b = sig_to_block.entry(s).or_insert_with(|| {
            let b = num_blocks;
            num_blocks += 1;
            b
        });
        block_of[i] = *b;
    }

    // Leaf nodes: two leaves with identical attributes start in the same block.
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

    let num_blocks = num_blocks as usize;

    // ── Block membership sets ─────────────────────────────────────────────────
    let mut blocks: Vec<HashSet<usize>> = vec![HashSet::new(); num_blocks];
    for i in 0..n {
        blocks[block_of[i] as usize].insert(i);
    }

    // ── Worklist: (block_id, field_number, label) splitters ───────────────────
    let all_field_labels: HashSet<(u32, u8)> = raw
        .edges
        .iter()
        .map(|e| (e.field_number, e.label))
        .collect();
    let mut worklist: VecDeque<(usize, u32, u8)> = VecDeque::new();
    for b in 0..num_blocks {
        for &(f, l) in &all_field_labels {
            worklist.push_back((b, f, l));
        }
    }

    // ── Refinement loop ───────────────────────────────────────────────────────
    let mut blocks = blocks;
    let mut num_blocks = num_blocks;

    while let Some((splitter_block, field, label)) = worklist.pop_front() {
        if splitter_block >= blocks.len() || blocks[splitter_block].is_empty() {
            continue;
        }

        // Predecessors of splitter_block via `(field, label)`.
        let predecessors: HashSet<usize> = blocks[splitter_block]
            .iter()
            .flat_map(|&dst| {
                rev[dst]
                    .iter()
                    .filter(|(_, f, l)| *f == field && *l == label)
                    .map(|(src, _, _)| *src)
            })
            .collect();

        if predecessors.is_empty() {
            continue;
        }

        let mut block_intersection: HashMap<usize, Vec<usize>> = HashMap::new();
        for &p in &predecessors {
            block_intersection
                .entry(block_of[p] as usize)
                .or_default()
                .push(p);
        }

        for (c, inside) in block_intersection {
            if inside.len() == blocks[c].len() {
                continue;
            }
            let new_block_id = blocks.len();
            let mut new_block: HashSet<usize> = HashSet::new();
            for &node in &inside {
                blocks[c].remove(&node);
                new_block.insert(node);
                block_of[node] = new_block_id as u32;
            }
            blocks.push(new_block);
            num_blocks += 1;

            let smaller = if inside.len() <= blocks[c].len() {
                new_block_id
            } else {
                c
            };
            for &(f, l) in &all_field_labels {
                worklist.push_back((smaller, f, l));
            }
        }
    }

    let _ = num_blocks;

    // ── Renumber: non-leaf blocks first, then leaf blocks ────────────────────
    let leaf_block_ids: Vec<u32> = (0..num_leaves).map(|li| block_of[msg_count + li]).collect();

    let mut old_to_new: HashMap<u32, u32> = HashMap::new();
    let mut next_msg_id: u32 = 0;
    for &b in &block_of {
        if !leaf_block_ids.contains(&b) && !old_to_new.contains_key(&b) {
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
