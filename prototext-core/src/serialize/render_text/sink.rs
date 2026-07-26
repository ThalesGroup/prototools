// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! The `Sink` abstraction unifying `render_message`'s production text
//! rendering, a lean structural-validity probe, and an offset-indexing
//! variant behind one generic dispatch body.  See
//! `docs/specs/0110-render-sink-unification.md` §1.

use std::ops::Range;

use super::FieldOrExt;

// ── Supporting payload types ────────────────────────────────────────────────

/// Tag-level anomaly facts shared by nearly every dispatch site — mirrors the
/// `tag_ohb`/`tag_oor`/`len_ohb` triple already threaded through nearly every
/// `render_*` function today. `len_ohb` is only ever populated for LEN-wire
/// fields (`None` for VARINT/FIXED32/FIXED64).
#[derive(Clone, Copy, Default)]
pub(super) struct TagFacts {
    pub(super) tag_ohb: Option<u64>,
    pub(super) tag_oor: bool,
    pub(super) len_ohb: Option<u64>,
}

/// Wire-kind-specific raw payload for a scalar field. Each variant carries
/// exactly what today's corresponding `render_*` function already takes as
/// input, before any schema-typed decoding — that decoding stays inside
/// `TextSink`'s own `scalar_field` implementation, never in the shared
/// dispatch loop.
pub(super) enum ScalarValue<'a> {
    /// VARINT wire type. `raw_val` is `parse_varint`'s own decoded output;
    /// `val_ohb` is the value varint's own overhang count.
    Varint { raw_val: u64, val_ohb: Option<u64> },
    /// FIXED64 wire type: the raw 8 bytes.
    Fixed64([u8; 8]),
    /// FIXED32 wire type: the raw 4 bytes.
    Fixed32([u8; 4]),
    /// LEN wire type, non-packed: string, bytes, or wire-type-mismatch bytes
    /// leaf (`TextSink` recomputes which, from `field_schema`/`kind`, exactly
    /// as today's `render_len_field` already does).
    Bytes(&'a [u8]),
    /// LEN wire type, packed-repeated: the whole wire record. `TextSink`
    /// decodes and emits one line per element (today's `render_packed`).
    Packed(&'a [u8]),
}

/// Distinguishes a LEN-delimited nested message from a GROUP wire record at
/// `begin_nested` — see that method's doc comment.
pub(super) enum NestedKind {
    Message,
    Group,
}

/// Discriminates the specific structurally-invalid case `malformed` reports,
/// collapsing today's separate `render_invalid*`/`render_truncated_bytes`
/// call shapes into one event with one payload shape.
pub(super) enum MalformedKind {
    InvalidTagType,
    InvalidVarint,
    InvalidFixed64,
    InvalidFixed32,
    InvalidLen,
    TruncatedBytes { missing: u64 },
    InvalidGroupEnd,
}

/// Facts about a group's own closing tag, only knowable *after* recursing
/// into the group and reaching its `END_GROUP` tag (or running out of
/// buffer) — never applicable to a LEN-delimited nested message, which has
/// no separate close tag at all.
pub(super) struct GroupCloseFacts {
    /// The `END_GROUP` tag's own varint was over-encoded (more bytes than
    /// the minimal encoding), and by how many.
    pub(super) end_tag_overhang_count: Option<u64>,
    /// The `END_GROUP` tag's own field number was itself out-of-range
    /// (mirrors `wfield_oor` on the open tag) — reported as `ETAG_OOR`.
    pub(super) end_tag_is_out_of_range: bool,
    /// The `END_GROUP` tag's field number didn't match the `START_GROUP`'s —
    /// structurally inconsistent, but non-fatal (parsing continues); carries
    /// the actual (mismatched) field number found.
    pub(super) mismatched_group_end: Option<u64>,
}

// ── The `Sink` trait ─────────────────────────────────────────────────────────

pub(super) trait Sink {
    /// Per-implementation "in-progress nested node" marker, returned by
    /// `begin_nested`/`begin_virtual_nested` and passed back to `end_nested`.
    type Mark;

    /// A scalar (non-recursive), schema-backed field has been fully parsed
    /// off the wire.
    #[allow(clippy::too_many_arguments)]
    fn scalar_field(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        value: ScalarValue<'_>,
        raw_range: Range<usize>,
        schema_present: bool,
    );

    /// A nested message or group is about to be parsed.
    ///
    /// `payload_start` is the local offset — within the *same* coordinate
    /// frame `raw_start` is expressed in — at which this node's own
    /// recursively-rendered content begins. For `NestedKind::Message` this
    /// is the position right after the field's tag + length prefix
    /// (`raw_range.end - data.len()`, since `data` is the field's full LEN
    /// payload); for `NestedKind::Group` it is always `0`, because group
    /// children are parsed in place within the *same* buffer as the
    /// group's own tag (no length prefix, hence no new coordinate frame —
    /// spec 0110 § Design rationale). Ignored by `TextSink`/`ProbeSink`;
    /// consumed by `IndexingTextSink` to translate every `NodeSpan::raw_range`
    /// into one absolute coordinate space (spec 0110 §3).
    fn begin_nested(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        kind: NestedKind,
        raw_start: usize,
        payload_start: usize,
    ) -> Self::Mark;

    /// The nested message/group finished.
    fn end_nested(
        &mut self,
        mark: Self::Mark,
        raw_range: Range<usize>,
        close_facts: Option<GroupCloseFacts>,
    );

    /// A synthetic "virtual field" scalar line — used only by the Any/
    /// MessageSet expansion wrappers.
    fn virtual_scalar(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        value_str: &str,
        raw_range: Range<usize>,
    );

    /// A synthetic "virtual field" nested-node opener — used only by the
    /// Any/MessageSet wrappers. Always paired with
    /// `end_nested(mark, raw_range, None)`.
    ///
    /// `type_fqdn` is the fully-qualified name of the *resolved* payload
    /// type when known (e.g. the Any's resolved message type, or the
    /// MessageSet extension's inner message type) — `None` when the wrapper
    /// node has no resolved type of its own (e.g. MessageSet's `Item`
    /// group wrapper). Ignored by `TextSink`; read by `IndexingTextSink` to
    /// populate `NodeSpan::type_fqdn` (spec 0110 §3).
    ///
    /// `payload_start` follows `begin_nested`'s own contract: the local
    /// offset, in the *same* frame `raw_start` is expressed in, at which
    /// this wrapper's recursively-rendered content begins — `0` when the
    /// wrapper doesn't itself establish a new coordinate frame (e.g.
    /// MessageSet's `Item` wrapper, whose children stay in the same frame
    /// as the enclosing MessageSet payload).
    fn begin_virtual_nested(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        type_fqdn: Option<&str>,
        raw_start: usize,
        payload_start: usize,
    ) -> Self::Mark;

    /// A structurally invalid field was encountered at the current level.
    /// `field_number` is `0` for `MalformedKind::InvalidTagType`.
    fn malformed(&mut self, field_number: u64, tag: TagFacts, kind: MalformedKind, raw: &[u8]);

    /// Whether `render_len_field` should treat every LEN-delimited field as
    /// opaque bytes — skipping its unknown-field cascade (nested-message
    /// probe, packed detection, Any/MessageSet expansion) entirely — rather
    /// than recursing into it (spec 0110 §2). `ProbeSink` overrides this to
    /// `true`: a probe only ever needs mandatory recursion into GROUPs
    /// (which have no length prefix, so their extent is unknowable without
    /// parsing through them); a LEN field's own length prefix already
    /// bounds-checked by `render_message` is sufficient. Default `false` for
    /// every other `Sink`.
    fn treat_len_as_opaque(&self) -> bool {
        false
    }

    /// Whether this sink's own rendering depends on `LEVEL`, the shared
    /// thread-local recursion-depth counter used for indentation. `enter_level`
    /// consults this before touching `LEVEL` at all. `ProbeSink` overrides
    /// this to `false`: it never indents anything (all its methods are
    /// no-ops), so it must not mutate `LEVEL` on behalf of the in-progress
    /// outer render (typically a `TextSink` pass) that invoked it as a
    /// read-only structural probe — `ProbeSink` mutates no shared state.
    /// Default `true` for every other `Sink`.
    fn tracks_level(&self) -> bool {
        true
    }
}

// ── `TextSink`: production text rendering ───────────────────────────────────

use prost_reflect::Kind;

use crate::helpers::{
    decode_double, decode_fixed32, decode_fixed64, decode_float, decode_sfixed32, decode_sfixed64,
    WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT,
};
use crate::serialize::common::{
    escape_bytes_into, escape_string_into, format_double_protoc, format_fixed32_protoc,
    format_fixed64_protoc, format_float_protoc, format_sfixed32_protoc, format_sfixed64_protoc,
    format_wire_fixed32_protoc, format_wire_fixed64_protoc,
};

use super::helpers::render_scalar;
use super::helpers::{
    field_decl, push_indent, push_tag_modifiers, render_invalid, wfl_prefix_n, wob_prefix_n,
    write_close_brace, write_dec_u64, AnnWriter, ScalarCtx,
};
use super::packed::{decode_packed_elems, render_packed};
use super::varint::{decode_varint_typed, render_varint_field, VarintKind};
use super::{ANNOTATIONS, CBL_START, HIDE_UNKNOWN};

/// Per-`TextSink` "in-progress nested node" marker (§1's `Sink::Mark`).
pub(super) enum TextMark {
    /// LEN-delimited nested message or virtual nested node: the opening line
    /// was already fully written (no post-hoc splice needed).
    Message,
    /// GROUP: the opening line was greedily written with only the `group`
    /// token; the rest (field_decl, tag/close-tag modifiers) is spliced in
    /// at `end_nested`, once the close facts are known.
    Group {
        header_nl_pos: usize,
        field_decl: Option<String>,
        is_mismatch: bool,
        tag: TagFacts,
    },
}

/// Production `Sink`: renders protoc-style text into an owned `Vec<u8>`.
pub(super) struct TextSink {
    pub(super) out: Vec<u8>,
    /// Number of `\n` bytes written so far — the sole writer is `newline()`
    /// (spec 0110 § Design rationale). `IndexingTextSink` reads this via
    /// `line_count()` to derive `NodeSpan::text_range`.
    line_count: usize,
}

impl TextSink {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            line_count: 0,
        }
    }

    pub(super) fn into_inner(self) -> Vec<u8> {
        self.out
    }

    /// Number of `\n` bytes written so far (spec 0110 § Design rationale).
    pub(super) fn line_count(&self) -> usize {
        self.line_count
    }

    /// Write raw bytes that may contain embedded `\n` bytes — used for the
    /// file-level `#@ prototext: protoc` header and the `EXTRA_HEADER`
    /// type-inference comment, both emitted before message-body rendering
    /// begins. Increments `line_count` by the number of `\n` bytes written,
    /// so header lines are counted just like body lines (otherwise every
    /// `NodeSpan::text_range` line number would be off by the header's line
    /// count).
    pub(super) fn write_header(&mut self, bytes: &[u8]) {
        self.line_count += bytes.iter().filter(|&&b| b == b'\n').count();
        self.out.extend_from_slice(bytes);
    }

    /// The sole writer of a single `\n` into `out` during message-body
    /// rendering, keeping `line_count` in lock-step with every newline —
    /// never written directly anywhere else in the body-rendering path, so
    /// that `line_count` is provably accurate regardless of what else
    /// touches `out` (e.g. `render_group_field`'s post-hoc splice, which
    /// never writes `\n`; spec 0110 § Design rationale).
    pub(super) fn newline(&mut self) {
        self.out.push(b'\n');
        self.line_count += 1;
    }
}

