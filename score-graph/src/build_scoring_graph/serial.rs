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
/// `wire_type` is the raw protobuf wire type this node's payload arrives on
/// (0=VARINT, 1=I64, 2=LEN, 3=START_GROUP, 5=I32).
///
/// `is_string`: true iff wire_type=2 and a UTF-8 check is required.
///
/// `enum_range_idx`: 0xFFFF = not an enum; otherwise index into `enum_ranges`.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct NodeEntry {
    pub state_id: u32,
    pub wire_type: u8,
    pub is_string: bool,
    pub enum_range_idx: u16,
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
    /// ENUM ranges in order; NodeEntry with enum_range_idx=i covers [enum_ranges[i].0, enum_ranges[i].1].
    pub enum_ranges: Vec<(i32, i32)>,
    pub num_states: u32,
}

// ── File format constants ─────────────────────────────────────────────────────

const MAGIC: &[u8; 8] = b"PTSGRAPH";
const VERSION: u32 = 2;

// ── Writing ───────────────────────────────────────────────────────────────────

/// Serialize `graph` to in-memory bytes in the spec 0047 §5 binary format.
pub fn to_bytes(graph: &CompiledGraph) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let rkyv_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(graph)?;

    // Fixed header: 8 magic + 4 version + 4 reserved + 8 offset = 24 bytes.
    let root_offset: u64 = 24;
    let mut buf: Vec<u8> = Vec::with_capacity(24 + rkyv_bytes.len());
    buf.write_all(MAGIC)?;
    buf.write_all(&VERSION.to_le_bytes())?;
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
