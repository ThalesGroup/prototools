// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

mod helpers;
mod packed;
mod sink;
mod varint;

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use prost_reflect::{Cardinality, ExtensionDescriptor, FieldDescriptor, Kind, MessageDescriptor};

use crate::helpers::{
    bytes_missing, parse_varint, parse_wiretag, payload_end, WiretagResult, MAX_WIRE_DEPTH,
    WT_END_GROUP, WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT,
};

use helpers::{render_group_field, render_len_field, scan_group_extent, FieldCtx};
use sink::{IndexingTextSink, MalformedKind, ScalarValue, Sink, TagFacts, TextSink};

pub use sink::NodeSpan;

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

    /// The name to use in field-line output.
    ///
    /// Regular field: `"name"` (bare field name).
    /// Extension field: `"[full.qualified.name]"`.
    pub(super) fn display_name(&self) -> String {
        match self {
            FieldOrExt::Field(f) => f.name().to_owned(),
            FieldOrExt::Ext(e) => format!("[{}]", e.full_name()),
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
    } = opts;
    let capacity = buf.len() * 8;
    let mut sink = TextSink::new(capacity);

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

    render_message(buf, 0, None, root_desc, schema_present, &mut sink);

    let out = sink.into_inner();

    // Development instrumentation — truncate event
    #[cfg(debug_assertions)]
    {
        let actual = out.len();
        if actual < capacity {
            eprintln!(
                "[render_text] truncate: input_len={} capacity={} actual={} ratio={:.2}x",
                buf.len(),
                capacity,
                actual,
                actual as f64 / buf.len().max(1) as f64
            );
        }
    }

    out
}

/// Sibling to `decode_and_render`, sharing the exact same parameter list,
/// but internally rendering through an `IndexingTextSink` instead of a bare
/// `TextSink`, and returning both the rendered text and its `NodeSpan`
/// index alongside it (spec 0110 §3). `decode_and_render` itself stays
/// `TextSink`-only: its production callers have no use for the index and
/// shouldn't pay `IndexingTextSink`'s small extra bookkeeping cost.
pub fn decode_and_render_indexed(
    buf: &[u8],
    root_desc: Option<&MessageDescriptor>,
    opts: DecodeRenderOpts,
) -> (Vec<u8>, Vec<NodeSpan>) {
    let DecodeRenderOpts {
        annotations,
        indent_size,
        expand_any,
        hide_unknown_fields,
        expand_message_set,
        initial_level,
        emit_header,
    } = opts;
    let capacity = buf.len() * 8;
    let mut sink = IndexingTextSink::new(capacity);

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

    render_message(buf, 0, None, root_desc, schema_present, &mut sink);

    sink.into_parts()
}

// ── Core recursive render-while-decode ───────────────────────────────────────