impl Sink for TextSink {
    type Mark = TextMark;

    fn scalar_field(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        value: ScalarValue<'_>,
        _raw_range: Range<usize>,
        schema_present: bool,
    ) {
        match value {
            ScalarValue::Varint { raw_val, val_ohb } => {
                let (content_kind, typed_val) = if let Some(fs) = field_schema {
                    decode_varint_typed(raw_val, fs)
                } else {
                    (VarintKind::Wire, raw_val)
                };
                render_varint_field(
                    field_number,
                    field_schema,
                    tag.tag_ohb,
                    tag.tag_oor,
                    val_ohb,
                    content_kind,
                    typed_val,
                    schema_present,
                    self,
                );
            }

            ScalarValue::Fixed64(data) => {
                let is_mismatch;
                let mut nan_bits: Option<u64> = None;
                let value_str = if let Some(fs) = field_schema {
                    match fs.kind() {
                        Kind::Double => {
                            is_mismatch = false;
                            let v = decode_double(&data);
                            if v.is_nan() {
                                let bits = v.to_bits();
                                if bits != f64::NAN.to_bits() {
                                    nan_bits = Some(bits);
                                }
                            }
                            format_double_protoc(v)
                        }
                        Kind::Fixed64 => {
                            is_mismatch = false;
                            format_fixed64_protoc(decode_fixed64(&data))
                        }
                        Kind::Sfixed64 => {
                            is_mismatch = false;
                            format_sfixed64_protoc(decode_sfixed64(&data))
                        }
                        _ => {
                            is_mismatch = true;
                            format_wire_fixed64_protoc(decode_fixed64(&data))
                        }
                    }
                } else {
                    is_mismatch = false;
                    format_wire_fixed64_protoc(decode_fixed64(&data))
                };
                render_scalar(
                    &ScalarCtx {
                        field_number,
                        field_schema,
                        tag_ohb: tag.tag_ohb,
                        tag_oor: tag.tag_oor,
                        len_ohb: None,
                        wire_type_name: "fixed64",
                        nan_bits,
                        type_mismatch: is_mismatch,
                        schema_present,
                    },
                    &value_str,
                    is_mismatch,
                    self,
                );
            }

            ScalarValue::Fixed32(data) => {
                let is_mismatch;
                let mut nan_bits: Option<u64> = None;
                let value_str = if let Some(fs) = field_schema {
                    match fs.kind() {
                        Kind::Float => {
                            is_mismatch = false;
                            let v = decode_float(&data);
                            if v.is_nan() {
                                let bits = v.to_bits();
                                if bits != f32::NAN.to_bits() {
                                    nan_bits = Some(bits as u64);
                                }
                            }
                            format_float_protoc(v)
                        }
                        Kind::Fixed32 => {
                            is_mismatch = false;
                            format_fixed32_protoc(decode_fixed32(&data))
                        }
                        Kind::Sfixed32 => {
                            is_mismatch = false;
                            format_sfixed32_protoc(decode_sfixed32(&data))
                        }
                        _ => {
                            is_mismatch = true;
                            format_wire_fixed32_protoc(decode_fixed32(&data))
                        }
                    }
                } else {
                    is_mismatch = false;
                    format_wire_fixed32_protoc(decode_fixed32(&data))
                };
                render_scalar(
                    &ScalarCtx {
                        field_number,
                        field_schema,
                        tag_ohb: tag.tag_ohb,
                        tag_oor: tag.tag_oor,
                        len_ohb: None,
                        wire_type_name: "fixed32",
                        nan_bits,
                        type_mismatch: is_mismatch,
                        schema_present,
                    },
                    &value_str,
                    is_mismatch,
                    self,
                );
            }

            ScalarValue::Bytes(data) => {
                let annotations = ANNOTATIONS.with(|c| c.get());
                match field_schema {
                    None => {
                        // Steps 2/3 of the unknown-LEN-field cascade (spec 0097):
                        // step 1 (message probe) already failed by the time
                        // `scalar_field` is reached with `field_schema: None`.
                        if let Ok(s) = std::str::from_utf8(data) {
                            wfl_prefix_n(field_number, None, true, &mut self.out);
                            self.out.push(b'"');
                            escape_string_into(s, &mut self.out);
                            self.out.push(b'"');
                            if annotations {
                                let mut aw = AnnWriter::new();
                                aw.push_wire(&mut self.out, "string");
                                push_tag_modifiers(
                                    &mut aw,
                                    &mut self.out,
                                    tag.tag_ohb,
                                    tag.tag_oor,
                                    tag.len_ohb,
                                );
                            }
                            self.newline();
                            CBL_START.with(|c| c.set(self.out.len()));
                        } else {
                            wfl_prefix_n(field_number, None, true, &mut self.out);
                            self.out.push(b'"');
                            escape_bytes_into(data, &mut self.out);
                            self.out.push(b'"');
                            if annotations {
                                let mut aw = AnnWriter::new();
                                aw.push_wire(&mut self.out, "bytes");
                                push_tag_modifiers(
                                    &mut aw,
                                    &mut self.out,
                                    tag.tag_ohb,
                                    tag.tag_oor,
                                    tag.len_ohb,
                                );
                            }
                            self.newline();
                            CBL_START.with(|c| c.set(self.out.len()));
                        }
                    }
                    Some(fs) if fs.kind() == Kind::String => match std::str::from_utf8(data) {
                        Ok(s) => {
                            wfl_prefix_n(field_number, Some(fs), false, &mut self.out);
                            self.out.push(b'"');
                            escape_string_into(s, &mut self.out);
                            self.out.push(b'"');
                            if annotations {
                                let mut aw = AnnWriter::new();
                                aw.push_field_decl(
                                    &mut self.out,
                                    field_number,
                                    Some(fs),
                                    None,
                                    None,
                                );
                                push_tag_modifiers(
                                    &mut aw,
                                    &mut self.out,
                                    tag.tag_ohb,
                                    tag.tag_oor,
                                    tag.len_ohb,
                                );
                            }
                            self.newline();
                            CBL_START.with(|c| c.set(self.out.len()));
                        }
                        Err(_) => {
                            render_invalid(
                                field_number,
                                Some(fs),
                                tag.tag_ohb,
                                tag.tag_oor,
                                "INVALID_STRING",
                                data,
                                self,
                            );
                        }
                    },
                    Some(fs) if fs.kind() == Kind::Bytes => {
                        wfl_prefix_n(field_number, Some(fs), false, &mut self.out);
                        self.out.push(b'"');
                        escape_bytes_into(data, &mut self.out);
                        self.out.push(b'"');
                        if annotations {
                            let mut aw = AnnWriter::new();
                            aw.push_field_decl(&mut self.out, field_number, Some(fs), None, None);
                            push_tag_modifiers(
                                &mut aw,
                                &mut self.out,
                                tag.tag_ohb,
                                tag.tag_oor,
                                tag.len_ohb,
                            );
                        }
                        self.newline();
                        CBL_START.with(|c| c.set(self.out.len()));
                    }
                    Some(fs) => {
                        // Wire-type mismatch: schema declares a non-LEN scalar
                        // type but the wire record is LEN.
                        let hide_unknown = HIDE_UNKNOWN.with(|c| c.get());
                        if hide_unknown || !annotations {
                            return;
                        }
                        wfl_prefix_n(field_number, Some(fs), true, &mut self.out);
                        self.out.push(b'"');
                        escape_bytes_into(data, &mut self.out);
                        self.out.push(b'"');
                        let mut aw = AnnWriter::new();
                        aw.push_wire(&mut self.out, "bytes");
                        aw.push(&mut self.out, b"TYPE_MISMATCH");
                        push_tag_modifiers(
                            &mut aw,
                            &mut self.out,
                            tag.tag_ohb,
                            tag.tag_oor,
                            tag.len_ohb,
                        );
                        self.newline();
                        CBL_START.with(|c| c.set(self.out.len()));
                    }
                }
            }

            ScalarValue::Packed(data) => {
                let fs = field_schema.expect("packed scalar requires a known field schema");
                render_packed(
                    field_number,
                    fs,
                    tag.tag_ohb,
                    tag.tag_oor,
                    tag.len_ohb,
                    data,
                    self,
                );
            }
        }
    }

