// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Decode a binary protobuf blob into rendered text plus a navigation tree.
//!
//! Mirrors (simplified) `prototext`'s own `DescriptorContext` / `infer_type`
//! machinery (`prototext/src/run.rs`) — same `LazyPool`/`index.rkyv` fast
//! path (spec 0197), but no embedded-WKT-descriptor fallback: spec 0111 v1
//! always requires an explicit `--descriptor-set`.

#[cfg(test)]
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};
use prost_reflect::{Cardinality, DescriptorPool, EnumDescriptor, MessageDescriptor};
use prototext_core::serialize::render_text::NO_FQDN;
use prototext_core::serialize::render_text::{
    decode_and_render_indexed, DecodeRenderOpts, FqdnTable, NodeSpan, NO_PACKED_RECORD,
};
use prototext_core::{
    build_arena, decode_pool, render_as_bytes, set_ext_loader, Arena, ExtLoaderGuard, RenderOpts,
};
use prototext_graph::score::load::{load_graph, LoadedGraph};
use prototext_schema::LazyPool;

use crate::blob::Blob;
use crate::provenance::{ProvenanceId, NOT_RENDERED};
use crate::sweep;

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DecodeError {
    Io(String),
    Schema(String),
    Determination(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Io(msg) => write!(f, "{msg}"),
            DecodeError::Schema(msg) => write!(f, "{msg}"),
            DecodeError::Determination(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for DecodeError {}

// ── DescriptorContext ─────────────────────────────────────────────────────

/// Why the on-demand path was declined and the whole descriptor set had to
/// be decoded up front (spec 0197 §S3). Rendered as a warning on three
/// channels — stderr, the splash pane and the status line — because the
/// fallback costs the user seconds and the remedy is a `reproto` re-run.
pub struct EagerFallback {
    pub message: String,
}

/// A resolved `--descriptor-set`: a pool for type lookup plus an optional
/// Hopcroft scoring graph (`<stem>/hopcroft.rkyv` sidecar, if present).
///
/// The pool comes from one of two branches (spec 0197). With an
/// `index.rkyv` sidecar beside the descriptor, `lazy` holds a `LazyPool`
/// that decodes a file's dependency closure only when a type from it is
/// first asked for; otherwise `pool` holds the whole descriptor set,
/// decoded eagerly.
///
/// **`App` must not hold a `MessageDescriptor` or `EnumDescriptor` across
/// an event-loop iteration.** prost-reflect's `add_file_descriptor_proto`
/// uses `Arc::make_mut`, so when a JIT load forks the pool, a descriptor
/// obtained before it is blind to everything registered after
/// (`prototext/src/run.rs:648-652`). Re-fetch at each use site.
pub struct DescriptorContext {
    /// The eager branch's pool, and the schemaless empty pool.
    pool: DescriptorPool,
    /// The on-demand branch. Its own `pool` field is the live one when set.
    lazy: Option<LazyPool>,
    /// `Arc` rather than a bare `LoadedGraph` (spec 0180 S2): protolens hands
    /// the graph to two background threads, and one of them — the detached
    /// root-type sweep in `tui::run` — is deliberately never joined. An
    /// owning handle is what lets that thread outlive `App` without reading
    /// an unmapped page. Cloning it is a refcount bump, not a copy.
    pub graph: Option<Arc<LoadedGraph>>,
    /// Where the descriptor set was read from. `descriptor_set_sha256`
    /// (spec 0117 §4) re-reads and hashes it at `:save` time rather than
    /// retaining the bytes: on googleapis that is 25 MB of resident memory
    /// held all session for a value most sessions never read (spec 0197 §S6).
    source: Option<PathBuf>,
    /// Set when the on-demand path was declined; see `EagerFallback`.
    pub fallback: Option<EagerFallback>,
}

/// Keeps the extension loader (spec 0248) installed, and keeps the context it
/// points at borrowed, until dropped.
///
/// The borrow is the point: the loader reaches the context through a raw
/// pointer, and this is what makes the compiler rule out a second path to it
/// while a render is running.
pub(crate) struct ExtLoaderScope<'a> {
    _guard: ExtLoaderGuard,
    _ctx: PhantomData<&'a mut DescriptorContext>,
}

impl DescriptorContext {
    /// The live pool. On the lazy branch this is the `LazyPool`'s own pool,
    /// which starts empty and grows as types are asked for — so a caller
    /// that needs a specific type must JIT-load it first via
    /// [`message`](Self::message) / [`enumeration`](Self::enumeration)
    /// rather than reaching in here.
    pub fn pool(&self) -> &DescriptorPool {
        match &self.lazy {
            Some(lazy) => &lazy.pool,
            None => &self.pool,
        }
    }

    /// Mutable pool access — needed by `tui.rs`'s `splice_override` (spec
    /// 0118 §4) to call `register_wrapper` for an arbitrary target type,
    /// mirroring `decode()`'s own (in-module, private-field) access.
    pub(crate) fn pool_mut(&mut self) -> &mut DescriptorPool {
        match &mut self.lazy {
            Some(lazy) => &mut lazy.pool,
            None => &mut self.pool,
        }
    }

    /// Resolve a message by name, loading its file's dependency closure
    /// first on the lazy branch. A load error is a miss, not a crash —
    /// same rule as `prototext`'s `install_any_loader`.
    pub(crate) fn message(&mut self, fqdn: &str) -> Option<MessageDescriptor> {
        if let Some(lazy) = self.lazy.as_mut() {
            let _ = lazy.get_message(fqdn);
        }
        self.pool()
            .get_message_by_name(fqdn.trim_start_matches('.'))
    }

    /// Resolve an enum by name; see [`message`](Self::message).
    pub(crate) fn enumeration(&mut self, fqdn: &str) -> Option<EnumDescriptor> {
        if let Some(lazy) = self.lazy.as_mut() {
            let _ = lazy.get_enum(fqdn);
        }
        self.pool().get_enum_by_name(fqdn.trim_start_matches('.'))
    }

    /// JIT-load the file declaring an extension on `extendee` at `number`
    /// (spec 0100 §5.1, MessageSet expansion). A no-op on the eager branch,
    /// where every extension is already in the pool.
    pub(crate) fn load_extension(&mut self, extendee: &str, number: u32) {
        if let Some(lazy) = self.lazy.as_mut() {
            let _ = lazy.get_extension(extendee, number);
        }
    }

    /// Install the render-time extension loader (spec 0248) for as long as
    /// the returned scope lives.
    ///
    /// The file declaring an extension is in nobody's dependency closure, so
    /// on the lazy branch it is never loaded by resolving the extendee. Without
    /// this the field renders as unknown — numeric key, no type.
    pub(crate) fn install_ext_loader(&mut self) -> ExtLoaderScope<'_> {
        let ctx_ptr: *mut DescriptorContext = self;
        let guard = set_ext_loader(Box::new(move |extendee: &str, number: u32| {
            // SAFETY: `ExtLoaderScope` holds the `&mut self` borrow for as
            // long as the loader is installed, so nothing else can reach the
            // context through a reference while a render is calling this.
            let ctx = unsafe { &mut *ctx_ptr };
            ctx.load_extension(extendee, number);
            // The lookup must restart from the pool: `prost_reflect` adds
            // files with `Arc::make_mut`, so a descriptor obtained before the
            // load above is blind to what the load registered.
            ctx.pool()
                .get_message_by_name(extendee)
                .and_then(|ed| ed.get_extension(number))
        }));
        ExtLoaderScope {
            _guard: guard,
            _ctx: PhantomData,
        }
    }

    /// Every type name the schema knows, sorted: messages and enums,
    /// nested types included. On the lazy branch this reads `index.rkyv`'s
    /// `type_to_file` and decodes nothing; the two sources are equal sets
    /// (spec 0197 §5).
    pub(crate) fn all_type_fqdns(&self) -> Vec<String> {
        match &self.lazy {
            Some(lazy) => lazy.all_type_fqdns(),
            None => crate::override_pane::all_type_fqdns(&self.pool),
        }
    }

    /// Load a `DescriptorContext` from a `--descriptor-set` path. v1 has no
    /// schemaless/embedded-WKT fallback (spec 0111 Goal 2): the caller must
    /// always supply a path.
    pub fn load(path: &Path) -> Result<Self, DecodeError> {
        let stem = path.with_extension("");
        let rkyv_path = stem.join("hopcroft.rkyv");
        let graph = if rkyv_path.exists() {
            Some(Arc::new(load_graph(&rkyv_path).map_err(|e| {
                DecodeError::Schema(format!("loading graph '{}': {e}", rkyv_path.display()))
            })?))
        } else {
            None
        };

        let name = path.display();
        let index_path = stem.join("index.rkyv");
        let fallback = if is_prototext_descriptor(path)? {
            // `LazyPool` slices FDPs out of the mmapped file by byte offset,
            // and those offsets describe the binary wire encoding. A `#@`
            // descriptor is converted to binary in memory by
            // `read_descriptor_file`, and that buffer was never indexed.
            Some(format!(
                "'{name}' is #@ prototext — loading the whole descriptor set; \
                 a binary .pb descriptor can be loaded on demand"
            ))
        } else if !index_path.exists() {
            Some(format!(
                "no index.rkyv beside '{name}' — loading the whole descriptor set; \
                 re-run reproto to build one"
            ))
        } else {
            // A version-skewed or corrupt sidecar degrades to the eager
            // path; it is never fatal.
            match LazyPool::open(path, &index_path, &[]) {
                Ok(lazy) => {
                    return Ok(DescriptorContext {
                        pool: DescriptorPool::new(),
                        lazy: Some(lazy),
                        graph,
                        source: Some(path.to_path_buf()),
                        fallback: None,
                    })
                }
                Err(e) => Some(format!(
                    "'{}': {e} — loading the whole descriptor set; \
                     re-run reproto to regenerate it",
                    index_path.display()
                )),
            }
        };

        let bytes = read_descriptor_file(path)?;
        let pool = decode_pool(&bytes)
            .map_err(|e| DecodeError::Schema(format!("descriptor '{}': {e}", path.display())))?;

        Ok(DescriptorContext {
            pool,
            lazy: None,
            graph,
            source: Some(path.to_path_buf()),
            fallback: fallback.map(|message| EagerFallback { message }),
        })
    }

    /// SHA-256 of the canonicalized binary descriptor bytes — i.e. after
    /// `read_descriptor_file`'s `#@ prototext`-to-binary conversion, the
    /// same normalization `main.rs` applies to the target blob (spec 0114
    /// §1.1). Basis for `descriptor_set_sha256` (spec 0117 §4).
    ///
    /// Re-read from disk on demand rather than retained: `:save` is a
    /// deliberate once-per-session action, and the bytes are large
    /// (spec 0197 §S6). A schemaless context hashes the empty string.
    ///
    /// The re-read can fail — the file may have been removed or its
    /// permissions changed since startup — and that failure is an error,
    /// not an empty descriptor set. Hashing the empty string on failure
    /// would make a `:save` write a digest that matches nothing and a
    /// `:restore` warn about a mismatch that never happened.
    pub(crate) fn descriptor_sha256(&self) -> Result<String, DecodeError> {
        let bytes = match &self.source {
            Some(path) => read_descriptor_file(path)?,
            None => Vec::new(),
        };
        Ok(crate::override_pane::sha256_hex(&bytes))
    }

    /// A trivially empty pool/no-graph context — `tui.rs`'s unit tests
    /// exercise `App` state directly against synthetic `Decoded` fixtures
    /// (no real `--descriptor-set` file), and `App` now needs *some*
    /// `DescriptorContext` to hold (spec 0114 §3's candidate-list
    /// computation reads `ctx.pool()`/`ctx.graph`). `pool`/`graph` are
    /// private, so this constructor — not a struct literal — is the only
    /// way for another module's tests to build one.
    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self::schemaless()
    }

    /// A schemaless `DescriptorContext` (spec 0157 G3): empty pool, no
    /// scoring graph. Used when `--descriptor-set` is absent — the
    /// production counterpart of `empty_for_test()` (same shape, kept
    /// separate so each call site's intent stays clear).
    pub(crate) fn schemaless() -> Self {
        DescriptorContext {
            pool: DescriptorPool::new(),
            lazy: None,
            graph: None,
            source: None,
            fallback: None,
        }
    }

    /// Same as `empty_for_test`, but with a real `LoadedGraph` attached
    /// (spec 0152 test plan) — for tests that exercise the worker
    /// thread's `inferred_candidates` call against a real, tiny,
    /// in-memory scoring graph (built via `build_from_strings` +
    /// `Box::leak` + `LoadedGraph::from_static_bytes`, no file I/O).
    #[cfg(test)]
    pub(crate) fn for_test_with_graph(graph: LoadedGraph) -> Self {
        DescriptorContext {
            graph: Some(Arc::new(graph)),
            ..Self::schemaless()
        }
    }
}

