// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Raw graph construction and compilation to CompiledGraph.

use std::collections::HashMap;

use super::hopcroft::Partition;
use super::load::{FieldLabel, Merged, NodeKind, ScoringField, ScoringKind};
use super::serial::CompiledGraph;
use super::serial::{NodeEntry, RootEntry, TransitionEntry, NO_EXT_RANGES};

// ── Leaf sentinel node IDs ────────────────────────────────────────────────────
//
// Message/group nodes get dense IDs 0..msg_count.  Leaf nodes occupy sentinel
// IDs near u32::MAX; they are converted to stable block IDs by compile().
//
// Fixed leaves (wire_type in NodeEntry):
//   UINT64   wire_type=0, range_idx=0xFFFF   int64/uint64/sint64
//   UINT32   wire_type=8                     uint32/sint32
//   INT32    wire_type=9                     int32
//   I64      wire_type=1
//   LEN      wire_type=2, not string
//   STRING   wire_type=2, is_string=true
//   I32      wire_type=5
//
// Dynamic RANGE leaves: wire_type=0, range_idx=i; allocated by LeafRegistry.
// Sentinel layout (descending from u32::MAX):
//   u32::MAX      = LEAF_I32
//   u32::MAX - 1  = LEAF_STRING
//   u32::MAX - 2  = LEAF_LEN
//   u32::MAX - 3  = LEAF_I64
//   u32::MAX - 4  = LEAF_UINT64
//   u32::MAX - 5  = LEAF_UINT32
//   u32::MAX - 6  = LEAF_INT32
//   u32::MAX - 7  = (gap — matches pre-existing sentinel arithmetic)
//   u32::MAX - 8  = RANGE leaf 0
//   u32::MAX - 9  = RANGE leaf 1
//   ...

pub const LEAF_INT32: u32 = u32::MAX - 6;
pub const LEAF_UINT32: u32 = u32::MAX - 5;
pub const LEAF_UINT64: u32 = u32::MAX - 4;
pub const LEAF_I64: u32 = u32::MAX - 3;
pub const LEAF_LEN: u32 = u32::MAX - 2;
pub const LEAF_STRING: u32 = u32::MAX - 1;
pub const LEAF_I32: u32 = u32::MAX;

pub const NUM_FIXED_LEAVES: usize = 7;

// ── LeafRegistry ──────────────────────────────────────────────────────────────

/// Tracks the fixed scalar leaves plus dynamically allocated RANGE leaves.
pub struct LeafRegistry {
    range_map: HashMap<(i32, i32), u32>,
    pub ranges: Vec<(i32, i32)>,
}

impl LeafRegistry {
    pub fn new() -> Self {
        Self {
            range_map: HashMap::new(),
            ranges: Vec::new(),
        }
    }

    /// Return the sentinel for a RANGE leaf with the given range, allocating
    /// a new one if this (min, max) pair has not been seen before.
    pub fn range_sentinel(&mut self, min: i32, max: i32) -> u32 {
        let idx = self.range_map.len();
        *self.range_map.entry((min, max)).or_insert_with(|| {
            self.ranges.push((min, max));
            // index 0 → u32::MAX - 8, index 1 → u32::MAX - 9, …
            u32::MAX - 7 - 1 - idx as u32
        })
    }

    /// Total number of leaves (fixed + range).
    pub fn num_leaves(&self) -> usize {
        NUM_FIXED_LEAVES + self.ranges.len()
    }
}

fn leaf_for_field(field: &ScoringField, reg: &mut LeafRegistry) -> u32 {
    match field.kind {
        ScoringKind::Int32 => LEAF_INT32,
        ScoringKind::Uint32 => LEAF_UINT32,
        ScoringKind::Uint64 => LEAF_UINT64,
        ScoringKind::Range => {
            let (min, max) = field.range.expect("Range field must have range");
            reg.range_sentinel(min, max)
        }
        ScoringKind::I64 => LEAF_I64,
        ScoringKind::LenBytes => LEAF_LEN,
        ScoringKind::LenString => LEAF_STRING,
        ScoringKind::I32 => LEAF_I32,
        ScoringKind::Node => {
            panic!("leaf_for_field called on node kind")
        }
    }
}

/// Attributes of a leaf node, used for Hopcroft initial partition and NodeEntry.
pub struct LeafAttrs {
    /// Wire type stored in NodeEntry; 8=UINT32, 9=INT32 for the new varint kinds.
    pub wire_type: u8,
    pub is_string: bool,
    /// 0xFFFF = no range (fixed leaf); otherwise index into `ranges`.
    pub range_idx: u16,
}