    fn begin_nested(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        kind: NestedKind,
        _raw_start: usize,
        _payload_start: usize,
    ) -> TextMark {
        match kind {
            NestedKind::Message => {
                let is_known = field_schema.is_some();
                wob_prefix_n(field_number, field_schema, !is_known, &mut self.out);
                if ANNOTATIONS.with(|c| c.get()) {
                    let mut aw = AnnWriter::new();
                    if is_known {
                        aw.push_field_decl(&mut self.out, field_number, field_schema, None, None);
                    } else {
                        aw.push_wire(&mut self.out, "message");
                    }
                    push_tag_modifiers(
                        &mut aw,
                        &mut self.out,
                        tag.tag_ohb,
                        tag.tag_oor,
                        tag.len_ohb,
                    );
                }
                self.newline();
                CBL_START.with(|c| c.set(self.out.len()));
                TextMark::Message
            }
            NestedKind::Group => {
                let annotations = ANNOTATIONS.with(|c| c.get());
                push_indent(&mut self.out);
                if let Some(fs) = field_schema.filter(|fs| fs.is_group()) {
                    if let Kind::Message(msg_desc) = fs.kind() {
                        self.out.extend_from_slice(msg_desc.name().as_bytes());
                    } else {
                        write_dec_u64(field_number, &mut self.out);
                    }
                } else {
                    write_dec_u64(field_number, &mut self.out);
                }
                self.out.extend_from_slice(b" {");
                if annotations {
                    let mut aw = AnnWriter::new();
                    aw.push(&mut self.out, b"group");
                }
                self.newline();
                CBL_START.with(|c| c.set(self.out.len()));
                let header_nl_pos = self.out.len() - 1;

                let is_mismatch = field_schema.is_some_and(|fs| !fs.is_group());
                let field_decl_str = if annotations && !is_mismatch {
                    field_decl(field_number, field_schema)
                } else {
                    None
                };

                TextMark::Group {
                    header_nl_pos,
                    field_decl: field_decl_str,
                    is_mismatch,
                    tag,
                }
            }
        }
    }

