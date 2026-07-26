// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Decode a binary protobuf blob into rendered text plus a navigation tree.
//!
//! Mirrors (simplified) `prototext`'s own `DescriptorContext` / `infer_type`
//! machinery (`prototext/src/run.rs`) — no `LazyPool`/`index.rkyv` fast path,
//! no embedded-WKT-descriptor fallback: spec 0111 v1 always requires an
//! explicit `--descriptor-set`.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};
use prost_reflect::{DescriptorPool, EnumDescriptor, MessageDescriptor};
use prototext_core::helpers::{write_tag, write_varint, WT_LEN};
use prototext_core::serialize::render_text::{
    decode_and_render_indexed, DecodeRenderOpts, NodeSpan,
};
use prototext_core::{decode_pool, render_as_bytes, RenderOpts};
use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{
    load::{load_graph, LoadedGraph},
    score_all, ScoringOpts,
};
use sha2::{Digest, Sha256};

use crate::colorize::{self, SyntaxRole};

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

/// A resolved `--descriptor-set`: a pool for type lookup plus an optional
/// Hopcroft scoring graph (`<stem>/hopcroft.rkyv` sidecar, if present).
pub struct DescriptorContext {
    pool: DescriptorPool,
    /// `Arc` rather than a bare `LoadedGraph` (spec 0180 S2): protolens hands
    /// the graph to two background threads, and one of them — the detached
    /// root-type sweep in `tui::run` — is deliberately never joined. An
    /// owning handle is what lets that thread outlive `App` without reading
    /// an unmapped page. Cloning it is a refcount bump, not a copy.
    pub graph: Option<Arc<LoadedGraph>>,
    /// Canonicalized binary bytes `load()` decoded `pool` from — i.e.
    /// after `read_descriptor_file`'s `#@ prototext`-to-binary conversion,
    /// same normalization `main.rs` applies to the target blob (spec 0114
    /// §1.1). Basis for `override_pane::sha256_hex`'s
    /// `descriptor_set_sha256` (spec 0117 §4).
    pub raw_bytes: Vec<u8>,
}

impl DescriptorContext {
    pub fn pool(&self) -> &DescriptorPool {
        &self.pool
    }

    /// Mutable pool access — needed by `tui.rs`'s `splice_override` (spec
    /// 0118 §4) to call `register_wrapper` for an arbitrary target type,
    /// mirroring `decode()`'s own (in-module, private-field) access.
    pub(crate) fn pool_mut(&mut self) -> &mut DescriptorPool {
        &mut self.pool
    }

    /// Load a `DescriptorContext` from a `--descriptor-set` path. v1 has no
    /// schemaless/embedded-WKT fallback (spec 0111 Goal 2): the caller must
    /// always supply a path.
    pub fn load(path: &Path) -> Result<Self, DecodeError> {
        let bytes = read_descriptor_file(path)?;
        let pool = decode_pool(&bytes)
            .map_err(|e| DecodeError::Schema(format!("descriptor '{}': {e}", path.display())))?;

        let stem = path.with_extension("");
        let rkyv_path = stem.join("hopcroft.rkyv");
        let graph = if rkyv_path.exists() {
            Some(Arc::new(load_graph(&rkyv_path).map_err(|e| {
                DecodeError::Schema(format!("loading graph '{}': {e}", rkyv_path.display()))
            })?))
        } else {
            None
        };

        Ok(DescriptorContext {
            pool,
            graph,
            raw_bytes: bytes,
        })
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
        DescriptorContext {
            pool: DescriptorPool::new(),
            graph: None,
            raw_bytes: Vec::new(),
        }
    }