pub fn leaf_attrs(sentinel: u32, reg: &LeafRegistry) -> LeafAttrs {
    match sentinel {
        s if s == LEAF_INT32 => LeafAttrs {
            wire_type: 9,
            is_string: false,
            range_idx: 0xFFFF,
        },
        s if s == LEAF_UINT32 => LeafAttrs {
            wire_type: 8,
            is_string: false,
            range_idx: 0xFFFF,
        },
        s if s == LEAF_UINT64 => LeafAttrs {
            wire_type: 0,
            is_string: false,
            range_idx: 0xFFFF,
        },
        s if s == LEAF_I64 => LeafAttrs {
            wire_type: 1,
            is_string: false,
            range_idx: 0xFFFF,
        },
        s if s == LEAF_LEN => LeafAttrs {
            wire_type: 2,
            is_string: false,
            range_idx: 0xFFFF,
        },
        s if s == LEAF_STRING => LeafAttrs {
            wire_type: 2,
            is_string: true,
            range_idx: 0xFFFF,
        },
        s if s == LEAF_I32 => LeafAttrs {
            wire_type: 5,
            is_string: false,
            range_idx: 0xFFFF,
        },
        s => {
            let idx = (u32::MAX - 8).wrapping_sub(s) as usize;
            assert!(idx < reg.ranges.len(), "unexpected leaf sentinel {s}");
            LeafAttrs {
                wire_type: 0,
                is_string: false,
                range_idx: idx as u16,
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
    /// Maps FQDN → dense NodeId (non-leaf nodes only).
    pub node_ids: HashMap<String, u32>,
    pub edges: Vec<RawEdge>,
    /// Total node count: non-leaf nodes + all leaves.
    pub num_nodes: u32,
    /// wire_type per non-leaf node ID (2=LENDEL, 3=GROUP).
    pub node_wire_types: HashMap<u32, u8>,
}

/// Reserved raw node ID for `google.protobuf.Any` (spec 0089 §9).
pub const ANY_NODE_ID: u32 = 0;
/// Reserved raw node ID for `google.protobuf.MessageSet` (spec 0089 §9, future).
pub const MESSAGE_SET_NODE_ID: u32 = 1;

pub fn build(merged: &Merged) -> (RawGraph, LeafRegistry) {
    let mut reg = LeafRegistry::new();

    // Pre-allocate reserved IDs for the two sentinel node types (spec 0089 §9).
    // These are inserted unconditionally so block IDs 0/1 are always reserved
    // for Any and MessageSet in the Hopcroft output.
    let mut node_ids: HashMap<String, u32> = HashMap::new();
    node_ids.insert("google.protobuf.Any".to_owned(), ANY_NODE_ID);
    node_ids.insert("google.protobuf.MessageSet".to_owned(), MESSAGE_SET_NODE_ID);

    // `merged.states` is a `HashMap`, so every loop below walks it in sorted
    // key order instead (spec 0177 S2). Iterating it directly made three
    // things differ between processes — node IDs, the order unreferenced child
    // FQDNs are discovered, and the order `LeafRegistry` first sees each
    // distinct range, which fixes `range_idx` — and each of those changes the
    // Hopcroft initial partition's labels and so the whole `hopcroft.rkyv`
    // byte layout. All three are internal, so ordering them changes nothing
    // but the bytes.
    let mut state_fqdns: Vec<&String> = merged.states.keys().collect();
    state_fqdns.sort_unstable();

    // Assign dense IDs to non-leaf nodes, starting from 2.
    let mut next_id: u32 = 2;
    for fqdn in &state_fqdns {
        node_ids.entry((*fqdn).clone()).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
    }
    // Also ensure every child FQDN referenced but not defined gets a node ID.
    for fqdn in &state_fqdns {
        for f in &merged.states[*fqdn] {
            if let Some(child) = &f.child {
                if !node_ids.contains_key(child) {
                    node_ids.insert(child.clone(), next_id);
                    next_id += 1;
                }
            }
        }
    }

    // Pre-compute wire_type per node ID from node_kinds (spec 0058).
    // This must happen before Hopcroft so the initial partition is correct.
    let mut node_wire_types: HashMap<u32, u8> = HashMap::new();
    for (fqdn, &node_id) in &node_ids {
        let wt = match merged.node_kinds.get(fqdn) {
            Some(NodeKind::Group) => 3u8,
            _ => 2u8,
        };
        node_wire_types.insert(node_id, wt);
    }

    let mut edges = Vec::new();
    for fqdn in &state_fqdns {
        let src = node_ids[*fqdn];
        for f in &merged.states[*fqdn] {
            let dst = if f.kind.is_node() {
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

    let node_count = node_ids.len() as u32;
    let num_nodes = node_count + reg.num_leaves() as u32;

    (
        RawGraph {
            node_ids,
            edges,
            num_nodes,
            node_wire_types,
        },
        reg,
    )
}

// ── Compilation ───────────────────────────────────────────────────────────────

pub fn compile(
    raw: &RawGraph,
    reg: &LeafRegistry,
    partition: &Partition,
    roots: &[String],
) -> CompiledGraph {
    let num_leaves = reg.num_leaves();
    let _num_msg_blocks = partition.num_blocks() - num_leaves;

    // ── Transition table ──────────────────────────────────────────────────────
    // After the Hopcroft fix (label part of bisimulation key), each
    // (src_block, field_number) pair has at most one label — no merge needed.
    let msg_count = raw.node_ids.len() as u32;
    let mut seen: HashMap<(u32, u32), (u32, u8)> = HashMap::new();
    for edge in &raw.edges {
        let src_block = partition.block_of(edge.src);
        let dst_block = if edge.dst < msg_count {
            partition.block_of(edge.dst)
        } else {
            partition.block_of_sentinel(edge.dst, reg)
        };
        seen.entry((src_block, edge.field_number))
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
    // Non-leaf nodes: wire_type is derived from node_wire_types, which was
    // populated before Hopcroft from node_kinds (spec 0058).  All nodes in the
    // same block have the same wire_type (guaranteed by initial partition).
    //
    // Leaf nodes: attributes come directly from LeafAttrs.

    // Map block_id → NodeEntry attributes.
    let mut node_attrs: HashMap<u32, (u8, bool, u16)> = HashMap::new();

    // Non-leaf node blocks: use pre-computed wire_types.
    for (&node_id, &wt) in &raw.node_wire_types {
        let block = partition.block_of(node_id);
        node_attrs.entry(block).or_insert((wt, false, 0xFFFF));
    }

    // Leaf node blocks.
    let msg_count = raw.num_nodes as usize - num_leaves;
    for li in 0..num_leaves {
        let sentinel = leaf_sentinel(li, reg);
        let block = partition.block_of_sentinel(sentinel, reg);
        let attrs = leaf_attrs(sentinel, reg);
        node_attrs.insert(block, (attrs.wire_type, attrs.is_string, attrs.range_idx));
    }

    let mut nodes: Vec<NodeEntry> = node_attrs
        .into_iter()
        .map(|(state_id, (wire_type, is_string, range_idx))| NodeEntry {
            state_id,
            wire_type,
            is_string,
            range_idx,
            ext_range_idx: NO_EXT_RANGES,
        })
        .collect();
    nodes.sort_by_key(|n| n.state_id);

    // ── Root entries ──────────────────────────────────────────────────────────
    let mut root_entries: Vec<RootEntry> = Vec::new();
    for fqdn in roots {
        if let Some(&node_id) = raw.node_ids.get(fqdn) {
            let state_id = partition.block_of(node_id);
            root_entries.push(RootEntry {
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
        roots: root_entries,
        ranges: reg.ranges.clone(),
        num_states: partition.num_blocks() as u32,
        ext_ranges: Vec::new(),
        ext_range_sets: Vec::new(),
        has_extension_ranges: false,
    }
}

/// Compile the raw graph (pre-Hopcroft) into a CompiledGraph using identity
/// state IDs — every raw node becomes its own state, so the result shows the
/// full unminimised structure.
pub fn compile_initial(raw: &RawGraph, reg: &LeafRegistry, roots: &[String]) -> CompiledGraph {
    let msg_count = raw.node_ids.len() as u32;
    let num_leaves = reg.num_leaves();

    // ── Transitions ───────────────────────────────────────────────────────────
    // Raw node IDs are dense 0..msg_count; leaf sentinels are remapped to
    // stable IDs msg_count..msg_count+num_leaves.
    let mut transitions: Vec<TransitionEntry> = raw
        .edges
        .iter()
        .map(|e| {
            let dst_id = if e.dst < msg_count {
                e.dst
            } else {
                msg_count + leaf_sentinel_to_index(e.dst, reg) as u32
            };
            TransitionEntry {
                state_id: e.src,
                field_number: e.field_number,
                label: e.label,
                child_state_id: dst_id,
            }
        })
        .collect();
    transitions.sort_by_key(|t| (t.state_id, t.field_number));

    // ── Nodes ─────────────────────────────────────────────────────────────────
    let mut nodes: Vec<NodeEntry> = Vec::new();

    // Non-leaf message nodes.
    for (&node_id, &wt) in &raw.node_wire_types {
        nodes.push(NodeEntry {
            state_id: node_id,
            wire_type: wt,
            is_string: false,
            range_idx: 0xFFFF,
            ext_range_idx: NO_EXT_RANGES,
        });
    }

    // Leaf nodes: use stable IDs msg_count..msg_count+num_leaves.
    for li in 0..num_leaves {
        let sentinel = leaf_sentinel(li, reg);
        let attrs = leaf_attrs(sentinel, reg);
        nodes.push(NodeEntry {
            state_id: msg_count + li as u32,
            wire_type: attrs.wire_type,
            is_string: attrs.is_string,
            range_idx: attrs.range_idx,
            ext_range_idx: NO_EXT_RANGES,
        });
    }
    nodes.sort_by_key(|n| n.state_id);

    // ── Roots ─────────────────────────────────────────────────────────────────
    let mut root_entries: Vec<RootEntry> = Vec::new();
    for fqdn in roots {
        if let Some(&node_id) = raw.node_ids.get(fqdn) {
            root_entries.push(RootEntry {
                fqdn: fqdn.clone(),
                state_id: node_id,
            });
        }
    }

    let num_states = msg_count + num_leaves as u32;
    CompiledGraph {
        nodes,
        transitions,
        roots: root_entries,
        ranges: reg.ranges.clone(),
        num_states,
        ext_ranges: Vec::new(),
        ext_range_sets: Vec::new(),
        has_extension_ranges: false,
    }
}

/// Map a leaf sentinel back to its 0-based index in the leaf slot array.
fn leaf_sentinel_to_index(sentinel: u32, reg: &LeafRegistry) -> usize {
    match sentinel {
        x if x == LEAF_UINT64 => 0,
        x if x == LEAF_UINT32 => 1,
        x if x == LEAF_INT32 => 2,
        x if x == LEAF_I64 => 3,
        x if x == LEAF_LEN => 4,
        x if x == LEAF_STRING => 5,
        x if x == LEAF_I32 => 6,
        x => {
            let idx = (u32::MAX - 8).wrapping_sub(x) as usize;
            assert!(idx < reg.ranges.len(), "unexpected leaf sentinel {x}");
            NUM_FIXED_LEAVES + idx
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the sentinel node ID for the i-th leaf (0-indexed across fixed + range).
fn leaf_sentinel(li: usize, _reg: &LeafRegistry) -> u32 {
    match li {
        0 => LEAF_UINT64,
        1 => LEAF_UINT32,
        2 => LEAF_INT32,
        3 => LEAF_I64,
        4 => LEAF_LEN,
        5 => LEAF_STRING,
        6 => LEAF_I32,
        i => u32::MAX - 7 - 1 - (i - NUM_FIXED_LEAVES) as u32,
    }
}

// ── Partition extension ───────────────────────────────────────────────────────

impl Partition {
    /// Resolve a node ID (which may be a leaf sentinel) to its block ID.
    pub fn block_of_sentinel(&self, node: u32, reg: &LeafRegistry) -> u32 {
        let msg_count = self.block_of.len() - reg.num_leaves();
        match node {
            x if x == LEAF_UINT64 => self.block_of[msg_count],
            x if x == LEAF_UINT32 => self.block_of[msg_count + 1],
            x if x == LEAF_INT32 => self.block_of[msg_count + 2],
            x if x == LEAF_I64 => self.block_of[msg_count + 3],
            x if x == LEAF_LEN => self.block_of[msg_count + 4],
            x if x == LEAF_STRING => self.block_of[msg_count + 5],
            x if x == LEAF_I32 => self.block_of[msg_count + 6],
            x => {
                let idx = (u32::MAX - 8).wrapping_sub(x) as usize;
                assert!(idx < reg.ranges.len(), "unexpected leaf sentinel {x}");
                self.block_of[msg_count + NUM_FIXED_LEAVES + idx]
            }
        }
    }
}
