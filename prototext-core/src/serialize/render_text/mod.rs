// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

mod arena;
mod fqdn;
mod helpers;
mod packed;
mod shape;
mod sink;
mod varint;

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use prost_reflect::{Cardinality, ExtensionDescriptor, FieldDescriptor, Kind, MessageDescriptor};

use crate::helpers::{
    bytes_missing, parse_varint, parse_wiretag, payload_end, WiretagResult, MAX_INDEXED_BUFFER,
    MAX_WIRE_DEPTH, WT_END_GROUP, WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT,
};
use crate::CodecError;

use helpers::{render_group_field, render_len_field, scan_group_extent, FieldCtx};
use sink::{IndexingTextSink, MalformedKind, ScalarValue, Sink, TagFacts, TextSink};

pub use arena::{build_arena, Arena};
pub use fqdn::{FqdnId, FqdnTable, NO_FQDN, UNINTERNED};
pub use shape::Shape;
pub use sink::{NodeSpan, NO_PACKED_RECORD};

// Magic prefix that identifies a textual prototext payload.
const PROTOTEXT_MAGIC: &[u8] = b"#@ prototext:";

// ── FieldOrExt adapter ────────────────────────────────────────────────────────

/// Unifies `FieldDescriptor` (regular field) and `ExtensionDescriptor`
/// (extension field) for the subset of accessors used by the renderer.
pub(super) enum FieldOrExt {
    Field(FieldDescriptor),
    Ext(ExtensionDescriptor),
}

impl FieldOrExt {
    pub(super) fn kind(&self) -> Kind {
        match self {
            FieldOrExt::Field(f) => f.kind(),
            FieldOrExt::Ext(e) => e.kind(),
        }
    }

    pub(super) fn cardinality(&self) -> Cardinality {
        match self {
            FieldOrExt::Field(f) => f.cardinality(),
            FieldOrExt::Ext(e) => e.cardinality(),
        }
    }

    /// Returns `true` only for regular group fields; extensions cannot be groups.
    pub(super) fn is_group(&self) -> bool {
        match self {
            FieldOrExt::Field(f) => f.is_group(),
            FieldOrExt::Ext(_) => false,
        }
    }

    pub(super) fn is_packed(&self) -> bool {
        match self {
            FieldOrExt::Field(f) => f.is_packed(),
            FieldOrExt::Ext(_) => false,
        }
    }

    /// Returns the raw value of the `packed` field option from the descriptor:
    /// - `None`  — option absent (proto3 default applies)
    /// - `Some(true)`  — `[packed=true]` explicitly set
    /// - `Some(false)` — `[packed=false]` explicitly set
    ///
    /// Uses `prost_types::FieldDescriptorProto.options.packed: Option<bool>` directly —
    /// O(1), zero allocation (no DynamicMessage decoding).
    #[cfg(feature = "prost-bug-workaround")]
    pub(super) fn raw_packed_option(&self) -> Option<bool> {
        let proto = match self {
            FieldOrExt::Field(f) => f.field_descriptor_proto(),
            FieldOrExt::Ext(e) => e.field_descriptor_proto(),
        };
        proto.options.as_ref().and_then(|o| o.packed)
    }

    #[cfg(feature = "prost-bug-workaround")]
    pub(super) fn parent_file_syntax(&self) -> prost_reflect::Syntax {
        match self {
            FieldOrExt::Field(f) => f.parent_file().syntax(),
            FieldOrExt::Ext(e) => e.parent_file().syntax(),
        }
    }

    /// Append the name to use in field-line output directly to `out`.
    ///
    /// Regular field: `name`. Extension field: `[full.qualified.name]`.
    ///
    /// Writes rather than returns, because both callers
    /// (`wfl_prefix_n`/`wob_prefix_n`) only ever append it to a buffer, and
    /// the `String` this used to return was allocated, copied and dropped
    /// once per schema-named line — 2511 times on the 18 KB
    /// `fixtures/descriptor.pb`, under two doc comments that claimed the
    /// opposite.
    pub(super) fn write_display_name(&self, out: &mut Vec<u8>) {
        match self {
            FieldOrExt::Field(f) => out.extend_from_slice(f.name().as_bytes()),
            FieldOrExt::Ext(e) => {
                out.push(b'[');
                out.extend_from_slice(e.full_name().as_bytes());
                out.push(b']');
            }
        }
    }

    /// Returns the underlying `FieldDescriptor` if this is a regular field,
    /// or `None` for extension fields.
    ///
    /// Used to pass to functions that still take `Option<&FieldDescriptor>`.
    #[allow(dead_code)]
    pub(super) fn as_field(&self) -> Option<&FieldDescriptor> {
        match self {
            FieldOrExt::Field(f) => Some(f),
            FieldOrExt::Ext(_) => None,
        }
    }
}

/// Boxed JIT loader callback for `Any`/`MessageSet` type resolution (spec 0099).
pub type AnyLoader = Box<dyn FnMut(&str) -> Option<Arc<MessageDescriptor>>>;

/// Boxed JIT loader callback for extension resolution (spec 0248).
///
/// Called with the extendee's fully-qualified name and the field number seen
/// on the wire, only after both the schema's own fields and its already-known
/// extensions have missed. The file declaring an extension is in nobody's
/// dependency closure, so on a lazily loaded pool it is routinely absent.
pub type ExtLoader = Box<dyn FnMut(&str, u32) -> Option<ExtensionDescriptor>>;

// ── Render-mode state ─────────────────────────────────────────────────────────
//
// `CBL_START` is set to `out.len()` by `write_close_brace` before writing a
// `}\n` line, and reset to `out.len()` (past-end) by every other write.  It
// is currently unused beyond being maintained; the close-brace folding feature
// it was intended to support has been removed.
//
thread_local! {
    pub(super) static CBL_START:   Cell<usize> = const { Cell::new(0) };
    // Set once per `decode_and_render` call; read by every internal render fn.
    pub(super) static ANNOTATIONS: Cell<bool>  = const { Cell::new(false) };
    pub(super) static INDENT_SIZE: Cell<usize> = const { Cell::new(2) };
    // Tracks recursion depth; managed via `enter_level()` / `LevelGuard`.
    pub(super) static LEVEL:       Cell<usize> = const { Cell::new(0) };
    // When true, google.protobuf.Any fields are expanded inline (spec 0089).
    pub(super) static EXPAND_ANY:  Cell<bool>  = const { Cell::new(true) };
    // When true, fields absent from the schema are suppressed (spec 0103).
    pub(super) static HIDE_UNKNOWN: Cell<bool> = const { Cell::new(false) };
    // When true, MessageSet groups are expanded inline (spec 0103).
    pub(super) static EXPAND_MESSAGE_SET: Cell<bool> = const { Cell::new(true) };
    // Optional header lines injected after the magic line (e.g. # Type / # Score).
    pub static EXTRA_HEADER: RefCell<String> = const { RefCell::new(String::new()) };
    // JIT loader for Any/MessageSet type resolution (spec 0099).
    // Set by `set_any_loader` before rendering; cleared by `clear_any_loader` after.
    // Safety invariant: the raw pointer inside the Box is valid for the duration
    // of the rendering call that set it.  Always cleared before the setting
    // stack frame returns.
    pub(super) static ANY_LOADER: RefCell<Option<AnyLoader>> = const { RefCell::new(None) };
    // JIT loader for extension resolution (spec 0248).
    // Installed by `set_ext_loader`, whose guard clears it on drop — a render
    // can return early on a `CodecError`, and a stale loader would hand the
    // next render on this thread a dangling pointer.
    pub(super) static EXT_LOADER: RefCell<Option<ExtLoader>> = const { RefCell::new(None) };
    // Spec 0171: actual `render_message` recursion depth, capped at
    // `MAX_WIRE_DEPTH`. Distinct from `LEVEL`, which is the *indentation*
    // counter and is deliberately not maintained by every sink.
    pub(super) static DEPTH:       Cell<usize>         = const { Cell::new(0) };
}

/// Install a JIT loader for `Any` (and future `MessageSet`) type resolution
/// (spec 0099).  Must be paired with `clear_any_loader` after rendering.
///
/// # Safety
/// The caller guarantees that the closure (and any references it captures)
/// remains valid until `clear_any_loader` is called.
pub fn set_any_loader(loader: AnyLoader) {
    ANY_LOADER.with(|l| *l.borrow_mut() = Some(loader));
}

/// Clear the JIT loader installed by `set_any_loader`.
pub fn clear_any_loader() {
    ANY_LOADER.with(|l| *l.borrow_mut() = None);
}

/// Clears `EXT_LOADER` when dropped.
pub struct ExtLoaderGuard(());

impl Drop for ExtLoaderGuard {
    fn drop(&mut self) {
        EXT_LOADER.with(|l| *l.borrow_mut() = None);
    }
}

/// Install a JIT loader for extension resolution (spec 0248), for as long as
/// the returned guard lives.
///
/// # Safety
/// The caller guarantees that the closure (and any references it captures)
/// remains valid until the guard is dropped.
pub fn set_ext_loader(loader: ExtLoader) -> ExtLoaderGuard {
    EXT_LOADER.with(|l| *l.borrow_mut() = Some(loader));
    ExtLoaderGuard(())
}

/// RAII guard for `LEVEL`: increments on construction, decrements on drop.
/// Guarantees the level is restored even if the callee panics.
pub(super) struct LevelGuard;

impl Drop for LevelGuard {
    fn drop(&mut self) {
        LEVEL.with(|l| l.set(l.get() - 1));
    }
}

/// Enter one recursion level, tracked via the shared thread-local `LEVEL`
/// counter — but only when `sink.tracks_level()` says this sink actually
/// depends on it (see that method's doc comment). Returns `None` for sinks
/// like `ProbeSink` that must not mutate shared render-mode state.
fn enter_level<S: Sink>(sink: &S) -> Option<LevelGuard> {
    if sink.tracks_level() {
        LEVEL.with(|l| l.set(l.get() + 1));
        Some(LevelGuard)
    } else {
        None
    }
}

/// Render one nested node's body, unless spec 0249 S1's row budget is
/// already spent. Returns whether the body ran.
///
/// Called between the node's own `begin_nested` and `end_nested`, so a
/// node the budget stops at is still opened and closed: it keeps its
/// header line — byte for byte what the unbounded render writes there —
/// and its footer, and loses only its children. That is what makes it
/// foldable, and it is why the cut falls on a node boundary rather than
/// mid-line as a byte budget's would.
///
/// The caller reports the stop with `Sink::note_undescended` *after*
/// `end_nested`, when the node's span exists.
///
/// `enter_level` is inside, so the indentation counter is entered only
/// when something is going to be written at that level.
fn descend<S: Sink>(sink: &mut S, body: impl FnOnce(&mut S)) -> bool {
    if sink.row_budget_spent() {
        return false;
    }
    let _guard = enter_level(sink);
    body(sink);
    true
}