    /// A schemaless `DescriptorContext` (spec 0157 G3): empty pool, no
    /// scoring graph. Used when `--descriptor-set` is absent — the
    /// production counterpart of `empty_for_test()` (same shape, kept
    /// separate so each call site's intent stays clear).
    pub(crate) fn schemaless() -> Self {
        DescriptorContext {
            pool: DescriptorPool::new(),
            graph: None,
            raw_bytes: Vec::new(),
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
            pool: DescriptorPool::new(),
            graph: Some(Arc::new(graph)),
            raw_bytes: Vec::new(),
        }
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

/// The `score_all` + veto/tie-break winner-selection rule shared by
/// `determine_root_type`'s synchronous graph branch and the TUI's
/// asynchronous root-type worker (spec NNNN) — factored out so both can
/// run the identical selection logic against a bare `ArchivedCompiledGraph`
/// reference (the worker thread has no `DescriptorContext`/pool, only an
/// owning `Arc<LoadedGraph>` and the blob, mirroring spec 0152's heat
/// worker — see spec 0180 S2 for why it owns rather than borrows). `None`
/// when there's no clean winner: no candidates, every candidate vetoed, or
/// a top-score tie.
pub(crate) fn resolve_root_winner_fqdn(
    blob: &[u8],
    graph: &ArchivedCompiledGraph,
) -> Option<String> {
    let scoring_opts = ScoringOpts::default();
    let mut results = score_all(blob, graph, &scoring_opts);
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(b.fqdn)),
    });

    if results.is_empty() {
        return None;
    }
    let non_vetoed: Vec<_> = results.iter().filter(|r| !r.vetoed).collect();
    if non_vetoed.is_empty() {
        return None;
    }

    let top_score = non_vetoed[0].score();
    let tied = non_vetoed.iter().filter(|r| r.score() == top_score).count();
    if tied > 1 {
        return None;
    }

    Some(non_vetoed[0].fqdn.to_owned())
}

/// Resolve the root message type to decode `blob` as.
///
/// - `type_override` given: looked up directly in the pool; a lookup
///   failure is a hard error (the user asked for a specific type).
/// - `type_override` absent: tries autoinference via a scoring graph
///   (`ctx.graph`). Returns `Ok(None)` — not an error — whenever inference
///   doesn't produce a clean winner (no graph available, no candidates,
///   all candidates vetoed, or a top-score tie): the caller then renders
///   the blob with no type known (spec 0114, "protolens command line
///   should not require --type").
pub fn determine_root_type(
    blob: &[u8],
    ctx: &DescriptorContext,
    type_override: Option<&str>,
) -> Result<Option<MessageDescriptor>, DecodeError> {
    if let Some(fqdn) = type_override {
        return ctx
            .pool()
            .get_message_by_name(fqdn)
            .map(Some)
            .ok_or_else(|| {
                DecodeError::Determination(format!("type '{fqdn}' not found in descriptor set"))
            });
    }

    let Some(graph) = ctx.graph.as_ref() else {
        return Ok(None);
    };

    let Some(fqdn) = resolve_root_winner_fqdn(blob, graph) else {
        return Ok(None);
    };
    Ok(ctx.pool().get_message_by_name(&fqdn))
}