    fn end_nested(
        &mut self,
        mark: TextMark,
        _raw_range: Range<usize>,
        close_facts: Option<GroupCloseFacts>,
    ) {
        match mark {
            TextMark::Message => {
                write_close_brace(self);
            }
            TextMark::Group {
                header_nl_pos,
                field_decl: decl_opt,
                is_mismatch,
                tag,
            } => {
                let annotations = ANNOTATIONS.with(|c| c.get());

                let mut close_mods: Vec<String> = Vec::new();
                match &close_facts {
                    None => close_mods.push("OPEN_GROUP".to_owned()),
                    Some(cf) => {
                        if let Some(ohb) = cf.end_tag_overhang_count {
                            close_mods.push(format!("etag_ohb: {}", ohb));
                        }
                        if cf.end_tag_is_out_of_range {
                            close_mods.push("ETAG_OOR".to_owned());
                        }
                        if let Some(end_field) = cf.mismatched_group_end {
                            close_mods.push(format!("END_MISMATCH: {}", end_field));
                        }
                    }
                }

                let mismatch_mod = annotations && is_mismatch;
                let has_field_decl = decl_opt.is_some();
                let has_open_tag_mods = annotations && (tag.tag_ohb.is_some() || tag.tag_oor);
                let has_close_mods = annotations && !close_mods.is_empty();

                if has_field_decl || mismatch_mod || has_open_tag_mods || has_close_mods {
                    let mut insert = String::new();
                    if let Some(ref d) = decl_opt {
                        insert.push_str("; ");
                        insert.push_str(d);
                    }
                    if mismatch_mod {
                        insert.push_str("; TYPE_MISMATCH");
                    }
                    if let Some(v) = tag.tag_ohb {
                        insert.push_str("; tag_ohb: ");
                        insert.push_str(&v.to_string());
                    }
                    if tag.tag_oor {
                        insert.push_str("; TAG_OOR");
                    }
                    for m in &close_mods {
                        insert.push_str("; ");
                        insert.push_str(m);
                    }
                    let insert_bytes = insert.as_bytes();
                    let n = insert_bytes.len();
                    self.out
                        .splice(header_nl_pos..header_nl_pos, insert_bytes.iter().copied());
                    CBL_START.with(|c| c.set(c.get() + n));
                }

                write_close_brace(self);
            }
        }
    }

