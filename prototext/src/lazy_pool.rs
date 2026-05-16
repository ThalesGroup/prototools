// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! LazyPool — on-demand FDP loading from a mmapped FDS + index.rkyv (spec 0069).

use std::collections::HashSet;
use std::path::Path;

use memmap2::Mmap;
use prost::Message;
use prost_reflect::{DescriptorPool, EnumDescriptor, MessageDescriptor};
use prost_types::FileDescriptorProto;
use rkyv::api::access_unchecked;
use score_graph_lib::fds_index::ArchivedFdsIndex;

const MAGIC: &[u8; 8] = b"PTSGRAPH";
const VERSION: u32 = 3;

// ── LazyPool ──────────────────────────────────────────────────────────────────

/// On-demand `FileDescriptorProto` loader backed by a mmapped FDS and its
/// zero-copy rkyv index (spec 0069).
///
/// The pool starts empty (`DescriptorPool::new()`).  FDPs are decoded from the
/// mmapped raw `.pb` bytes only when a specific type is first requested.
pub struct LazyPool {
    /// Mmapped raw .pb bytes (kept alive so `raw_bytes` remains valid).
    _raw_mmap: Mmap,

    /// Mmapped index.rkyv bytes (kept alive so `index` remains valid).
    _idx_mmap: Mmap,

    /// Zero-copy typed view into `_idx_backing`.
    index: &'static ArchivedFdsIndex,

    /// Raw .pb bytes as a slice (borrowed from `_raw_backing`; valid for
    /// the lifetime of this struct because `_raw_backing` is owned).
    raw_bytes: &'static [u8],

    /// The prost-reflect pool.  Starts empty (`DescriptorPool::new()`).
    pub pool: DescriptorPool,

    /// Files fully added to the pool.
    loaded: HashSet<String>,

    /// Files currently being loaded (mid-DFS); used to detect cycles.
    in_progress: HashSet<String>,
}

// ── Header validation ─────────────────────────────────────────────────────────

fn check_header(bytes: &[u8], label: &str) -> Result<usize, Box<dyn std::error::Error>> {
    if bytes.len() < 24 {
        return Err(format!("{label}: file too short for PTSGRAPH header").into());
    }
    if &bytes[0..8] != MAGIC {
        return Err(format!("{label}: bad magic (expected PTSGRAPH)").into());
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into()?);
    if version != VERSION {
        return Err(format!("{label}: unsupported version {version} (expected {VERSION})").into());
    }
    let root_offset = u64::from_le_bytes(bytes[16..24].try_into()?) as usize;
    Ok(root_offset)
}

// ── Constructor ───────────────────────────────────────────────────────────────

impl LazyPool {
    /// Open a lazy pool from a `.pb` FDS file and its `index.rkyv` sidecar.
    ///
    /// The pool starts empty (`DescriptorPool::new()`).  Validates the
    /// PTSGRAPH header (version 3) before the rkyv pointer cast.
    pub fn open(pb_path: &Path, idx_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let raw_file =
            std::fs::File::open(pb_path).map_err(|e| format!("{}: {e}", pb_path.display()))?;
        let idx_file =
            std::fs::File::open(idx_path).map_err(|e| format!("{}: {e}", idx_path.display()))?;

        // Safety: we hold the Mmap in _raw_backing and _idx_backing for the
        // lifetime of the struct; no one else mutates these files at runtime.
        let raw_mmap = unsafe { Mmap::map(&raw_file) }
            .map_err(|e| format!("{}: mmap: {e}", pb_path.display()))?;
        let idx_mmap = unsafe { Mmap::map(&idx_file) }
            .map_err(|e| format!("{}: mmap: {e}", idx_path.display()))?;

        let root_offset = check_header(&idx_mmap, &idx_path.display().to_string())?;

        // Safety: bytes were written by rkyv::to_bytes with the same types and
        // we validated magic + version above.  The Mmap is held in _idx_backing
        // for the lifetime of this struct, so the 'static cast is safe.
        let index: &'static ArchivedFdsIndex = unsafe {
            let slice: &[u8] = &*(&idx_mmap[root_offset..] as *const [u8]);
            access_unchecked::<ArchivedFdsIndex>(slice)
        };