// ── Navigation tree ─────────────────────────────────────────────────────────

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
    pub parent: Option<usize>,
    pub first_child: Option<usize>,
    pub last_child: Option<usize>,
    pub next_sibling: Option<usize>,
    pub prev_sibling: Option<usize>,
    pub doc_next: Option<usize>,
    pub doc_prev: Option<usize>,
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
        .map(|span| TreeNode {
            span,
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            doc_next: None,
            doc_prev: None,
            rendered_as: None,
        })
        .collect();

    // Stack of (index, level) for completed subtree roots not yet claimed
    // by a parent.
    let mut stack: Vec<(usize, usize)> = Vec::new();

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
            nodes[c].parent = Some(i);
        }
        if let Some(&first) = children.first() {
            nodes[i].first_child = Some(first);
        }
        if let Some(&last) = children.last() {
            nodes[i].last_child = Some(last);
        }

        if let Some(&(top, top_level)) = stack.last() {
            if top_level == level {
                nodes[i].prev_sibling = Some(top);
                nodes[top].next_sibling = Some(i);
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
        nodes[a].doc_next = Some(b);
        nodes[b].doc_prev = Some(a);
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
    /// Syntax-highlighting spans (spec 0116 §7), one entry per `lines`,
    /// each holding that line's `(column range, role)` pairs — the
    /// initial-load counterpart of `apply_override`'s per-splice
    /// colorize pass (`protolens/src/tui.rs`).
    pub style_hints: Vec<Vec<(std::ops::Range<usize>, SyntaxRole)>>,
    /// `true` when `decode()`'s caller asked to skip synchronous graph-
    /// based root-type inference (`defer_root_type`, spec NNNN) — always
    /// paired with `root_type == "<raw / no type>"`. `App::new` picks this
    /// up to know whether it should hand root-type resolution off to the
    /// TUI's background worker instead of treating the raw fallback as
    /// final.
    pub root_type_deferred: bool,
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

pub fn decode(
    blob: &[u8],
    ctx: &mut DescriptorContext,
    type_override: Option<&str>,
    indent_size: usize,
    defer_root_type: bool,
) -> Result<Decoded, DecodeError> {
    // `defer_root_type` (spec NNNN): an explicit `type_override` is an
    // O(1) pool lookup, never worth deferring — only the graph-based
    // `score_all` sweep is. Skipping it here leaves `root_desc` `None`,
    // same as `determine_root_type`'s own "no clean winner" outcome; the
    // caller (`App::new`, via `Decoded::root_type_deferred`) is
    // responsible for actually resolving it later and applying the
    // result, or the root just stays the raw/no-type fallback forever.
    let root_type_deferred = defer_root_type && type_override.is_none() && ctx.graph.is_some();
    let root_desc = if root_type_deferred {
        None
    } else {
        determine_root_type(blob, ctx, type_override)?
    };
    let (root_type, wrapper_desc) = match &root_desc {
        Some(desc) => (
            desc.full_name().to_string(),
            Some(register_wrapper(
                &mut ctx.pool,
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
    let (text, spans) = decode_and_render_indexed(&wrapped_blob, wrapper_desc.as_ref(), opts);

    let text = String::from_utf8(text)
        .map_err(|e| DecodeError::Schema(format!("rendered text is not valid UTF-8: {e}")))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut style_hints = colorize::hints_by_line(&lines, &colorize::colorize(&text));
    // Spec 0135 §G2: `register_wrapper`'s sole field is always named the
    // fixed placeholder `"_"` — patch in the real display name (the root
    // is always field `1` of the virtual encompassing message, wrapped
    // via `wrap_blob(1, ..)` above, and has no schema field name of its
    // own to show instead).
    if wrapper_desc.is_some() {
        if let Some(patched) = patch_synthetic_field_name(&lines[0], "1") {
            lines[0] = patched;
            style_hints[0] =
                colorize::hints_by_line(&lines[..1], &colorize::colorize(&lines[0])).remove(0);
        }
    }
    let tree = build_tree(spans);

    Ok(Decoded {
        lines,
        tree,
        root_type,
        blob: wrapped_blob,
        wrapper_offset,
        style_hints,
        root_type_deferred,
    })
}

#[cfg(test)]
mod tests {
    use prost::Message as _;
    use prost_reflect::prost_types::FileDescriptorSet;

    use super::*;

    #[test]
    fn determine_root_type_returns_none_without_override_or_graph() {
        let ctx = DescriptorContext::empty_for_test();
        let blob = [0x08u8, 0x05];
        let resolved = determine_root_type(&blob, &ctx, None).unwrap();
        assert!(resolved.is_none());
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

        let decoded = decode(&blob, &mut ctx, None, 2, false).unwrap();
        assert_eq!(decoded.root_type, "<raw / no type>");
        // The wrapper's own top-level field (the "virtual encompassing
        // message", spec 0114 §1.1) — level 0, no type resolved.
        let wrapper = decoded
            .tree
            .iter()
            .find(|n| n.span.level == 0)
            .expect("tree must contain the wrapper's top-level node");
        assert!(wrapper.span.is_message);
        assert_eq!(wrapper.span.type_fqdn, None);
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
        let decoded = decode(&blob, &mut ctx, Some("test.Msg"), 2, false).unwrap();
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

        let decoded = decode(&blob, &mut ctx, Some("acme.Container"), 2, false).unwrap();
        let any_idx = decoded
            .tree
            .iter()
            .position(|n| n.span.type_fqdn.as_deref() == Some("google.protobuf.Any"))
            .expect("tree must contain the unexpanded Any node itself");
        let type_url_idx = decoded.tree[any_idx]
            .first_child
            .expect("Any node must have type_url as its first child");
        let value_idx = decoded.tree[type_url_idx]
            .next_sibling
            .expect("Any node must have value as its second child");
        assert_eq!(decoded.tree[type_url_idx].span.field_number, 1);
        assert_eq!(decoded.tree[value_idx].span.field_number, 2);
        assert!(
            decoded.tree[value_idx].span.type_fqdn.is_none(),
            "value must stay unexpanded (plain bytes) at decode() layer: {:#?}",
            decoded.tree[value_idx].span
        );
        assert!(
            !decoded
                .tree
                .iter()
                .any(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload")),
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
}