    fn virtual_scalar(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        value_str: &str,
        _raw_range: Range<usize>,
    ) {
        push_indent(&mut self.out);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(b": ");
        self.out.extend_from_slice(value_str.as_bytes());
        if ANNOTATIONS.with(|c| c.get()) {
            if let Some(ann) = annotation {
                let mut aw = AnnWriter::new();
                aw.push(&mut self.out, ann.as_bytes());
            }
        }
        self.newline();
        CBL_START.with(|c| c.set(self.out.len()));
    }

    fn begin_virtual_nested(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        _type_fqdn: Option<&str>,
        _raw_start: usize,
        _payload_start: usize,
    ) -> TextMark {
        push_indent(&mut self.out);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(b" {");
        if ANNOTATIONS.with(|c| c.get()) {
            if let Some(ann) = annotation {
                let mut aw = AnnWriter::new();
                aw.push(&mut self.out, ann.as_bytes());
            }
        }
        self.newline();
        CBL_START.with(|c| c.set(self.out.len()));
        TextMark::Message
    }

    fn malformed(&mut self, field_number: u64, tag: TagFacts, kind: MalformedKind, raw: &[u8]) {
        use super::helpers::render_truncated_bytes;
        match kind {
            MalformedKind::InvalidTagType => {
                super::helpers::render_invalid_tag_type(raw, self);
            }
            MalformedKind::InvalidVarint => {
                render_invalid(
                    field_number,
                    None,
                    tag.tag_ohb,
                    tag.tag_oor,
                    "INVALID_VARINT",
                    raw,
                    self,
                );
            }
            MalformedKind::InvalidFixed64 => {
                render_invalid(
                    field_number,
                    None,
                    tag.tag_ohb,
                    tag.tag_oor,
                    "INVALID_FIXED64",
                    raw,
                    self,
                );
            }
            MalformedKind::InvalidFixed32 => {
                render_invalid(
                    field_number,
                    None,
                    tag.tag_ohb,
                    tag.tag_oor,
                    "INVALID_FIXED32",
                    raw,
                    self,
                );
            }
            MalformedKind::InvalidLen => {
                render_invalid(
                    field_number,
                    None,
                    tag.tag_ohb,
                    tag.tag_oor,
                    "INVALID_LEN",
                    raw,
                    self,
                );
            }
            MalformedKind::TruncatedBytes { missing } => {
                render_truncated_bytes(
                    field_number,
                    tag.tag_ohb,
                    tag.tag_oor,
                    tag.len_ohb,
                    missing,
                    raw,
                    self,
                );
            }
            MalformedKind::InvalidGroupEnd => {
                render_invalid(
                    field_number,
                    None,
                    tag.tag_ohb,
                    tag.tag_oor,
                    "INVALID_GROUP_END",
                    raw,
                    self,
                );
            }
        }
    }
}

// ── `ProbeSink`: lean structural-validity probe ─────────────────────────────