/// Whether `path` holds a `#@ prototext` descriptor rather than a binary
/// one — decided from the two magic bytes, without reading the rest.
fn is_prototext_descriptor(path: &Path) -> Result<bool, DecodeError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|e| DecodeError::Io(format!("cannot read '{}': {e}", path.display())))?;
    let mut magic = [0u8; 2];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == b"#@"),
        // Shorter than two bytes: not `#@`, and the eager path will report
        // whatever is actually wrong with it.
        Err(_) => Ok(false),
    }
}

/// Read a descriptor file: accepts binary `FileDescriptorSet`, `#@` prototext
/// `FileDescriptorSet`, or a single `FileDescriptorProto` — same acceptance
/// rule as `prototext`'s own `read_descriptor_file` (prototext/src/run.rs).
/// `pub(crate)`: also reused by `complete::complete_type_names`, so `--type`
/// completion accepts the same descriptor formats as decoding itself.
pub(crate) fn read_descriptor_file(path: &Path) -> Result<Vec<u8>, DecodeError> {
    let bytes = std::fs::read(path)
        .map_err(|e| DecodeError::Io(format!("cannot read '{}': {e}", path.display())))?;
    // The prototext_core parser handles both binary and #@ prototext FDS/FDP
    // transparently via render_as_bytes — but we need raw binary FDS bytes for
    // decode_pool. If the file starts with the #@ magic, decode it first.
    if bytes.starts_with(b"#@") {
        let opts = RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
            expand_any: false,
            ..RenderOpts::default()
        };
        render_as_bytes(&bytes, opts)
            .map(|b| b.into_owned())
            .map_err(|e| {
                DecodeError::Schema(format!(
                    "decoding prototext descriptor '{}': {e}",
                    path.display()
                ))
            })
    } else {
        Ok(bytes)
    }
}

// ── Root-type determination ────────────────────────────────────────────────

/// Score-descending, non-vetoed `(fqdn, score)` pairs from one
/// `score_all` sweep — the shape `override_pane::inferred_candidates`
/// and `heat_worker::HeatCaches` both traffic in, named here so the
/// signatures that thread it through say what they carry.
pub type RankedCandidates = Vec<(String, i64)>;

/// The veto/tie-break winner-selection rule, applied to an
/// already-ranked candidate list.
///
/// `None` when there is no clean winner: no candidates at all (every one
/// of them vetoed, which `sweep::ranked` has already filtered out), or a
/// top-score tie.
///
/// Only the first two entries are ever read, which is why the ranking is
/// produced by a merge that can stop early (spec 0217 S3) rather than by
/// a sort that cannot.
pub(crate) fn pick_winner(candidates: &RankedCandidates) -> Option<String> {
    let (fqdn, top) = candidates.first()?;
    match candidates.get(1) {
        Some((_, second)) if second == top => None,
        _ => Some(fqdn.clone()),
    }
}

/// Which type the caller wants the blob decoded as — the three mutually
/// exclusive startup modes, one per command-line shape.
///
/// One enum rather than an `Option<&str>` plus a separate deferral
/// boolean: that pair makes "a named type, but also deferred"
/// expressible and meaningless, where here the three modes are
/// exhaustive and the impossible combination cannot be written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RootType<'a> {
    /// Infer it from the scoring graph (`protolens` with no type flag).
    /// Falls back to no type at all when inference can't produce a clean
    /// winner.
    #[default]
    Infer,
    /// Decode as exactly this type (`protolens --type <fqdn>`). A
    /// pool-lookup failure is a hard error — the user named it.
    Named(&'a str),
    /// Decode with no type at all (`protolens --raw`), skipping the
    /// inference sweep entirely. The escape hatch for when inference is
    /// wrong, or just too slow to wait for on a large schema database.
    Raw,
}

/// Resolve the root message type to decode `blob` as, along with the
/// ranked candidate list the `Infer` sweep produced on its way to the
/// winner.
///
/// `Named` is looked up directly in the pool; a lookup failure is a hard
/// error (the user asked for a specific type). `Raw` is `Ok(None)`
/// without touching the graph. `Infer` tries autoinference via a scoring
/// graph (`ctx.graph`), and also returns `Ok(None)` — not an error —
/// whenever inference doesn't produce a clean winner (no graph available,
/// no candidates, all candidates vetoed, or a top-score tie): the caller
/// then renders the blob with no type known (spec 0114, "protolens
/// command line should not require --type").
///
/// The candidate list is empty for `Named`/`Raw`, and for `Infer` with
/// no graph, since no sweep ran in those cases. It is returned (spec
/// 0168 G3) so startup can seed `HeatCaches` for the root range from the
/// sweep it already had to run: that range is the single most expensive
/// one in the document, and it is the one the cursor starts on, so
/// leaving the heat cue and the override pane to re-score it was paying
/// for the same sweep twice.
///
/// `meanwhile` is run on this thread while the sweep's shards walk (spec
/// 0217 S6). Only the `Infer` path has anything to overlap with; the
/// other two resolve in constant time, so `meanwhile` simply runs before
/// they return. It runs exactly once either way.
pub fn determine_root_type_meanwhile<T>(
    blob: &[u8],
    ctx: &mut DescriptorContext,
    root_type: RootType<'_>,
    jobs: usize,
    meanwhile: impl FnOnce() -> T,
) -> Result<(Option<MessageDescriptor>, RankedCandidates, T), DecodeError> {
    match root_type {
        RootType::Named(fqdn) => {
            let meanwhile = meanwhile();
            ctx.message(fqdn)
                .map(|desc| (Some(desc), Vec::new(), meanwhile))
                .ok_or_else(|| {
                    DecodeError::Determination(format!("type '{fqdn}' not found in descriptor set"))
                })
        }
        RootType::Raw => Ok((None, Vec::new(), meanwhile())),
        RootType::Infer => {
            let Some(graph) = ctx.graph.clone() else {
                return Ok((None, Vec::new(), meanwhile()));
            };
            let (candidates, meanwhile) =
                sweep::ranked_with(blob, graph.graph(), jobs, None, meanwhile);
            let desc = pick_winner(&candidates).and_then(|fqdn| ctx.message(&fqdn));
            Ok((desc, candidates, meanwhile))
        }
    }
}

// ── Navigation tree ─────────────────────────────────────────────────────────

/// Whether two *adjacent* sibling spans belong to the same packed wire
/// record (spec 0184 S1) — the single definition of the record boundary
/// that positional-path ordinals are counted over. Shared by
/// `build_tree`'s `sibling_ordinal` derivation, `render_overrides_inner`'s
/// forward ordinal counter, and `nth_child`'s resolution, so the three
/// cannot drift apart.
///
/// Note the shape: this is deliberately **not**
/// `a.packed_record_start == b.packed_record_start`. Two adjacent
/// ordinary scalars both carry `NO_PACKED_RECORD`, and that comparison
/// would fuse them into one ordinal — renumbering nearly every path in
/// nearly every document. The sentinel means "not part of a packed
/// record", never "the same record as".
pub(crate) fn same_packed_record(a: &NodeSpan, b: &NodeSpan) -> bool {
    a.packed_record_start != NO_PACKED_RECORD && a.packed_record_start == b.packed_record_start
}

