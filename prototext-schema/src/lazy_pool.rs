// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! LazyPool — on-demand FDP loading from a mmapped FDS + index.rkyv (spec 0069).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use memmap2::Mmap;
use prost::Message;
use prost_reflect::{DescriptorPool, EnumDescriptor, MessageDescriptor};
use prost_types::{FileDescriptorProto, FileDescriptorSet};
use prototext_graph::fds_index::ArchivedFdsIndex;
use rkyv::api::access_unchecked;

const MAGIC: &[u8; 8] = b"PTSGRAPH";
const VERSION: u32 = 4;

// ── LazyPool ──────────────────────────────────────────────────────────────────

/// On-demand `FileDescriptorProto` loader backed by a mmapped FDS and its
/// zero-copy rkyv index (spec 0069).
///
/// The pool starts empty (`DescriptorPool::new()`).  FDPs are decoded from the
/// mmapped raw `.pb` bytes only when a specific type is first requested.
/// When a transitive dependency is absent from the FDS (e.g. a WKT injected
/// as an embedded fallback by reproto), it is loaded from the embedded WKT
/// descriptor as a fallback — so the FDS version always takes priority.
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

    /// FDPs from the embedded WKT descriptor, keyed by filename.
    /// Used as fallback when a dep is absent from the FDS span map.
    wkt_fdps: HashMap<String, FileDescriptorProto>,

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
        return Err(format!(
            "{label}: unsupported version {version} (expected {VERSION}); \
             please re-run reproto to regenerate the index"
        )
        .into());
    }
    let root_offset = u64::from_le_bytes(bytes[16..24].try_into()?) as usize;
    Ok(root_offset)
}

// ── Constructor ───────────────────────────────────────────────────────────────

impl LazyPool {
    /// Open a lazy pool from a `.pb` FDS file and its `index.rkyv` sidecar.
    ///
    /// The pool starts empty.  Validates the PTSGRAPH header (version 3)
    /// before the rkyv pointer cast.
    ///
    /// `wkt_fallback` is an encoded `FileDescriptorSet` whose files stand in
    /// for dependencies absent from the FDS span map.  Callers that build
    /// self-contained descriptor sets (`protoc --include_imports`, which is
    /// what `reproto` emits) can pass `&[]`: the fallback branch is then
    /// unreachable, because every dependency has a span.
    pub fn open(
        pb_path: &Path,
        idx_path: &Path,
        wkt_fallback: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error>> {
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

        // Parse the embedded WKT descriptor into individual FDPs keyed by
        // filename.  These serve as fallbacks for deps absent from the FDS
        // (e.g. google/protobuf/*.proto injected by reproto as fallbacks).
        // The FDS version always takes priority: ensure_loaded checks the span
        // map first and only falls back here when no span is found.
        let wkt_fds = FileDescriptorSet::decode(wkt_fallback)
            .map_err(|e| format!("decoding embedded WKT descriptor: {e}"))?;
        let wkt_fdps: HashMap<String, FileDescriptorProto> = wkt_fds
            .file
            .into_iter()
            .map(|f| (f.name().to_owned(), f))
            .collect();

        Ok(LazyPool {
            _raw_mmap: raw_mmap,
            _idx_mmap: idx_mmap,
            index,
            raw_bytes,
            pool: DescriptorPool::new(),
            wkt_fdps,
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
        // Check the index dep_graph first; fall back to the WKT FDP's own
        // dependency list for files that are not in the index (e.g. WKTs).
        let deps: Vec<String> = if let Some(v) = self.index.dep_graph.get(file) {
            v.iter().map(|s| s.as_str().to_owned()).collect()
        } else if let Some(wkt_fdp) = self.wkt_fdps.get(file) {
            wkt_fdp
                .dependency
                .iter()
                .map(|s| s.as_str().to_owned())
                .collect()
        } else {
            vec![]
        };

        for dep in &deps {
            self.ensure_loaded(dep)?;
        }

        // Decode this FDP from the mmapped raw bytes.
        // Priority: FDS span map first; embedded WKT fallback second.
        // This mirrors protoc's -I behaviour: an explicit file in the input
        // wins over the built-in WKT copy.  Files absent from both are errors.
        let fdp = if let Some(span) = self.index.file_to_span.get(file) {
            let (start, end) = (span.0.to_native() as usize, span.1.to_native() as usize);
            FileDescriptorProto::decode(&self.raw_bytes[start..end])
                .map_err(|e| format!("decoding FDP for '{file}': {e}"))?
        } else if let Some(wkt_fdp) = self.wkt_fdps.get(file) {
            wkt_fdp.clone()
        } else {
            return Err(format!("'{file}' not found in FDS index or embedded WKT fallback").into());
        };

        self.pool
            .add_file_descriptor_proto(fdp)
            .map_err(|e| format!("adding FDP '{file}' to pool: {e}"))?;

        self.in_progress.remove(file);
        self.loaded.insert(file.to_owned());
        Ok(())
    }

    /// Resolve `fqdn` to the filename that defines it: index first, WKT fallback second.
    fn resolve_file(&self, fqdn: &str) -> Option<String> {
        if let Some(f) = self.index.type_to_file.get(fqdn) {
            return Some(f.as_str().to_owned());
        }
        self.wkt_fdps
            .iter()
            .find(|(_, fdp)| {
                let pkg = fdp.package();
                let prefixed = |name: &str| {
                    if pkg.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{pkg}.{name}")
                    }
                };
                fdp.message_type.iter().any(|m| prefixed(m.name()) == fqdn)
                    || fdp.enum_type.iter().any(|e| prefixed(e.name()) == fqdn)
            })
            .map(|(fname, _)| fname.clone())
    }

    pub fn get_message(
        &mut self,
        fqdn: &str,
    ) -> Result<Option<MessageDescriptor>, Box<dyn std::error::Error>> {
        let fqdn = fqdn.trim_start_matches('.');
        let Some(file) = self.resolve_file(fqdn) else {
            return Ok(None);
        };
        self.ensure_loaded(&file)?;
        Ok(self.pool.get_message_by_name(fqdn))
    }

    pub fn get_enum(
        &mut self,
        fqdn: &str,
    ) -> Result<Option<EnumDescriptor>, Box<dyn std::error::Error>> {
        let fqdn = fqdn.trim_start_matches('.');
        let Some(file) = self.resolve_file(fqdn) else {
            return Ok(None);
        };
        self.ensure_loaded(&file)?;
        Ok(self.pool.get_enum_by_name(fqdn))
    }

    /// JIT-load the FDP that declares an extension on `extendee_fqdn` at
    /// `field_number` (spec 0100 §5.1).  After this call,
    /// `pool.get_message_by_name(extendee_fqdn).get_extension(field_number)`
    /// will find the extension.  If the key is absent from `ext_to_file`
    /// the call is a no-op and graceful fallback applies.
    pub fn get_extension(
        &mut self,
        extendee_fqdn: &str,
        field_number: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let key = format!("{extendee_fqdn}/{field_number}");
        if let Some(file) = self.index.ext_to_file.get(key.as_str()) {
            self.ensure_loaded(file.as_str())?;
        }
        Ok(())
    }

    /// Every type name the index knows, sorted.
    ///
    /// Messages and enums, nested types included — this set is equal to the
    /// eagerly decoded pool's `all_messages() + all_enums()` (spec 0197 §5),
    /// so it can serve the override pane's lexicographic list without
    /// decoding anything.
    pub fn all_type_fqdns(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .index
            .type_to_file
            .keys()
            .map(|s| s.as_str().to_owned())
            .collect();
        names.sort_unstable();
        names
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