/// RAII guard for `DEPTH` (spec 0171): increments on construction,
/// decrements on drop, so an unwind cannot leave the counter high.
struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get() - 1));
    }
}

impl DepthGuard {
    /// Enter one `render_message` frame, or return `None` when doing so
    /// would exceed `MAX_WIRE_DEPTH`.
    ///
    /// Unlike `enter_level`, this is maintained by *every* sink, including
    /// `ProbeSink`, and that does not violate `ProbeSink`'s "must not
    /// disturb shared render-mode state" invariant: `DEPTH` counts real
    /// stack frames, a probe's frames sit on top of the outer render's, so
    /// the outer depth is exactly the right starting point — and the guard
    /// restores the previous value on the way out.
    ///
    /// Returning `None` is a *backstop*, not the mechanism. The two places
    /// that recurse — `render_len_field` and `render_message`'s
    /// `WT_START_GROUP` arm — consult [`at_depth_cap`] first and degrade
    /// locally, so this path should never be taken. It exists so that a
    /// future recursion site added without a matching check fails safely
    /// rather than exhausting the stack.
    fn enter() -> Option<DepthGuard> {
        // One `with` rather than a get/set pair: this runs on every
        // `render_message` call, and a schemaless render makes two of those
        // per nested message (the spec-0097 probe, then the real render).
        DEPTH.with(|c| {
            let d = c.get();
            if d >= MAX_WIRE_DEPTH {
                return None;
            }
            c.set(d + 1);
            Some(DepthGuard)
        })
    }
}

/// Whether another `render_message` frame would exceed `MAX_WIRE_DEPTH`
/// (spec 0171 §S4).
///
/// Consulted at each recursion site *before* anything is written, so the
/// over-deep node can be rendered opaquely in place while its siblings and
/// every enclosing level render normally.
#[inline]
pub(super) fn at_depth_cap() -> bool {
    DEPTH.with(Cell::get) >= MAX_WIRE_DEPTH
}

/// A declared length that cannot be re-encoded (spec 0311 S6).
///
/// `fill_placeholder` writes the restored length varint flush-right into
/// `varint_room_base(5) + ohb` bytes, and a sixth minimal byte writes over
/// the `next_placeholder` link with no panic — a wrong output rather than a
/// crash. Five varint bytes hold `2^35 - 1`.
///
/// Ordinarily unreachable, because a length is bounded by the buffer that
/// satisfied it. A *truncated* field's length is bounded by nothing: it is
/// a number read out of a file that, being cut, may be corrupt in other
/// ways too.
const MAX_REENCODABLE_LEN: u64 = 1 << 35;

/// Spec 0311 S1 and spec 0312 S1: whether a LEN field whose length prefix
/// overran should be handed to `render_len_field` over the bytes that are
/// present, rather than given up on as `TRUNCATED_BYTES` right here.
///
/// Two conditions say the field is one `render_len_field` has an answer
/// for:
///
/// - the schema declares a non-group message — the same test
///   `render_len_field` itself applies, so the two agree on what the
///   ordinary case is (spec 0311);
/// - or the schema declares nothing at all for this field number, in which
///   case the spec 0097 cascade decides, with the probe now able to forgive
///   a tail cut (spec 0312). Note that `render_len_field` may still decline;
///   S3 makes that decline emit the same `TRUNCATED_BYTES` this function's
///   `false` would have.
///
/// Three more are the absence of a way to lose bytes:
///
/// - the declared length is re-encodable (`MAX_REENCODABLE_LEN`);
/// - the sink reads LEN payloads opaquely (`ProbeSink`), in which case
///   `render_len_field` would return before the nested path and hand the
///   bytes back with no declared length — and, more to the point, the
///   malformity is exactly what the probe is there to see;
/// - the depth cap, where `render_len_field` deliberately drops the schema
///   and re-emits the payload as unknown bytes, which again has nowhere to
///   put the shortfall.
///
/// The rule is stated here, before anything is written, rather than left
/// for `render_len_field` to discover: spec 0174 G1's round-trip promise is
/// unconditional, and it holds only if the renderer never emits text the
/// encoder cannot honor.
fn descends_when_truncated<S: Sink>(
    field_schema: Option<&FieldOrExt>,
    length: u64,
    sink: &S,
) -> bool {
    let handled = match field_schema {
        Some(fs) => !fs.is_group() && matches!(fs.kind(), Kind::Message(_)),
        None => true,
    };
    handled && length < MAX_REENCODABLE_LEN && !sink.treat_len_as_opaque() && !at_depth_cap()
}

