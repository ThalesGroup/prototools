// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Serialization of CompiledGraph to the binary file format (spec 0047 §5).

use std::io::Write;
use std::path::Path;

use rkyv::{Archive, Deserialize, Serialize};

// ── Data types ────────────────────────────────────────────────────────────────

/// One node (state) in the compiled graph.
///
/// `wire_type`: 0=UINT64, 1=I64, 2=LEN, 3=START_GROUP, 5=I32, 8=UINT32, 9=INT32.
///
/// `is_string`: true iff wire_type=2 and a UTF-8 check is required.
///
/// `range_idx`: 0xFFFF = no range (fixed leaf or non-leaf node);
///              otherwise index into `ranges` (RANGE dynamic leaf).
///
/// `ext_range_idx`: 0xFFFF = this state declares no extension ranges, i.e.
///              it is closed; otherwise an index into `ext_range_sets`
///              (spec 0238 S5). Distinct from `range_idx`, which is about
///              legal *values* of a leaf; this is about legal *field
///              numbers* of a message.
///
/// Adding `ext_range_idx` took this struct from 8 bytes to 12: the first
/// four fields pack exactly into 8 at alignment 4, leaving no hole for a
/// fifth. A `u8` index would not have helped — 9 bytes pads to 12 just the
/// same — so the width matches `range_idx` for consistency. The two
/// transition-run fields then take it to 20; on googleapis that is the node
/// table's whole cost, 0.20 MB to 0.33 MB, and it stays comfortably inside
/// L2 where its own binary search lives.
/// `trans_offset`/`trans_len` delimit this state's own run in `transitions`,
/// which is sorted by `(state_id, field_number)` and so holds every edge out
/// of one state contiguously.
///
/// Carrying the run is what lets the walk search *this state's* fields —
/// mean 5.14, median 3 on googleapis — instead of binary-searching the whole
/// 85 806-entry table for every candidate on every tag. The global search
/// was 16.48% of a protolens startup, and it missed cache on most of its ~16
/// probes because the table is 1.37 MB.
///
/// `u32` for the length rather than the `u16` the observed fanout (max 250)
/// would allow: the value is derived from a schema, and a schema is input.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct NodeEntry {
    pub state_id: u32,
    pub wire_type: u8,
    pub is_string: bool,
    pub range_idx: u16,
    pub ext_range_idx: u16,
    pub trans_offset: u32,
    pub trans_len: u32,
}

/// `NodeEntry.ext_range_idx` sentinel: this state declares no extension
/// ranges. Distinct from an *empty* range set, which cannot occur — a
/// message with no `extensions` clause gets the sentinel, not a zero-length
/// slice.
pub const NO_EXT_RANGES: u16 = 0xFFFF;

/// The extension ranges of one message, as a slice of `ext_ranges`.
///
/// Flat `(offset, len)` into a single concatenated range table rather than a
/// `Vec<Vec<_>>`, because the archived form is read through
/// `access_unchecked` and a flat slice is friendlier there (spec 0238 S8).
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct ExtRangeSet {
    pub offset: u32,
    pub len: u32,
}

/// One edge in the compiled graph, sorted by (state_id, field_number).
///
/// `state_id` is the source node.
/// `field_number` is the protobuf field number on this edge.
/// `label` is the cardinality: 0=optional, 1=required, 2=repeated.
/// `child_state_id` is the destination node.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct TransitionEntry {
    pub state_id: u32,
    pub field_number: u32,
    pub label: u8,
    /// The protobuf wire type the child expects, already normalized: the
    /// internal discriminants 8 (UINT32) and 9 (INT32) are stored as 0, and a
    /// child with no node entry as `u8::MAX`. Filled by `link_transitions`.
    ///
    /// Denormalized off the child's `NodeEntry` because the walk's verdict
    /// loop needs exactly this and nothing else about the child: it used to
    /// binary-search all 16 696 nodes for it, once per candidate per tag,
    /// which was **6.07%** of a googleapis startup. It lives in padding
    /// `TransitionEntry` already had, so the table does not grow.
    pub child_wire_type: u8,
    pub child_state_id: u32,
}

#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct RootEntry {
    pub fqdn: String,
    pub state_id: u32,
}