/// Read-only structural probe: walks a wire record via `render_message`,
/// counting malformities, without producing any output. Used to check "does
/// this payload parse as a well-formed message?" (spec 0097's unknown-LEN-
/// field cascade Step 1) without paying for tree construction (spec 0110
/// §2).
///
/// Always assumes its argument is a message being probed for plausibility.
/// LEN-delimited fields are treated as opaque bytes and never recursed into
/// (`treat_len_as_opaque` returns `true`) — a LEN field's own length prefix,
/// already bounds-checked by `render_message` before dispatch, is all the
/// validation needed at this level. GROUP fields still get mandatory
/// recursion (they have no length prefix, so their extent can only be
/// found by parsing through them), and any malformities found inside a
/// nested group roll up into this same counter automatically, since the
/// same `&mut ProbeSink` is threaded through every recursion level
/// (spec 0110 Open Issue #1).
///
/// Never mutates any shared render-mode thread-local state (`tracks_level`
/// returns `false`): it is a read-only helper that may be invoked from the
/// middle of an in-progress outer render (typically a `TextSink` pass), and
/// must not disturb that render's own state.
#[derive(Default)]
pub(super) struct ProbeSink {
    malformity_count: u64,
}

impl ProbeSink {
    pub(super) fn malformity_count(&self) -> u64 {
        self.malformity_count
    }
}

impl Sink for ProbeSink {
    type Mark = ();

    fn scalar_field(
        &mut self,
        _field_number: u64,
        _field_schema: Option<&FieldOrExt>,
        _tag: TagFacts,
        _value: ScalarValue<'_>,
        _raw_range: Range<usize>,
        _schema_present: bool,
    ) {
    }

    fn begin_nested(
        &mut self,
        _field_number: u64,
        _field_schema: Option<&FieldOrExt>,
        _tag: TagFacts,
        _kind: NestedKind,
        _raw_start: usize,
        _payload_start: usize,
    ) {
    }

    fn end_nested(
        &mut self,
        _mark: (),
        _raw_range: Range<usize>,
        _close_facts: Option<GroupCloseFacts>,
    ) {
    }

    fn virtual_scalar(
        &mut self,
        _name: &str,
        _annotation: Option<&str>,
        _value_str: &str,
        _raw_range: Range<usize>,
    ) {
        unreachable!(
            "ProbeSink is shallow for LEN fields (spec 0110 §2); Any/MessageSet \
             expansion never triggers under ProbeSink"
        )
    }

    fn begin_virtual_nested(
        &mut self,
        _name: &str,
        _annotation: Option<&str>,
        _type_fqdn: Option<&str>,
        _raw_start: usize,
        _payload_start: usize,
    ) {
        unreachable!(
            "ProbeSink is shallow for LEN fields (spec 0110 §2); Any/MessageSet \
             expansion never triggers under ProbeSink"
        )
    }

    fn malformed(&mut self, _field_number: u64, _tag: TagFacts, _kind: MalformedKind, _raw: &[u8]) {
        self.malformity_count += 1;
    }

    fn treat_len_as_opaque(&self) -> bool {
        true
    }

    fn tracks_level(&self) -> bool {
        false
    }
}

// ── `IndexingTextSink`: `TextSink` + `NodeSpan` index ───────────────────────

use super::LEVEL;

/// One node's raw/text extent + metadata, recorded by `IndexingTextSink`
/// (spec 0110 §3).
#[derive(Debug, Clone)]
pub struct NodeSpan {
    /// The field's wire field number. `0` for a virtual wrapper node
    /// (Any's `value {}`, MessageSet's `Item {}`/`message {}`), which has
    /// no real field number of its own — mirrors `Sink::malformed`'s own
    /// `field_number: 0` convention for `MalformedKind::InvalidTagType`.
    pub field_number: u64,
    /// Byte range in the source protobuf, absolute w.r.t. the original
    /// top-level buffer passed to `decode_and_render_indexed` — not local
    /// to this node's immediate parent, even though `render_message`
    /// itself recurses on re-sliced sub-buffers reset to a local `0`
    /// origin at every LEN-delimited descent (spec 0110 § Design
    /// rationale). `IndexingTextSink` reconstructs the absolute offset via
    /// an internal `raw_base` accumulator so consumers never need to
    /// re-derive it themselves.
    pub raw_range: Range<usize>,
    /// Line-number range in the rendered text — not a byte range (spec 0110
    /// § Design rationale): survives `render_group_field`'s post-hoc splice
    /// unmodified, since that splice always lengthens a line but never
    /// inserts a `\n`.
    pub text_range: Range<usize>,
    /// Indentation depth (matches `render_text::LEVEL` at the time this
    /// node was opened).
    pub level: usize,
    /// FQDN of the type this node was rendered as, when known: the
    /// declared field type for a regular nested message/group, or — for an
    /// Any/MessageSet-expanded wrapper node — the *resolved* type, which
    /// generally differs from the field's own declared type. `None` for
    /// scalar fields and any node whose type genuinely isn't known — this
    /// is *not* a scalar/message discriminator (see `is_message`): a
    /// message/group node with no resolved schema also has `type_fqdn:
    /// None`.
    pub type_fqdn: Option<String>,
    /// `true` for a nested message/group node (`begin_nested`/
    /// `begin_virtual_nested`..`end_nested`), `false` for a scalar field
    /// (`scalar_field`) — set independently of `type_fqdn`, which is
    /// `None` for both a scalar *and* a schema-unresolved message/group.
    /// This is the structural shape discriminator consumers should use
    /// (e.g. `protolens`'s override-target validation); `type_fqdn.is_some
    /// ()` alone is ambiguous (spec 0114 §1.2).
    pub is_message: bool,
    /// Absolute offset (same coordinate space as `raw_range`) of the
    /// *enclosing packed record's own* tag, when this span is one element
    /// of a packed-repeated field — `None` for every other node, including
    /// non-packed scalars (which have their own tag) and the record's own
    /// summary span in the empty/invalid fallback cases (§2.4/§2.5), which
    /// again carry their own tag. Purely an internal discriminator (spec
    /// 0115 Non-goals) — not surfaced in any user-visible rendering.
    pub packed_record_start: Option<usize>,
    /// The wire type this node's own value was decoded from — one of
    /// `crate::helpers::{WT_VARINT, WT_I64, WT_LEN, WT_START_GROUP,
    /// WT_I32}`. Set independently of `is_message`/`type_fqdn`: a `LEN`
    /// field rendered as a scalar string/bytes still carries `WT_LEN`, so
    /// consumers can tell it apart from a genuinely non-recursable scalar
    /// (`WT_VARINT`/`WT_I64`/`WT_I32`) without re-parsing the wire tag
    /// themselves. For a packed-repeated element, this is the *element's*
    /// own wire type (from the field's declared `Kind`), not the packed
    /// record's outer `WT_LEN`. For a virtual wrapper node (Any/MessageSet
    /// expansion), `WT_LEN` is used regardless of the original wire
    /// encoding — always message-shaped, so the exact value only matters
    /// for the `WT_LEN | WT_START_GROUP` recursion-eligibility check,
    /// which either value would satisfy.
    pub wire_type: u32,
}

