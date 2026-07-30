// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Decode a binary protobuf blob into rendered text plus a navigation tree.
//!
//! Mirrors (simplified) `prototext`'s own `DescriptorContext` / `infer_type`
//! machinery (`prototext/src/run.rs`) — same `LazyPool`/`index.rkyv` fast
//! path (spec 0197), but no embedded-WKT-descriptor fallback: spec 0111 v1
//! always requires an explicit `--descriptor-set`.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};
use prost_reflect::{DescriptorPool, EnumDescriptor, MessageDescriptor};
use prototext_core::helpers::{write_tag, write_varint, WT_LEN};
// Every `NO_FQDN` here is in a test building a literal `NodeSpan`.
#[cfg(test)]
use prototext_core::serialize::render_text::NO_FQDN;
use prototext_core::serialize::render_text::{
    decode_and_render_indexed, DecodeRenderOpts, FqdnTable, NodeSpan, NO_PACKED_RECORD,
};
use prototext_core::{decode_pool, render_as_bytes, RenderOpts};
use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{
    load::{load_graph, LoadedGraph},
    score_all, ScoringOpts,
};
use prototext_schema::LazyPool;
use sha2::{Digest, Sha256};

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
/// decoded eagerly as before.
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
    pub(crate) fn descriptor_sha256(&self) -> String {
        let bytes = match &self.source {
            Some(path) => read_descriptor_file(path).unwrap_or_default(),
            None => Vec::new(),
        };
        crate::override_pane::sha256_hex(&bytes)
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
        render_as_bytes(&bytes, opts).map_err(|e| {
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

/// The `score_all` + veto/tie-break winner-selection rule, plus the full
/// ranked list of non-vetoed candidates the same sweep already produced.
///
/// Spec 0168 G3: startup has to run this sweep anyway to know what type to
/// decode the blob as, and it scores exactly the root node's payload range
/// — the single most expensive range in the document, and the one the
/// cursor starts on. Returning the candidates alongside the winner lets
/// the caller seed `HeatCaches` with them, so neither `heat_cue_for` nor
/// the override pane re-scores those same bytes.
///
/// The winner is `None` when there's no clean one: no candidates, every
/// candidate vetoed, or a top-score tie. The candidate list is
/// score-descending and excludes vetoed entries, matching what
/// `override_pane::inferred_candidates` computes from the identical sweep.
pub(crate) fn resolve_root_winner_and_candidates(
    blob: &[u8],
    graph: &ArchivedCompiledGraph,
) -> (Option<String>, RankedCandidates) {
    let scoring_opts = ScoringOpts::default();
    let mut results = score_all(blob, graph, &scoring_opts);
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(b.fqdn)),
    });

    let candidates: RankedCandidates = results
        .iter()
        .filter(|r| !r.vetoed)
        .map(|r| (r.fqdn.to_owned(), r.score()))
        .collect();

    if candidates.is_empty() {
        return (None, candidates);
    }
    let top_score = candidates[0].1;
    let tied = candidates.iter().filter(|c| c.1 == top_score).count();
    if tied > 1 {
        return (None, candidates);
    }

    (Some(candidates[0].0.clone()), candidates)
}