/// A span's `u32` range widened to the `usize` range that indexes a slice
/// or a line vector (spec 0212 S2).
///
/// The mirror image of `prototext-core`'s own `narrow`: the library stores
/// these ranges as `u32` because a document holds millions of them, while
/// everything that *uses* one indexes with a `usize`. Widening is always
/// lossless, so unlike narrowing it needs no check — which is why this is a
/// bare cast and its counterpart is not.
#[inline]
pub(crate) fn widen(r: &Range<u32>) -> Range<usize> {
    r.start as usize..r.end as usize
}

/// A `usize` range narrowed back down to what a span stores (spec 0212 S2).
///
/// Needed where protolens *writes* a range into a span it is about to hand
/// on — re-deriving a stale `text_range` from the line counters, say. The
/// checked conversion is deliberate: `MAX_INDEXED_BUFFER` bounds every such
/// range, so an overflow here is a broken invariant rather than a large
/// document, and a silent wraparound would have a consumer reslice
/// unrelated bytes and report success.
#[inline]
pub(crate) fn narrow(r: Range<usize>) -> Range<u32> {
    let cvt = |v: usize| u32::try_from(v).expect("an offset within MAX_INDEXED_BUFFER fits a u32");
    cvt(r.start)..cvt(r.end)
}

/// How a node names another node (spec 0211 S1).
///
/// The arena itself is indexed by `usize` — it is a `Vec` — but a
/// *stored* index only ever has to span the arena, and the largest one
/// ever observed here held 4.74 M slots: a three-orders-of-magnitude
/// margin under `u32`. Spec 0211 introduced this to shrink seven
/// `Option<usize>` links to 4 bytes each; spec 0216 then deleted the
/// links outright, and what is left of the type is the arena's own
/// arrays and `build_tree`'s span-to-slot map.
///
/// Nothing bounds the arena at `NodeIdx::MAX` directly. What bounds it
/// is `MAX_INDEXED_BUFFER`: every slot covers at least a tag byte, so a
/// buffer `u32` offsets can address cannot hold more slots than a `u32`
/// can count.
pub type NodeIdx = u32;

/// No slot (spec 0211 S1).
///
/// `NodeIdx::MAX` and not `0`, because index 0 is a real node — under
/// spec 0216's level order it is in fact *the* root. An index-plus-one
/// encoding would let `Option<NonZeroU32>` carry absence for free, but
/// it would put an off-by-one at every site in exchange for nothing:
/// `NodeIdx::MAX` is not a reachable index, so spending it on the
/// sentinel costs no representable node.
pub const NO_NODE: NodeIdx = NodeIdx::MAX;

/// What the *current interpretation* says about one arena slot
/// (spec 0216 S12).
///
/// Indexed by slot, not by render order, and one per slot in the arena —
/// including the slots this interpretation does not show, which are
/// `vacant`. The structure is not here: it is the arena's, it is a
/// function of the bytes alone, and it does not change when the type
/// assignment does. Read it through `App`'s `parent`/`first_child`/
/// `next_sibling` accessors, which is where the two halves meet.
///
/// Being slot-indexed is what makes a node's preceding siblings a
/// contiguous run (S23) and its path a chain of adds (S17).
#[derive(Debug)]
pub struct TreeNode {
    /// What the render said about this slot.
    ///
    /// `raw_range` is overwritten with the slot's own range at build
    /// time, which matters for exactly one kind of node: a packed
    /// record, whose N elements collapse onto this one slot (S22) and
    /// whose individual element ranges are consequently not stored.
    /// Recovering element k means re-parsing the record's payload —
    /// the deliberate trade of S19, storing nothing the bytes already
    /// say.
    pub span: NodeSpan,
    /// Spec 0210 S1: how many rendered lines this node's whole subtree
    /// occupies, its own header and footer included. A *size*, not a
    /// position — the absolute line number is derived by summing the
    /// counts of preceding siblings up the root path (`App::
    /// absolute_start`). Storing size rather than position is what makes
    /// a commit O(depth) instead of O(nodes after the splice): a change
    /// rewrites the node and its ancestors, never its followers.
    ///
    /// **Zero means this slot is not rendered at all** under the current
    /// interpretation — the greedy walk descended into a payload this
    /// type assignment prints as a scalar. A rendered node always has at
    /// least its own header line, so the two states cannot be confused
    /// and no separate flag is needed.
    ///
    /// For a *bracketed* node (`span.is_message`) the invariant is
    /// `lines_total = 1 + Σ children + 1` — header, body, footer. For a
    /// flat one it is the node's own row count, which is 1 for an
    /// ordinary scalar and the element count for a packed record
    /// (spec 0216 S7). Either way it holds exactly, rather than
    /// approximately, because every rendered line belongs to exactly one
    /// node — which is why `IndexingTextSink::malformed` had to start
    /// emitting a span.
    ///
    /// `span.text_range` is the source of this value at build time and
    /// must not be read afterwards: a splice leaves it stale, and
    /// nothing repairs it.
    pub lines_total: u32,
    /// Spec 0210 S1: the same count with folded subtrees collapsed to
    /// their single header line — `if folded { 1 } else { 1 + Σ
    /// children + footer }`. This is what makes "the Nth visible row" a
    /// descent rather than a lookup into a 5.28 M-entry vector, and what
    /// makes folding O(depth) rather than a full rebuild.
    ///
    /// Freshly built nodes are never folded, so `build_tree` sets it
    /// equal to `lines_total`.
    pub lines_visible: u32,
    /// Which override (if any) currently produced this node's rendering,
    /// paired with the field name it was rendered under (spec 0118 §2.1,
    /// extended by spec 0119 G4) — `NOT_RENDERED` until the first
    /// `render()` pass touches it (freshly built by `build_tree`, whether
    /// from the initial raw decode or a splice's local tree). Both halves
    /// of the pair are inputs to the actual rendered text (the type via
    /// `splice_override`'s target, the name via a synthetic wrapper's
    /// field label), so either one changing must trigger a re-splice —
    /// tracking only the type here would miss a name-only change (e.g.
    /// spec 0119 G4's per-entry rename).
    ///
    /// Spec 0213: the value itself lives once in `App::provenance` and
    /// this is a 4-byte index into it. The pair is interned as a whole
    /// rather than half by half — see `provenance.rs` for why — so the
    /// three states of the type half survive intact and nothing here
    /// owns a heap allocation.
    pub rendered_as: ProvenanceId,
}

/// Spec 0211 G1, narrowed by spec 0216 S12. This size is paid once per
/// *arena* slot — 4.74 M on a large descriptor set, a few percent more
/// than the 4.5 M nodes an interpretation renders — and only once,
/// because the arena is immutable: a retype rewrites the overlay under a
/// slot but allocates none, so no superseded copy of a subtree is ever
/// left behind.
///
/// An equality rather than a bound, because growth is the regression
/// this is here to catch; a spec that legitimately moves the number
/// moves this line too.
const _: () = assert!(std::mem::size_of::<TreeNode>() == 44);

impl TreeNode {
    #[inline]
    fn unpack(idx: NodeIdx) -> Option<usize> {
        (idx != NO_NODE).then_some(idx as usize)
    }

    /// An arena slot this interpretation does not show.
    ///
    /// The greedy walk descends into every length-delimited payload
    /// (spec 0216 S2), so a blob has slots for structure no single type
    /// assignment displays — a `bytes` field whose contents happen to
    /// parse, for instance. Those slots exist, and stay vacant until
    /// some override makes them a message.
    ///
    /// The span is a placeholder and must not be read; `lines_total ==
    /// 0` is what says so.
    pub(crate) fn vacant() -> Self {
        TreeNode {
            span: NodeSpan {
                field_number: 0,
                raw_range: 0..0,
                text_range: 0..0,
                type_fqdn: NO_FQDN,
                packed_record_start: NO_PACKED_RECORD,
                level: 0,
                wire_type: 0,
                is_message: false,
            },
            lines_total: 0,
            lines_visible: 0,
            rendered_as: NOT_RENDERED,
        }
    }

    /// Whether this slot is part of the current interpretation's tree.
    #[inline]
    pub fn is_rendered(&self) -> bool {
        self.lines_total > 0
    }

    /// Whether this node is drawn as `name {` ... `}` — a distinct
    /// header and footer with its children between them.
    ///
    /// The discriminator for every line question (spec 0216 S7). A
    /// bracketed node's own lines are its first and its last, whatever
    /// its subtree does in between; a flat node's lines are simply its
    /// own, which is one row for an ordinary scalar and N for a packed
    /// record. Not `lines_total > 1`, which a collapsed packed run
    /// answers wrongly.
    #[inline]
    pub fn is_bracketed(&self) -> bool {
        self.span.is_message
    }
}