/// Per-`IndexingTextSink` "in-progress nested node" marker: captures what's
/// needed to build a `NodeSpan` at `end_nested`, plus the wrapped
/// `TextSink`'s own `Mark` to delegate through.
pub(super) struct IndexMark {
    field_number: u64,
    text_start: usize,
    level: usize,
    type_fqdn: Option<String>,
    is_message: bool,
    wire_type: u32,
    /// `IndexingTextSink::raw_base` as it was *before* this node was
    /// opened — i.e. the base to translate this node's own `raw_range`
    /// with at `end_nested`, and to restore `raw_base` to once this
    /// node's children are done being visited.
    raw_base: usize,
    inner: TextMark,
}

/// FQDN of a field's *declared* type, when it's a message-kinded field with
/// a known schema (covers both LEN-delimited nested messages and GROUPs,
/// whose `FieldOrExt::kind()` is also `Kind::Message`) — `None` otherwise
/// (unknown field, or a wire-type mismatch where the schema doesn't
/// describe a message).
fn declared_type_fqdn(field_schema: Option<&FieldOrExt>) -> Option<String> {
    field_schema.and_then(|fs| match fs.kind() {
        Kind::Message(desc) => Some(desc.full_name().to_owned()),
        _ => None,
    })
}

/// Wraps a `TextSink` by composition, delegating every `Sink` call to it
/// unchanged — so its text output is byte-for-byte identical to a plain
/// `TextSink` — while additionally recording one `NodeSpan` per
/// `scalar_field`/`end_nested` call (spec 0110 §3).
pub(super) struct IndexingTextSink {
    inner: TextSink,
    spans: Vec<NodeSpan>,
    /// Absolute offset (w.r.t. the original top-level buffer) that local
    /// offset `0` currently maps to — i.e. the base of whatever coordinate
    /// frame is "active" at this point in the recursive descent. Starts at
    /// `0` since the top-level buffer's own frame origin *is* absolute `0`.
    /// Pushed/popped across `begin_nested`/`begin_virtual_nested` /
    /// `end_nested` pairs via `IndexMark::raw_base` (spec 0110 §3 —
    /// absolute `raw_range`).
    raw_base: usize,
}

impl IndexingTextSink {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            inner: TextSink::new(capacity),
            spans: Vec::new(),
            raw_base: 0,
        }
    }

    /// Write raw header bytes (see `TextSink::write_header`), keeping
    /// `line_count` — and therefore every later `NodeSpan::text_range` —
    /// accurate across the file-level header / extra-header comment.
    pub(super) fn write_header(&mut self, bytes: &[u8]) {
        self.inner.write_header(bytes);
    }

    /// Current output length in bytes (mirrors `TextSink::out.len()`, used
    /// to initialise `CBL_START` the same way `decode_and_render` does).
    pub(super) fn out_len(&self) -> usize {
        self.inner.out.len()
    }

    /// Consume `self`, returning the rendered text and its `NodeSpan` index.
    pub(super) fn into_parts(self) -> (Vec<u8>, Vec<NodeSpan>) {
        (self.inner.into_inner(), self.spans)
    }
}

impl Sink for IndexingTextSink {
    type Mark = IndexMark;