#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct CompiledGraph {
    /// Node table, sorted by state_id.
    pub nodes: Vec<NodeEntry>,
    /// Transition table, sorted by (state_id, field_number).
    pub transitions: Vec<TransitionEntry>,
    pub roots: Vec<RootEntry>,
    /// RANGE leaf ranges in order; NodeEntry with range_idx=i covers [ranges[i].0, ranges[i].1].
    pub ranges: Vec<(i32, i32)>,
    pub num_states: u32,
    /// Every interned extension range, concatenated; sliced by `ext_range_sets`.
    /// Inclusive at both ends, sorted by start, non-overlapping (spec 0238 S3).
    pub ext_ranges: Vec<(u32, u32)>,
    /// Interned extension range sets; `NodeEntry.ext_range_idx` indexes this.
    pub ext_range_sets: Vec<ExtRangeSet>,
    /// True iff reproto was asked to emit extension ranges. A **precondition**
    /// for the SCAN policy, not a behavior switch: SCAN treats an empty range
    /// set as "this message is closed", and a graph built without the flag has
    /// an empty set on every message, so honoring that silently would
    /// terminate on the first custom option of every descriptor and produce
    /// plausible, wrong answers (spec 0238 S9).
    pub has_extension_ranges: bool,
}

// ── File format constants ─────────────────────────────────────────────────────

const MAGIC: &[u8; 8] = b"PTSGRAPH";
/// 2 → 3: `NodeEntry.ext_range_idx`, `ext_ranges`, `ext_range_sets` and
/// `has_extension_ranges` (spec 0238 S10). Databases are build artifacts
/// regenerated by reproto, so a v2 file is rejected with a rebuild
/// instruction rather than read through a compatibility shim (spec 0238 N4).
///
/// 3 → 4: `NodeEntry.trans_offset`/`trans_len`. Same policy — the field is
/// pure derived data (it is recomputed from `transitions` by
/// `link_transitions`), but reading a v3 file would silently give every
/// state an empty transition run and so score every field as unknown, which
/// is exactly the plausible-wrong-answer failure the version check exists to
/// prevent.
///
/// 4 → 5: `TransitionEntry.child_wire_type`. Derived data again, and again
/// rejected rather than shimmed: a v4 file read as v5 would find whatever
/// byte the old padding held there and compare it against the tag's wire
/// type, so every field would be judged a match or a mismatch at random.
pub const GRAPH_VERSION: u32 = 5;

// ── Writing ───────────────────────────────────────────────────────────────────

/// Serialize `graph` to in-memory bytes in the spec 0047 §5 binary format.
pub fn to_bytes(graph: &CompiledGraph) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let rkyv_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(graph)?;

    // Fixed header: 8 magic + 4 version + 4 reserved + 8 offset = 24 bytes.
    let root_offset: u64 = 24;
    let mut buf: Vec<u8> = Vec::with_capacity(24 + rkyv_bytes.len());
    buf.write_all(MAGIC)?;
    buf.write_all(&GRAPH_VERSION.to_le_bytes())?;
    buf.write_all(&0u32.to_le_bytes())?; // reserved
    buf.write_all(&root_offset.to_le_bytes())?;
    buf.write_all(&rkyv_bytes)?;
    Ok(buf)
}

/// Serialize `graph` to `path` in the spec 0047 §5 binary format.
/// Returns the number of bytes written.
pub fn write(graph: &CompiledGraph, path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let buf = to_bytes(graph)?;
    std::fs::write(path, &buf)?;
    Ok(buf.len())
}

// ── YAML dump (spec 0059 §2) ──────────────────────────────────────────────────

/// Serialize `graph` to human-readable YAML (spec 0059 §2 format).
pub fn dump_compiled(graph: &CompiledGraph) -> String {
    let mut out = String::new();

    // states
    out.push_str("states:\n");
    for n in &graph.nodes {
        out.push_str(&format!("  - id: {}\n", n.state_id));
        let type_str = match n.wire_type {
            9 => "int32",
            8 => "uint32",
            0 if n.range_idx == 0xFFFF => "uint64",
            0 => "range",
            1 => "double",
            2 if n.is_string => "string",
            2 => "bytes",
            3 => "group",
            5 => "float",
            _ => "unknown",
        };
        out.push_str(&format!("    type: {type_str}\n"));
        if n.range_idx != 0xFFFF {
            let (min, max) = graph.ranges[n.range_idx as usize];
            out.push_str(&format!("    range: [{min}, {max}]\n"));
        }
    }

    // transitions
    out.push_str("transitions:\n");
    for t in &graph.transitions {
        out.push_str(&format!("  - from: {}\n", t.state_id));
        out.push_str(&format!("    field: {}\n", t.field_number));
        let label = match t.label {
            0 => "optional",
            1 => "required",
            2 => "repeated",
            _ => "unknown",
        };
        out.push_str(&format!("    label: {}\n", label));
        out.push_str(&format!("    to: {}\n", t.child_state_id));
    }

    // roots
    out.push_str("roots:\n");
    let mut roots: Vec<&RootEntry> = graph.roots.iter().collect();
    roots.sort_by(|a, b| a.fqdn.cmp(&b.fqdn));
    for r in roots {
        out.push_str(&format!("  - fqdn: {}\n", r.fqdn));
        out.push_str(&format!("    state: {}\n", r.state_id));
    }

    out
}
