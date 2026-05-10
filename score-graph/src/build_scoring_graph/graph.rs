// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Raw graph construction and compilation to CompiledGraph.

use std::collections::HashMap;

use super::hopcroft::Partition;
use super::load::{FieldLabel, Merged, ScoringField, ScoringKind};
use super::serial::CompiledGraph;
use super::serial::{NodeEntry, RootEntry, TransitionEntry};

// ── Leaf sentinel node IDs ────────────────────────────────────────────────────
//
// Message/group nodes get dense IDs 0..msg_count.  Leaf nodes occupy sentinel
// IDs near u32::MAX; they are converted to stable block IDs by compile().
//
// Fixed leaves (by wire_type + attributes):
//   VARINT         wire_type=0, not string, not enum
//   I64            wire_type=1
//   LEN (bytes)    wire_type=2, not string
//   LEN_STRING     wire_type=2, is_string=true
//   I32            wire_type=5
//
// Dynamic ENUM leaves: wire_type=0, enum_range_idx=i; allocated by LeafRegistry.

pub const LEAF_VARINT: u32 = u32::MAX - 4;
pub const LEAF_I64: u32 = u32::MAX - 3;
pub const LEAF_LEN: u32 = u32::MAX - 2;
pub const LEAF_STRING: u32 = u32::MAX - 1;
pub const LEAF_I32: u32 = u32::MAX;

pub const NUM_FIXED_LEAVES: usize = 5;

// ── LeafRegistry ──────────────────────────────────────────────────────────────

/// Tracks the fixed scalar leaves plus dynamically allocated ENUM leaves.
pub struct LeafRegistry {
    enum_map: HashMap<(i32, i32), u32>,
    pub enum_ranges: Vec<(i32, i32)>,
}

impl LeafRegistry {
    pub fn new() -> Self {
        Self {
            enum_map: HashMap::new(),
            enum_ranges: Vec::new(),
        }
    }

    /// Return the sentinel for an ENUM leaf with the given range, allocating
    /// a new one if this (min, max) pair has not been seen before.
    pub fn enum_sentinel(&mut self, min: i32, max: i32) -> u32 {
        let idx = self.enum_map.len();
        *self.enum_map.entry((min, max)).or_insert_with(|| {
            self.enum_ranges.push((min, max));
            // index 0 → u32::MAX - 5, index 1 → u32::MAX - 6, …
            u32::MAX - 4 - 1 - idx as u32
        })
    }

    /// Total number of leaves (fixed + enum).
    pub fn num_leaves(&self) -> usize {
        NUM_FIXED_LEAVES + self.enum_ranges.len()
    }
}

fn leaf_for_field(field: &ScoringField, reg: &mut LeafRegistry) -> u32 {
    match field.kind {
        ScoringKind::Varint => LEAF_VARINT,
        ScoringKind::I64 => LEAF_I64,
        ScoringKind::LenBytes | ScoringKind::LenPacked => LEAF_LEN,
        ScoringKind::LenString => LEAF_STRING,
        ScoringKind::I32 => LEAF_I32,
        ScoringKind::Enum => {
            let (min, max) = field.enum_range.expect("Enum field must have enum_range");
            reg.enum_sentinel(min, max)
        }
        ScoringKind::LenMsg | ScoringKind::Group => {
            panic!("leaf_for_field called on message kind")
        }
    }
}

/// Attributes of a leaf node, used for Hopcroft initial partition and NodeEntry.
pub struct LeafAttrs {
    pub wire_type: u8,
    pub is_string: bool,
    /// 0xFFFF = not an enum.
    pub enum_range_idx: u16,
}

pub fn leaf_attrs(sentinel: u32, reg: &LeafRegistry) -> LeafAttrs {
    match sentinel {
        s if s == LEAF_VARINT => LeafAttrs {
            wire_type: 0,
            is_string: false,
            enum_range_idx: 0xFFFF,
        },
        s if s == LEAF_I64 => LeafAttrs {
            wire_type: 1,
            is_string: false,
            enum_range_idx: 0xFFFF,
        },
        s if s == LEAF_LEN => LeafAttrs {
            wire_type: 2,
            is_string: false,
            enum_range_idx: 0xFFFF,
        },
        s if s == LEAF_STRING => LeafAttrs {
            wire_type: 2,
            is_string: true,
            enum_range_idx: 0xFFFF,
        },
        s if s == LEAF_I32 => LeafAttrs {
            wire_type: 5,
            is_string: false,
            enum_range_idx: 0xFFFF,
        },
        s => {
            let idx = (u32::MAX - 5).wrapping_sub(s) as usize;
            assert!(idx < reg.enum_ranges.len(), "unexpected leaf sentinel {s}");
            LeafAttrs {
                wire_type: 0,
                is_string: false,
                enum_range_idx: idx as u16,
            }
        }
    }
}