/// The arena slot each rendered span occupies, by span index
/// (spec 0216 S22).
///
/// Two linear passes and no hash map. The obvious join — index the arena
/// by `raw_start` and look each span up — costs a 4.7 M-entry table at
/// load; this instead *derives* each slot from its parent's, using the
/// one property S17 already relies on: a rendered node's k-th distinct
/// child is at `first_child[slot] + k`.
///
/// Pass 1 recovers each span's parent and its ordinal among that
/// parent's children, which is possible in one sweep because
/// `IndexingTextSink` emits post-order — a node's children are complete,
/// and in left-to-right order, by the time the node itself arrives.
/// Pass 2 then runs *backwards*, because reversed post-order visits
/// every parent before its children, which is exactly the order a
/// top-down derivation needs.
///
/// A packed run's elements share one ordinal and therefore one slot;
/// that is the many-to-one of S22, and `same_packed_record` is the same
/// predicate `build_tree` uses for the same purpose.
///
/// `root` is the slot the render's own root occupies: 0 for a whole
/// document, and the re-typed node's own slot for a splice's local
/// render. A render with several parentless spans is a packed run, whose
/// elements all belong to `root` — the only way one field's bytes can
/// produce more than one top-level span.
///
/// `NO_NODE` for a span the arena has no slot for. The maximal walk sees
/// every structure any interpretation can produce (spec 0216), so on a
/// render of real bytes this cannot happen — and [`overlay_spans`]
/// asserts that it does not. The one construction that could produce it
/// is a *byte*-budgeted preview (spec 0174), whose cut can fall inside a
/// record; no caller splices one, and spec 0249 turns on that being
/// true. See `overlay_spans`.
fn slots_for_spans(spans: &[NodeSpan], arena: &Arena, root: usize) -> Vec<u32> {
    let n = spans.len();
    let mut parent = vec![NO_NODE; n];
    let mut ordinal = vec![0u32; n];

    // Pass 1: post-order, so a node's children are the stack entries
    // deeper than it — the same claim `build_tree` makes.
    let mut stack: Vec<(usize, u16)> = Vec::new();
    let mut children: Vec<usize> = Vec::new();
    for i in 0..n {
        let level = spans[i].level;
        children.clear();
        while let Some(&(top, top_level)) = stack.last() {
            if top_level <= level {
                break;
            }
            children.push(top);
            stack.pop();
        }
        children.reverse(); // restore left-to-right document order

        let mut k = 0u32;
        let mut previous: Option<usize> = None;
        for &c in &children {
            parent[c] = i as NodeIdx;
            if let Some(p) = previous {
                if !same_packed_record(&spans[p], &spans[c]) {
                    k += 1;
                }
            }
            ordinal[c] = k;
            previous = Some(c);
        }
        stack.push((i, level));
    }

    // Pass 2: reversed post-order visits parents first.
    let first_child = arena.first_child();
    let mut slots = vec![NO_NODE; n];
    for i in (0..n).rev() {
        slots[i] = match TreeNode::unpack(parent[i]) {
            None => root as u32,
            Some(p) if slots[p] != NO_NODE => {
                let parent_slot = slots[p] as usize;
                let slot = first_child[parent_slot] + ordinal[i];
                if slot < first_child[parent_slot + 1] {
                    slot
                } else {
                    NO_NODE
                }
            }
            // The parent had no slot, so neither can anything under it.
            Some(_) => NO_NODE,
        };
    }
    slots
}

/// The maximal tree of `bytes`, for the fixtures that assemble a
/// [`Decoded`] by hand instead of going through [`render_resolved`].
#[cfg(test)]
pub(crate) fn arena_of(bytes: &[u8]) -> Arena {
    build_arena(bytes).expect("fixture blob is walkable")
}

/// Whether the arena really is a superset of what the render produced
/// (spec 0216), and if not, which span broke it.
///
/// This is the property the byte-derived arena rests on: the schema names
/// and types an occurrence but never moves a boundary, so the arena —
/// built with no schema at all — must contain every interpretation's tree.
/// A gap here is a node the reader could navigate to and the arena could
/// not address.
///
/// The check has three parts, and none of them implies the others.
///
/// 1. **Coverage.** Every span joins, by its tag byte, to a slot whose
///    byte range is its own. The tag identifies a slot uniquely because
///    `raw_start` points at it and no two occurrences share one (S19). A
///    packed element is the one span that is not a slot of its own — it
///    has no tag, so it joins on its record's and is required to fall
///    *inside* that slot rather than to equal it (S22).
///
/// 2. **Agreement.** The slot [`slots_for_spans`] *derives* for a span is
///    the slot the tag join *finds*. The two answer the same question by
///    opposite routes — one descends from the root through `first_child`
///    and the render's own nesting, the other is a byte lookup — so their
///    agreeing on every span at once says the arena nests and orders the
///    slots exactly as the render nests and orders its nodes. This is
///    what S15 puts the decomposition in one place to guarantee.
///
/// 3. **All-or-nothing.** A rendered node's children are either the whole
///    of its slot's child block or none of it, never a selection. S17's
///    `first_child[i] + step` needs this and neither of the others gives
///    it: a path step counts *rendered* children while the arithmetic
///    counts *arena* children, so a node showing 2 of its slot's 3
///    children would silently address the wrong sibling. The escape that
///    makes it hold is the scalar case — a payload the greedy walk
///    descended into but this interpretation prints as a string has zero
///    rendered children.
#[cfg(test)]
fn arena_gap(spans: &[NodeSpan], arena: &Arena) -> Option<String> {
    let (raw_start, raw_end) = (arena.raw_start(), arena.raw_end());
    let first_child = arena.first_child();
    let mut by_start: HashMap<u32, u32> = HashMap::with_capacity(arena.len());
    for (slot, &start) in raw_start.iter().enumerate() {
        by_start.insert(start, slot as u32);
    }

    let derived = slots_for_spans(spans, arena, 0);
    // How many distinct child slots each slot's rendered children use.
    let mut used = vec![0u32; arena.len()];
    let mut previous_child = vec![NO_NODE; arena.len()];

    for (i, span) in spans.iter().enumerate() {
        let packed = span.packed_record_start != NO_PACKED_RECORD;
        let tag = if packed {
            span.packed_record_start
        } else {
            span.raw_range.start
        };
        let Some(&slot) = by_start.get(&tag) else {
            return Some(format!(
                "span {i} ({:?}) has no arena slot starting at {tag}",
                span.raw_range
            ));
        };
        let (start, end) = (raw_start[slot as usize], raw_end[slot as usize]);
        let covered = if packed {
            span.raw_range.start >= start && span.raw_range.end <= end
        } else {
            span.raw_range.start == start && span.raw_range.end == end
        };
        if !covered {
            return Some(format!(
                "span {i} ({:?}) does not match arena slot {slot} ({start}..{end})",
                span.raw_range
            ));
        }
        if derived[i] != slot {
            return Some(format!(
                "span {i} ({:?}) joins to slot {slot} but derives to slot {}",
                span.raw_range, derived[i]
            ));
        }
        // Count this span against its parent's block, a packed run's N
        // elements counting once between them.
        let parent = arena.parent()[slot as usize];
        if parent != slot && previous_child[parent as usize] != slot {
            previous_child[parent as usize] = slot;
            used[parent as usize] += 1;
        }
    }

    for slot in 0..arena.len() {
        let block = first_child[slot + 1] - first_child[slot];
        if used[slot] != 0 && used[slot] != block {
            return Some(format!(
                "slot {slot} renders {} of its {block} children, so a path step \
                 would not count what the arithmetic counts",
                used[slot]
            ));
        }
    }
    None
}

/// The current interpretation's view of the arena: one entry per slot,
/// built from the spans the render emitted (spec 0216 S12).
///
/// No structure is computed here — the arena already holds it, and it
/// holds it for every interpretation at once. All this does is decide
/// which slots this rendering shows and how many lines each occupies.
///
/// A packed run's N elements land on the record's single slot (S22), so
/// their line counts add up rather than overwriting each other, and the
/// slot keeps the first element's span with the record's own byte range
/// substituted in. That range substitution is the reason a caller
/// wanting element k's bytes has to re-parse the payload: the elements'
/// individual ranges are exactly what collapsing throws away.
pub(crate) fn build_tree(
    spans: Vec<NodeSpan>,
    lines: &[String],
    arena: &Arena,
) -> (Vec<TreeNode>, Vec<Option<Box<str>>>) {
    let mut nodes: Vec<TreeNode> = (0..arena.len()).map(|_| TreeNode::vacant()).collect();
    let mut text: Vec<Option<Box<str>>> = vec![None; arena.len()];
    // The document render is never bounded — a bounded one is issued for
    // one node, at confirm (spec 0249 S5) — so there is nothing to report.
    let stopped = overlay_spans(&mut nodes, &mut text, spans, lines, arena, 0, &[]);
    debug_assert!(stopped.is_empty());
    (nodes, text)
}

/// Spec 0222 S2: the closing line of a bracketed node, derived from its
/// header.
///
/// `write_close_brace` emits an indent, one `}` and a newline — no
/// annotation and no suffix — so the whole of a footer line is a
/// function of how far its header is indented. The indent is taken from
/// the header rather than from `span.level` because the header's
/// indentation is the render's own output, while the level is the wire
/// walk's depth, and under a synthetic wrapper the two need not agree.
///
/// The single definition on purpose: `overlay_spans` asserts the
/// renderer's real footer equals this, and the TUI draws this in place
/// of a footer it no longer stores. Two copies would let the assertion
/// pass while the drawing was wrong.
pub(crate) fn derived_close(header: &str) -> String {
    let indent = header.len() - header.trim_start_matches(' ').len();
    let mut out = String::with_capacity(indent + 1);
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('}');
    out
}

/// The lines `idx`'s subtree draws, in document order — spec 0222 S1's
/// per-node text put back together.
///
/// Structural, so it lives beside the overlay it reads: the TUI's
/// export and clipboard paths want one subtree, and the tests want the
/// whole document ([`document_lines`]), but it is the same walk.
pub(crate) fn subtree_lines(
    tree: &[TreeNode],
    text: &[Option<Box<str>>],
    arena: &Arena,
    idx: usize,
) -> Vec<String> {
    let mut out = Vec::with_capacity(tree[idx].lines_total as usize);
    push_subtree_lines(tree, text, arena, idx, &mut out);
    out
}