/// Return `true` when `data` is already rendered prototext text (fast-path).
pub fn is_prototext_text(data: &[u8]) -> bool {
    data.starts_with(PROTOTEXT_MAGIC)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Options for `decode_and_render`/`decode_and_render_indexed` (spec 0110
/// §8). Mirrors `RenderOpts` (`lib.rs`) plus two render-internal knobs
/// (`initial_level`, `emit_header`) not exposed by the public `RenderOpts`
/// API.
pub struct DecodeRenderOpts {
    /// Emit inline `#@ ...` annotations (wire type, field decl, modifiers).
    pub annotations: bool,
    /// Indentation step in spaces.
    pub indent_size: usize,
    /// Expand `google.protobuf.Any` fields inline (spec 0089).
    pub expand_any: bool,
    /// Suppress fields absent from the schema (spec 0103).
    pub hide_unknown_fields: bool,
    /// Expand MessageSet groups inline (spec 0103).
    pub expand_message_set: bool,
    /// Starting indentation depth, for sub-renders spliced into an existing
    /// document at a non-zero nesting level.
    pub initial_level: usize,
    /// Emit the `#@ prototext: protoc\n` header. `false` for sub-renders
    /// destined to be spliced into an existing document's text, which must
    /// not repeat the header.
    pub emit_header: bool,
    /// Spec 0249 S1: stop *descending* once this many rows have been
    /// emitted. `None` is the unbounded render every caller but a bounded
    /// one wants.
    ///
    /// A node reached with the budget spent is still opened and closed —
    /// it keeps its own header line, byte for byte what the unbounded
    /// render writes there, and its footer — but its body is empty, and
    /// `decode_and_render_indexed` reports it as undescended. The walk
    /// stays depth-first in document order, so the emitted rows are a
    /// true prefix of the unbounded render's, and the output is
    /// `row_budget` plus the breadth of the walk's right frontier: the
    /// unwind still emits the siblings it had not reached, one folded row
    /// each, so a consumer's line counts stay exact.
    pub row_budget: Option<usize>,
    /// Spec 0303 S3: when the outermost field being rendered is a
    /// TRUNCATED_BYTES node opened as a message, the number of bytes
    /// missing from the original declared length.  The renderer annotates
    /// the outermost `begin_nested` header with `TRUNCATED_MESSAGE; MISSING:
    /// N` (or `TRUNCATED_GROUP; MISSING: N` for group framing) so the count
    /// survives in prototext output and the encoder can reconstruct the
    /// original declared length on re-encode.  `None` for every normal
    /// render; only `splice_override` sets this, and only on the commit path.
    pub missing_payload_bytes: Option<u64>,
}

impl Default for DecodeRenderOpts {
    fn default() -> Self {
        DecodeRenderOpts {
            annotations: false,
            indent_size: 1,
            expand_any: true,
            hide_unknown_fields: false,
            expand_message_set: true,
            initial_level: 0,
            emit_header: false,
            row_budget: None,
            missing_payload_bytes: None,
        }
    }
}

/// Decode raw protobuf binary and render as protoc-style text in one pass.
///
/// Writes field lines into a pre-allocated `Vec<u8>`.  When `opts.annotations`
/// is true, a `#@ prototext: protoc\n` header is prepended; without
/// annotations the header is omitted (encode is not possible without field
/// annotations regardless).
///
/// `root_desc` is the already-resolved root message descriptor, if any (the
/// caller is responsible for resolving it from whatever pool it has — see
/// spec 0106 S4). `None` means no schema is active (`--raw`/no-descriptor
/// mode).
pub fn decode_and_render(
    buf: &[u8],
    root_desc: Option<&MessageDescriptor>,
    opts: DecodeRenderOpts,
) -> Vec<u8> {
    let DecodeRenderOpts {
        annotations,
        indent_size,
        expand_any,
        hide_unknown_fields,
        expand_message_set,
        initial_level,
        emit_header,
        row_budget,
        missing_payload_bytes: _, // not applicable to the non-indexed path
    } = opts;
    let capacity = buf.len() * 8;
    let mut sink = TextSink::new(capacity);
    sink.set_row_budget(row_budget);

    // Header — only emitted when annotations are on and the caller wants
    // one; without field-level annotations prototext encode cannot
    // reconstruct the binary anyway, so the header would be misleading.
    // `emit_header: false` is used for sub-renders destined to be spliced
    // into an existing document's text, which must not repeat the header.
    if annotations && emit_header {
        sink.write_header(b"#@ prototext: protoc\n");
    }
    EXTRA_HEADER.with(|h| {
        let h = h.borrow();
        if !h.is_empty() {
            sink.write_header(h.as_bytes());
        }
    });
    // Initialise render-mode state.
    // CBL_START past the end so the first write_close_brace always takes
    // the fresh-write path.
    CBL_START.with(|c| c.set(sink.out.len()));
    ANNOTATIONS.with(|c| c.set(annotations));
    INDENT_SIZE.with(|c| c.set(indent_size));
    LEVEL.with(|c| c.set(initial_level));
    EXPAND_ANY.with(|c| c.set(expand_any));
    HIDE_UNKNOWN.with(|c| c.set(hide_unknown_fields));
    EXPAND_MESSAGE_SET.with(|c| c.set(expand_message_set));
    DEPTH.with(|c| c.set(0));

    let schema_present = root_desc.is_some();

    // Spec 0312 S2: the document's own end is the one place `true` is
    // written down. Every nested frame derives its answer from this.
    render_message(buf, 0, None, true, root_desc, schema_present, &mut sink);

    sink.into_inner()
}

/// The output of [`decode_and_render_indexed`].
///
/// A named struct rather than a tuple so that a future addition to the
/// output is not itself a breaking change — this signature has already been
/// widened twice (spec 0212 S5).
#[derive(Debug)]
pub struct IndexedRender {
    /// The rendered text, byte-for-byte what `decode_and_render` produces
    /// for the same input.
    pub text: Vec<u8>,
    /// One span per node, in post-order (a container follows all of its
    /// descendants).
    pub spans: Vec<NodeSpan>,
    /// Spec 0249 S1: indices into `spans` of the nested nodes
    /// `DecodeRenderOpts::row_budget` stopped at — emitted with their own
    /// header and footer but no body. Always empty when `row_budget` is
    /// `None`, which is every caller that has not asked to be bounded.
    ///
    /// Ascending, and a consumer can map each one to its own structure
    /// with the same span-to-node correspondence it uses for `spans`.
    pub undescended: Vec<u32>,
}

/// Sibling to `decode_and_render`, sharing the exact same parameter list,
/// but internally rendering through an `IndexingTextSink` instead of a bare
/// `TextSink`, and returning both the rendered text and its `NodeSpan`
/// index alongside it (spec 0110 §3). `decode_and_render` itself stays
/// `TextSink`-only: its production callers have no use for the index and
/// shouldn't pay `IndexingTextSink`'s small extra bookkeeping cost.
///
/// `fqdns` is the table each node's `type_fqdn` is interned into. It is the
/// caller's, not this function's, because a `FqdnId` is only meaningful
/// against the table that produced it: a caller that renders a sub-document
/// and splices its spans into a larger one — `protolens` does exactly that
/// on every override — must pass the same table both times, or the two sets
/// of ids will silently disagree about what type `3` is (spec 0212 S4).
///
/// Fails if `buf` exceeds `MAX_INDEXED_BUFFER`, the bound that keeps a
/// `NodeSpan`'s `u32` offsets sound. This replaces an abort: the
/// unconditional 8× output reservation below already made a buffer that
/// size fatal, just without saying so.
pub fn decode_and_render_indexed(
    buf: &[u8],
    root_desc: Option<&MessageDescriptor>,
    fqdns: &mut FqdnTable,
    opts: DecodeRenderOpts,
) -> Result<IndexedRender, CodecError> {
    if buf.len() > MAX_INDEXED_BUFFER {
        return Err(CodecError::InputTooLarge {
            len: buf.len(),
            max: MAX_INDEXED_BUFFER,
        });
    }
    let DecodeRenderOpts {
        annotations,
        indent_size,
        expand_any,
        hide_unknown_fields,
        expand_message_set,
        initial_level,
        emit_header,
        row_budget,
        missing_payload_bytes,
    } = opts;
    let capacity = buf.len() * 8;
    let mut sink = IndexingTextSink::new(capacity, fqdns);
    sink.set_row_budget(row_budget);
    if let Some(missing) = missing_payload_bytes {
        sink.set_missing_payload_bytes(missing);
    }

    if annotations && emit_header {
        sink.write_header(b"#@ prototext: protoc\n");
    }
    EXTRA_HEADER.with(|h| {
        let h = h.borrow();
        if !h.is_empty() {
            sink.write_header(h.as_bytes());
        }
    });
    CBL_START.with(|c| c.set(sink.out_len()));
    ANNOTATIONS.with(|c| c.set(annotations));
    INDENT_SIZE.with(|c| c.set(indent_size));
    LEVEL.with(|c| c.set(initial_level));
    EXPAND_ANY.with(|c| c.set(expand_any));
    HIDE_UNKNOWN.with(|c| c.set(hide_unknown_fields));
    EXPAND_MESSAGE_SET.with(|c| c.set(expand_message_set));
    DEPTH.with(|c| c.set(0));

    let schema_present = root_desc.is_some();

    // Spec 0312 S2: the document's own end is the one place `true` is
    // written down. Every nested frame derives its answer from this.
    render_message(buf, 0, None, true, root_desc, schema_present, &mut sink);

    let (text, spans, undescended) = sink.into_parts();
    Ok(IndexedRender {
        text,
        spans,
        undescended,
    })
}

// ── Core recursive render-while-decode ───────────────────────────────────────

/// Parse and render one protobuf message into `sink`.
///
/// Returns `(next_pos, group_end_tag)`:
/// - `next_pos`: byte position after this message (for the caller to
///   continue its own parse loop, or for GROUP end detection).
/// - `group_end_tag`: `Some(tag)` when parsing terminated on a `WT_END_GROUP`.
///
/// `frame_ends_at_eof` (spec 0312 S2) says `buf`'s end is where the
/// available bytes stop, rather than a boundary an enclosing length prefix
/// declared. It is the renderer's copy of the one rule protolens spells
/// `override_pane::ends_where_the_bytes_end` and the scoring walk spells
/// `ScoringOpts::end_undeclared`; the three must not drift. It is a
/// parameter and not a thread-local offset because it is frame-relative by
/// construction — a nested payload resets the coordinate frame, so an
/// offset would not survive the descent.
fn render_message<'a, S: Sink>(
    buf: &'a [u8],
    start: usize,
    my_group: Option<u64>,
    frame_ends_at_eof: bool,
    schema: Option<&MessageDescriptor>,
    schema_present: bool,
    sink: &mut S,
) -> (usize, Option<WiretagResult<'a>>) {
    let buflen = buf.len();
    let mut pos = start;

    // Spec 0171 backstop. Nesting depth on the wire is bounded only by the
    // input's length — a LEN level costs two bytes, a group level one — so
    // without a cap a 1 MB blob can demand hundreds of thousands of stack
    // frames. The cap is normally applied at the recursion sites, which
    // degrade the one over-deep node to opaque bytes and carry on; reaching
    // here means a recursion site is missing its `at_depth_cap` check, and
    // all we can still do is hand the remainder back verbatim.
    let Some(_depth_guard) = DepthGuard::enter() else {
        sink.malformed(
            0,
            TagFacts::default(),
            MalformedKind::InvalidTagType,
            &buf[start..],
            start..buflen,
        );
        return (buflen, None);
    };

    loop {
        if pos == buflen {
            return (pos, None);
        }

        // ── Parse wire tag ────────────────────────────────────────────────────

        let field_start = pos;
        let tag = parse_wiretag(buf, pos);

        if let Some(wtag_gar) = tag.wtag_gar {
            // Invalid wire tag: consume rest of buffer as INVALID_TAG_TYPE
            sink.malformed(
                0,
                TagFacts::default(),
                MalformedKind::InvalidTagType,
                wtag_gar,
                field_start..buflen,
            );
            return (buflen, None);
        }

        let field_number = tag.wfield.unwrap();
        let wire_type = tag.wtype.unwrap();
        let tag_ohb = tag.wfield_ohb;
        let tag_oor = tag.wfield_oor.is_some();
        pos = tag.next_pos;

        // ── Schema lookup ─────────────────────────────────────────────────────

        let field_schema: Option<FieldOrExt> = schema.and_then(|s| {
            if let Some(f) = s.get_field(field_number as u32) {
                Some(FieldOrExt::Field(f))
            } else if let Some(e) = s.get_extension(field_number as u32) {
                Some(FieldOrExt::Ext(e))
            } else {
                // Spec 0248: the file declaring an extension is in nobody's
                // dependency closure, so on a lazily loaded pool `s` routinely
                // does not have it yet. Being inside `schema.and_then` is what
                // keeps the schema-less walkers (`ProbeSink`, `ArenaSink`, raw
                // mode) from ever reaching this.
                EXT_LOADER
                    .with(|l| {
                        l.borrow_mut()
                            .as_mut()
                            .and_then(|load| load(s.full_name(), field_number as u32))
                    })
                    .map(FieldOrExt::Ext)
            }
        });

        // ── Wire-type dispatch ────────────────────────────────────────────────

        match wire_type {
            // ── VARINT ───────────────────────────────────────────────────────
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if let Some(varint_gar) = vr.varint_gar {
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidVarint,
                        varint_gar,
                        field_start..buflen,
                    );
                    return (buflen, None);
                }
                pos = vr.next_pos;
                let val_ohb = vr.varint_ohb;
                let val = vr.varint.unwrap();

                sink.scalar_field(
                    field_number,
                    field_schema.as_ref(),
                    TagFacts {
                        tag_ohb,
                        tag_oor,
                        len_ohb: None,
                    },
                    ScalarValue::Varint {
                        raw_val: val,
                        val_ohb,
                    },
                    field_start..pos,
                    schema_present,
                );
            }

            // ── FIXED64 ──────────────────────────────────────────────────────
            WT_I64 => {
                let Some(end) = payload_end(pos, 8, buflen) else {
                    let raw = &buf[pos..];
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidFixed64,
                        raw,
                        field_start..buflen,
                    );
                    return (buflen, None);
                };
                let mut data = [0u8; 8];
                data.copy_from_slice(&buf[pos..end]);
                pos = end;

                sink.scalar_field(
                    field_number,
                    field_schema.as_ref(),
                    TagFacts {
                        tag_ohb,
                        tag_oor,
                        len_ohb: None,
                    },
                    ScalarValue::Fixed64(data),
                    field_start..pos,
                    schema_present,
                );
            }

            // ── LENGTH-DELIMITED ─────────────────────────────────────────────
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if let Some(varint_gar) = lr.varint_gar {
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidLen,
                        varint_gar,
                        field_start..buflen,
                    );
                    return (buflen, None);
                }
                let len_ohb = lr.varint_ohb;
                pos = lr.next_pos;
                let length = lr.varint.unwrap();

                let Some(end) = payload_end(pos, length, buflen) else {
                    // Spec 0311 S1: the bounds check consults the schema
                    // before giving up on the field. A declared non-group
                    // message is descended into over the bytes that are
                    // present, carrying the shortfall so the header can say
                    // it and the re-encode can restore the declared length.
                    // Spec 0312 S1 adds the field with no schema at all,
                    // where the cascade decides instead.
                    let missing = bytes_missing(pos, length, buflen);
                    if descends_when_truncated(field_schema.as_ref(), length, sink) {
                        render_len_field(
                            FieldCtx {
                                field_number,
                                field_schema: field_schema.as_ref(),
                                tag: TagFacts {
                                    tag_ohb,
                                    tag_oor,
                                    len_ohb,
                                },
                                // The payload is `buf[pos..buflen]`, so it
                                // ends exactly where this frame does.
                                frame_ends_at_eof,
                            },
                            schema_present,
                            field_start..buflen,
                            &buf[pos..buflen],
                            Some(missing),
                            sink,
                        );
                        return (buflen, None);
                    }
                    let raw = &buf[pos..];
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb,
                        },
                        MalformedKind::TruncatedBytes { missing },
                        raw,
                        field_start..buflen,
                    );
                    return (buflen, None);
                };
                let data = &buf[pos..end];
                pos = end;

                render_len_field(
                    FieldCtx {
                        field_number,
                        field_schema: field_schema.as_ref(),
                        tag: TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb,
                        },
                        // Spec 0312 S2: the payload inherits the answer only
                        // if it reaches this frame's own end. A satisfied
                        // length prefix followed by more bytes is precisely
                        // the case that must not be forgiven.
                        frame_ends_at_eof: frame_ends_at_eof && end == buflen,
                    },
                    schema_present,
                    field_start..pos,
                    data,
                    None,
                    sink,
                );
            }

            // ── START GROUP ──────────────────────────────────────────────────
            //
            // Spec 0171 §S4: at the depth cap, do not recurse. A group has
            // no length prefix, so its extent has to be found by scanning
            // to the matching END_GROUP — iteratively, hence no stack cost.
            // The whole span, its own tag included, is then handed back
            // verbatim as INVALID_TAG_TYPE (the one production that
            // re-encodes tagless, so this round-trips byte-for-byte) and the
            // loop continues, leaving every sibling and every enclosing
            // level to render normally.
            //
            // `None` for the expected close: the field number on the closing
            // tag does not affect the extent, and demanding a match would be
            // stricter than the uncapped path — which tolerates a mismatch
            // and records END_MISMATCH — while costing every following
            // sibling on the fallback to `buflen`.
            WT_START_GROUP if at_depth_cap() => {
                let end = scan_group_extent(buf, pos, None).unwrap_or(buflen);
                sink.malformed(
                    0,
                    TagFacts::default(),
                    MalformedKind::InvalidTagType,
                    &buf[field_start..end],
                    field_start..end,
                );
                pos = end;
            }

            WT_START_GROUP => {
                render_group_field(
                    buf,
                    &mut pos,
                    FieldCtx {
                        field_number,
                        field_schema: field_schema.as_ref(),
                        tag: TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        // A group has no length prefix: it continues in this
                        // same buffer, so it ends where this frame ends.
                        frame_ends_at_eof,
                    },
                    schema_present,
                    field_start,
                    sink,
                );
            }

            // ── END GROUP ────────────────────────────────────────────────────
            WT_END_GROUP => {
                if my_group.is_none() {
                    // Unexpected END_GROUP outside a group
                    let raw = &buf[pos..];
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidGroupEnd,
                        raw,
                        field_start..buflen,
                    );
                    return (buflen, None);
                }
                // Valid END_GROUP: return to parent without rendering a field.
                return (pos, Some(tag));
            }

            // ── FIXED32 ──────────────────────────────────────────────────────
            WT_I32 => {
                let Some(end) = payload_end(pos, 4, buflen) else {
                    let raw = &buf[pos..];
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidFixed32,
                        raw,
                        field_start..buflen,
                    );
                    return (buflen, None);
                };
                let mut data = [0u8; 4];
                data.copy_from_slice(&buf[pos..end]);
                pos = end;

                sink.scalar_field(
                    field_number,
                    field_schema.as_ref(),
                    TagFacts {
                        tag_ohb,
                        tag_oor,
                        len_ohb: None,
                    },
                    ScalarValue::Fixed32(data),
                    field_start..pos,
                    schema_present,
                );
            }

            _ => unreachable!("wire type > 5 caught by parse_wiretag"),
        }
    }
}