    fn scalar_field(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        value: ScalarValue<'_>,
        raw_range: Range<usize>,
        schema_present: bool,
    ) {
        let text_start = self.inner.line_count();
        let level = LEVEL.with(|c| c.get());
        // Captured before `value` is moved into the delegated call below,
        // so we can still tell (after that call) whether this was a packed
        // field — spec 0115 §2 — and what wire type it came off the wire
        // as (spec 0118 §8).
        let packed_data = match &value {
            ScalarValue::Packed(data) => Some(*data),
            _ => None,
        };
        let wire_type = match &value {
            ScalarValue::Varint { .. } => WT_VARINT,
            ScalarValue::Fixed64(_) => WT_I64,
            ScalarValue::Fixed32(_) => WT_I32,
            ScalarValue::Bytes(_) | ScalarValue::Packed(_) => WT_LEN,
        };
        self.inner.scalar_field(
            field_number,
            field_schema,
            tag,
            value,
            raw_range.clone(),
            schema_present,
        );
        let text_end = self.inner.line_count();
        let base = self.raw_base;

        // Packed-repeated scalar field: one `NodeSpan` per element instead
        // of one span for the whole record (spec 0115 §2), reusing
        // `decode_packed_elems`'s output purely for byte-range bookkeeping
        // — `render_packed`'s own line-writing logic (already run via the
        // delegated call above) is untouched and not duplicated here.
        if let Some(data) = packed_data {
            let fs = field_schema.expect("packed scalar requires a known field schema");
            // Each element's own wire type (not the packed record's outer
            // `WT_LEN`) — mirrors the fixed/varint split
            // `decode_packed_elems` itself already makes.
            let elem_wire_type = match fs.kind() {
                Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => WT_I64,
                Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => WT_I32,
                _ => WT_VARINT,
            };
            if let Ok(elems) = decode_packed_elems(data, fs) {
                if !elems.is_empty() {
                    let payload_start = raw_range.end - data.len();
                    let packed_record_start = Some(base + raw_range.start);
                    for (i, elem) in elems.iter().enumerate() {
                        self.spans.push(NodeSpan {
                            field_number,
                            raw_range: (base + payload_start + elem.byte_range.start)
                                ..(base + payload_start + elem.byte_range.end),
                            text_range: (text_start + i)..(text_start + i + 1),
                            level,
                            type_fqdn: None,
                            is_message: false,
                            packed_record_start,
                            wire_type: elem_wire_type,
                        });
                    }
                    return;
                }
            }
        }

        // Every other scalar (including an empty or undecodable packed
        // record — spec 0115 §2.4/§2.5): one span for the whole field.
        self.spans.push(NodeSpan {
            field_number,
            raw_range: (base + raw_range.start)..(base + raw_range.end),
            text_range: text_start..text_end,
            level,
            type_fqdn: None,
            is_message: false,
            packed_record_start: None,
            wire_type,
        });
    }

    fn begin_nested(
        &mut self,
        field_number: u64,
        field_schema: Option<&FieldOrExt>,
        tag: TagFacts,
        kind: NestedKind,
        raw_start: usize,
        payload_start: usize,
    ) -> IndexMark {
        let text_start = self.inner.line_count();
        let level = LEVEL.with(|c| c.get());
        let type_fqdn = declared_type_fqdn(field_schema);
        let wire_type = match kind {
            NestedKind::Message => WT_LEN,
            NestedKind::Group => WT_START_GROUP,
        };
        let raw_base = self.raw_base;
        let inner = self.inner.begin_nested(
            field_number,
            field_schema,
            tag,
            kind,
            raw_start,
            payload_start,
        );
        self.raw_base = raw_base + payload_start;
        IndexMark {
            field_number,
            text_start,
            level,
            type_fqdn,
            is_message: true,
            wire_type,
            raw_base,
            inner,
        }
    }

    fn end_nested(
        &mut self,
        mark: IndexMark,
        raw_range: Range<usize>,
        close_facts: Option<GroupCloseFacts>,
    ) {
        let IndexMark {
            field_number,
            text_start,
            level,
            type_fqdn,
            is_message,
            wire_type,
            raw_base,
            inner,
        } = mark;
        self.inner.end_nested(inner, raw_range.clone(), close_facts);
        self.raw_base = raw_base;
        let text_end = self.inner.line_count();
        self.spans.push(NodeSpan {
            field_number,
            raw_range: (raw_base + raw_range.start)..(raw_base + raw_range.end),
            text_range: text_start..text_end,
            level,
            type_fqdn,
            is_message,
            packed_record_start: None,
            wire_type,
        });
    }

    fn virtual_scalar(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        value_str: &str,
        raw_range: Range<usize>,
    ) {
        // Any's `type_url` / MessageSet's `type_id` lines: not listed as a
        // NodeSpan-producing event (spec 0110 §3) — delegate only.
        self.inner
            .virtual_scalar(name, annotation, value_str, raw_range);
    }

    fn begin_virtual_nested(
        &mut self,
        name: &str,
        annotation: Option<&str>,
        type_fqdn: Option<&str>,
        raw_start: usize,
        payload_start: usize,
    ) -> IndexMark {
        let text_start = self.inner.line_count();
        let level = LEVEL.with(|c| c.get());
        let raw_base = self.raw_base;
        let inner =
            self.inner
                .begin_virtual_nested(name, annotation, type_fqdn, raw_start, payload_start);
        self.raw_base = raw_base + payload_start;
        IndexMark {
            field_number: 0,
            text_start,
            level,
            type_fqdn: type_fqdn.map(str::to_owned),
            is_message: true,
            // Always message-shaped; see `NodeSpan::wire_type` doc comment.
            wire_type: WT_LEN,
            raw_base,
            inner,
        }
    }

    fn malformed(&mut self, field_number: u64, tag: TagFacts, kind: MalformedKind, raw: &[u8]) {
        self.inner.malformed(field_number, tag, kind, raw);
    }

    fn treat_len_as_opaque(&self) -> bool {
        self.inner.treat_len_as_opaque()
    }

    fn tracks_level(&self) -> bool {
        self.inner.tracks_level()
    }
}