        // Safety: the Mmap is held in _raw_backing for the lifetime of this
        // struct, so extending the slice lifetime to 'static is safe.
        let raw_bytes: &'static [u8] = unsafe { &*(&raw_mmap[..] as *const [u8]) };

        Ok(LazyPool {
            _raw_mmap: raw_mmap,
            _idx_mmap: idx_mmap,
            index,
            raw_bytes,
            pool: DescriptorPool::new(),
            loaded: HashSet::new(),
            in_progress: HashSet::new(),
        })
    }
}

// ── Resolution ────────────────────────────────────────────────────────────────

impl LazyPool {
    /// Ensure the FDP for `file` and all its transitive dependencies are loaded
    /// into the pool in topological order (deps before dependents).
    fn ensure_loaded(&mut self, file: &str) -> Result<(), Box<dyn std::error::Error>> {
        if self.loaded.contains(file) {
            return Ok(());
        }
        if self.in_progress.contains(file) {
            return Err(format!("cycle detected in FDS dependency graph: '{file}'").into());
        }

        self.in_progress.insert(file.to_owned());

        // Recurse into dependencies first (DFS).
        let deps: Vec<String> = self
            .index
            .dep_graph
            .get(file)
            .map(|v| v.iter().map(|s| s.as_str().to_owned()).collect())
            .unwrap_or_default();

        for dep in &deps {
            self.ensure_loaded(dep)?;
        }

        // Decode this FDP from the mmapped raw bytes.
        let span = self
            .index
            .file_to_span
            .get(file)
            .ok_or_else(|| format!("'{file}' has no span in index (not part of this FDS)"))?;
        let (start, end) = (span.0.to_native() as usize, span.1.to_native() as usize);
        let fdp = FileDescriptorProto::decode(&self.raw_bytes[start..end])
            .map_err(|e| format!("decoding FDP for '{file}': {e}"))?;

        self.pool
            .add_file_descriptor_proto(fdp)
            .map_err(|e| format!("adding FDP '{file}' to pool: {e}"))?;

        self.in_progress.remove(file);
        self.loaded.insert(file.to_owned());
        Ok(())
    }

    /// Ensure the FDP defining `fqdn` and all its transitive deps are in the
    /// pool, then return the `MessageDescriptor`.
    pub fn get_message(
        &mut self,
        fqdn: &str,
    ) -> Result<Option<MessageDescriptor>, Box<dyn std::error::Error>> {
        let fqdn = fqdn.trim_start_matches('.');
        let file = match self.index.type_to_file.get(fqdn) {
            Some(f) => f.as_str().to_owned(),
            None => return Ok(None),
        };
        self.ensure_loaded(&file)?;
        Ok(self.pool.get_message_by_name(fqdn))
    }

    /// Ensure the FDP defining `fqdn` and all its transitive deps are in the
    /// pool, then return the `EnumDescriptor`.
    pub fn get_enum(
        &mut self,
        fqdn: &str,
    ) -> Result<Option<EnumDescriptor>, Box<dyn std::error::Error>> {
        let fqdn = fqdn.trim_start_matches('.');
        let file = match self.index.type_to_file.get(fqdn) {
            Some(f) => f.as_str().to_owned(),
            None => return Ok(None),
        };
        self.ensure_loaded(&file)?;
        Ok(self.pool.get_enum_by_name(fqdn))
    }

    /// Load every FDP in the index into the pool.
    ///
    /// Use when the full type namespace is required (e.g. `list-schemas`).
    pub fn load_all(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let files: Vec<String> = self
            .index
            .file_to_span
            .keys()
            .map(|s| s.as_str().to_owned())
            .collect();
        for file in files {
            self.ensure_loaded(&file)?;
        }
        Ok(())
    }
}