/// [`subtree_lines`] over every root — the whole document, as the
/// `Vec<String>` spec 0222 deleted would have held it.
///
/// Test-only: production never wants the whole text at once, which is
/// the point of the spec.
#[cfg(test)]
pub(crate) fn document_lines(
    tree: &[TreeNode],
    text: &[Option<Box<str>>],
    arena: &Arena,
) -> Vec<String> {
    let parent = arena.parent();
    let roots = (0..parent.len())
        .position(|i| parent[i] != i as u32)
        .unwrap_or(parent.len());
    let mut out = Vec::new();
    for root in 0..roots.min(tree.len()) {
        push_subtree_lines(tree, text, arena, root, &mut out);
    }
    out
}

fn push_subtree_lines(
    tree: &[TreeNode],
    text: &[Option<Box<str>>],
    arena: &Arena,
    idx: usize,
    out: &mut Vec<String>,
) {
    let Some(own) = text[idx].as_deref() else {
        return;
    };
    if !tree[idx].is_bracketed() {
        out.extend(own.split('\n').map(str::to_owned));
        return;
    }
    out.push(own.to_owned());
    let first_child = arena.first_child();
    for child in first_child[idx] as usize..first_child[idx + 1] as usize {
        push_subtree_lines(tree, text, arena, child, out);
    }
    out.push(derived_close(own));
}

/// Write one render's spans into the slot-indexed overlay, rooted at
/// slot `root`.
///
/// [`build_tree`] is this over a fresh all-vacant overlay and the whole
/// document; `splice_override` is this over the live overlay and one
/// node's re-render. That there is one function rather than two is the
/// point of spec 0216: a splice does not build a tree and stitch it in,
/// because the structure it would build is already there.
///
/// The caller is responsible for vacating whatever the previous
/// interpretation of `root`'s subtree occupied — this only writes.
///
/// Spec 0222 S1: `text` is the slot-indexed store of the lines each node
/// draws *itself*, and `lines` is the render output the spans were
/// numbered against — the whole document for [`build_tree`], one
/// subtree's own render for a splice. A bracketed node keeps only its
/// header line (its footer is derived, S2); a flat one keeps all of its
/// rows joined by `\n` in a single allocation.
///
/// Spec 0249 S1: `undescended` is the render's own report of the nodes
/// it emitted without a body, as indices into `spans`; the return value
/// is the same list as slots. Both are empty unless the render was
/// row-budgeted.
pub(crate) fn overlay_spans(
    nodes: &mut [TreeNode],
    text: &mut [Option<Box<str>>],
    spans: Vec<NodeSpan>,
    lines: &[String],
    arena: &Arena,
    root: usize,
    undescended: &[u32],
) -> Vec<usize> {
    let slots = slots_for_spans(&spans, arena, root);
    let (raw_start, raw_end) = (arena.raw_start(), arena.raw_end());

    for (i, mut span) in spans.into_iter().enumerate() {
        // Spec 0249: a render the arena has no slot for is a structural
        // disagreement between the render and the maximal walk, and the
        // overlay it produces is wrong wherever it lands. Fail loudly
        // rather than dropping the span and leaving a node holding some
        // other node's text. See [`slots_for_spans`].
        assert!(
            slots[i] != NO_NODE,
            "render span {i} has no arena slot under root {root}"
        );
        let slot = slots[i] as usize;
        // Spec 0210 S1. `text_range` is exact here and only here — it is
        // the render's own line counter, read before any splice can
        // invalidate it. Taking the count directly is equivalent to
        // summing the children (every line belongs to exactly one node)
        // and is O(1) rather than a second pass.
        let own_lines = &lines[widen(&span.text_range)];
        let line_count = span.text_range.end - span.text_range.start;
        if nodes[slot].is_rendered() {
            // The second and later elements of a packed run: one more
            // row of a slot that already exists. Its rows are contiguous
            // in the render output, so they extend the one string rather
            // than starting a second (spec 0222 S1).
            nodes[slot].lines_total += line_count;
            nodes[slot].lines_visible += line_count;
            let held = text[slot]
                .take()
                .expect("a rendered slot always holds its own text");
            let mut joined = String::from(held);
            for line in own_lines {
                joined.push('\n');
                joined.push_str(line);
            }
            text[slot] = Some(joined.into_boxed_str());
            continue;
        }
        // Spec 0222 S1/S2: a bracketed node's own lines are its first
        // and its last, and the last is `indent + "}"` — derivable from
        // the first, so only the first is kept.
        text[slot] = Some(if span.is_message {
            debug_assert_eq!(
                own_lines[own_lines.len() - 1],
                derived_close(&own_lines[0]),
                "spec 0222 S2: a closing line must be its header's \
                 indentation and a brace, nothing else"
            );
            Box::from(own_lines[0].as_str())
        } else {
            own_lines.join("\n").into_boxed_str()
        });
        // Spec 0216 S19: both byte coordinates come from the arena
        // rather than from the span. That is what makes a splice's local
        // render — whose spans are numbered from the re-decoded field's
        // own first byte — need no translation at all, and it is also
        // the *only* correct source for a packed record, whose slot
        // covers the whole run rather than this one element.
        span.raw_range = raw_start[slot]..raw_end[slot];
        if span.packed_record_start != NO_PACKED_RECORD {
            span.packed_record_start = raw_start[slot];
        }
        nodes[slot] = TreeNode {
            span,
            lines_total: line_count,
            // Nothing is folded at build time.
            lines_visible: line_count,
            rendered_as: NOT_RENDERED,
        };
    }

    // Spec 0249 S1/S3: the render reports the nodes it stopped at as
    // indices into `spans`, and the caller wants them as slots.
    // Translated here rather than by the caller because `slots` is
    // derived here and `spans` is consumed above — so the report goes
    // through the same derivation, and the same assertion, as every
    // span it accompanies. Empty for every render that asked for no
    // budget, which is every render but a bounded one.
    undescended
        .iter()
        .map(|&i| slots[i as usize] as usize)
        .collect()
}

// ── Public entry point ──────────────────────────────────────────────────────

pub struct Decoded {
    /// How many lines the render emitted, for the startup progress
    /// message alone. Spec 0222 deleted the `Vec<String>` this used to
    /// be `len()` of; the live count is `App::total_lines`, derived from
    /// the roots' own counters so that a splice cannot leave it stale.
    pub total_lines: usize,
    /// Spec 0222 S1: the lines each arena slot draws itself, its
    /// children's excluded — `None` for a slot this interpretation does
    /// not render. Parallel to `tree`, and indexed the same way.
    pub node_text: Vec<Option<Box<str>>>,
    pub tree: Vec<TreeNode>,
    /// The blob's structural decomposition, derived from the bytes alone
    /// (spec 0216 S1). Unlike `tree` it does not depend on the type
    /// assignment, so it is built once here and never rebuilt.
    pub arena: Arena,
    pub root_type: String,
    /// The wrapped blob actually decoded (spec 0114 §1.1): a real tag+length
    /// prefix (field 1, `WT_LEN`) ahead of the file's own bytes, so every
    /// `NodeSpan::raw_range` in `tree` is relative to *this* blob, not to
    /// the file's own numbering.
    ///
    /// Shared rather than owned (spec 0216 S28): it is wrapped once, at
    /// load, and the heat worker reads the same bytes from another thread.
    pub blob: Arc<Blob>,
    /// Width in bytes of the wrapper's own tag+length prefix — subtract this
    /// from any `raw_range` coordinate to recover the caller's original
    /// (pre-wrap) numbering.
    pub wrapper_offset: usize,
    /// Score-descending, non-vetoed candidate types for the root's own
    /// payload range, as produced by the root-type inference sweep
    /// (spec 0168 G3). Empty whenever no sweep ran — `RootType::Named`,
    /// `RootType::Raw`, or no scoring graph. See
    /// `determine_root_type`.
    pub root_candidates: RankedCandidates,
    /// The type names every `NodeSpan::type_fqdn` in `tree` indexes into
    /// (spec 0212 S4).
    ///
    /// It is born here and lives as long as the document does, because a
    /// `FqdnId` only means anything against the table that produced it.
    /// Every later render of a *part* of this document — every override
    /// splice — must be handed this same table, or the spliced spans'
    /// ids will disagree with the arena's about what each type is. That
    /// is also why a re-decode must either keep this table or clear the
    /// render cache alongside it: cache entries hold ids from whichever
    /// table produced them.
    pub fqdns: FqdnTable,
}

#[cfg(test)]
impl Decoded {
    /// The whole rendering as lines — the `Vec<String>` this struct
    /// carried before spec 0222, rebuilt from the nodes for the tests
    /// that assert on what the document says.
    pub(crate) fn document_lines(&self) -> Vec<String> {
        document_lines(&self.tree, &self.node_text, &self.arena)
    }
}

/// Deterministic short name for a synthetic one-field wrapper descriptor
/// (spec 0135 §G2): `protolens_internal.x<32 lowercase hex chars>`, the
/// hex being the first 16 bytes (128 bits) of `SHA-256(format!(
/// "{field_number}:{type_str}:{type_name}"))`. `type_str` is `field_type.
/// as_str_name()` (prost's canonical accessor, e.g. `"TYPE_MESSAGE"`) —
/// deliberately not `{:?}` Debug formatting. Leading `x` (not `_`):
/// the `.proto` identifier grammar requires the first character to be a
/// letter. Generic over any `Type`/`type_name` pair — including
/// `Type::Enum`, though this spec never constructs that case (Non-goals).
///
/// Spec 0253 S3: `cardinality` is part of the key because this name is
/// the pool key `register_wrapper`'s early return depends on. Two
/// wrappers that differ only in their label — field 1 `optional int32`
/// and field 1 `required int32` — would otherwise collide on whichever
/// registered first, and the second node would render under the first's
/// declaration. Appended only when it is not `Optional`, so no name that
/// existed before that spec changes; the names are session-local pool
/// entries that nothing persists, so there is no compatibility question
/// either way.
fn synthetic_wrapper_name(
    field_number: u32,
    field_type: Type,
    type_name: &str,
    packed: bool,
    cardinality: Cardinality,
) -> String {
    let mut key = format!("{field_number}:{}:{type_name}", field_type.as_str_name());
    if packed {
        key.push_str(":packed");
    }
    match cardinality {
        Cardinality::Optional => {}
        Cardinality::Required => key.push_str(":required"),
        Cardinality::Repeated => key.push_str(":repeated"),
    }
    // Truncated to the first 16 bytes: this is a name-collision guard,
    // not a signature, and 32 hex characters already sit inside every
    // synthetic FQDN the user reads.
    let hex = crate::override_pane::sha256_hex(key.as_bytes());
    format!("protolens_internal.x{}", &hex[..32])
}