/// Which type the caller wants the blob decoded as — the three mutually
/// exclusive startup modes, one per command-line shape.
///
/// This used to be an `Option<&str>` plus a separate `defer_root_type`
/// boolean, which made "a named type, but also deferred" expressible and
/// meaningless. As one enum the three modes are exhaustive and the
/// impossible combination cannot be written.
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
/// for the same `score_all` twice.
pub fn determine_root_type(
    blob: &[u8],
    ctx: &mut DescriptorContext,
    root_type: RootType<'_>,
) -> Result<(Option<MessageDescriptor>, RankedCandidates), DecodeError> {
    match root_type {
        RootType::Named(fqdn) => ctx
            .message(fqdn)
            .map(|desc| (Some(desc), Vec::new()))
            .ok_or_else(|| {
                DecodeError::Determination(format!("type '{fqdn}' not found in descriptor set"))
            }),
        RootType::Raw => Ok((None, Vec::new())),
        RootType::Infer => {
            let Some(graph) = ctx.graph.clone() else {
                return Ok((None, Vec::new()));
            };
            let (winner, candidates) = resolve_root_winner_and_candidates(blob, &graph);
            let desc = winner.and_then(|fqdn| ctx.message(&fqdn));
            Ok((desc, candidates))
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
/// *stored* index only ever has to span the arena, and the largest arena
/// ever observed here held 13 499 684 nodes at a capacity of 18 004 056:
/// a 238x margin under `u32`. Storing each link as an `Option<usize>`
/// paid 16 bytes for that (8 of payload plus 8 of discriminant using one
/// bit of them), and there are seven links, so they were 112 of the
/// slot's 272 bytes. See `docs/protolens/design/arena-and-batch.md`'s
/// annex for the whole slot.
///
/// Nothing bounds the arena at `NodeIdx::MAX` directly. What keeps a
/// document from approaching it is spec 0202's refusal, which turns down
/// any batch whose arena would not fit in half of `MemAvailable` — at
/// 184 bytes a node that binds on available memory long before it binds
/// on the index type.
pub type NodeIdx = u32;

/// The absent link (spec 0211 S1).
///
/// `NodeIdx::MAX` and not `0`, because index 0 is a real node:
/// `build_tree` emits post-order, so slot 0 holds a leaf. An
/// index-plus-one encoding would let `Option<NonZeroU32>` carry absence
/// for free, but it would put an off-by-one at every link site in
/// exchange for nothing — `NodeIdx::MAX` is not a reachable index, so
/// spending it on the sentinel costs no representable node.
pub const NO_NODE: NodeIdx = NodeIdx::MAX;

/// One node of the local arena built over the flat `Vec<NodeSpan>` returned
/// by `decode_and_render_indexed` — see spec 0111 "Tree construction
/// (ingestion)".
///
/// NOT index-parallel to document order: `IndexingTextSink` pushes a
/// container's own `NodeSpan` only in `end_nested`, i.e. *after* all of its
/// descendants — the emitted `Vec<NodeSpan>` is post-order, not pre-order.
/// (Spec 0111 originally assumed pre-order; corrected here after discovering
/// the actual `begin_nested`/`end_nested` call shape in
/// `prototext-core/src/serialize/render_text/sink.rs`.) `doc_next`/
/// `doc_prev` provide an explicit document-order chain (by `raw_range.start`)
/// for `j`/`k`, since raw array-index arithmetic no longer gives that for
/// free.
#[derive(Debug)]
pub struct TreeNode {
    pub span: NodeSpan,
    /// Spec 0211: the seven links are `NodeIdx`, with `NO_NODE` for
    /// absent. Read them through the same-named accessors below —
    /// `node.parent()` hands back the `Option<usize>` the call sites
    /// want, and the conversion compiles away. The raw fields stay
    /// public because a `TreeNode` is built by struct literal in a
    /// dozen places (it has no `Default` — see below) and those want to
    /// write `parent: NO_NODE` directly.
    pub parent: NodeIdx,
    pub first_child: NodeIdx,
    pub last_child: NodeIdx,
    pub next_sibling: NodeIdx,
    pub prev_sibling: NodeIdx,
    pub doc_next: NodeIdx,
    pub doc_prev: NodeIdx,
    /// Spec 0192 S1: this node's 1-based position among its siblings,
    /// counting a whole packed run as a single position (spec 0184 S2 —
    /// every element of a packed record shares the record's positional
    /// path). Precomputed here so `sibling_position` is a field read and
    /// `positional_path` is O(depth) rather than O(depth x sibling
    /// ordinal); see `App::positional_path` for why that matters.
    ///
    /// A derived structural fact, not a cache: it is written wherever
    /// `prev_sibling` is written, and cannot go stale independently of
    /// it.
    pub sibling_ordinal: u32,
    /// Spec 0210 S1: how many rendered lines this node's whole subtree
    /// occupies, its own header and footer included. A *size*, not a
    /// position — the absolute line number is derived by summing the
    /// counts of preceding siblings up the root path (`App::
    /// absolute_start`). Storing size rather than position is what makes
    /// a commit O(depth) instead of O(nodes after the splice): a change
    /// rewrites the node and its ancestors, never its followers.
    ///
    /// The invariant is `lines_total = 1 + Σ children + footer`, where
    /// the header is always exactly one line and the footer is one line
    /// iff `lines_total > 1`. It holds exactly (rather than
    /// approximately) because every rendered line belongs to exactly one
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
    /// extended by spec 0119 G4) — `None` until the first `render()` pass
    /// touches it (freshly built by `build_tree`, whether from the
    /// initial raw decode or a splice's local tree). Both halves of the
    /// pair are inputs to the actual rendered text (the type via
    /// `splice_override`'s target, the name via a synthetic wrapper's
    /// field label), so either one changing must trigger a re-splice —
    /// tracking only the type here would miss a name-only change (e.g.
    /// spec 0119 G4's per-entry rename).
    pub rendered_as: Option<(Option<Option<String>>, String)>,
}

/// Spec 0211 G1. The slot is paid 4.5 M times over on a large descriptor
/// set, and four times over during a commit — once for the live arena,
/// once for the superseded half a batch leaves behind, once for
/// `local_tree`, and once more if the render cache holds a copy. Read
/// `docs/protolens/design/arena-and-batch.md`'s annex before changing
/// it. An equality rather than a bound, because growth is the
/// regression this is here to catch; a spec that legitimately moves the
/// number moves this line too.
const _: () = assert!(std::mem::size_of::<TreeNode>() == 120);

impl TreeNode {
    /// Spec 0211 S2: pack an optional index into the stored
    /// representation. The `debug_assert` documents the ceiling rather
    /// than enforcing it — see `NodeIdx` for what actually bounds the
    /// arena.
    ///
    /// Public because a `TreeNode` is built by struct literal in a dozen
    /// places, so a builder needs to name a link's stored form; the
    /// setters below are how to change a link that already exists.
    #[inline]
    pub fn pack(idx: Option<usize>) -> NodeIdx {
        match idx {
            Some(i) => {
                debug_assert!(
                    i < NO_NODE as usize,
                    "node index {i} does not fit in a NodeIdx"
                );
                i as NodeIdx
            }
            None => NO_NODE,
        }
    }

    #[inline]
    fn unpack(idx: NodeIdx) -> Option<usize> {
        (idx != NO_NODE).then_some(idx as usize)
    }
}

/// Spec 0211 S2: one `fn name(&self) -> Option<usize>` and one
/// `fn set_name(&mut self, Option<usize>)` per link. A method may share
/// a field's name in Rust, so the call sites read `node.parent()` and the
/// struct literals still write `parent: NO_NODE` — no site had to invent
/// a new name for a link.
macro_rules! link_accessors {
    ($($field:ident, $setter:ident;)*) => {
        impl TreeNode {
            $(
                #[inline]
                pub fn $field(&self) -> Option<usize> {
                    Self::unpack(self.$field)
                }

                #[inline]
                pub fn $setter(&mut self, idx: Option<usize>) {
                    self.$field = Self::pack(idx);
                }
            )*
        }
    };
}

link_accessors! {
    parent, set_parent;
    first_child, set_first_child;
    last_child, set_last_child;
    next_sibling, set_next_sibling;
    prev_sibling, set_prev_sibling;
    doc_next, set_doc_next;
    doc_prev, set_doc_prev;
}

/// Build the navigation arena from a flat, level-annotated, post-order
/// `Vec<NodeSpan>` in a single `O(n)` pass, using a stack of "subtree roots
/// completed so far" (spec 0111 "Tree construction (ingestion)").
///
/// For each incoming node `i` at `level`, every stack entry with a greater
/// level is one of `i`'s children (post-order guarantees they were fully
/// built already) — pop them all, in left-to-right order, as `i`'s children.
/// The (now topmost) remaining stack entry, if at the same level as `i`, is
/// `i`'s immediate previous sibling — link incrementally, so by the time any
/// node is pushed its sibling chain up to that point is already correct.
pub(crate) fn build_tree(spans: Vec<NodeSpan>) -> Vec<TreeNode> {
    let mut nodes: Vec<TreeNode> = spans
        .into_iter()
        .map(|span| {
            // Spec 0210 S1. `text_range` is exact here and only here —
            // it is the render's own line counter, read before any
            // splice can invalidate it. Taking the count directly is
            // equivalent to summing the children (every line belongs to
            // exactly one node) and is O(1) rather than a second pass.
            let lines = span.text_range.end - span.text_range.start;
            TreeNode {
                span,
                parent: NO_NODE,
                first_child: NO_NODE,
                last_child: NO_NODE,
                next_sibling: NO_NODE,
                prev_sibling: NO_NODE,
                doc_next: NO_NODE,
                doc_prev: NO_NODE,
                sibling_ordinal: 1,
                lines_total: lines,
                // Nothing is folded at build time, neither in the initial
                // decode nor in a splice's local tree.
                lines_visible: lines,
                rendered_as: None,
            }
        })
        .collect();

    // Stack of (index, level) for completed subtree roots not yet claimed
    // by a parent.
    let mut stack: Vec<(usize, u16)> = Vec::new();

    for i in 0..nodes.len() {
        let level = nodes[i].span.level;

        let mut children = Vec::new();
        while let Some(&(top, top_level)) = stack.last() {
            if top_level > level {
                children.push(top);
                stack.pop();
            } else {
                break;
            }
        }
        children.reverse(); // restore left-to-right document order

        for &c in &children {
            nodes[c].set_parent(Some(i));
        }
        if let Some(&first) = children.first() {
            nodes[i].set_first_child(Some(first));
        }
        if let Some(&last) = children.last() {
            nodes[i].set_last_child(Some(last));
        }

        if let Some(&(top, top_level)) = stack.last() {
            if top_level == level {
                nodes[i].set_prev_sibling(Some(top));
                nodes[top].set_next_sibling(Some(i));
                // Spec 0192 S1: derived in the same place `prev_sibling`
                // is, which is what keeps the two from diverging. A
                // packed run's elements all share one record, so they
                // all share one position — the same rule
                // `sibling_position` used to apply while walking.
                nodes[i].sibling_ordinal = nodes[top].sibling_ordinal
                    + u32::from(!same_packed_record(&nodes[top].span, &nodes[i].span));
            }
        }

        stack.push((i, level));
    }

    // Document-order chain: sort by raw_range.start. Every NodeSpan is
    // backed by a real tag/length prefix of its own (spec 0120 — Any/
    // MessageSet expansion is disabled at the prototext-core level, so
    // no virtual wrapper node with a borrowed/shared range ever reaches
    // this function), so raw_range.start values never tie.
    let mut doc_order: Vec<usize> = (0..nodes.len()).collect();
    doc_order.sort_by_key(|&i| nodes[i].span.raw_range.start);
    for w in doc_order.windows(2) {
        let (a, b) = (w[0], w[1]);
        nodes[a].set_doc_next(Some(b));
        nodes[b].set_doc_prev(Some(a));
    }

    nodes
}

// ── Public entry point ──────────────────────────────────────────────────────

pub struct Decoded {
    pub lines: Vec<String>,
    pub tree: Vec<TreeNode>,
    pub root_type: String,
    /// The wrapped blob actually decoded (spec 0114 §1.1): a real tag+length
    /// prefix (field 1, `WT_LEN`) prepended to the caller's original blob,
    /// so every `NodeSpan::raw_range` in `tree` is relative to *this* blob,
    /// not the caller's original one.
    pub blob: Vec<u8>,
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

/// Deterministic short name for a synthetic one-field wrapper descriptor
/// (spec 0135 §G2): `protolens_internal.x<32 lowercase hex chars>`, the
/// hex being the first 16 bytes (128 bits) of `SHA-256(format!(
/// "{field_number}:{type_str}:{type_name}"))`. `type_str` is `field_type.
/// as_str_name()` (prost's canonical accessor, e.g. `"TYPE_MESSAGE"`) —
/// deliberately not `{:?}` Debug formatting. Leading `x` (not `_`):
/// the `.proto` identifier grammar requires the first character to be a
/// letter. Generic over any `Type`/`type_name` pair — including
/// `Type::Enum`, though this spec never constructs that case (Non-goals).
fn synthetic_wrapper_name(field_number: u64, field_type: Type, type_name: &str) -> String {
    let key = format!("{field_number}:{}:{type_name}", field_type.as_str_name());
    let digest = Sha256::digest(key.as_bytes());
    let mut hex = String::with_capacity(32);
    for byte in &digest[..16] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("protolens_internal.x{hex}")
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
/// previously-unseen wrapper — the "cursor move to a new override
/// candidate is slow" bug diagnosed 2026-07-20. Already-registered
/// wrappers are unaffected (the early `get_message_by_name` return
/// above never reaches the mutating call at all).
pub(crate) fn register_wrapper(
    pool: &mut DescriptorPool,
    field_number: u64,
    field_type: Type,
    target: Option<WrapperTarget>,
) -> Result<MessageDescriptor, DecodeError> {
    let type_name = target.as_ref().map(|t| format!(".{}", t.full_name()));
    let full_name =
        synthetic_wrapper_name(field_number, field_type, type_name.as_deref().unwrap_or(""));
    if let Some(existing) = pool.get_message_by_name(&full_name) {
        return Ok(existing);
    }
    let short_name = full_name
        .strip_prefix("protolens_internal.")
        .expect("synthetic_wrapper_name always returns a protolens_internal.-prefixed name");

    let field = FieldDescriptorProto {
        name: Some("_".to_string()),
        number: Some(field_number as i32),
        label: Some(Label::Optional as i32),
        r#type: Some(field_type as i32),
        type_name,
        ..Default::default()
    };
    let message = DescriptorProto {
        name: Some(short_name.to_string()),
        field: vec![field],
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
    let file = FileDescriptorProto {
        name: Some(format!("protolens_internal/{short_name}.proto")),
        package: Some("protolens_internal".to_string()),
        dependency,
        syntax: Some("proto2".to_string()),
        message_type: vec![message],
        ..Default::default()
    };
    pool.add_file_descriptor_proto(file)
        .map_err(|e| DecodeError::Schema(format!("registering wrapper descriptor: {e}")))?;
    pool.get_message_by_name(&full_name)
        .ok_or_else(|| DecodeError::Schema("wrapper descriptor registered but not found".into()))
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
/// bare `_` character: the naive `.replacen('_', ..)` approach
/// previously matched the `_` inside an unrelated `TYPE_MISMATCH`
/// annotation on a mismatched line, corrupting it (2026-07-18
/// feedback, spec 0143). Returns `None` (caller keeps the original
/// line untouched) when no placeholder was actually written.
pub(crate) fn patch_synthetic_field_name(line: &str, field_name: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let after = rest.strip_prefix('_')?;
    if after.starts_with(": ") || after.starts_with(" {") {
        Some(format!("{indent}{field_name}{after}"))
    } else {
        None
    }
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
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let after = rest.strip_prefix(field_number.to_string().as_str())?;
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
/// occurrence in the document — genuinely nesting it under each
/// distinct MessageSet's own FQDN (e.g. `google.protobuf.MessageSet.
/// Item`) turned out to be structurally impossible: the descriptor pool
/// (matching real `protoc`) rejects a package literally equal to an
/// already-registered message's own full name, and there is no API to
/// reopen an already-loaded foreign message to append a real nested
/// type (2026-07-18 feedback item 4, reverting an earlier attempt).
/// Never shown to the user directly — `message_set_item_display_fqdn`
/// computes a friendly, MessageSet-specific label for the two places
/// this FQDN would otherwise leak into the UI (the status line, the
/// manage pane). Named `Item` (not, say, `MessageSetItem`) so that its
/// short name — `TextSink::begin_nested`'s group-header label (spec
/// 0135 G1: always the group's message type name, never the field's
/// own name) — already reads `Item {`, matching `prototext-core`'s own
/// native MessageSet rendering convention (`message_set_field.rs`'s
/// hardcoded `"Item"` virtual-node label) with no post-render header
/// patch needed (spec 0135, 2026-07-17 review).
pub(crate) const MESSAGE_SET_ITEM_FQDN: &str = "protolens_internal.Item";

/// The friendly, MessageSet-specific FQDN to display in place of the
/// internal, globally-shared `MESSAGE_SET_ITEM_FQDN` wherever a tier-1
/// Item node's type is shown to the user (2026-07-18 feedback item 4)
/// — e.g. `google.protobuf.MessageSet.Item` for a MessageSet whose own
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
    let message = DescriptorProto {
        name: Some("Item".to_string()),
        field: vec![type_id_field, message_field],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("protolens_internal/message_set_item.proto".to_string()),
        package: Some("protolens_internal".to_string()),
        syntax: Some("proto2".to_string()),
        message_type: vec![message],
        ..Default::default()
    };
    pool.add_file_descriptor_proto(file)
        .map_err(|e| DecodeError::Schema(format!("registering MessageSetItem descriptor: {e}")))?;
    pool.get_message_by_name(MESSAGE_SET_ITEM_FQDN)
        .ok_or_else(|| {
            DecodeError::Schema("MessageSetItem descriptor registered but not found".into())
        })
}

/// Prepend a real tag(`field_number`, `WT_LEN`)+length-varint prefix to
/// `blob`, making it a genuinely valid encoding of a wrapper message's
/// sole field (spec 0114 §1.1, generalized spec 0118 §4 to an arbitrary
/// field number). `pub(crate)`: also called from `tui.rs`'s
/// `splice_override`.
pub(crate) fn wrap_blob(field_number: u64, blob: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(blob.len() + 10);
    write_tag(field_number as u32, WT_LEN, &mut out);
    write_varint(blob.len() as u64, &mut out);
    out.extend_from_slice(blob);
    out
}

/// Resolve the root type, then render the whole document under it.
///
/// Spec 0168 G1: the resolution happens before the render, always. This
/// used to take a `defer_root_type` flag that skipped the graph sweep,
/// leaving `root_desc` `None` so the TUI could render the document raw
/// and resolve the type on a background thread — which then re-decoded
/// and re-rendered the whole document through the splice machinery, at
/// several times the cost of the decode it was refining, and swapped it
/// under the reader. The document is now decoded once, as what it is.
/// Callers who don't want to pay for the sweep at all ask for
/// `RootType::Raw` and get a raw render that stays raw.
pub fn decode(
    blob: &[u8],
    ctx: &mut DescriptorContext,
    root_type_request: RootType<'_>,
    indent_size: usize,
) -> Result<Decoded, DecodeError> {
    let (root_desc, root_candidates) = determine_root_type(blob, ctx, root_type_request)?;
    render_resolved(blob, ctx, root_desc, root_candidates, indent_size)
}

/// `decode`'s second half, with the root type already resolved.
///
/// Split out for `main`, which needs to announce the two phases
/// separately: the sweep and the render are both multi-second on a large
/// blob, and a single message spanning both makes whichever one is
/// running look like a hang in the other. Every other caller wants
/// `decode`.
pub fn render_resolved(
    blob: &[u8],
    ctx: &mut DescriptorContext,
    root_desc: Option<MessageDescriptor>,
    root_candidates: RankedCandidates,
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
            )?),
        ),
        None => ("<raw / no type>".to_string(), None),
    };

    let wrapped_blob = wrap_blob(1, blob);
    let wrapper_offset = wrapped_blob.len() - blob.len();

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
    let rendered =
        decode_and_render_indexed(&wrapped_blob, wrapper_desc.as_ref(), &mut fqdns, opts)
            .map_err(|e| DecodeError::Schema(e.to_string()))?;

    let text = String::from_utf8(rendered.text)
        .map_err(|e| DecodeError::Schema(format!("rendered text is not valid UTF-8: {e}")))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // Spec 0135 §G2: `register_wrapper`'s sole field is always named the
    // fixed placeholder `"_"` — patch in the real display name (the root
    // is always field `1` of the virtual encompassing message, wrapped
    // via `wrap_blob(1, ..)` above, and has no schema field name of its
    // own to show instead).
    //
    // Spec 0187 S5: the text patch is all there is. There is no longer a
    // parallel `style_hints` to repair, so the single-line re-`colorize`
    // that used to follow — one of flaw D4's two "highlight a line in
    // isolation" sites, which is exactly the unsound primitive S3 exists
    // to avoid — is simply gone. The patched line gets highlighted in
    // its real context when it is next drawn.
    if wrapper_desc.is_some() {
        if let Some(patched) = patch_synthetic_field_name(&lines[0], "1") {
            lines[0] = patched;
        }
    }
    let tree = build_tree(rendered.spans);

    Ok(Decoded {
        lines,
        tree,
        root_type,
        blob: wrapped_blob,
        wrapper_offset,
        root_candidates,
        fqdns,
    })
}

#[cfg(test)]
mod tests {
    use prost::Message as _;
    use prost_reflect::prost_types::FileDescriptorSet;
    use prototext_graph::build_scoring_graph::build_from_strings;

    use super::*;

    /// Spec 0211 test item 4. A link is stored as a `NodeIdx` with
    /// `NO_NODE` standing for absence, so the one thing the encoding
    /// could plausibly get wrong is confusing "no link" with a real
    /// node — and specifically with node **0**, which an
    /// index-plus-one encoding would have gotten wrong and which is
    /// always a real node here (`build_tree` emits post-order, so slot
    /// 0 holds a leaf).
    #[test]
    fn every_link_round_trips_through_index_zero_and_through_absence() {
        let mut node = build_tree(vec![NodeSpan {
            field_number: 1,
            raw_range: 0..2,
            text_range: 0..1,
            level: 0,
            type_fqdn: NO_FQDN,
            is_message: false,
            packed_record_start: NO_PACKED_RECORD,
            wire_type: WT_LEN as u8,
        }])
        .pop()
        .expect("one span builds one node");

        macro_rules! check {
            ($($get:ident, $set:ident;)*) => {$(
                assert_eq!(
                    node.$get(), None,
                    "a freshly built node has no {}", stringify!($get),
                );
                node.$set(Some(0));
                assert_eq!(
                    node.$get(), Some(0),
                    "{} lost node 0", stringify!($get),
                );
                node.$set(Some(7));
                assert_eq!(node.$get(), Some(7), "{} lost node 7", stringify!($get));
                node.$set(None);
                assert_eq!(
                    node.$get(), None,
                    "{} kept a stale link after being cleared", stringify!($get),
                );
            )*};
        }
        check! {
            parent, set_parent;
            first_child, set_first_child;
            last_child, set_last_child;
            next_sibling, set_next_sibling;
            prev_sibling, set_prev_sibling;
            doc_next, set_doc_next;
            doc_prev, set_doc_prev;
        }
    }

    #[test]
    fn determine_root_type_returns_none_without_override_or_graph() {
        let mut ctx = DescriptorContext::empty_for_test();
        let blob = [0x08u8, 0x05];
        let (resolved, candidates) = determine_root_type(&blob, &mut ctx, RootType::Infer).unwrap();
        assert!(resolved.is_none());
        assert!(candidates.is_empty(), "no graph means no sweep ran");
    }

    /// A minimal real scoring graph, built in memory with no file I/O,
    /// so the `Raw`/`Named` branches can be shown to short-circuit
    /// *despite* a graph being available — which is the only
    /// interesting case: with no graph they would return `None`
    /// regardless and prove nothing.
    fn one_entry_graph() -> LoadedGraph {
        let yaml = "entries:\n- Msg\nmessages:\n  Msg:\n    fields:\n    - number: 1\n      \
                    type: uint64\n"
            .to_string();
        let (bytes, _, _) =
            build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
        LoadedGraph::from_static_bytes(Box::leak(bytes.into_boxed_slice()))
            .expect("test graph must load")
    }

    /// Spec 0168 (`--raw`): the sweep is skipped outright, not merely
    /// ignored. Asserted via the empty candidate list — a sweep that
    /// ran would have produced entries for the graph's one message —
    /// since skipping it is the entire point of the flag on a large
    /// descriptor pool.
    #[test]
    fn raw_never_sweeps_even_when_a_graph_is_loaded() {
        let mut ctx = DescriptorContext::for_test_with_graph(one_entry_graph());
        let blob = [0x08u8, 0x05];

        let (resolved, candidates) = determine_root_type(&blob, &mut ctx, RootType::Raw).unwrap();

        assert!(resolved.is_none());
        assert!(candidates.is_empty(), "--raw must not run the sweep");
    }

    /// Spec 0168 G6: `--type` is an O(1) pool lookup with no sweep. A
    /// name the pool does not know is an error, not a quiet fallback to
    /// inference — otherwise a typo would silently open the wrong type
    /// and look like a scoring bug.
    #[test]
    fn a_named_type_is_looked_up_not_swept_and_a_bad_name_errors() {
        let mut ctx = DescriptorContext::for_test_with_graph(one_entry_graph());
        let blob = [0x08u8, 0x05];

        let err =
            determine_root_type(&blob, &mut ctx, RootType::Named("no.such.Type")).unwrap_err();

        assert!(
            matches!(err, DecodeError::Determination(_)),
            "unknown --type must error: {err:?}"
        );
    }

    /// The two explicit modes against one blob and one pool: `--type`
    /// decodes under the named type, `--raw` decodes the same bytes
    /// under none. Pins `--raw`'s actual guarantee — the render stays
    /// raw — which is what distinguishes it from the deleted
    /// `defer_root_type` flag, whose raw render was replaced under the
    /// reader seconds later.
    #[test]
    fn named_types_the_root_and_raw_leaves_it_untyped() {
        let file = FileDescriptorProto {
            name: Some("test_root_type_modes.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("Inner".to_string()),
                field: vec![FieldDescriptorProto {
                    name: Some("id".to_string()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Int32 as i32),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        };
        let fds = FileDescriptorSet { file: vec![file] };
        let descriptor_path = std::env::temp_dir().join("protolens-root-type-modes-descriptor.pb");
        std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
        let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();

        let blob = [0x08u8, 0x05];

        let named = decode(&blob, &mut ctx, RootType::Named("test.Inner"), 2).unwrap();
        assert_eq!(named.root_type, "test.Inner");
        assert!(named.root_candidates.is_empty(), "no sweep for --type");

        let raw = decode(&blob, &mut ctx, RootType::Raw, 2).unwrap();
        assert_eq!(raw.root_type, "<raw / no type>");
        assert!(raw.root_candidates.is_empty(), "no sweep for --raw");
    }

    /// Spec 0143: the placeholder is anchored on its exact structural
    /// position (immediately after indentation, followed by `": "` or
    /// `" {"`), not found by a bare `_`-anywhere-in-the-line search.
    #[test]
    fn patch_synthetic_field_name_replaces_a_scalar_value_line_placeholder() {
        assert_eq!(
            patch_synthetic_field_name("_: 5", "id"),
            Some("id: 5".to_string())
        );
    }

    #[test]
    fn patch_synthetic_field_name_replaces_a_message_header_placeholder() {
        assert_eq!(
            patch_synthetic_field_name("_ {", "inner"),
            Some("inner {".to_string())
        );
    }

    #[test]
    fn patch_synthetic_field_name_preserves_leading_indentation() {
        assert_eq!(
            patch_synthetic_field_name("    _: 5", "id"),
            Some("    id: 5".to_string())
        );
    }

    /// 2026-07-18 feedback: the exact regression case — a wire-type
    /// mismatch line never writes the placeholder, so the line must
    /// come back untouched instead of having a field name spliced
    /// into the middle of `TYPE_MISMATCH`.
    #[test]
    fn patch_synthetic_field_name_leaves_a_type_mismatch_line_untouched() {
        assert_eq!(
            patch_synthetic_field_name("2: 525005305  #@ varint; TYPE_MISMATCH", "type_id"),
            None
        );
    }

    #[test]
    fn patch_synthetic_field_name_leaves_a_plain_numeric_key_line_untouched() {
        assert_eq!(patch_synthetic_field_name("5: 3  #@ int32 = 5", "id"), None);
    }

    /// Spec 0114: `--type` is optional — with no graph (autoinference
    /// unavailable), `decode()` must not error but instead render the
    /// blob with no known type. The virtual wrapper's own top-level node
    /// (spec 0114 §1.1) still probes as message-shaped even with no
    /// schema (spec 0097's unknown-LEN-field cascade), so it is the sole
    /// node in the tree, with `type_fqdn: None` — the same representation
    /// `apply_override(None)` would produce, i.e. this initial render
    /// already stands in for "the first override of the session".
    #[test]
    fn decode_without_type_override_or_graph_renders_raw_not_error() {
        let inner_desc = DescriptorProto {
            name: Some("Inner".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let file = FileDescriptorProto {
            name: Some("test_decode_raw_fallback.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![inner_desc],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        };
        let fds = FileDescriptorSet { file: vec![file] };

        let descriptor_path =
            std::env::temp_dir().join("protolens-decode-raw-fallback-descriptor.pb");
        std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
        let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();

        // A single varint field (tag 0x08, value 5) — no --type, and this
        // context has no hopcroft.rkyv, so autoinference is unavailable.
        let blob = [0x08u8, 0x05];

        let decoded = decode(&blob, &mut ctx, RootType::Infer, 2).unwrap();
        assert_eq!(decoded.root_type, "<raw / no type>");
        // The wrapper's own top-level field (the "virtual encompassing
        // message", spec 0114 §1.1) — level 0, no type resolved.
        let wrapper = decoded
            .tree
            .iter()
            .find(|n| n.span.level == 0)
            .expect("tree must contain the wrapper's top-level node");
        assert!(wrapper.span.is_message);
        assert_eq!(wrapper.span.type_fqdn, NO_FQDN);
    }

    /// The document root is field number 1 of the virtual encompassing
    /// message, and its field number is always shown, same as any other
    /// unnamed field — the root is not special-cased.
    #[test]
    fn decode_shows_the_root_field_number_in_the_header_line() {
        let msg_desc = DescriptorProto {
            name: Some("Msg".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let file = FileDescriptorProto {
            name: Some("test_decode_root_name.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![msg_desc],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        };
        let fds = FileDescriptorSet { file: vec![file] };

        let descriptor_path = std::env::temp_dir().join("protolens-decode-root-name-descriptor.pb");
        std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
        let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();

        let blob = [0x08u8, 0x05];
        let decoded = decode(&blob, &mut ctx, RootType::Named("test.Msg"), 2).unwrap();
        assert!(
            decoded.lines[0].starts_with("1 "),
            "root header line must show the root field number: {:?}",
            decoded.lines[0]
        );
    }

    /// Spec 0120: `decode()` disables `expand_any`/`expand_message_set`,
    /// so a `google.protobuf.Any` field is *not* auto-expanded at this
    /// layer (that's `tui.rs`'s `render_overrides`/`auto_expand_type`'s
    /// job, spec 0120 §G1) — instead it falls through to ordinary
    /// nested-message rendering under Any's own real 2-field descriptor,
    /// giving `type_url` (field 1) and `value` (field 2) real,
    /// correctly-ordered `NodeSpan`s of their own (no virtual wrapper, no
    /// fabricated `field_number: 0`). Fixture mirrors `prototext/tests/
    /// node_span.rs`'s own `any_schema`/`any_wire_bytes`.
    #[test]
    fn decode_leaves_any_fields_unexpanded_with_real_type_url_and_value_spans() {
        let any_msg = DescriptorProto {
            name: Some("Any".to_string()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("type_url".to_string()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::String as i32),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("value".to_string()),
                    number: Some(2),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Bytes as i32),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let any_file = FileDescriptorProto {
            name: Some("google/protobuf/any.proto".to_string()),
            syntax: Some("proto3".to_string()),
            package: Some("google.protobuf".to_string()),
            message_type: vec![any_msg],
            ..Default::default()
        };

        let payload_msg = DescriptorProto {
            name: Some("Payload".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("label".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let container_msg = DescriptorProto {
            name: Some("Container".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("payload".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".google.protobuf.Any".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let acme_file = FileDescriptorProto {
            name: Some("acme.proto".to_string()),
            syntax: Some("proto2".to_string()),
            package: Some("acme".to_string()),
            dependency: vec!["google/protobuf/any.proto".to_string()],
            message_type: vec![payload_msg, container_msg],
            ..Default::default()
        };
        let fds = FileDescriptorSet {
            file: vec![any_file, acme_file],
        };

        let descriptor_path =
            std::env::temp_dir().join("protolens-decode-any-expansion-descriptor.pb");
        std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
        let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();

        // Container { payload: Any { type_url:
        // "type.googleapis.com/acme.Payload", value: Payload { label:
        // "hello" } } }.
        let label = b"hello";
        let mut payload_bytes = vec![0x0au8, label.len() as u8];
        payload_bytes.extend_from_slice(label);
        let type_url = b"type.googleapis.com/acme.Payload";
        let mut any_bytes = vec![0x0au8, type_url.len() as u8];
        any_bytes.extend_from_slice(type_url);
        any_bytes.push(0x12);
        any_bytes.push(payload_bytes.len() as u8);
        any_bytes.extend_from_slice(&payload_bytes);
        let mut blob = vec![0x0au8, any_bytes.len() as u8];
        blob.extend_from_slice(&any_bytes);

        let decoded = decode(&blob, &mut ctx, RootType::Named("acme.Container"), 2).unwrap();
        let any = decoded.fqdns.id_of("google.protobuf.Any");
        let any_idx = decoded
            .tree
            .iter()
            .position(|n| n.span.type_fqdn == any)
            .expect("tree must contain the unexpanded Any node itself");
        let type_url_idx = decoded.tree[any_idx]
            .first_child()
            .expect("Any node must have type_url as its first child");
        let value_idx = decoded.tree[type_url_idx]
            .next_sibling()
            .expect("Any node must have value as its second child");
        assert_eq!(decoded.tree[type_url_idx].span.field_number, 1);
        assert_eq!(decoded.tree[value_idx].span.field_number, 2);
        assert!(
            decoded.tree[value_idx].span.type_fqdn == NO_FQDN,
            "value must stay unexpanded (plain bytes) at decode() layer: {:#?}",
            decoded.tree[value_idx].span
        );
        let payload = decoded.fqdns.id_of("acme.Payload");
        assert!(
            !decoded.tree.iter().any(|n| n.span.type_fqdn == payload),
            "acme.Payload must not appear — auto-expansion is tui.rs's job, \
             not decode()'s: {:#?}",
            decoded.lines
        );
        // Real tag/length-backed ranges, in document order: type_url's
        // range must end before value's own range starts.
        assert!(
            decoded.tree[type_url_idx].span.raw_range.end
                <= decoded.tree[value_idx].span.raw_range.start,
            "type_url and value must have real, non-overlapping, correctly \
             ordered raw ranges: {:#?}",
            (
                &decoded.tree[type_url_idx].span,
                &decoded.tree[value_idx].span
            )
        );
    }

    // ── Spec 0197: on-demand descriptor loading ──────────────────────────

    /// A four-file descriptor set shaped so the on-demand branch can be
    /// told apart from the eager one by observation alone:
    ///
    /// - `leaf.proto` — `t.Leaf`, in `t.Root`'s closure, with an extension
    ///   range so an extension can be hung off it.
    /// - `root.proto` — `t.Root`, importing `leaf.proto`, carrying one
    ///   nested message and one nested enum (spec 0137's namespace).
    /// - `stray.proto` — `t.Stray` and `t.Mood`, imported by nobody, so
    ///   they are *outside* `t.Root`'s closure and must not appear in a
    ///   lazy pool until asked for.
    /// - `ext.proto` — extends `t.Leaf` at field 100, likewise outside the
    ///   closure, so `ext_to_file` is the only way to find it.
    ///
    /// proto2 throughout: proto3 has no extension ranges.
    fn fixture_files() -> Vec<FileDescriptorProto> {
        use prost_reflect::prost_types::{
            descriptor_proto::ExtensionRange, EnumDescriptorProto, EnumValueDescriptorProto,
        };

        let scalar = |name: &str, number: i32| FieldDescriptorProto {
            name: Some(name.to_string()),
            number: Some(number),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        };

        let leaf = FileDescriptorProto {
            name: Some("leaf.proto".to_string()),
            package: Some("t".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("Leaf".to_string()),
                field: vec![scalar("id", 1)],
                extension_range: vec![ExtensionRange {
                    start: Some(100),
                    end: Some(200),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            syntax: Some("proto2".to_string()),
            ..Default::default()
        };

        let root = FileDescriptorProto {
            name: Some("root.proto".to_string()),
            package: Some("t".to_string()),
            dependency: vec!["leaf.proto".to_string()],
            message_type: vec![DescriptorProto {
                name: Some("Root".to_string()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("leaf".to_string()),
                        number: Some(1),
                        label: Some(Label::Optional as i32),
                        r#type: Some(Type::Message as i32),
                        type_name: Some(".t.Leaf".to_string()),
                        ..Default::default()
                    },
                    scalar("n", 2),
                ],
                nested_type: vec![DescriptorProto {
                    name: Some("Nested".to_string()),
                    field: vec![scalar("k", 1)],
                    ..Default::default()
                }],
                enum_type: vec![EnumDescriptorProto {
                    name: Some("Color".to_string()),
                    value: vec![EnumValueDescriptorProto {
                        name: Some("RED".to_string()),
                        number: Some(0),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            syntax: Some("proto2".to_string()),
            ..Default::default()
        };

        let stray = FileDescriptorProto {
            name: Some("stray.proto".to_string()),
            package: Some("t".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("Stray".to_string()),
                field: vec![scalar("s", 1)],
                ..Default::default()
            }],
            enum_type: vec![EnumDescriptorProto {
                name: Some("Mood".to_string()),
                value: vec![EnumValueDescriptorProto {
                    name: Some("CALM".to_string()),
                    number: Some(0),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            syntax: Some("proto2".to_string()),
            ..Default::default()
        };

        let ext = FileDescriptorProto {
            name: Some("ext.proto".to_string()),
            package: Some("t".to_string()),
            dependency: vec!["leaf.proto".to_string()],
            extension: vec![FieldDescriptorProto {
                name: Some("tag".to_string()),
                number: Some(100),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                extendee: Some(".t.Leaf".to_string()),
                ..Default::default()
            }],
            syntax: Some("proto2".to_string()),
            ..Default::default()
        };

        vec![leaf, root, stray, ext]
    }

    /// Each file's `(start, end)` byte span within an encoded FDS,
    /// keyed by file name.
    type FdsSpans = Vec<(String, (u64, u64))>;

    /// The FDS wire encoding plus each file's span within it. Laid out
    /// by hand rather than via `FileDescriptorSet::encode` because the
    /// spans only exist while the records are being written, and they
    /// are precisely what `LazyPool` slices FDPs out of.
    fn encode_fds(files: &[FileDescriptorProto]) -> (Vec<u8>, FdsSpans) {
        let mut buf = Vec::new();
        let mut spans = Vec::new();
        for file in files {
            let body = file.encode_to_vec();
            write_tag(1, WT_LEN, &mut buf);
            write_varint(body.len() as u64, &mut buf);
            let start = buf.len() as u64;
            buf.extend_from_slice(&body);
            spans.push((file.name().to_owned(), (start, buf.len() as u64)));
        }
        (buf, spans)
    }

    fn collect_types(
        prefix: &str,
        msg: &DescriptorProto,
        file: &str,
        out: &mut Vec<(String, String)>,
    ) {
        let fqdn = format!("{prefix}{}", msg.name());
        out.push((fqdn.clone(), file.to_owned()));
        let inner = format!("{fqdn}.");
        for nested in &msg.nested_type {
            collect_types(&inner, nested, file, out);
        }
        for e in &msg.enum_type {
            out.push((format!("{inner}{}", e.name()), file.to_owned()));
        }
    }

    /// Build the `FdsIndex` `reproto` would have written for `files`.
    ///
    /// There is no Rust-side index builder to reuse — `reproto` assembles
    /// the maps in Python across the pyo3 boundary — so this reproduces
    /// the four maps `fds_index.rs:57-79` documents, through
    /// `canonical_map` so the archive is laid out the same way.
    fn build_index(
        files: &[FileDescriptorProto],
        spans: Vec<(String, (u64, u64))>,
    ) -> prototext_graph::fds_index::FdsIndex {
        use prototext_graph::fds_index::{canonical_map, FdsIndex};

        let mut types = Vec::new();
        let mut deps = Vec::new();
        let mut exts = Vec::new();
        for file in files {
            let fname = file.name().to_owned();
            let pkg = file.package();
            let prefix = if pkg.is_empty() {
                String::new()
            } else {
                format!("{pkg}.")
            };
            for msg in &file.message_type {
                collect_types(&prefix, msg, &fname, &mut types);
            }
            for e in &file.enum_type {
                types.push((format!("{prefix}{}", e.name()), fname.clone()));
            }
            deps.push((fname.clone(), file.dependency.clone()));
            for x in &file.extension {
                let extendee = x.extendee().trim_start_matches('.');
                exts.push((format!("{extendee}/{}", x.number()), fname.clone()));
            }
        }

        FdsIndex {
            type_to_file: canonical_map(types),
            file_to_span: canonical_map(spans),
            dep_graph: canonical_map(deps),
            ext_to_file: canonical_map(exts),
        }
    }

    /// `<dir>/schema.pb` plus the `<dir>/schema/` sidecar directory — the
    /// layout `DescriptorContext::load`'s probe expects, since it derives
    /// the sidecar directory from `path.with_extension("")`.
    struct Fixture {
        dir: PathBuf,
        pb: PathBuf,
        index: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Fixture {
        /// A binary descriptor with no sidecar: the eager branch.
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("protolens-0197-{tag}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("schema")).unwrap();
            let pb = dir.join("schema.pb");
            std::fs::write(&pb, encode_fds(&fixture_files()).0).unwrap();
            Fixture {
                index: dir.join("schema").join("index.rkyv"),
                pb,
                dir,
            }
        }

        /// Add the sidecar, enabling the on-demand branch.
        fn with_index(self) -> Self {
            let (_, spans) = encode_fds(&fixture_files());
            let index = build_index(&fixture_files(), spans);
            prototext_graph::fds_index::write(&index, &self.index).unwrap();
            self
        }

        /// Add a sidecar whose PTSGRAPH version field is one the reader
        /// does not accept — the state every existing schema-db is in the
        /// moment this ships.
        fn with_version_skewed_index(self) -> Self {
            let (_, spans) = encode_fds(&fixture_files());
            let index = build_index(&fixture_files(), spans);
            let mut bytes = prototext_graph::fds_index::to_bytes(&index).unwrap();
            bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
            std::fs::write(&self.index, &bytes).unwrap();
            self
        }

        /// Rewrite the descriptor as `#@ prototext` text. Round-tripped
        /// through the codec so `read_descriptor_file` recovers exactly
        /// the binary the sidecar's spans were measured against.
        fn with_prototext_descriptor(self) -> Self {
            let binary = std::fs::read(&self.pb).unwrap();
            let text = prototext_core::render_as_text(
                &binary,
                None,
                RenderOpts {
                    assume_binary: true,
                    include_annotations: true,
                    indent: 1,
                    expand_any: false,
                    ..RenderOpts::default()
                },
            )
            .unwrap();
            std::fs::write(&self.pb, &text).unwrap();
            self
        }

        fn load(&self) -> DescriptorContext {
            DescriptorContext::load(&self.pb).unwrap()
        }
    }

    /// `Root { leaf { id: 5 } n: 7 }`.
    const ROOT_BLOB: &[u8] = &[0x0a, 0x02, 0x08, 0x05, 0x10, 0x07];

    fn message_names(ctx: &DescriptorContext) -> Vec<String> {
        let mut names: Vec<String> = ctx
            .pool()
            .all_messages()
            .map(|m| m.full_name().to_string())
            .collect();
        names.sort_unstable();
        names
    }

    /// Spec 0197 test 1 + G5. The branch is an implementation detail: the
    /// same blob under the same root type must render the same text
    /// whether the schema arrived one file at a time or all at once.
    #[test]
    fn both_branches_render_the_same_document() {
        let lazy = Fixture::new("same-render-lazy").with_index();
        let eager = Fixture::new("same-render-eager");

        let mut lazy_ctx = lazy.load();
        let mut eager_ctx = eager.load();
        assert!(lazy_ctx.lazy.is_some(), "sidecar present: on-demand branch");
        assert!(eager_ctx.lazy.is_none(), "no sidecar: eager branch");

        let from_lazy = decode(ROOT_BLOB, &mut lazy_ctx, RootType::Named("t.Root"), 2).unwrap();
        let from_eager = decode(ROOT_BLOB, &mut eager_ctx, RootType::Named("t.Root"), 2).unwrap();

        assert_eq!(from_lazy.lines, from_eager.lines);
        assert_eq!(from_lazy.root_type, from_eager.root_type);
    }

    /// Spec 0197 tests 2 and 3. The whole point of the branch — the pool
    /// starts with nothing in it, grows to exactly the root's file
    /// closure, and grows again only when a name outside that closure is
    /// asked for.
    #[test]
    fn the_lazy_pool_starts_empty_and_grows_only_on_demand() {
        let fixture = Fixture::new("grows-on-demand").with_index();
        let mut ctx = fixture.load();

        assert!(
            message_names(&ctx).is_empty(),
            "a freshly opened lazy pool holds no types at all"
        );

        ctx.message("t.Root").expect("root must resolve");
        assert_eq!(
            message_names(&ctx),
            vec![
                "t.Leaf".to_string(),
                "t.Root".to_string(),
                "t.Root.Nested".to_string()
            ],
            "exactly root.proto's closure — leaf.proto came in as its import, \
             stray.proto did not come in at all"
        );

        ctx.message("t.Stray")
            .expect("a name outside the closure must still resolve");
        assert!(
            message_names(&ctx).contains(&"t.Stray".to_string()),
            "the on-demand load must have added stray.proto"
        );
    }

    /// Spec 0197 tests 4 and 5, and the measured property of §5: the
    /// index's key set *is* the pool's type namespace, enums included.
    /// Equality of the ordered lists, not just the sets — the override
    /// pane's lexicographic mode indexes into this by row.
    #[test]
    fn all_type_fqdns_agrees_across_branches_and_keeps_enums() {
        let lazy = Fixture::new("fqdns-lazy").with_index();
        let eager = Fixture::new("fqdns-eager");

        let from_index = lazy.load().all_type_fqdns();
        let from_pool = eager.load().all_type_fqdns();

        assert_eq!(from_index, from_pool);
        // Spec 0137's enums, one nested and one top-level, both present.
        assert!(from_index.contains(&"t.Root.Color".to_string()));
        assert!(from_index.contains(&"t.Mood".to_string()));
    }

    /// Spec 0197 test 6 (§S3, first cause). A missing sidecar is the
    /// ordinary case for a hand-built descriptor: eager, and the user is
    /// told which file to make and how.
    #[test]
    fn a_missing_index_falls_back_and_names_the_missing_sidecar() {
        let fixture = Fixture::new("missing-index");
        let ctx = fixture.load();

        assert!(ctx.lazy.is_none());
        let warning = &ctx
            .fallback
            .as_ref()
            .expect("fallback must be recorded")
            .message;
        assert!(
            warning.contains("no index.rkyv beside") && warning.contains("re-run reproto"),
            "{warning}"
        );
    }

    /// Spec 0197 test 7 (§S3, second cause). The load must *degrade*, not
    /// fail: a version-skewed sidecar is what every existing schema-db
    /// looks like on the day this ships, and erroring there would make
    /// the feature a breaking change.
    #[test]
    fn a_version_skewed_index_falls_back_without_erroring() {
        let fixture = Fixture::new("skewed-index").with_version_skewed_index();
        let ctx = fixture.load();

        assert!(ctx.lazy.is_none());
        let warning = &ctx
            .fallback
            .as_ref()
            .expect("fallback must be recorded")
            .message;
        assert!(
            warning.contains("unsupported version 99") && warning.contains("re-run reproto"),
            "{warning}"
        );
    }

    /// Spec 0197 test 8 (§S3, third cause / §S7). The sidecar's spans
    /// index the binary encoding; a `#@` descriptor on disk is not that
    /// encoding, so a present and perfectly valid sidecar must still be
    /// declined.
    #[test]
    fn a_prototext_descriptor_falls_back_even_with_a_valid_index() {
        let fixture = Fixture::new("prototext-descriptor")
            .with_index()
            .with_prototext_descriptor();
        let mut ctx = fixture.load();

        assert!(ctx.lazy.is_none());
        let warning = ctx
            .fallback
            .as_ref()
            .expect("fallback must be recorded")
            .message
            .clone();
        assert!(warning.contains("is #@ prototext"), "{warning}");
        // And the eager pool it fell back to is a working one.
        assert!(ctx.message("t.Root").is_some());
    }

    /// Spec 0197 test 11 (§S6). The bytes are no longer retained, so the
    /// hash is recomputed from `source` — and must not have changed
    /// value, since spec 0117's `descriptor_set_sha256` is written into
    /// override files that already exist.
    #[test]
    fn the_descriptor_hash_is_identical_across_branches() {
        let lazy = Fixture::new("hash-lazy").with_index();
        let eager = Fixture::new("hash-eager");

        let from_lazy = lazy.load().descriptor_sha256();
        let from_eager = eager.load().descriptor_sha256();

        assert_eq!(from_lazy, from_eager);
        assert_eq!(
            from_lazy,
            crate::override_pane::sha256_hex(&encode_fds(&fixture_files()).0),
            "the hash is still over the canonicalized descriptor bytes"
        );
    }

    /// Spec 0197 test 12 (§S6, last paragraph). No `--descriptor-set` at
    /// all: nothing to be lazy about, nothing to warn about, and the hash
    /// keeps the value it has always had.
    #[test]
    fn the_schemaless_context_has_no_lazy_branch_and_hashes_the_empty_string() {
        let ctx = DescriptorContext::schemaless();

        assert!(ctx.lazy.is_none());
        assert!(ctx.fallback.is_none());
        assert!(ctx.pool().all_messages().next().is_none());
        assert_eq!(
            ctx.descriptor_sha256(),
            crate::override_pane::sha256_hex(&[])
        );
    }

    /// Spec 0197 test 14 (§S5, the staleness rule). Overriding a range to
    /// a type from outside the root's closure loads a file into a pool
    /// that already has live descriptors handed out of it, which is
    /// exactly when prost-reflect's `Arc::make_mut` forks it. The wrapper
    /// registration and the re-render that follow must see the new
    /// symbol, not the pre-fork snapshot.
    #[test]
    fn a_type_loaded_after_the_root_can_still_be_rendered_through() {
        let fixture = Fixture::new("override-fresh-type").with_index();
        let mut ctx = fixture.load();

        decode(ROOT_BLOB, &mut ctx, RootType::Named("t.Root"), 2).unwrap();

        // `Stray { s: 9 }`, the payload an override would splice.
        let stray_blob = [0x08u8, 0x09];
        let desc = ctx.message("t.Stray").expect("must load on demand");
        let decoded = render_resolved(&stray_blob, &mut ctx, Some(desc), Vec::new(), 2).unwrap();

        assert_eq!(decoded.root_type, "t.Stray");
        assert!(
            decoded.lines.iter().any(|l| l.contains("s: 9")),
            "the freshly loaded type must render by name: {:?}",
            decoded.lines
        );
    }

    /// Spec 0197 test 13 (§S5). MessageSet expansion resolves a payload
    /// through an *extension* on the enclosing type, and the file
    /// declaring that extension need never be in the root's closure —
    /// `ext_to_file` is the only route to it, which is why
    /// `load_extension` exists alongside `message`.
    #[test]
    fn an_extension_jit_loads_from_outside_the_root_closure() {
        let fixture = Fixture::new("extension-jit").with_index();
        let mut ctx = fixture.load();

        ctx.message("t.Root").expect("root must resolve");
        assert!(
            ctx.pool()
                .get_message_by_name("t.Leaf")
                .expect("Leaf is in the closure")
                .get_extension(100)
                .is_none(),
            "ext.proto is outside the closure, so its extension is not there yet"
        );

        ctx.load_extension("t.Leaf", 100);

        assert!(
            ctx.pool()
                .get_message_by_name("t.Leaf")
                .expect("Leaf is still there")
                .get_extension(100)
                .is_some(),
            "load_extension must have pulled ext.proto in"
        );
    }
}
