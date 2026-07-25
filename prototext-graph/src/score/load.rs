// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Zero-copy loading of a CompiledGraph binary (spec 0047 §5).

use std::path::Path;

use memmap2::Mmap;
use rkyv::{access, api::access_unchecked, util::AlignedVec};

use crate::build_scoring_graph::serial::ArchivedCompiledGraph;

const MAGIC: &[u8; 8] = b"PTSGRAPH";

enum GraphBacking {
    Mmap { _mmap: Mmap },
    Aligned, // copy leaked into aligned heap allocation
}

pub struct LoadedGraph {
    _backing: GraphBacking,
    /// Zero-copy view into the backing storage.
    pub graph: &'static ArchivedCompiledGraph,
}

impl std::ops::Deref for LoadedGraph {
    type Target = ArchivedCompiledGraph;
    fn deref(&self) -> &Self::Target {
        self.graph
    }
}

fn check_header(bytes: &[u8], label: &str) -> Result<usize, Box<dyn std::error::Error>> {
    if bytes.len() < 24 {
        return Err(format!("{label}: file too short").into());
    }
    if &bytes[0..8] != MAGIC {
        return Err(format!("{label}: bad magic").into());
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into()?);
    if version != 2 {
        return Err(format!("{label}: unsupported version {version}").into());
    }
    let root_offset = u64::from_le_bytes(bytes[16..24].try_into()?) as usize;
    // Spec 0172 S4: `root_offset` is attacker-controlled. Unvalidated, it
    // made `mmap.len() - root_offset` underflow in `load_graph` and
    // `from_raw_parts` fabricate a slice of nearly `usize::MAX` bytes —
    // UB before rkyv's validator ever ran. Checking it here is what makes
    // this function's return value safe to slice with.
    //
    // The bound is deliberately just `> bytes.len()` rather than "room for
    // the archived root": the mmap path runs rkyv's checked `access` over
    // the resulting slice, which already rejects a payload too short for
    // the root, so a size check here would only duplicate that in a second
    // place that can drift out of date with the archived layout.
    if root_offset > bytes.len() {
        return Err(format!(
            "{label}: root offset {root_offset} past end of file ({} bytes)",
            bytes.len()
        )
        .into());
    }
    Ok(root_offset)
}

/// `score_all` addresses candidates by `u16` index
/// (`ActiveEntry::entries`), so a graph with more roots than that cannot
/// be scored. Rejecting at load (spec 0172 S5) is what makes the walk's
/// `debug_assert!` a genuine invariant rather than a live abort in a
/// background thread: a corpus with more than 65 535 message types is
/// input, not a programming error.
///
/// Whether 65 535 is the *right* ceiling is a separate, open question —
/// widening `ActiveEntry::entries` is deferred decision D-h
/// (`docs/protolens/rendering-worklist.md`), since `entries` is the
/// hottest structure in the walk and wants a measurement rather than an
/// assumption. This check is correct wherever that lands.
fn check_root_count(
    graph: &ArchivedCompiledGraph,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if graph.roots.len() > u16::MAX as usize {
        return Err(format!(
            "{label}: scoring graph has {} root entries, exceeding the {} the scorer can address",
            graph.roots.len(),
            u16::MAX
        )
        .into());
    }
    Ok(())
}

impl LoadedGraph {
    /// Construct a `LoadedGraph` from a `'static` byte slice (e.g. from
    /// `include_bytes!`).  Copies into a leaked `AlignedVec` so that rkyv's
    /// alignment requirements are satisfied in both debug and release builds.
    pub fn from_static_bytes(bytes: &'static [u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let root_offset = check_header(bytes, "<embedded>")?;
        // Copy into an aligned allocation so that rkyv's debug-mode alignment
        // assert passes.  include_bytes! gives only 1-byte alignment, which
        // satisfies release builds (access_unchecked skips the check) but
        // triggers a debug_assert in rkyv 0.8.x.  Leaking the AlignedVec gives
        // a 'static reference, matching the field type on LoadedGraph.
        let mut aligned = AlignedVec::<16>::new();
        aligned.extend_from_slice(&bytes[root_offset..]);
        let aligned: &'static [u8] = Box::leak(aligned.into_boxed_slice());
        // Safety: bytes were written by rkyv::to_bytes with the same types and
        // we validated magic + version above.  The buffer is now correctly
        // aligned, so access_unchecked's preconditions are satisfied.
        let graph: &'static ArchivedCompiledGraph =
            unsafe { access_unchecked::<ArchivedCompiledGraph>(aligned) };
        check_root_count(graph, "<embedded>")?;
        Ok(LoadedGraph {
            _backing: GraphBacking::Aligned,
            graph,
        })
    }
}

pub fn load_graph(path: &Path) -> Result<LoadedGraph, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;

    let root_offset = check_header(&mmap, &path.display().to_string())?;

    // Spec 0172 S4: slice safely rather than fabricating the slice from a
    // raw pointer — the bounds are now established by the slicing itself,
    // with `check_header` having already rejected an out-of-range
    // `root_offset`.
    let payload = &mmap[root_offset..];

    let graph: &'static ArchivedCompiledGraph = unsafe {
        // Safety: the only remaining unsafety is the lifetime extension.
        // `payload` borrows `mmap`, which `LoadedGraph` keeps alive for
        // exactly as long as `graph`. The bytes themselves are validated
        // by rkyv's checked `access` below.
        let payload: &'static [u8] = std::mem::transmute::<&[u8], &'static [u8]>(payload);
        access::<ArchivedCompiledGraph, rkyv::rancor::Error>(payload)
            .map_err(|e| format!("{}: rkyv access failed: {e}", path.display()))?
    };
    check_root_count(graph, &path.display().to_string())?;

    Ok(LoadedGraph {
        _backing: GraphBacking::Mmap { _mmap: mmap },
        graph,
    })
}