/// Whether protobuf allows `[packed=true]` on a field of this type —
/// every numeric scalar and `bool`/`enum`. `string`/`bytes`/`message`/
/// `group` are length-delimited already and can never be packed.
pub(crate) fn is_packable(field_type: Type) -> bool {
    matches!(
        field_type,
        Type::Double
            | Type::Float
            | Type::Int64
            | Type::Uint64
            | Type::Int32
            | Type::Fixed64
            | Type::Fixed32
            | Type::Bool
            | Type::Uint32
            | Type::Enum
            | Type::Sfixed32
            | Type::Sfixed64
            | Type::Sint32
            | Type::Sint64
    )
}

/// A wrapper-target descriptor: either a message/group FQDN target, or
/// (spec 0137 §G3) an enum FQDN target. `register_wrapper` only ever
/// needs `full_name()`/`parent_file()` from either kind, so this is a
/// thin owned enum (both `MessageDescriptor`/`EnumDescriptor` are cheap
/// to clone), not a trait object.
pub(crate) enum WrapperTarget {
    Message(MessageDescriptor),
    Enum(EnumDescriptor),
}

impl WrapperTarget {
    fn full_name(&self) -> &str {
        match self {
            WrapperTarget::Message(d) => d.full_name(),
            WrapperTarget::Enum(d) => d.full_name(),
        }
    }

    fn parent_file_name(&self) -> String {
        match self {
            WrapperTarget::Message(d) => d.parent_file().name().to_string(),
            WrapperTarget::Enum(d) => d.parent_file().name().to_string(),
        }
    }
}

impl DescriptorContext {
    /// What `register_wrapper` needs in order to build a synthetic field
    /// over `name`: the target descriptor (absent for a primitive) and
    /// the field's own type. `None` means `name` resolved as nothing at
    /// all.
    ///
    /// The ladder is ordered message → primitive keyword → enum, and
    /// that order is a rule, not a convenience: it is what makes a
    /// message named `bool` resolve as a message. It used to be written
    /// twice — once where the splice resolves the highlighted candidate,
    /// once where the pane warms the visible ones ahead of it — so a
    /// reordering would have made warming register a wrapper the splice
    /// then never looks up.
    ///
    /// `is_group` distinguishes the two message framings; the caller
    /// reads it off the node being overridden, which this does not see.
    pub(crate) fn wrapper_target_for(
        &mut self,
        name: &str,
        is_group: bool,
    ) -> Option<(Option<WrapperTarget>, Type)> {
        if let Some(desc) = self.message(name) {
            let ft = if is_group { Type::Group } else { Type::Message };
            Some((Some(WrapperTarget::Message(desc)), ft))
        } else if let Some(prim) = primitive_type_for_keyword(name) {
            Some((None, prim))
        } else {
            let enum_desc = self.enumeration(name)?;
            Some((Some(WrapperTarget::Enum(enum_desc)), Type::Enum))
        }
    }
}

