// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! FdsIndex — zero-copy index for lazy FDS loading (spec 0068).

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::io::Write;
use std::path::Path;

use rkyv::{Archive, Deserialize, Serialize};

// ── Data type ─────────────────────────────────────────────────────────────────

/// The `BuildHasher` for every `FdsIndex` map (spec 0177 S1).
///
/// `ArchivedHashTable::serialize_from_iter` assigns each key the *first empty*
/// slot in its probe sequence, and writes the key/value data region in
/// iteration order, so the archived bytes depend on the order the source map
/// iterates. `std::HashMap`'s default `RandomState` is seeded per process,
/// which made every build of the same input produce a different file.
///
/// A fixed seed is necessary but not sufficient — see [`canonical_map`], which
/// is the only supported way to populate these fields.
///
/// This costs nothing on the read side: the source hasher never reaches the
/// archive — `HashMap<K, V, S>` archives to `ArchivedHashMap<K, V>` with no
/// `S` — and archived lookups already use `FxHasher64` regardless. So the
/// archived layout, the `PTSGRAPH` version and lookup cost are all unchanged,
/// and pre-existing `index.rkyv` files stay readable.
pub type FxBuildHasher = BuildHasherDefault<rkyv::hash::FxHasher64>;

/// Collect `entries` into an `FdsIndex` map whose layout is a function of the
/// key set alone (spec 0177 S1).
///
/// Sorting before inserting is the other half of the fix: hashbrown resolves a
/// probe-sequence collision in favor of whichever colliding key was inserted
/// *first*, and that slot assignment is what the map's iteration order — and
/// hence the archived byte layout — is read out of. So a randomly ordered
/// source (a `HashMap` at the pyo3 boundary, or a Python `set`) still leaks
/// into the bytes even once the seed is fixed.
pub fn canonical_map<V>(
    entries: impl IntoIterator<Item = (String, V)>,
) -> HashMap<String, V, FxBuildHasher> {
    let mut sorted: Vec<(String, V)> = entries.into_iter().collect();
    sorted.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
    let mut map = HashMap::with_capacity_and_hasher(sorted.len(), FxBuildHasher::default());
    map.extend(sorted);
    map
}

/// Index over a self-contained FileDescriptorSet for lazy per-type loading.
///
/// All maps cover every file in the FDS, including WKT files.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct FdsIndex {
    /// Fully-qualified type name (no leading dot) → proto file name.
    /// Covers top-level messages, nested messages (recursively), and enums.
    pub type_to_file: HashMap<String, String, FxBuildHasher>,

    /// Proto file name → (start, end) byte offsets within the raw .pb file.
    /// `raw[start..end]` is the wire encoding of that FileDescriptorProto.
    /// u64 (not usize) for portability: rkyv archives usize as pointer-sized.
    pub file_to_span: HashMap<String, (u64, u64), FxBuildHasher>,

    /// Proto file name → list of direct import file names (FileDescriptorProto.dependency).
    ///
    /// Invariant: the FDS is self-contained (built with --include_imports),
    /// so every name in any value list also appears as a key here and has a
    /// span in file_to_span.  The runtime can recurse blindly.
    pub dep_graph: HashMap<String, Vec<String>, FxBuildHasher>,

    /// "extendee_fqdn/field_number" → proto file name.
    /// Enables O(1) JIT-loading of extension FDPs (spec 0100 §5).
    /// extendee_fqdn has no leading dot, matching prost-reflect convention.
    /// Key format matches the sentinel key used by the ANY_LOADER (spec 0100 §5.2).
    pub ext_to_file: HashMap<String, String, FxBuildHasher>,
}

// ── File format constants ─────────────────────────────────────────────────────

const MAGIC: &[u8; 8] = b"PTSGRAPH";
const VERSION: u32 = 4;

// ── Writing ───────────────────────────────────────────────────────────────────

/// Serialize `index` to in-memory bytes with the PTSGRAPH header (version 4).
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