/// Parse and render one protobuf message into `sink`.
///
/// Returns `(next_pos, group_end_tag)`:
/// - `next_pos`: byte position after this message (for the caller to
///   continue its own parse loop, or for GROUP end detection).
/// - `group_end_tag`: `Some(tag)` when parsing terminated on a `WT_END_GROUP`.
fn render_message<S: Sink>(
    buf: &[u8],
    start: usize,
    my_group: Option<u64>,
    schema: Option<&MessageDescriptor>,
    schema_present: bool,
    sink: &mut S,
) -> (usize, Option<WiretagResult>) {
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

        if let Some(ref wtag_gar) = tag.wtag_gar {
            // Invalid wire tag: consume rest of buffer as INVALID_TAG_TYPE
            sink.malformed(
                0,
                TagFacts::default(),
                MalformedKind::InvalidTagType,
                wtag_gar,
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
            } else {
                s.get_extension(field_number as u32).map(FieldOrExt::Ext)
            }
        });

        // ── Wire-type dispatch ────────────────────────────────────────────────

        match wire_type {
            // ── VARINT ───────────────────────────────────────────────────────
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if let Some(ref varint_gar) = vr.varint_gar {
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidVarint,
                        varint_gar,
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
                if let Some(ref varint_gar) = lr.varint_gar {
                    sink.malformed(
                        field_number,
                        TagFacts {
                            tag_ohb,
                            tag_oor,
                            len_ohb: None,
                        },
                        MalformedKind::InvalidLen,
                        varint_gar,
                    );
                    return (buflen, None);
                }
                let len_ohb = lr.varint_ohb;
                pos = lr.next_pos;
                let length = lr.varint.unwrap();

                let Some(end) = payload_end(pos, length, buflen) else {
                    let missing = bytes_missing(pos, length, buflen);
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
                    },
                    schema_present,
                    field_start..pos,
                    data,
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
    use super::*;

    // field 1 (varint) = 42: tag 0x08, value 0x2A.
    const VARINT_FIELD: [u8; 2] = [0x08, 0x2A];

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

    // ── `NodeSpan::natural_annotation` (spec 0122 Test Plan item 1) ────────

    /// Build a one-file `ParsedSchema` from the given message descriptors,
    /// rooted at `root_msg_name`. Mirrors `schema.rs`'s own test-fixture
    /// convention.
    fn build_schema(
        message_type: Vec<prost_types::DescriptorProto>,
        root_msg_name: &str,
    ) -> crate::schema::ParsedSchema {
        use prost::Message as ProstMessage;
        let file = prost_types::FileDescriptorProto {
            name: Some("test_natural_annotation.proto".into()),
            syntax: Some("proto2".into()),
            message_type,
            ..Default::default()
        };
        let fds = prost_types::FileDescriptorSet { file: vec![file] };
        let mut buf = Vec::new();
        fds.encode(&mut buf).unwrap();
        crate::schema::parse_schema(&buf, root_msg_name).unwrap()
    }

    fn int32_field(name: &str, number: i32) -> prost_types::FieldDescriptorProto {
        use prost_types::field_descriptor_proto::{Label, Type};
        prost_types::FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(Type::Int32 as i32),
            label: Some(Label::Optional as i32),
            ..Default::default()
        }
    }

    #[test]
    fn natural_annotation_captures_a_plain_message_field_decl() {
        use prost_types::field_descriptor_proto::{Label, Type};
        use prost_types::{DescriptorProto, FieldDescriptorProto};

        let inner = DescriptorProto {
            name: Some("Inner".into()),
            field: vec![int32_field("x", 1)],
            ..Default::default()
        };
        let outer = DescriptorProto {
            name: Some("Outer".into()),
            field: vec![FieldDescriptorProto {
                name: Some("inner".into()),
                number: Some(1),
                r#type: Some(Type::Message as i32),
                type_name: Some(".Inner".into()),
                label: Some(Label::Optional as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let schema = build_schema(vec![outer, inner], "Outer");
        let root = schema.root_descriptor().unwrap();

        // Outer { inner: Inner { x: 1 } } — field 1 (LEN), payload = field 1
        // (varint) = 1.
        let buf = [0x0A, 0x02, 0x08, 0x01];
        let (_, spans) = decode_and_render_indexed(
            &buf,
            Some(&root),
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let span = spans
            .iter()
            .find(|s| s.type_fqdn.as_deref() == Some("Inner"))
            .expect("Inner span must exist");
        assert_eq!(span.natural_annotation.as_deref(), Some("#@ Inner = 1"));
    }

    #[test]
    fn natural_annotation_captures_a_resolvable_group_field_decl() {
        use prost_types::field_descriptor_proto::{Label, Type};
        use prost_types::{DescriptorProto, FieldDescriptorProto};

        let my_group = DescriptorProto {
            name: Some("MyGroup".into()),
            field: vec![int32_field("id", 1)],
            ..Default::default()
        };
        let outer = DescriptorProto {
            name: Some("Outer".into()),
            field: vec![FieldDescriptorProto {
                name: Some("grp".into()),
                number: Some(5),
                r#type: Some(Type::Group as i32),
                type_name: Some(".MyGroup".into()),
                label: Some(Label::Optional as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let schema = build_schema(vec![outer, my_group], "Outer");
        let root = schema.root_descriptor().unwrap();

        // START_GROUP(5), id=7, END_GROUP(5).
        let buf = [0x2B, 0x08, 0x07, 0x2C];
        let (_, spans) = decode_and_render_indexed(
            &buf,
            Some(&root),
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let span = spans
            .iter()
            .find(|s| s.type_fqdn.as_deref() == Some("MyGroup"))
            .expect("MyGroup span must exist");
        assert_eq!(
            span.natural_annotation.as_deref(),
            Some("#@ group; MyGroup = 5")
        );
    }

    #[test]
    fn natural_annotation_captures_a_bare_group_with_no_schema() {
        // No schema at all — field 5 is entirely unknown.
        // START_GROUP(5), id=7, END_GROUP(5).
        let buf = [0x2B, 0x08, 0x07, 0x2C];
        let (_, spans) = decode_and_render_indexed(
            &buf,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let span = spans
            .iter()
            .find(|s| s.is_message)
            .expect("group span must exist");
        assert_eq!(span.natural_annotation.as_deref(), Some("#@ group"));
    }

    #[test]
    fn natural_annotation_preserves_a_tag_overhang_modifier() {
        use prost_types::field_descriptor_proto::{Label, Type};
        use prost_types::{DescriptorProto, FieldDescriptorProto};

        let my_group = DescriptorProto {
            name: Some("MyGroup".into()),
            field: vec![int32_field("id", 1)],
            ..Default::default()
        };
        let outer = DescriptorProto {
            name: Some("Outer".into()),
            field: vec![FieldDescriptorProto {
                name: Some("grp".into()),
                number: Some(5),
                r#type: Some(Type::Group as i32),
                type_name: Some(".MyGroup".into()),
                label: Some(Label::Optional as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let schema = build_schema(vec![outer, my_group], "Outer");
        let root = schema.root_descriptor().unwrap();

        // START_GROUP(5) encoded with one overhang byte: 0x2B's minimal
        // varint (0x2B) re-encoded as a 2-byte non-minimal varint
        // (0xAB, 0x00) — same value, tag_ohb: 1. Then id=7, END_GROUP(5).
        let buf = [0xAB, 0x00, 0x08, 0x07, 0x2C];
        let (_, spans) = decode_and_render_indexed(
            &buf,
            Some(&root),
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        let span = spans
            .iter()
            .find(|s| s.type_fqdn.as_deref() == Some("MyGroup"))
            .expect("MyGroup span must exist");
        assert_eq!(
            span.natural_annotation.as_deref(),
            Some("#@ group; MyGroup = 5; tag_ohb: 1")
        );
    }

    #[test]
    fn natural_annotation_is_none_for_a_scalar_field() {
        let (_, spans) = decode_and_render_indexed(
            &VARINT_FIELD,
            None,
            DecodeRenderOpts {
                annotations: true,
                indent_size: 2,
                ..Default::default()
            },
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].natural_annotation, None);
    }

    // ── Bounds arithmetic and depth caps (spec 0171) ────────────────────────

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
}