/// Build (or reuse, if already registered) a synthetic one-field message
/// descriptor whose sole field `field_number` has type `field_type`
/// (message/group/primitive/enum, spec 0135 §G3, spec 0137 §G3) and,
/// for a message/group/enum target, references `target` — the virtual
/// encompassing protobuf of spec 0114 §1.1, generalized (spec 0118 §4)
/// to an arbitrary field number so `splice_override` can wrap any
/// node's own field, not just the document root (always field `1`),
/// (spec 0135 §G3) to primitive wire types, not just message/group, and
/// (spec 0137 §G3) to enum targets. The field's own name is always
/// the fixed placeholder `"_"` (spec 0135 §G2) — the real display name
/// is patched in as a post-render substring replacement, so it's no
/// longer part of the descriptor's identity. `target` is `None` for a
/// primitive target; `Some` for a message/group/enum target, supplying
/// both `type_name` (`.{fqdn}`) and the `dependency` file entry.
/// `pub(crate)`: also called from `tui.rs`'s `splice_override`.
///
/// `target` is taken *by value*, not `&WrapperTarget` — deliberately,
/// so it can be dropped (below) before `add_file_descriptor_proto` is
/// called. `target` owns a `MessageDescriptor`/`EnumDescriptor`, which
/// in turn owns its own clone of `pool`'s `Arc<DescriptorPoolInner>`;
/// leaving it alive across the mutating call would make `pool`'s own
/// `Arc` non-unique right when `add_file_descriptor_proto` needs to
/// mutate it, forcing prost-reflect's internal `Arc::make_mut` to deep-
/// clone the *entire* pool (every file/message/enum) on every
/// not-yet-seen wrapper, which is enough to make a cursor move to a new
/// override candidate visibly slow. Already-registered wrappers are
/// unaffected (the early `get_message_by_name` return above never
/// reaches the mutating call at all).
pub(crate) fn register_wrapper(
    pool: &mut DescriptorPool,
    field_number: u32,
    field_type: Type,
    target: Option<WrapperTarget>,
    packed: bool,
    cardinality: Cardinality,
) -> Result<MessageDescriptor, DecodeError> {
    let packed = packed && is_packable(field_type);
    // Spec 0253 S1: the caller owns the label, and this is the one rule
    // kept here — because it is protobuf's, not a preference. A packed
    // field must be repeated, so a caller that asks for `optional` on a
    // packed run gets `repeated` anyway. `packed` is already gated on
    // `is_packable`, so this only fires where packing is legal at all.
    let cardinality = if packed {
        Cardinality::Repeated
    } else {
        cardinality
    };
    let type_name = target.as_ref().map(|t| format!(".{}", t.full_name()));
    let full_name = synthetic_wrapper_name(
        field_number,
        field_type,
        type_name.as_deref().unwrap_or(""),
        packed,
        cardinality,
    );
    if let Some(existing) = pool.get_message_by_name(&full_name) {
        return Ok(existing);
    }
    let short_name = full_name
        .strip_prefix("protolens_internal.")
        .expect("synthetic_wrapper_name always returns a protolens_internal.-prefixed name");

    let field = FieldDescriptorProto {
        name: Some("_".to_string()),
        // Exact: every caller's field number comes from a `NodeSpan`, and
        // a tag whose field number does not fit protobuf's own 2^29 bound
        // never reaches a span — the sink reports it as malformed instead
        // (spec 0212; `render_text::sink::NodeSpan::field_number`).
        number: Some(field_number as i32),
        // Spec 0253: the node's own cardinality in its parent's schema,
        // so an override does not silently drop a `repeated`/`required`
        // qualifier the un-overridden node showed. `Required` is only
        // legal because `register_synthetic` declares the file `proto2`.
        label: Some(match cardinality {
            Cardinality::Optional => Label::Optional,
            Cardinality::Required => Label::Required,
            Cardinality::Repeated => Label::Repeated,
        } as i32),
        r#type: Some(field_type as i32),
        type_name,
        options: packed.then(|| prost_reflect::prost_types::FieldOptions {
            packed: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let dependency = target
        .as_ref()
        .map(|t| vec![t.parent_file_name()])
        .unwrap_or_default();
    // See the doc comment above: drop the last `target` handle (and
    // with it, its private clone of `pool`'s `Arc`) before mutating
    // `pool`, so the mutation below never has to deep-clone it.
    drop(target);
    register_synthetic(
        pool,
        &full_name,
        short_name,
        &format!("protolens_internal/{short_name}.proto"),
        dependency,
        vec![field],
        "wrapper",
    )
}

/// Registers a one-message file under `protolens_internal` holding
/// `fields` as `full_name`, and hands the new descriptor back.
///
/// `file_name` is a parameter rather than derived from `short_name`:
/// the MessageSet item's message is `Item` but its file is
/// `message_set_item.proto`, and renaming either would change a pool
/// key.
///
/// The "already registered?" early return stays with each caller. It is
/// what keeps a repeat wrapper off the mutating path entirely — see
/// `register_wrapper`'s doc comment for why that matters — and asking
/// here would mean building the field list before finding out.
fn register_synthetic(
    pool: &mut DescriptorPool,
    full_name: &str,
    short_name: &str,
    file_name: &str,
    dependency: Vec<String>,
    fields: Vec<FieldDescriptorProto>,
    what: &str,
) -> Result<MessageDescriptor, DecodeError> {
    let file = FileDescriptorProto {
        name: Some(file_name.to_string()),
        package: Some("protolens_internal".to_string()),
        dependency,
        syntax: Some("proto2".to_string()),
        message_type: vec![DescriptorProto {
            name: Some(short_name.to_string()),
            field: fields,
            ..Default::default()
        }],
        ..Default::default()
    };
    pool.add_file_descriptor_proto(file)
        .map_err(|e| DecodeError::Schema(format!("registering {what} descriptor: {e}")))?;
    pool.get_message_by_name(full_name)
        .ok_or_else(|| DecodeError::Schema(format!("{what} descriptor registered but not found")))
}

/// Patch `register_wrapper`'s synthetic placeholder field name (the
/// fixed literal `"_"`, spec 0135 G2) into `line`, if — and only if —
/// the render actually wrote that placeholder there.
/// `wfl_prefix_n`/`wob_prefix_n` (prototext-core) write the schema
/// field's own name only when the render resolved to a known,
/// non-mismatched field; on any wire-type mismatch they write the
/// numeric field key instead, and no placeholder is emitted anywhere
/// on the line. Detected precisely by anchoring on the exact two
/// prefix shapes both writers document — `"_: "` (scalar/value line)
/// or `"_ {"` (nested-message header line) — immediately after the
/// line's leading indentation, rather than searching the line for a
/// bare `_` character: a naive `.replacen('_', ..)` also matches the
/// `_` inside an unrelated `TYPE_MISMATCH` annotation on a mismatched
/// line, corrupting it (spec 0143). Returns `None` (caller keeps the
/// original line untouched) when no placeholder was actually written.
pub(crate) fn patch_synthetic_field_name(line: &str, field_name: &str) -> Option<String> {
    patch_field_key(line, "_", field_name)
}

/// Same substring patch as `patch_synthetic_field_name`, but for a raw
/// (schema-less) render: with no wrapper descriptor at all, there is no
/// `"_"` placeholder to begin with — the header line already shows the
/// node's own numeric field number (decoded straight off the wire tag),
/// in the same two shapes (`"N: "` scalar/value or `"N {"` nested-
/// message header). Used only when an active override entry's rename
/// (spec 0119 §G4) applies to an otherwise-raw node, so its custom name
/// shows in place of the bare field number. Returns `None` (line left
/// untouched) when the line doesn't actually start with that exact
/// field number in one of those two shapes.
pub(crate) fn patch_raw_field_name(
    line: &str,
    field_number: u64,
    field_name: &str,
) -> Option<String> {
    patch_field_key(line, &field_number.to_string(), field_name)
}

/// `line` with its leading field key rewritten to `field_name`, or
/// `None` when the key there is not exactly `key`.
///
/// The `": "` / `" {"` anchor pair is the load-bearing part, and lives
/// here so it is stated once: those are the only two shapes
/// `wfl_prefix_n`/`wob_prefix_n` write a field key in, and matching
/// `key` without them would also fire on a `key` occurring anywhere
/// else the line happens to start with it (spec 0143).
fn patch_field_key(line: &str, key: &str, field_name: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let after = rest.strip_prefix(key)?;
    if after.starts_with(": ") || after.starts_with(" {") {
        Some(format!("{indent}{field_name}{after}"))
    } else {
        None
    }
}

/// Resolve a `:type-as` primitive type keyword (spec 0135 §G3/§G4) to its
/// `Type`. Covers exactly the fifteen keywords listed in G4 — `string`/
/// `bytes` included, even though they share `WT_LEN` framing with
/// `message`/`group` targets (which are resolved separately, via FQDN
/// lookup, not through this function). Returns `None` for anything else
/// (including message FQDNs and unrecognized text), so callers can fall
/// through to their own FQDN lookup.
pub(crate) fn primitive_type_for_keyword(keyword: &str) -> Option<Type> {
    Some(match keyword {
        "int32" => Type::Int32,
        "sint32" => Type::Sint32,
        "uint32" => Type::Uint32,
        "int64" => Type::Int64,
        "sint64" => Type::Sint64,
        "uint64" => Type::Uint64,
        "fixed32" => Type::Fixed32,
        "sfixed32" => Type::Sfixed32,
        "float" => Type::Float,
        "fixed64" => Type::Fixed64,
        "sfixed64" => Type::Sfixed64,
        "double" => Type::Double,
        "bool" => Type::Bool,
        "string" => Type::String,
        "bytes" => Type::Bytes,
        _ => return None,
    })
}

/// Every primitive keyword (spec 0135 §G3/§G4) wire-compatible with
/// `wire_type` — the reverse direction of `primitive_type_for_keyword`,
/// used for `:type-as` wire-compatibility rejection and tab-completion.
/// `WT_START_GROUP` yields no primitives at all (Background): group
/// framing can never be validly reinterpreted as a primitive scalar, only
/// as a message/group FQDN target (resolved separately). `enum` is
/// deliberately absent from the `WT_VARINT` list — recorded in G3's
/// compatibility rule for a future spec, but this spec wires up no enum
/// target path anywhere (Non-goals).
pub(crate) fn primitive_keywords_for_wire_type(wire_type: u32) -> &'static [&'static str] {
    use prototext_core::helpers::{WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT};
    match wire_type {
        WT_VARINT => &[
            "int32", "int64", "uint32", "uint64", "sint32", "sint64", "bool",
        ],
        WT_I32 => &["fixed32", "sfixed32", "float"],
        WT_I64 => &["fixed64", "sfixed64", "double"],
        WT_LEN => &["string", "bytes"],
        WT_START_GROUP => &[],
        _ => &[],
    }
}

/// The wire type `span` presents to anything that reinterprets it —
/// spec 0135 §G1's rule that a packed element's own effective wire type
/// is `WT_LEN`, per its reconstructed record, not the element's decoded
/// `wire_type` field.
///
/// The single definition, because completion and validation both apply
/// it: `complete_type_as_fqdn` offers the keywords compatible with this,
/// and `type_as` rejects the keywords incompatible with it. If the two
/// spelled the rule separately and drifted, the prompt would offer a
/// candidate the commit then refuses. `export --descriptor`'s untyped-
/// field guess reads it too, for the same node.
pub(crate) fn effective_wire_type(span: &NodeSpan) -> u32 {
    if span.packed_record_start != NO_PACKED_RECORD {
        prototext_core::helpers::WT_LEN
    } else {
        u32::from(span.wire_type)
    }
}

/// Whether a synthetic field built over `span` must be declared
/// `repeated [packed=true]` rather than `optional` (spec 0219 S3): its
/// framing is a length-delimited record, which on a packable primitive
/// is the only reading that is not a wire-type mismatch. It is also what
/// lets the user ask for a packed run at all — the override pane offers
/// the element type, never `[packed=true]` itself.
///
/// Written as `effective_wire_type(span) == WT_LEN` because that is what
/// it is, not merely what it happens to equal: both say "these bytes are
/// framed as a LEN record". Spelling it out again as its own two-way
/// disjunction would put the packed-element rule in the crate a third
/// time.
///
/// Neither half of that disjunction works alone, which is why it is not
/// simply `wire_type == WT_LEN`. `packed_record_start` holds only while
/// the run is still *rendered* as a run: override it to `None` and the
/// node's span comes back from `extract.rs` with `NO_PACKED_RECORD`, so
/// by deletion time nothing recalls it was packed. And `wire_type` on a
/// live run *member* is the element's, not the record's LEN.
/// `register_wrapper` ignores this for a type protobuf cannot pack, so
/// `string`/`bytes`/message targets read the record whole as before.
pub(crate) fn packed_framing(span: &NodeSpan) -> bool {
    effective_wire_type(span) == prototext_core::helpers::WT_LEN
}

/// Every primitive keyword `primitive_type_for_keyword` recognizes,
/// alphabetically pre-sorted (spec 0137 §G1) — used by the override
/// pane's alphabetic-mode candidate list. Must stay in sync with that
/// function's match arms (the same duplication precedent
/// `primitive_keywords_for_wire_type` already accepts).
pub(crate) const ALL_PRIMITIVE_KEYWORDS: &[&str] = &[
    "bool", "bytes", "double", "fixed32", "fixed64", "float", "int32", "int64", "sfixed32",
    "sfixed64", "sint32", "sint64", "string", "uint32", "uint64",
];

/// Internal FQDN of the synthetic "Item" shape used to represent a
/// MessageSet group entry generically — `type_id` (field 2) and
/// `message` (field 3, raw bytes) — before the specific extension type
/// is known (spec 0120 §G2 tier 1). A single descriptor, globally
/// shared and registered once per pool, reused across every MessageSet
/// occurrence in the document. Genuinely nesting it under each distinct
/// MessageSet's own FQDN (e.g. `google.protobuf.MessageSet.Item`) is
/// structurally impossible: the descriptor pool (matching real
/// `protoc`) rejects a package literally equal to an already-registered
/// message's own full name, and there is no API to reopen an
/// already-loaded foreign message to append a real nested type.
/// Never shown to the user directly — `message_set_item_display_fqdn`
/// computes a friendly, MessageSet-specific label for the two places
/// this FQDN would otherwise leak into the UI (the status line, the
/// manage pane). Named `Item` (not, say, `MessageSetItem`) so that its
/// short name — `TextSink::begin_nested`'s group-header label (spec
/// 0135 G1: always the group's message type name, never the field's
/// own name) — already reads `Item {`, matching `prototext-core`'s own
/// native MessageSet rendering convention (`message_set_field.rs`'s
/// hardcoded `"Item"` virtual-node label) with no post-render header
/// patch needed (spec 0135).
pub(crate) const MESSAGE_SET_ITEM_FQDN: &str = "protolens_internal.Item";

/// The friendly, MessageSet-specific FQDN to display in place of the
/// internal, globally-shared `MESSAGE_SET_ITEM_FQDN` wherever a tier-1
/// Item node's type is shown to the user — e.g.
/// `google.protobuf.MessageSet.Item` for a MessageSet whose own
/// FQDN is `google.protobuf.MessageSet`. Display-only: never stored on
/// a tree node or an override entry, and never registered in the
/// descriptor pool (see `MESSAGE_SET_ITEM_FQDN`'s doc comment for why
/// genuine nesting isn't possible).
pub(crate) fn message_set_item_display_fqdn(message_set_fqdn: &str) -> String {
    format!("{message_set_fqdn}.Item")
}

/// Build (or reuse, if already registered) the synthetic, globally
/// shared `MESSAGE_SET_ITEM_FQDN` descriptor: `type_id: int32 = 2`,
/// `message: bytes = 3` — the generic tier-1 shape for a MessageSet
/// group entry (spec 0120 §G2). Unlike `register_wrapper`, the shape
/// itself never varies, so the descriptor is registered once per pool
/// and reused across every MessageSet occurrence in the document.
/// `pub(crate)`: also called from `tui.rs`'s `auto_expand_type`.
pub(crate) fn register_message_set_item(
    pool: &mut DescriptorPool,
) -> Result<MessageDescriptor, DecodeError> {
    if let Some(existing) = pool.get_message_by_name(MESSAGE_SET_ITEM_FQDN) {
        return Ok(existing);
    }
    let type_id_field = FieldDescriptorProto {
        name: Some("type_id".to_string()),
        number: Some(2),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Int32 as i32),
        ..Default::default()
    };
    let message_field = FieldDescriptorProto {
        name: Some("message".to_string()),
        number: Some(3),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Bytes as i32),
        ..Default::default()
    };
    register_synthetic(
        pool,
        MESSAGE_SET_ITEM_FQDN,
        "Item",
        "protolens_internal/message_set_item.proto",
        Vec::new(),
        vec![type_id_field, message_field],
        "MessageSetItem",
    )
}