// ── Raw graph ─────────────────────────────────────────────────────────────────

pub struct RawEdge {
    pub src: u32,
    pub field_number: u32,
    pub dst: u32,
    pub label: u8, // 0=optional, 1=required, 2=repeated
}

pub struct RawGraph {
    /// Maps FQDN → dense NodeId (message nodes only).
    pub node_ids: HashMap<String, u32>,
    pub edges: Vec<RawEdge>,
    /// Total node count: message nodes + all leaves.
    pub num_nodes: u32,
}

pub fn build(merged: &Merged) -> (RawGraph, LeafRegistry) {
    let mut reg = LeafRegistry::new();

    // Assign dense IDs to message nodes.
    let mut node_ids: HashMap<String, u32> = HashMap::new();
    for fqdn in merged.states.keys() {
        let id = node_ids.len() as u32;
        node_ids.entry(fqdn.clone()).or_insert(id);
    }
    // Also ensure every child FQDN referenced but not defined gets a node ID.
    for fields in merged.states.values() {
        for f in fields {
            if let Some(child) = &f.child {
                if !node_ids.contains_key(child) {
                    let id = node_ids.len() as u32;
                    node_ids.insert(child.clone(), id);
                }
            }
        }
    }

    let mut edges = Vec::new();
    for (fqdn, fields) in &merged.states {
        let src = node_ids[fqdn];
        for f in fields {
            let dst = if f.kind.is_message() {
                let child_fqdn = f.child.as_deref().unwrap();
                node_ids[child_fqdn]
            } else {
                leaf_for_field(f, &mut reg)
            };
            let label = match f.label {
                FieldLabel::Optional => 0u8,
                FieldLabel::Required => 1u8,
                FieldLabel::Repeated => 2u8,
            };
            edges.push(RawEdge {
                src,
                field_number: f.number,
                dst,
                label,
            });
        }
    }

    let msg_count = node_ids.len() as u32;
    let num_nodes = msg_count + reg.num_leaves() as u32;

    (
        RawGraph {
            node_ids,
            edges,
            num_nodes,
        },
        reg,
    )
}

// ── Compilation ───────────────────────────────────────────────────────────────