// ── Tests: `decode_and_render`'s `initial_level`/`emit_header` params ─────────

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;

    // field 1 (varint) = 42: tag 0x08, value 0x2A.
    const VARINT_FIELD: [u8; 2] = [0x08, 0x2A];

    /// Spec 0212 S1: over the cap the render refuses instead of aborting
    /// the process on the 8× output reservation, and instead of wrapping a
    /// `NodeSpan` offset to a plausible-looking wrong value.
    ///
    /// The half-gigabyte buffer costs address space, not memory: it is
    /// zero-allocated, so the pages are never faulted in, and the size
    /// check runs before anything reads a byte of it.
    #[test]
    fn a_buffer_over_the_cap_is_refused() {
        let oversize = vec![0u8; MAX_INDEXED_BUFFER + 1];
        let mut fqdns = FqdnTable::new();
        let err =
            decode_and_render_indexed(&oversize, None, &mut fqdns, DecodeRenderOpts::default())
                .expect_err("over the cap");
        match err {
            CodecError::InputTooLarge { len, max } => {
                assert_eq!(len, MAX_INDEXED_BUFFER + 1);
                assert_eq!(max, MAX_INDEXED_BUFFER);
            }
            other => panic!("expected InputTooLarge, got {other:?}"),
        }
        // The message names the actual size, so a user can tell how far
        // over they are rather than only that they are over.
        assert!(err_text(MAX_INDEXED_BUFFER + 1).contains(&(MAX_INDEXED_BUFFER + 1).to_string()));
    }

    /// Counts nested openings, and nothing else. `greedy` is what
    /// `Sink::unknown_len_is_message` reports, so one type covers both sides
    /// of the comparison.
    #[derive(Default)]
    struct NestCounter {
        greedy: bool,
        nested: usize,
    }

    impl Sink for NestCounter {
        type Mark = ();

        fn scalar_field(
            &mut self,
            _field_number: u64,
            _field_schema: Option<&FieldOrExt>,
            _tag: TagFacts,
            _value: sink::ScalarValue<'_>,
            _raw_range: Range<usize>,
            _schema_present: bool,
        ) {
        }

        fn begin_nested(
            &mut self,
            _field_number: u64,
            _field_schema: Option<&FieldOrExt>,
            _tag: TagFacts,
            _kind: sink::NestedKind,
            _raw_start: usize,
            _payload_start: usize,
        ) {
            self.nested += 1;
        }

        fn end_nested(
            &mut self,
            _mark: (),
            _raw_range: Range<usize>,
            _close_facts: Option<sink::GroupCloseFacts>,
        ) {
        }

        fn virtual_scalar(
            &mut self,
            _name: &str,
            _annotation: Option<&str>,
            _value_str: &str,
            _raw_range: Range<usize>,
        ) {
        }

        fn begin_virtual_nested(
            &mut self,
            _name: &str,
            _annotation: Option<&str>,
            _type_fqdn: Option<&str>,
            _raw_start: usize,
            _payload_start: usize,
        ) {
        }

        fn malformed(
            &mut self,
            _field_number: u64,
            _tag: TagFacts,
            _kind: sink::MalformedKind,
            _raw: &[u8],
            _raw_range: Range<usize>,
        ) {
        }

        fn unknown_len_is_message(&self) -> bool {
            self.greedy
        }
    }

    /// Spec 0216 S14. Field 1, LEN, payload `"hello"` — five bytes that are
    /// a string, not a message: read as wire format they give field 13 as a
    /// varint and then an unmatched `END_GROUP`, so spec 0097's probe
    /// declines them and the default cascade renders the field as a scalar
    /// with no children.
    ///
    /// That verdict is exactly what the maximal-tree walk cannot accept: a
    /// later type override could declare this field a message, and the
    /// render would then descend into a payload the arena never gave slots
    /// to. Asking for `unknown_len_is_message` recurses instead.
    #[test]
    fn greedy_recurses_where_the_probe_declines() {
        let buf: &[u8] = &[0x0A, 0x05, b'h', b'e', b'l', b'l', b'o'];

        let mut probing = NestCounter::default();
        render_message(buf, 0, None, false, None, false, &mut probing);
        assert_eq!(probing.nested, 0, "the probe declines `hello`");

        let mut greedy = NestCounter {
            greedy: true,
            nested: 0,
        };
        render_message(buf, 0, None, false, None, false, &mut greedy);
        assert_eq!(greedy.nested, 1, "greedy opens it regardless");
    }

    /// Spec 0249 S1, and the property everything after it assumes: a
    /// row-budgeted render emits the *same bytes* as the unbounded one for
    /// as far as it goes, because the two use one renderer and the budget
    /// only decides whether to descend.
    ///
    /// The two renders are not prefixes of each other outright — the first
    /// undescended node closes immediately instead of holding its
    /// children — so the test locates the first differing line and pins
    /// three things about it: it is at or past the budget, it is a closing
    /// brace, and the node it closes was reported as undescended.
    #[test]
    fn a_row_budgeted_render_is_the_start_of_the_full_one() {
        let pb: &[u8] = include_bytes!("../../../fixtures/descriptor.pb");
        let schema = crate::parse_schema(pb, "google.protobuf.FileDescriptorSet")
            .expect("descriptor.pb is self-describing");
        let desc = schema.root_descriptor();

        let render = |budget: Option<usize>| {
            let mut fqdns = FqdnTable::new();
            decode_and_render_indexed(
                pb,
                desc.as_ref(),
                &mut fqdns,
                DecodeRenderOpts {
                    annotations: true,
                    indent_size: 2,
                    row_budget: budget,
                    ..DecodeRenderOpts::default()
                },
            )
            .expect("descriptor.pb is well within MAX_INDEXED_BUFFER")
        };

        let full = render(None);
        assert!(
            full.undescended.is_empty(),
            "an unbounded render stops at nothing"
        );
        let full_lines: Vec<&str> = std::str::from_utf8(&full.text).unwrap().lines().collect();

        for budget in [1usize, 2, 5, 20, 100] {
            let bounded = render(Some(budget));
            let text = std::str::from_utf8(&bounded.text).unwrap();
            let lines: Vec<&str> = text.lines().collect();

            assert!(
                !bounded.undescended.is_empty(),
                "budget {budget} is far under {} lines",
                full_lines.len()
            );
            assert!(
                lines.len() < full_lines.len(),
                "budget {budget} bounded nothing"
            );

            let diff = (0..lines.len().min(full_lines.len()))
                .find(|&i| lines[i] != full_lines[i])
                .expect("a bounded render cannot equal the full one line for line");

            // Every line before the cut is the full render's own byte for
            // byte — that is the whole point of bounding rows rather than
            // bytes, and it is what makes the frame after a confirm final.
            assert_eq!(&lines[..diff], &full_lines[..diff]);
            assert!(
                diff >= budget,
                "budget {budget} diverged at line {diff}, short of what was asked for"
            );
            assert!(
                lines[diff].trim_start().starts_with('}'),
                "budget {budget}: line {diff} is {:?}, not the close of an undescended node",
                lines[diff]
            );

            // The cut is reported, and it is reported against a node whose
            // body is exactly the two lines the renderer wrote for it.
            let stopped: Vec<&NodeSpan> = bounded
                .undescended
                .iter()
                .map(|&i| &bounded.spans[i as usize])
                .collect();
            assert!(stopped.iter().all(|s| s.is_message));
            assert!(
                stopped
                    .iter()
                    .all(|s| s.text_range.end - s.text_range.start == 2),
                "an undescended node is its header and its derived footer"
            );
            assert!(
                stopped
                    .iter()
                    .any(|s| s.text_range.end as usize == diff + 1),
                "the first line not emitted belongs to a node reported as undescended"
            );
            assert!(
                bounded.undescended.windows(2).all(|w| w[0] < w[1]),
                "the report is ascending, so a consumer can zip it with the spans"
            );
        }
    }

    fn err_text(len: usize) -> String {
        CodecError::InputTooLarge {
            len,
            max: MAX_INDEXED_BUFFER,
        }
        .to_string()
    }

    /// Spec 0212 S4: the table is shared precisely so that spans from two
    /// different renders can be compared by id. A per-call table would
    /// make this assertion fail while every span still looked valid.
    #[test]
    fn one_table_makes_two_renders_ids_comparable() {
        let pb: &[u8] = include_bytes!("../../../fixtures/descriptor.pb");
        let schema = crate::parse_schema(pb, "google.protobuf.FileDescriptorSet")
            .expect("descriptor.pb is self-describing");
        let desc = schema.root_descriptor();
        let mut fqdns = FqdnTable::new();
        let opts = || DecodeRenderOpts {
            annotations: true,
            ..Default::default()
        };
        let a = decode_and_render_indexed(pb, desc.as_ref(), &mut fqdns, opts())
            .expect("within the cap");
        let b = decode_and_render_indexed(pb, desc.as_ref(), &mut fqdns, opts())
            .expect("within the cap");

        let want = fqdns.id_of("google.protobuf.FileDescriptorProto");
        assert_ne!(want, UNINTERNED, "the fixture contains this type");
        let count = |r: &IndexedRender| r.spans.iter().filter(|s| s.type_fqdn == want).count();
        assert!(count(&a) > 0);
        assert_eq!(count(&a), count(&b));
        // The second render interned nothing new.
        let before = fqdns.len();
        let mut once_more = FqdnTable::new();
        let _ = decode_and_render_indexed(pb, desc.as_ref(), &mut once_more, opts());
        assert_eq!(before, once_more.len());
    }

    /// Spec 0173 S4: `FieldOrExt::write_display_name` replaced a
    /// `display_name() -> String` that allocated once per schema-named
    /// line. The change is meant to be invisible in the output, and the
    /// benches' own fixtures are the widest schema-named corpus committed
    /// to this repo — every well-known type, 2511 named lines — so pin the
    /// rendering against them byte for byte.
    ///
    /// Extension names (`[pkg.ext]`, the other arm of `write_display_name`)
    /// are not reachable from a `FileDescriptorSet` payload; they are
    /// covered by `prototext/tests/roundtrip.rs`'s `[acme.blade_count]`
    /// assertion.
    #[test]
    fn descriptor_fixture_renders_byte_for_byte() {
        let pb: &[u8] = include_bytes!("../../../fixtures/descriptor.pb");
        let expected: &[u8] = include_bytes!("../../../fixtures/descriptor_protoc.txt");
        let schema = crate::parse_schema(pb, "google.protobuf.FileDescriptorSet")
            .expect("descriptor.pb is self-describing");
        let out = decode_and_render(
            pb,
            schema.root_descriptor().as_ref(),
            DecodeRenderOpts {
                annotations: true,
                emit_header: true,
                ..Default::default()
            },
        );
        assert_eq!(
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(expected)
        );
    }

    #[test]
    fn initial_level_indents_output() {
        let out = decode_and_render(
            &VARINT_FIELD,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                initial_level: 3,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        let first_line = text.lines().next().unwrap();
        let indent = first_line.len() - first_line.trim_start().len();
        assert_eq!(indent, 2 * 3); // indent_size * initial_level
    }

    #[test]
    fn initial_level_zero_matches_default() {
        let out = decode_and_render(
            &VARINT_FIELD,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        let first_line = text.lines().next().unwrap();
        assert_eq!(first_line, "1: 42");
    }

    #[test]
    fn emit_header_true_writes_header() {
        let out = decode_and_render(
            &VARINT_FIELD,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                emit_header: true,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("#@ prototext: protoc\n"));
    }

    #[test]
    fn emit_header_false_suppresses_header() {
        let out = decode_and_render(
            &VARINT_FIELD,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(!text.starts_with("#@ prototext: protoc\n"));
    }

    // ── `ProbeSink` (spec 0110 Step 4 / Open Issue #1) ─────────────────────

    #[test]
    fn probe_sink_recognizes_valid_nested_message() {
        // field 1 (unknown, LEN) whose payload is itself a well-formed
        // message: field 1 (varint) = 42.
        let buf = [0x0A, 0x02, 0x08, 0x2A];
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "1 {\n  1: 42\n}\n");
    }

    #[test]
    fn probe_sink_rolls_up_nested_group_malformity() {
        // field 1 (unknown, LEN) whose payload is: a GROUP (field 5) opened,
        // containing a field 1 varint with a truncated (garbage) varint byte
        // (0x80, continuation bit set, no terminating byte).  The nested
        // group's own malformity must roll up into the outer probe's count
        // (spec 0110 Open Issue #1), causing Step 1 (nested-message probe) to
        // fail and fall through to the raw-bytes fallback — rather than being
        // incorrectly accepted as a valid nested message.
        let payload = [0x2B, 0x08, 0x80];
        let mut buf = vec![0x0A, payload.len() as u8];
        buf.extend_from_slice(&payload);
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        // Fallback rendering (invalid-UTF-8 payload -> escaped bytes leaf),
        // not a nested `1 { ... }` block.
        assert!(!text.starts_with("1 {"), "got: {text}");
        assert!(text.starts_with("1: \""), "got: {text}");
    }

    #[test]
    fn probe_sink_rejects_a_group_that_never_closes() {
        // A group carries no length prefix, so an unterminated one is found
        // only by parsing to the end of the buffer — at which point the
        // probe's other two tests are both satisfied: nothing called
        // `malformed`, and the group consumed every remaining byte so
        // `next_pos == data.len()`. Counting the missing `END_GROUP` is the
        // only thing standing between this payload and a nested render.
        //
        // The payload is the real string that exposed this: an unknown
        // `google.api.method_signature` on a `MethodOptions`. Its bytes
        // parse as fixed64 `d_break,`, fixed32 `pdat`, fixed32 `_mas`, and
        // then a trailing `k` — 0x6b, which is field 13, START_GROUP.
        let payload = b"ad_break,update_mask";
        let mut buf = vec![0x0A, payload.len() as u8];
        buf.extend_from_slice(payload);
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "1: \"ad_break,update_mask\"\n");
    }

    // ── Any invalid token disqualifies the payload (spec 0266) ──────────────

    /// Render an unknown LEN field carrying `payload`, at indent 2.
    fn render_unknown_len(payload: &[u8]) -> String {
        let mut buf = vec![0x0A];
        buf.push(u8::try_from(payload.len()).expect("fixture payload is short"));
        buf.extend_from_slice(payload);
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                ..Default::default()
            },
        );
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_string_that_opens_a_group_is_not_a_message() {
        // The reported payload. Every "field" in it is a letter: `A` (0x41)
        // is a FIXED64 tag for field 8, `P` (0x50) a VARINT tag for field 10
        // with `D` as its value, and the pair `C` `T` is START_GROUP field 8
        // followed by END_GROUP field 10 — which closes, consuming the last
        // byte, so the only thing standing between this string and a nested
        // render is that the close does not match the open.
        let text = render_unknown_len(b"ANALYST_UPDATE_VERDICT");
        assert_eq!(text, "1: \"ANALYST_UPDATE_VERDICT\"\n");
    }

    #[test]
    fn a_mismatched_group_end_fails_the_probe() {
        // `C` opens a group on field 8, `T` closes one on field 10. Nothing
        // else is wrong: both field numbers are in range and every byte is
        // consumed. `END_MISMATCH` is invalid, so this is a string.
        assert_eq!(render_unknown_len(b"CT"), "1: \"CT\"\n");
    }

    #[test]
    fn an_out_of_range_field_number_fails_the_probe() {
        // `0x00` is a tag for field 0 — a VARINT, whose value is the second
        // NUL. Without `TAG_OOR` counting, every NUL in a string helps that
        // string pass for a message.
        let text = render_unknown_len(b"\x00\x00");
        assert!(!text.starts_with("1 {"), "got: {text}");
        assert!(text.starts_with("1: \""), "got: {text}");
    }

    #[test]
    fn a_mismatched_end_tag_out_of_range_fails_the_probe() {
        // `ETAG_OOR` is not writable on its own — an end tag whose field
        // number is out of range cannot match the open tag either, so it
        // always arrives together with `END_MISMATCH: 536870912`. Here a
        // group opens on field 1 and closes with the five-byte tag for
        // field 2^29, one past the last legal field number.
        let payload = [0x0B, 0x84, 0x80, 0x80, 0x80, 0x10];
        let text = render_unknown_len(&payload);
        assert!(!text.starts_with("1 {"), "got: {text}");
    }

    #[test]
    fn an_over_encoded_tag_still_probes_as_a_message() {
        // The other half of the rule: a non-canonical token does *not*
        // disqualify. `0x88 0x00` is the field-1 VARINT tag written in two
        // bytes instead of one — legal protobuf, round-trips exactly, and
        // the render says so in lower case (`tag_ohb`). A payload from an
        // eccentric but working encoder is still a message.
        let buf = [0x0A, 0x03, 0x88, 0x00, 0x2A];
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("1 {"), "got: {text}");
        assert!(text.contains("tag_ohb"), "got: {text}");
    }

    // ── Bounds arithmetic and depth caps (spec 0171) ────────────────────────

    /// Build a one-file `ParsedSchema` from the given message descriptors,
    /// rooted at `root_msg_name`. Mirrors `schema.rs`'s own test-fixture
    /// convention.
    fn build_schema(
        message_type: Vec<prost_types::DescriptorProto>,
        root_msg_name: &str,
    ) -> crate::schema::ParsedSchema {
        use prost::Message as ProstMessage;
        let file = prost_types::FileDescriptorProto {
            name: Some("test_render_text.proto".into()),
            syntax: Some("proto2".into()),
            message_type,
            ..Default::default()
        };
        let fds = prost_types::FileDescriptorSet { file: vec![file] };
        let mut buf = Vec::new();
        fds.encode(&mut buf).unwrap();
        crate::schema::parse_schema(&buf, root_msg_name).unwrap()
    }

    /// Encode `v` as a canonical protobuf varint, appended to `out`.
    fn push_varint(v: u64, out: &mut Vec<u8>) {
        let mut v = v;
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    #[test]
    fn len_prefix_near_u64_max_does_not_panic() {
        // Field 1, wire type LEN, with a length prefix of `u64::MAX`. The
        // old `pos + length > buflen` check wrapped to 10 in release mode,
        // so the guard passed and `&buf[11..10]` panicked with
        // "slice index starts at 11 but ends at 10".
        let mut buf = vec![0x0A];
        push_varint(u64::MAX, &mut buf);
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("TRUNCATED_BYTES"), "got: {text}");
        assert!(
            text.contains(&format!("MISSING: {}", u64::MAX)),
            "got: {text}"
        );
    }

    #[test]
    fn deeply_nested_len_does_not_overflow_the_stack() {
        // 2 000 levels of `field 1 (LEN) { ... }`, twice the
        // `MAX_WIRE_DEPTH` cap, wrapping a varint leaf.
        let mut payload = vec![0x08, 0x2A]; // field 1 (varint) = 42
        for _ in 0..2000 {
            let mut wrapped = vec![0x0A];
            push_varint(payload.len() as u64, &mut wrapped);
            wrapped.extend_from_slice(&payload);
            payload = wrapped;
        }
        let out = decode_and_render(
            &payload,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.lines().count() < 2000,
            "got {} lines",
            text.lines().count()
        );
        assert!(text.starts_with("1 {"), "got: {text}");
    }

    #[test]
    fn deeply_nested_len_degrades_to_bytes_at_the_cap() {
        // `Node { optional Node child = 1; }` — self-recursive, so every
        // level has a schema and `render_len_field` recurses directly with
        // no spec-0097 probe in the way. This is where the LEN decision
        // site is observable: `render_len_field` running at `MAX_WIRE_DEPTH`
        // must render its payload opaquely instead of opening one more
        // message.
        use prost_types::field_descriptor_proto::{Label, Type};
        use prost_types::{DescriptorProto, FieldDescriptorProto};

        let node = DescriptorProto {
            name: Some("Node".into()),
            field: vec![FieldDescriptorProto {
                name: Some("child".into()),
                number: Some(1),
                r#type: Some(Type::Message as i32),
                type_name: Some(".Node".into()),
                label: Some(Label::Optional as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let schema = build_schema(vec![node], "Node");
        let root = schema.root_descriptor().unwrap();

        let mut payload = Vec::new();
        for _ in 0..2000 {
            let mut wrapped = vec![0x0A];
            push_varint(payload.len() as u64, &mut wrapped);
            wrapped.extend_from_slice(&payload);
            payload = wrapped;
        }
        let out = decode_and_render(
            &payload,
            Some(&root),
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        // The root `render_message` frame is depth 1, and each opened brace
        // costs one more, so the last `render_len_field` free to recurse
        // runs at depth `MAX_WIRE_DEPTH - 1`.
        assert_eq!(
            text.matches('{').count(),
            MAX_WIRE_DEPTH - 1,
            "expected the cap to stop brace nesting"
        );
        // No new grammar: the degraded node is an ordinary opaque scalar.
        assert!(
            text.lines().any(|l| l.trim_start().starts_with("1: \"")),
            "innermost node should be a quoted scalar"
        );
    }

    #[test]
    fn tripping_the_depth_cap_does_not_leak_the_counter() {
        // `DEPTH` is a thread-local, and protolens reuses render threads:
        // a guard that failed to unwind would silently cap every later
        // render on the same thread. Trip the cap, then render a trivial
        // payload on the same thread and require the ordinary output.
        let _ = decode_and_render(&vec![0x0Bu8; 20_000], None, DecodeRenderOpts::default());
        let out = decode_and_render(
            &VARINT_FIELD,
            None,
            DecodeRenderOpts {
                indent_size: 2,
                ..Default::default()
            },
        );
        assert_eq!(String::from_utf8(out).unwrap().trim_end(), "1: 42");
    }

    /// `varint 1 = 7`, then a `field 2` group nested `depth` levels deep and
    /// properly closed, then `varint 3 = 9`. `close_field` is the field
    /// number on the outermost END_GROUP tag.
    fn scalar_deep_group_scalar(depth: usize, close_field: u8) -> Vec<u8> {
        let mut buf = vec![0x08, 0x07]; // 1: 7
        buf.extend(std::iter::repeat_n(0x13u8, depth)); // START_GROUP field 2
        buf.extend(std::iter::repeat_n(0x14u8, depth - 1)); // END_GROUP field 2
        buf.push((close_field << 3) | 4); // outermost END_GROUP
        buf.extend_from_slice(&[0x18, 0x09]); // 3: 9
        buf
    }

    #[test]
    fn over_deep_group_costs_only_itself() {
        // The whole point of the local cap: the over-deep group collapses to
        // one opaque line, and the sibling that follows it still renders. A
        // cap that abandoned the buffer would lose `3: 9`.
        let buf = scalar_deep_group_scalar(1200, 2);
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("1: 7"),
            "got: {}",
            &text[..80.min(text.len())]
        );
        assert_eq!(text.matches("INVALID_TAG_TYPE").count(), 1, "got: {text}");
        assert!(
            text.lines().any(|l| l.trim_start().starts_with("3: 9")),
            "the sibling after the over-deep group must survive"
        );
    }

    #[test]
    fn over_deep_group_with_a_mismatched_close_still_costs_only_itself() {
        // The depth-cap site asks `scan_group_extent` for an extent, not for
        // a validity judgement, so the field number on the closing tag is
        // irrelevant. Requiring a match here would fall back to `buflen` and
        // swallow `3: 9` — exactly what the local cap exists to avoid.
        let buf = scalar_deep_group_scalar(1200, 7);
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.lines().any(|l| l.trim_start().starts_with("3: 9")),
            "got: {text}"
        );
    }

    #[test]
    fn capped_render_still_round_trips() {
        // No new grammar (spec 0171 G4): `INVALID_TAG_TYPE` re-encodes
        // tagless and verbatim, so a capped render is still lossless. Any
        // tag-emitting alternative fails here.
        let buf = scalar_deep_group_scalar(1200, 2);
        let text = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                emit_header: true,
                ..Default::default()
            },
        );
        let wire = crate::serialize::encode_text::encode_text_to_binary(&text);
        assert_eq!(wire, buf, "capped render did not round-trip");
    }

    /// The marker `MalformedKind` renders as. Exhaustive by construction
    /// (no wildcard arm): a variant added without a marker here fails to
    /// compile, and one added without an `encode_text/fields.rs` arm
    /// fails the round-trip below (spec 0174 G1).
    fn malformity_marker(kind: &super::sink::MalformedKind) -> &'static str {
        use super::sink::MalformedKind as K;
        match kind {
            K::InvalidTagType => "INVALID_TAG_TYPE",
            K::InvalidVarint => "INVALID_VARINT",
            K::InvalidFixed64 => "INVALID_FIXED64",
            K::InvalidFixed32 => "INVALID_FIXED32",
            K::InvalidLen => "INVALID_LEN",
            K::TruncatedBytes { .. } => "TRUNCATED_BYTES",
            K::InvalidGroupEnd => "INVALID_GROUP_END",
        }
    }

    /// Spec 0174 G1: the round-trip promise is unconditional — every
    /// production the renderer can emit re-encodes to the exact bytes it
    /// came from. `NODE_BUDGET_EXCEEDED` was the sole exception (it
    /// reported a length and dropped the bytes on purpose), which is why
    /// the budget left this crate.
    #[test]
    fn every_malformity_marker_round_trips() {
        use super::sink::MalformedKind as K;
        // One minimal wire input per variant, each provoking exactly
        // that malformity.
        let cases: [(K, &[u8]); 7] = [
            // Field 1, wire type 7 — no such wire type.
            (K::InvalidTagType, &[0x0F]),
            // Field 1 VARINT, whose value varint never terminates.
            (K::InvalidVarint, &[0x08, 0xFF]),
            // Field 1 FIXED64 with 1 of its 8 bytes present.
            (K::InvalidFixed64, &[0x09, 0x01]),
            // Field 1 FIXED32 with 1 of its 4 bytes present.
            (K::InvalidFixed32, &[0x0D, 0x01]),
            // Field 1 LEN, whose length varint never terminates.
            (K::InvalidLen, &[0x0A, 0xFF]),
            // Field 1 LEN declaring 10 bytes with 2 present.
            (K::TruncatedBytes { missing: 8 }, &[0x0A, 0x0A, 0x01, 0x02]),
            // Field 1 END_GROUP with no matching START_GROUP.
            (K::InvalidGroupEnd, &[0x0C]),
        ];
        for (kind, buf) in cases {
            let marker = malformity_marker(&kind);
            let rendered = decode_and_render(
                buf,
                None,
                DecodeRenderOpts {
                    annotations: true,
                    emit_header: true,
                    ..Default::default()
                },
            );
            let text = String::from_utf8(rendered.clone()).unwrap();
            assert!(text.contains(marker), "{marker} not rendered: {text}");
            let wire = crate::serialize::encode_text::encode_text_to_binary(&rendered);
            assert_eq!(wire, buf, "{marker} did not round-trip: {text}");
        }
    }

    #[test]
    fn deeply_nested_unterminated_groups_do_not_overflow_the_stack() {
        // 200 000 START_GROUP tags for field 1 with no matching ends, so
        // `scan_group_extent` finds no extent and the `unwrap_or(buflen)`
        // arm is taken.
        let buf = vec![0x0Bu8; 200_000];
        let out = decode_and_render(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("INVALID_TAG_TYPE"), "got: {text}");
    }

    // ── Spec 0311: a truncated field still has its declared type ────────────

    /// An `optional` field. `type_name` is `Some` only for message, group and
    /// enum kinds.
    fn opt_field(
        name: &str,
        number: i32,
        ty: prost_types::field_descriptor_proto::Type,
        type_name: Option<&str>,
    ) -> prost_types::FieldDescriptorProto {
        use prost_types::field_descriptor_proto::Label;
        prost_types::FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(ty as i32),
            type_name: type_name.map(str::to_string),
            label: Some(Label::Optional as i32),
            ..Default::default()
        }
    }

    fn message(
        name: &str,
        field: Vec<prost_types::FieldDescriptorProto>,
    ) -> prost_types::DescriptorProto {
        prost_types::DescriptorProto {
            name: Some(name.into()),
            field,
            ..Default::default()
        }
    }

    /// `Outer { Mid mid = 1; int32 tail = 3; }`,
    /// `Mid { Inner inner = 1; string label = 2; }`,
    /// `Inner { string s = 1; int32 n = 2; }` — three spine depths, so a cut
    /// past the outer header truncates one frame at every level (G3).
    fn nested_schema() -> crate::schema::ParsedSchema {
        use prost_types::field_descriptor_proto::Type;
        build_schema(
            vec![
                message(
                    "Inner",
                    vec![
                        opt_field("s", 1, Type::String, None),
                        opt_field("n", 2, Type::Int32, None),
                    ],
                ),
                message(
                    "Mid",
                    vec![
                        opt_field("inner", 1, Type::Message, Some(".Inner")),
                        opt_field("label", 2, Type::String, None),
                    ],
                ),
                message(
                    "Outer",
                    vec![
                        opt_field("mid", 1, Type::Message, Some(".Mid")),
                        opt_field("tail", 3, Type::Int32, None),
                    ],
                ),
            ],
            "Outer",
        )
    }

    /// `Outer { mid { inner { s: "abc" n: 42 } label: "xy" } tail: 7 }`.
    fn nested_blob() -> Vec<u8> {
        let inner = [0x0A, 0x03, b'a', b'b', b'c', 0x10, 0x2A];
        let mut mid = vec![0x0A, inner.len() as u8];
        mid.extend_from_slice(&inner);
        mid.extend_from_slice(&[0x12, 0x02, b'x', b'y']);
        let mut outer = vec![0x0A, mid.len() as u8];
        outer.extend_from_slice(&mid);
        outer.extend_from_slice(&[0x18, 0x07]);
        outer
    }

    /// One message carrying every kind whose truncation this spec declines to
    /// change: a declared `string`, a declared `bytes`, a packed `int32`
    /// field, a group, and an unknown field — beside the declared sub-message
    /// that it does change.
    fn mixed_schema() -> crate::schema::ParsedSchema {
        use prost_types::field_descriptor_proto::{Label, Type};
        let packed = prost_types::FieldDescriptorProto {
            label: Some(Label::Repeated as i32),
            options: Some(prost_types::FieldOptions {
                packed: Some(true),
                ..Default::default()
            }),
            ..opt_field("nums", 4, Type::Int32, None)
        };
        build_schema(
            vec![
                message("Inner", vec![opt_field("s", 1, Type::String, None)]),
                message("Grp", vec![opt_field("v", 1, Type::Int32, None)]),
                message(
                    "Mixed",
                    vec![
                        opt_field("sub", 1, Type::Message, Some(".Inner")),
                        opt_field("s", 2, Type::String, None),
                        opt_field("b", 3, Type::Bytes, None),
                        packed,
                        opt_field("grp", 5, Type::Group, Some(".Grp")),
                    ],
                ),
            ],
            "Mixed",
        )
    }

    fn mixed_blob() -> Vec<u8> {
        vec![
            0x0A, 0x04, 0x0A, 0x02, b'a', b'b', // sub { s: "ab" }
            0x12, 0x02, b'h', b'i', // s: "hi"
            0x1A, 0x02, 0x01, 0x02, // b: "\001\002"
            0x22, 0x04, 0x01, 0x02, 0xAC, 0x02, // nums: [1, 2, 300], packed
            0x2B, 0x08, 0x09, 0x2C, // grp { v: 9 }
            0x48, 0x05, // unknown field 9, varint
        ]
    }

    fn annotated(blob: &[u8], root: Option<&prost_reflect::MessageDescriptor>) -> String {
        let out = decode_and_render(
            blob,
            root,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        String::from_utf8(out).unwrap()
    }

    /// Render with a header, so the result can be fed back to the encoder.
    fn for_round_trip(blob: &[u8], root: Option<&prost_reflect::MessageDescriptor>) -> Vec<u8> {
        decode_and_render(
            blob,
            root,
            DecodeRenderOpts {
                annotations: true,
                emit_header: true,
                ..Default::default()
            },
        )
    }

    /// Render `blob`, re-encode the rendering, and require the original bytes.
    fn assert_round_trips(blob: &[u8], root: Option<&prost_reflect::MessageDescriptor>) -> String {
        let rendered = for_round_trip(blob, root);
        let wire = crate::serialize::encode_text::encode_text_to_binary(&rendered);
        let text = String::from_utf8(rendered).unwrap();
        assert_eq!(wire, blob, "did not round-trip:\n{text}");
        text
    }

    /// Spec 0311 test plan A. Render every prefix of `blob` and require each
    /// rendering to re-encode to exactly the prefix it came from.
    ///
    /// Every cut position is covered by construction: inside a tag byte,
    /// inside a length varint, inside a payload at every nesting depth, and
    /// exactly on a field boundary — where nothing is truncated and the sweep
    /// merely re-asserts the ordinary round trip.
    fn sweep_prefixes(blob: &[u8], root: Option<&prost_reflect::MessageDescriptor>) {
        for k in 1..=blob.len() {
            let cut = &blob[..k];
            let rendered = for_round_trip(cut, root);
            let wire = crate::serialize::encode_text::encode_text_to_binary(&rendered);
            assert_eq!(
                wire,
                cut,
                "cut at {k}/{} did not round-trip:\n{}",
                blob.len(),
                String::from_utf8_lossy(&rendered)
            );
        }
    }

    #[test]
    fn truncating_anywhere_round_trips() {
        let nested = nested_schema();
        let nested_root = nested.root_descriptor().unwrap();
        // G1's path: three spine depths, each cut carrying its own count.
        sweep_prefixes(&nested_blob(), Some(&nested_root));
        // The same bytes with no schema. Pins N1 across the whole sweep, and
        // becomes spec 0312's primary fixture when the carve-out lands — at
        // which point the rendering changes and this assertion does not.
        sweep_prefixes(&nested_blob(), None);

        let mixed = mixed_schema();
        let mixed_root = mixed.root_descriptor().unwrap();
        // Cuts through the string, bytes and packed fields pin N2 and N3 for
        // free; a cut past the START_GROUP pins N4.
        sweep_prefixes(&mixed_blob(), Some(&mixed_root));
    }

    // ── B — round-trip cases a tail cut cannot produce ──────────────────────

    #[test]
    fn a_lying_length_prefix_round_trips() {
        // `Mid.inner` declares 32 bytes with 3 present, and the file
        // continues afterwards: impossible from a tail cut, trivial to write.
        // The re-encode must restore the *declared* 32, not the actual 3.
        let blob: &[u8] = &[
            0x0A, 0x05, // mid, 5 bytes
            0x0A, 0x20, 0x0A, 0x01, b'z', // inner, declared 32, 3 present
            0x18, 0x07, // tail: 7
        ];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = assert_round_trips(blob, Some(&root));
        assert!(text.contains("MISSING: 29"), "got: {text}");
    }

    #[test]
    fn non_minimal_lengths_survive_truncation() {
        // The truncated field's tag and length varint both carry an overhead
        // byte. `fill_placeholder` writes the inflated length flush-right
        // into room sized by `ohb`, and that is the one arithmetic in the
        // chain that is not obviously right.
        let blob: &[u8] = &[
            0x0A, 0x07, // mid, 7 bytes
            0x8A, 0x00, // inner's tag, two bytes instead of one
            0xA0, 0x00, // its length, 32, two bytes instead of one
            0x0A, 0x01, b'z', // 3 bytes present
            0x18, 0x07, // tail: 7
        ];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = assert_round_trips(blob, Some(&root));
        assert!(text.contains("tag_ohb"), "got: {text}");
        assert!(text.contains("len_ohb"), "got: {text}");
        assert!(text.contains("MISSING: 29"), "got: {text}");
    }

    #[test]
    fn a_truncated_message_with_no_available_bytes_round_trips() {
        // Declared 32, zero present: an empty body whose entire content is
        // the missing count. `child_len_compacted` is 0 here, so an encoder
        // that treats `missing` as an adjustment rather than an addend fails.
        let blob: &[u8] = &[0x0A, 0x02, 0x0A, 0x20, 0x18, 0x07];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = assert_round_trips(blob, Some(&root));
        assert!(
            text.contains("TRUNCATED_MESSAGE; MISSING: 32"),
            "got: {text}"
        );
    }

    #[test]
    fn truncated_bytes_inside_a_truncated_message_round_trips() {
        // One document, both encoder arms: the placeholder path for the
        // overrunning sub-message, and `encode_text/fields.rs`'s direct write
        // for the overrunning declared string inside it. They compute the
        // declared length by different routes and this is the only case that
        // makes them agree in one buffer.
        let blob: &[u8] = &[
            0x0A, 0x06, // mid, 6 bytes
            0x0A, 0x0A, // inner, declared 10, 4 present
            0x0A, 0x08, b'a', b'b', // s, declared 8, 2 present
            0x18, 0x07, // tail: 7
        ];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = assert_round_trips(blob, Some(&root));
        assert!(
            text.contains("TRUNCATED_MESSAGE; MISSING: 6"),
            "got: {text}"
        );
        assert!(text.contains("TRUNCATED_BYTES; MISSING: 6"), "got: {text}");
    }

    // ── C — the guard, and the hazard behind it ─────────────────────────────

    #[test]
    fn an_unrepresentable_declared_length_does_not_descend() {
        // Spec 0311 S6. A declared length of `2^35` needs six varint bytes,
        // one more than `fill_placeholder`'s flush-right room, so the
        // renderer must not emit `TRUNCATED_MESSAGE` for it at all. Without
        // the guard the encoder writes over the `next_placeholder` link and
        // returns a wrong buffer with no panic.
        let mut blob = vec![0x0A];
        push_varint(1 << 35, &mut blob);
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = assert_round_trips(&blob, Some(&root));
        assert!(text.contains("TRUNCATED_BYTES"), "got: {text}");
        assert!(!text.contains("TRUNCATED_MESSAGE"), "got: {text}");

        // One below the cap still descends: the guard is a ceiling, not a
        // blanket refusal.
        let mut just_under = vec![0x0A];
        push_varint((1 << 35) - 1, &mut just_under);
        let text = assert_round_trips(&just_under, Some(&root));
        assert!(text.contains("TRUNCATED_MESSAGE"), "got: {text}");
    }

    // ── D — rendering, and the non-goal pins ────────────────────────────────

    #[test]
    fn a_declared_message_field_that_overruns_descends() {
        // `mid` declares 13 bytes with 10 present: the complete `inner` and
        // one stray tag byte. G1 — the children that fit are shown, rather
        // than one opaque bytes line.
        let blob = &nested_blob()[..12];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = annotated(blob, Some(&root));
        assert!(text.starts_with("mid {"), "got: {text}");
        assert!(text.contains("s: \"abc\""), "got: {text}");
        assert!(text.contains("n: 42"), "got: {text}");
    }

    #[test]
    fn the_truncated_header_carries_missing() {
        let blob = &nested_blob()[..12];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = annotated(blob, Some(&root));
        let header = text.lines().next().unwrap();
        assert!(
            header.contains("TRUNCATED_MESSAGE; MISSING: 3"),
            "got: {header}"
        );
    }

    #[test]
    fn truncation_is_counted_at_every_spine_level() {
        // Cut so that `mid`, `inner` and `inner.s` all overrun. Each frame
        // reports its own shortfall against its own declared length: 13−5,
        // 7−3, 3−1. G3 — the test a one-shot implementation fails.
        let blob = &nested_blob()[..7];
        let schema = nested_schema();
        let root = schema.root_descriptor().unwrap();
        let text = annotated(blob, Some(&root));
        assert!(
            text.contains("TRUNCATED_MESSAGE; MISSING: 8"),
            "got: {text}"
        );
        assert!(
            text.contains("TRUNCATED_MESSAGE; MISSING: 4"),
            "got: {text}"
        );
        assert!(text.contains("TRUNCATED_BYTES; MISSING: 2"), "got: {text}");
    }

    #[test]
    fn a_declared_string_field_that_overruns_stays_bytes() {
        // N2. `Inner.s` declares 3 bytes with 1 present.
        let schema = nested_schema();
        let inner = schema
            .get_descriptor("Inner")
            .expect("Inner is in the pool");
        let text = annotated(&[0x0A, 0x03, b'a'], Some(&inner));
        assert!(text.contains("TRUNCATED_BYTES; MISSING: 2"), "got: {text}");
        assert!(!text.contains("TRUNCATED_MESSAGE"), "got: {text}");
    }

    #[test]
    fn an_unknown_truncated_field_is_unchanged() {
        // N1, and the guard that stops this spec's change from drifting into
        // the probe: with no schema a cut prefix is still one opaque line.
        //
        // Still true after spec 0312, and for its reason rather than by
        // luck: this prefix cuts `Mid.inner`'s payload before a single whole
        // field of it has been seen, so the threshold is not met.
        let text = annotated(&nested_blob()[..7], None);
        assert!(!text.contains("TRUNCATED_MESSAGE"), "got: {text}");
        assert_eq!(text.matches("TRUNCATED_BYTES").count(), 1, "got: {text}");
    }

    /// `Holder { google.protobuf.Any a = 1; }` plus a `Payload` for the
    /// `Any` to carry, in a second file so `Any` keeps its real package.
    fn any_schema() -> crate::schema::ParsedSchema {
        use prost::Message as ProstMessage;
        use prost_types::field_descriptor_proto::Type;
        let any_file = prost_types::FileDescriptorProto {
            name: Some("google/protobuf/any.proto".into()),
            package: Some("google.protobuf".into()),
            syntax: Some("proto3".into()),
            message_type: vec![message(
                "Any",
                vec![
                    opt_field("type_url", 1, Type::String, None),
                    opt_field("value", 2, Type::Bytes, None),
                ],
            )],
            ..Default::default()
        };
        let test_file = prost_types::FileDescriptorProto {
            name: Some("test_0311_any.proto".into()),
            syntax: Some("proto2".into()),
            dependency: vec!["google/protobuf/any.proto".into()],
            message_type: vec![
                message("Payload", vec![opt_field("v", 1, Type::Int32, None)]),
                message(
                    "Holder",
                    vec![opt_field(
                        "a",
                        1,
                        Type::Message,
                        Some(".google.protobuf.Any"),
                    )],
                ),
            ],
            ..Default::default()
        };
        let fds = prost_types::FileDescriptorSet {
            file: vec![any_file, test_file],
        };
        let mut buf = Vec::new();
        fds.encode(&mut buf).unwrap();
        crate::schema::parse_schema(&buf, "Holder").unwrap()
    }

    #[test]
    fn a_truncated_any_does_not_expand() {
        // N6. Both expansions synthesize virtual fields whose round-trip is
        // defined against complete bytes, so a cut payload falls through to
        // the plain nested-message rendering of S2.
        let schema = any_schema();
        let root = schema.root_descriptor().unwrap();
        let payload = schema
            .get_descriptor("Payload")
            .expect("Payload is in the pool");

        let url = b"type.googleapis.com/Payload";
        let mut any = vec![0x0A, url.len() as u8];
        any.extend_from_slice(url);
        any.extend_from_slice(&[0x12, 0x02, 0x08, 0x09]);
        let mut blob = vec![0x0A, any.len() as u8];
        blob.extend_from_slice(&any);

        let payload_for_loader = payload.clone();
        crate::set_any_loader(Box::new(move |fqdn| {
            (fqdn == "Payload").then(|| std::sync::Arc::new(payload_for_loader.clone()))
        }));

        // Intact: the expansion runs, so the assertion below is not vacuous.
        let intact = annotated(&blob, Some(&root));
        // Cut by two bytes: `a` overruns, and the expansion is skipped.
        let cut = annotated(&blob[..blob.len() - 2], Some(&root));
        crate::clear_any_loader();

        assert!(
            intact.contains("Payload = 2"),
            "expansion did not run: {intact}"
        );
        assert!(cut.contains("TRUNCATED_MESSAGE; MISSING: 2"), "got: {cut}");
        assert!(!cut.contains("Payload = 2"), "got: {cut}");
    }

    // ── Spec 0312: enough of a message is a message ─────────────────────────
    //
    // Spec 0312's test-plan group A is `truncating_anywhere_round_trips`
    // above, whose schema-less sweep asserts the round trip at *every* cut
    // offset — the forgiven ones and, which is the point of S3, the declined
    // ones too. A declined cut that re-encoded three bytes short would fail
    // there and nowhere else.

    /// One length-delimited field carrying `payload`, so that the probe is
    /// consulted at all: `decode_and_render`'s own buffer has no enclosing
    /// LEN field and is never probed (N5).
    fn wrapped(payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 128, "one length byte");
        let mut blob = vec![0x0A, payload.len() as u8];
        blob.extend_from_slice(payload);
        blob
    }

    /// Whether the renderer opened `blob`'s first field as a message *on
    /// this spec's account* — that is, descended into a payload it had to
    /// forgive a cut to accept.
    ///
    /// Not simply "descended". Spec 0266 already admits the occasional
    /// binary scrap whose bytes happen to parse clean to the last one, and
    /// this spec neither adds to that set nor is allowed to: what it may
    /// change is only the verdict on a payload whose tail is cut. A
    /// descended block carrying no truncation is therefore the old verdict,
    /// unchanged, whatever it is.
    fn forgave_a_cut(blob: &[u8]) -> bool {
        let text = annotated(blob, None);
        text.contains('{') && text.contains("TRUNCATED_")
    }

    /// Field 1 varint 1, then field 2 declaring nine payload bytes and
    /// delivering one — spec 0312 S5's own example of the scrap of binary
    /// that `P` has to rule on. One whole field precedes the cut.
    const ONE_FIELD_THEN_CUT: &[u8] = &[0x08, 0x01, 0x12, 0x09, b'x'];

    #[test]
    fn a_cut_tail_after_enough_fields_is_a_message() {
        // G1. The measured `P` is 1, so one whole field is enough.
        let blob = wrapped(ONE_FIELD_THEN_CUT);
        let text = assert_round_trips(&blob, None);
        assert!(text.contains('{'), "the payload stayed opaque: {text}");
        assert!(text.contains("TRUNCATED_BYTES; MISSING: 8"), "got: {text}");
    }

    #[test]
    fn a_cut_tail_after_too_few_fields_is_bytes() {
        // G4, at `P - 1` = no whole field at all: the same cut field with
        // nothing in front of it. This is the assertion that fails if `P` is
        // ever lowered to zero.
        let blob = wrapped(&ONE_FIELD_THEN_CUT[2..]);
        let text = assert_round_trips(&blob, None);
        assert!(!text.contains('{'), "the cut was forgiven: {text}");
    }

    #[test]
    fn a_lying_length_prefix_is_still_not_a_message() {
        // G2, and the test that fails if `frame_ends_at_eof` is dropped or
        // hardcoded true. The payload is byte-for-byte the forgiven one
        // above; all that changes is that the file continues past the field
        // holding it, so the overrun cannot be "the bytes ran out".
        let mut blob = wrapped(ONE_FIELD_THEN_CUT);
        blob.extend_from_slice(&[0x18, 0x07]);
        let text = assert_round_trips(&blob, None);
        assert!(!text.contains('{'), "a lying length was forgiven: {text}");
    }

    #[test]
    fn frame_ends_at_eof_is_false_below_a_satisfied_prefix() {
        // The threading itself, as one file: the same seven bytes twice
        // over. The first copy's frame ends where the second begins, the
        // second's ends with the file, and only the second may descend.
        //
        // An offset-valued thread-local cannot express this — both copies
        // are the same bytes at the same frame-relative positions — which is
        // S2's argument turned into an assertion.
        let mut blob = wrapped(ONE_FIELD_THEN_CUT);
        blob.extend_from_slice(&wrapped(ONE_FIELD_THEN_CUT));
        let text = assert_round_trips(&blob, None);
        assert_eq!(
            text.matches('{').count(),
            1,
            "exactly the copy at the end of the file descends: {text}",
        );
    }

    #[test]
    fn a_cut_tail_plus_one_more_flaw_is_not_a_message() {
        // G3. Wire type 6 does not exist, so the field ahead of the cut is
        // an ordinary spec 0266 disqualification and `invalid_count` is
        // non-zero however forgivable the tail is.
        let blob = wrapped(&[0x08, 0x01, 0x0E, 0x12, 0x09, b'x']);
        let text = assert_round_trips(&blob, None);
        assert!(!text.contains('{'), "a second flaw was forgiven: {text}");
    }

    #[test]
    fn a_cut_group_is_still_not_a_message() {
        // N3. A group has no length prefix, so a cut one has no `MISSING`
        // to report and nothing to restore on re-encode. It is also the
        // case `ProbeSink::end_nested` singles out: a trailing `START_GROUP`
        // byte must not be allowed to rescue a string.
        let blob = wrapped(&[0x08, 0x01, 0x0B]);
        let text = assert_round_trips(&blob, None);
        assert!(!text.contains('{'), "a cut group was forgiven: {text}");
    }

    #[test]
    fn binary_files_do_not_become_messages() {
        // Spec 0312 S5's negative controls, frozen. Cut at each of 72 742
        // offsets, these five files produced one false positive, and it is
        // the same one at every `P` from 0 to 16 — a ten-byte PNG prefix
        // that parses clean as a single `fixed64`, forgiving nothing. That
        // is spec 0266's rate, and this spec is not allowed to move it.
        //
        // So the assertion is not "nothing descends" — something already
        // did, before this spec existed. It is that nothing descends *by
        // being forgiven*, at every offset of all five.
        let png: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x06\x00\x00\x00\x1f\xf3\xffa";
        let elf: &[u8] = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00>\x00\x01\x00\x00\x00\x50\x10\x00\x00\x00\x00\x00\x00";
        let gz: &[u8] = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\xcbH\xcd\xc9\xc9W(\xcf/\xcaI\x01\x00\x85\x11J\r\x0b\x00\x00\x00";
        let prose: &[u8] = b"The quick brown fox jumps over the lazy dog.";
        let json: &[u8] = br#"{"a":1,"b":[2,3],"c":"hello","d":null}"#;

        for (name, file) in [
            ("png", png),
            ("elf", elf),
            ("gz", gz),
            ("prose", prose),
            ("json", json),
        ] {
            for k in 1..=file.len() {
                let blob = wrapped(&file[..k]);
                assert!(
                    !forgave_a_cut(&blob),
                    "{name} cut at {k} was read as a message:\n{}",
                    annotated(&blob, None),
                );
            }
        }
    }
}