/// Resolve the root type, then render the whole document under it, on
/// one thread.
///
/// Spec 0168 G1: the resolution happens before the render, always. The
/// rejected alternative is to skip the sweep, render raw, and resolve
/// the type on a background thread: that re-decodes and re-renders the
/// whole document through the splice machinery at several times the
/// cost of the decode it is refining, and swaps it under the reader.
/// The document is decoded once, as what it is. Callers who don't want
/// to pay for the sweep at all ask for `RootType::Raw` and get a raw
/// render that stays raw.
///
/// `main` does not go through here: since spec 0217 it needs the
/// resolution and the render apart, to run the arena build between them
/// and to spend the session's CPU budget on the sweep. What is left is
/// the shape every test wants — the whole decode, in one call.
#[cfg(test)]
pub fn decode(
    blob: Arc<Blob>,
    ctx: &mut DescriptorContext,
    root_type_request: RootType<'_>,
    indent_size: usize,
) -> Result<Decoded, DecodeError> {
    let (root_desc, root_candidates, arena) =
        resolve_root_type_and_arena(&blob, ctx, root_type_request, 1)?;
    render_resolved(blob, ctx, root_desc, root_candidates, arena, indent_size)
}

/// Steps 3 and 4 of spec 0217's startup sequence, run at the same time.
///
/// The arena is a function of the wrapped bytes alone (spec 0216) — it
/// does not depend on the root type, so it is handed to the sweep as its
/// `meanwhile` and runs on this thread while the shards walk. On
/// googleapis the walk is ~70 ms against a sweep measured in seconds, so
/// this hides the arena rather than the reverse: it does not make
/// startup faster, it takes the arena off startup's critical path.
///
/// Split out from [`decode`] rather than inlined because `main` needs the
/// two halves apart to announce them separately.
pub fn resolve_root_type_and_arena(
    blob: &Arc<Blob>,
    ctx: &mut DescriptorContext,
    root_type_request: RootType<'_>,
    jobs: usize,
) -> Result<(Option<MessageDescriptor>, RankedCandidates, Arena), DecodeError> {
    let (root_desc, root_candidates, arena) =
        determine_root_type_meanwhile(blob.payload(), ctx, root_type_request, jobs, || {
            // Spec 0216 S1: the maximal tree is a function of the
            // wrapped bytes, so it is built from the whole blob — slot 0
            // is the wrapper itself and the top-level occurrences are
            // its children.
            build_arena(blob.as_ref()).map_err(|e| DecodeError::Schema(e.to_string()))
        })?;
    Ok((root_desc, root_candidates, arena?))
}

/// `decode`'s second half, with the root type already resolved.
///
/// Split out for `main`, which needs to announce the two phases
/// separately: the sweep and the render are both multi-second on a large
/// blob, and a single message spanning both makes whichever one is
/// running look like a hang in the other. Every other caller wants
/// `decode`.
pub fn render_resolved(
    blob: Arc<Blob>,
    ctx: &mut DescriptorContext,
    root_desc: Option<MessageDescriptor>,
    root_candidates: RankedCandidates,
    arena: Arena,
    indent_size: usize,
) -> Result<Decoded, DecodeError> {
    let (root_type, wrapper_desc) = match &root_desc {
        Some(desc) => (
            desc.full_name().to_string(),
            Some(register_wrapper(
                ctx.pool_mut(),
                1,
                Type::Message,
                Some(WrapperTarget::Message(desc.clone())),
                false,
                // Spec 0253 N4: the document root has no parent and so
                // no cardinality to inherit.
                Cardinality::Optional,
            )?),
        ),
        None => ("<raw / no type>".to_string(), None),
    };

    let opts = DecodeRenderOpts {
        // Always on (spec 0133): annotations are now a pure main-pane
        // *display* concern (`App.annotations`/`a` key), not a
        // decode-time input — the underlying render always carries
        // full `#@ ...` annotations, which the display layer can hide
        // per line without re-decoding (see `App::annotation_start`).
        annotations: true,
        indent_size,
        // Any/MessageSet expansion is handled by protolens itself, as
        // automatic overrides (spec 0120), not by prototext-core's own
        // virtual-node expansion — disabling both here lets Any/
        // MessageSet-typed fields fall through to ordinary
        // nested-message / unknown-field rendering, giving every field
        // (including `type_url`/`type_id`) a real `NodeSpan`.
        expand_any: false,
        expand_message_set: false,
        ..Default::default()
    };
    // Spec 0212 S4: the table is created here, at the document's own
    // birth, and handed to every later sub-render of it.
    let mut fqdns = FqdnTable::new();
    // Spec 0248: `wrapper_desc` is already in hand, so the context is free to
    // be lent to the loader for the duration of the render.
    let _ext_scope = ctx.install_ext_loader();
    let rendered = decode_and_render_indexed(&blob, wrapper_desc.as_ref(), &mut fqdns, opts)
        .map_err(|e| DecodeError::Schema(e.to_string()))?;

    let text = String::from_utf8(rendered.text)
        .map_err(|e| DecodeError::Schema(format!("rendered text is not valid UTF-8: {e}")))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // Spec 0135 §G2: `register_wrapper`'s sole field is always named the
    // fixed placeholder `"_"` — patch in the real display name (the root
    // is always field `1` of the virtual encompassing message the `Blob`
    // was wrapped in, and has no schema field name of its own to show
    // instead).
    //
    // Spec 0187 S5: the text patch is all there is. No parallel
    // `style_hints` needs repairing, so nothing re-`colorize`s the line
    // on its own here — highlighting a line in isolation is exactly the
    // unsound primitive S3 exists to avoid. The patched line gets
    // highlighted in its real context when it is next drawn.
    //
    // `first()` rather than `[0]`: every current caller renders at least
    // one line, but that is a property of the callers, not of anything
    // stated or checked here, and a render that produced no text at all
    // simply has no synthetic field name to patch.
    if wrapper_desc.is_some() {
        if let Some(patched) = lines
            .first()
            .and_then(|first| patch_synthetic_field_name(first, "1"))
        {
            lines[0] = patched;
        }
    }
    // The superset property is what the whole design rests on and is not
    // checkable after the fact: `build_tree` consumes the spans, and the
    // overlay it leaves behind is already expressed in the arena's own
    // terms, so past that point it cannot disagree with it. Checking
    // here, while both halves are still in hand, is the only place it
    // means anything — and being `cfg(test)` it costs the shipped
    // binary nothing.
    #[cfg(test)]
    if let Some(gap) = arena_gap(&rendered.spans, &arena) {
        panic!("spec 0216: the arena is not a superset of the render — {gap}");
    }
    let (tree, node_text) = build_tree(rendered.spans, &lines, &arena);
    // Spec 0222, test-plan item 3. Same argument as the check above, and
    // the same only-place-it-means-anything: `lines` dies at the end of
    // this function, so a systematic off-by-one in S1's ownership split
    // — a node keeping one line too many, a footer derived from the
    // wrong header — is unfalsifiable after it. Being `cfg(test)` this
    // also reaches the `#[ignore]`d corpus harness, which is where the
    // shapes nobody wrote a fixture for live.
    #[cfg(test)]
    {
        let rebuilt = document_lines(&tree, &node_text, &arena);
        assert_eq!(
            rebuilt.len(),
            lines.len(),
            "spec 0222 S1: the nodes must own every rendered line, once"
        );
        if let Some(i) = (0..lines.len()).find(|&i| rebuilt[i] != lines[i]) {
            panic!(
                "spec 0222 S1: line {i} reassembles as {:?}, but the render \
                 emitted {:?}",
                rebuilt[i], lines[i]
            );
        }
    }
    Ok(Decoded {
        total_lines: lines.len(),
        node_text,
        tree,
        arena,
        root_type,
        wrapper_offset: blob.wrapper_offset(),
        blob,
        root_candidates,
        fqdns,
    })
}

/// A `DescriptorContext` over `fds`, with nothing left on disk.
///
/// `DescriptorContext::load` takes a path, not bytes, so a test with a
/// schema has to put one there. The guard is what makes the removal
/// safe: the hand-written form this replaced ended in
/// `remove_file(..).unwrap()`, which an assertion failing above it
/// skips, so every red run leaked a file into the temp directory. The
/// counter keeps concurrent tests off each other's path.
///
/// Sits here rather than in `tests.rs` because `extract`'s tests want it
/// too, and a sibling module cannot reach into `decode`'s private test
/// module.
#[cfg(test)]
pub(crate) fn ctx_from_fds(
    tag: &str,
    fds: &prost_reflect::prost_types::FileDescriptorSet,
) -> DescriptorContext {
    use prost::Message as _;

    struct Remove(PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("protolens-decode-{n}-{tag}.pb"));
    std::fs::write(&path, fds.encode_to_vec()).unwrap();
    let guard = Remove(path);
    DescriptorContext::load(&guard.0).unwrap()
}

#[cfg(test)]
mod tests;
