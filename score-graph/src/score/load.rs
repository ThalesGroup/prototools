// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Zero-copy loading of a CompiledGraph binary (spec 0047 §5).

use std::path::Path;

use memmap2::Mmap;
use rkyv::access;

use crate::build_scoring_graph::serial::ArchivedCompiledGraph;

const MAGIC: &[u8; 8] = b"PTSGRAPH";

pub struct LoadedGraph {
    /// Backing mmap; must outlive `graph`.
    _mmap: Mmap,
    /// Zero-copy view into the mmap.
    pub graph: &'static ArchivedCompiledGraph,
}

impl std::ops::Deref for LoadedGraph {
    type Target = ArchivedCompiledGraph;
    fn deref(&self) -> &Self::Target {
        self.graph
    }
}

pub fn load_graph(path: &Path) -> Result<LoadedGraph, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;

    if mmap.len() < 24 {
        return Err(format!("{}: file too short", path.display()).into());
    }
    if &mmap[0..8] != MAGIC {
        return Err(format!("{}: bad magic", path.display()).into());
    }
    let version = u32::from_le_bytes(mmap[8..12].try_into()?);
    if version != 2 {
        return Err(format!("{}: unsupported version {version}", path.display()).into());
    }
    let root_offset = u64::from_le_bytes(mmap[16..24].try_into()?) as usize;

    // Safety: the bytes were written by rkyv::to_bytes with the same types.
    // We extend the lifetime to 'static, which is safe as long as _mmap lives
    // as long as graph — enforced by keeping both in LoadedGraph.
    let graph: &'static ArchivedCompiledGraph = unsafe {
        let bytes: &'static [u8] =
            std::slice::from_raw_parts(mmap.as_ptr().add(root_offset), mmap.len() - root_offset);
        access::<ArchivedCompiledGraph, rkyv::rancor::Error>(bytes)
            .map_err(|e| format!("{}: rkyv access failed: {e}", path.display()))?
    };

    Ok(LoadedGraph { _mmap: mmap, graph })
}