pub fn compile(
    raw: &RawGraph,
    reg: &LeafRegistry,
    partition: &Partition,
    merged: &Merged,
) -> CompiledGraph {
    let num_leaves = reg.num_leaves();
    let _num_msg_blocks = partition.num_blocks() - num_leaves;

    // ── Transition table ──────────────────────────────────────────────────────
    // Key: (src_block, field_number) → (dst_block, label)
    // label merge: most permissive wins — repeated(2) > optional(0) > required(1).
    let msg_count = raw.node_ids.len() as u32;
    let mut seen: HashMap<(u32, u32), (u32, u8)> = HashMap::new();
    for edge in &raw.edges {
        let src_block = partition.block_of(edge.src);
        // edge.dst is either a dense message node ID (< msg_count) or a leaf sentinel.
        let dst_block = if edge.dst < msg_count {
            partition.block_of(edge.dst)
        } else {
            partition.block_of_sentinel(edge.dst, reg)
        };
        seen.entry((src_block, edge.field_number))
            .and_modify(|e| {
                let rank = |l: u8| if l == 2 { 2 } else { 1 - l }; // 2→2, 0→1, 1→0
                if rank(edge.label) > rank(e.1) {
                    e.1 = edge.label;
                }
            })
            .or_insert((dst_block, edge.label));
    }
    let mut transitions: Vec<TransitionEntry> = seen
        .into_iter()
        .map(
            |((state_id, field_number), (child_state_id, label))| TransitionEntry {
                state_id,
                field_number,
                label,
                child_state_id,
            },
        )
        .collect();
    transitions.sort_by_key(|t| (t.state_id, t.field_number));

    // ── Node table ────────────────────────────────────────────────────────────
    // Message/group nodes: wire_type comes from the ScoringKind of any edge
    // pointing *to* them — all edges to the same block share the same wire_type
    // after Hopcroft.  We derive it from the merged field definitions.
    //
    // Leaf nodes: attributes come directly from LeafAttrs.

    // Map block_id → NodeEntry attributes.
    let mut node_attrs: HashMap<u32, (u8, bool, u16)> = HashMap::new();

    // Message/group node blocks: derive wire_type from the field kind.
    for fields in merged.states.values() {
        for f in fields {
            if !f.kind.is_message() {
                continue;
            }
            let child_fqdn = f.child.as_deref().unwrap();
            if let Some(&child_node) = raw.node_ids.get(child_fqdn) {
                let block = partition.block_of(child_node);
                let wt = f.kind.wire_type() as u8;
                node_attrs.entry(block).or_insert((wt, false, 0xFFFF));
            }
        }
    }
    // Message nodes that are only roots (no incoming edge) default to LEN (wire_type=2).
    for fqdn in &merged.roots {
        if let Some(&node_id) = raw.node_ids.get(fqdn) {
            let block = partition.block_of(node_id);
            node_attrs.entry(block).or_insert((2, false, 0xFFFF));
        }
    }

    // Leaf node blocks.
    let msg_count = raw.num_nodes as usize - num_leaves;
    for li in 0..num_leaves {
        let sentinel = leaf_sentinel(li, reg);
        let block = partition.block_of_sentinel(sentinel, reg);
        let attrs = leaf_attrs(sentinel, reg);
        node_attrs.insert(
            block,
            (attrs.wire_type, attrs.is_string, attrs.enum_range_idx),
        );
    }

    let mut nodes: Vec<NodeEntry> = node_attrs
        .into_iter()
        .map(
            |(state_id, (wire_type, is_string, enum_range_idx))| NodeEntry {
                state_id,
                wire_type,
                is_string,
                enum_range_idx,
            },
        )
        .collect();
    nodes.sort_by_key(|n| n.state_id);

    // ── Root entries ──────────────────────────────────────────────────────────
    let mut roots: Vec<RootEntry> = Vec::new();
    for fqdn in &merged.roots {
        if let Some(&node_id) = raw.node_ids.get(fqdn) {
            let state_id = partition.block_of(node_id);
            roots.push(RootEntry {
                fqdn: fqdn.clone(),
                state_id,
            });
        } else {
            eprintln!("warning: root entry '{fqdn}' has no node in graph; skipping");
        }
    }

    let _ = msg_count; // used only for the sentinel loop above

    CompiledGraph {
        nodes,
        transitions,
        roots,
        enum_ranges: reg.enum_ranges.clone(),
        num_states: partition.num_blocks() as u32,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the sentinel node ID for the i-th leaf (0-indexed across fixed + enum).
fn leaf_sentinel(li: usize, _reg: &LeafRegistry) -> u32 {
    match li {
        0 => LEAF_VARINT,
        1 => LEAF_I64,
        2 => LEAF_LEN,
        3 => LEAF_STRING,
        4 => LEAF_I32,
        i => u32::MAX - 4 - 1 - (i - NUM_FIXED_LEAVES) as u32,
    }
}

// ── Partition extension ───────────────────────────────────────────────────────

impl Partition {
    /// Resolve a node ID (which may be a leaf sentinel) to its block ID.
    pub fn block_of_sentinel(&self, node: u32, reg: &LeafRegistry) -> u32 {
        let msg_count = self.block_of.len() - reg.num_leaves();
        match node {
            x if x == LEAF_VARINT => self.block_of[msg_count],
            x if x == LEAF_I64 => self.block_of[msg_count + 1],
            x if x == LEAF_LEN => self.block_of[msg_count + 2],
            x if x == LEAF_STRING => self.block_of[msg_count + 3],
            x if x == LEAF_I32 => self.block_of[msg_count + 4],
            x => {
                let idx = (u32::MAX - 5).wrapping_sub(x) as usize;
                assert!(idx < reg.enum_ranges.len(), "unexpected leaf sentinel {x}");
                self.block_of[msg_count + NUM_FIXED_LEAVES + idx]
            }
        }
    }
}
