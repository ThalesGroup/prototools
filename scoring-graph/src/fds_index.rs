// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! FdsIndex — zero-copy index for lazy FDS loading (spec 0068).

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use rkyv::{Archive, Deserialize, Serialize};

// ── Data type ─────────────────────────────────────────────────────────────────

/// Index over a self-contained FileDescriptorSet for lazy per-type loading.
///
/// All three maps cover every file in the FDS, including WKT files.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct FdsIndex {
    /// Fully-qualified type name (no leading dot) → proto file name.
    /// Covers top-level messages, nested messages (recursively), and enums.
    pub type_to_file: HashMap<String, String>,

    /// Proto file name → (start, end) byte offsets within the raw .pb file.
    /// `raw[start..end]` is the wire encoding of that FileDescriptorProto.
    /// u64 (not usize) for portability: rkyv archives usize as pointer-sized.
    pub file_to_span: HashMap<String, (u64, u64)>,

    /// Proto file name → list of direct import file names (FileDescriptorProto.dependency).
    ///
    /// Invariant: the FDS is self-contained (built with --include_imports),
    /// so every name in any value list also appears as a key here and has a
    /// span in file_to_span.  The runtime can recurse blindly.
    pub dep_graph: HashMap<String, Vec<String>>,
}

// ── File format constants ─────────────────────────────────────────────────────

const MAGIC: &[u8; 8] = b"PTSGRAPH";
const VERSION: u32 = 3;

// ── Writing ───────────────────────────────────────────────────────────────────

/// Serialize `index` to in-memory bytes with the PTSGRAPH header (version 3).
pub fn to_bytes(index: &FdsIndex) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let rkyv_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(index)?;

    let root_offset: u64 = 24;
    let mut buf: Vec<u8> = Vec::with_capacity(24 + rkyv_bytes.len());
    buf.write_all(MAGIC)?;
    buf.write_all(&VERSION.to_le_bytes())?;
    buf.write_all(&0u32.to_le_bytes())?; // reserved
    buf.write_all(&root_offset.to_le_bytes())?;
    buf.write_all(&rkyv_bytes)?;
    Ok(buf)
}

/// Serialize `index` to `path` with the PTSGRAPH header.
/// Returns the number of bytes written.
pub fn write(index: &FdsIndex, path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let buf = to_bytes(index)?;
    std::fs::write(path, &buf)?;
    Ok(buf.len())
}
