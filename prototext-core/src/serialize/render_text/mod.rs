// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

mod helpers;
mod packed;
mod varint;

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use prost_reflect::{Cardinality, ExtensionDescriptor, FieldDescriptor, Kind, MessageDescriptor};

use crate::helpers::{
    decode_double, decode_fixed32, decode_fixed64, decode_float, decode_sfixed32, decode_sfixed64,
};
use crate::helpers::{
    parse_varint, parse_wiretag, WiretagResult, WT_END_GROUP, WT_I32, WT_I64, WT_LEN,
    WT_START_GROUP, WT_VARINT,
};
use crate::schema::ParsedSchema;
use crate::serialize::common::{
    format_double_protoc, format_fixed32_protoc, format_fixed64_protoc, format_float_protoc,
    format_sfixed32_protoc, format_sfixed64_protoc, format_wire_fixed32_protoc,
    format_wire_fixed64_protoc,
};

use helpers::{
    render_group_field, render_invalid, render_invalid_tag_type, render_len_field, render_scalar,
    render_truncated_bytes, ScalarCtx,
};
use varint::{decode_varint_typed, render_varint_field, VarintKind};

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

// ── Render-mode state ─────────────────────────────────────────────────────────
//
// `CBL_START` is set to `out.len()` by `write_close_brace` before writing a
// `}\n` line, and reset to `out.len()` (past-end) by every other write.  It
// is currently unused beyond being maintained; the close-brace folding feature
// it was intended to support has been removed.
//
thread_local! {
    pub(super) static CBL_START:    Cell<usize> = const { Cell::new(0) };
    // Set once per `decode_and_render` call; read by every internal render fn.
    pub(super) static ANNOTATIONS:  Cell<bool>  = const { Cell::new(false) };
    pub(super) static INDENT_SIZE:  Cell<usize> = const { Cell::new(2) };
    // Tracks recursion depth; managed via `enter_level()` / `LevelGuard`.
    pub(super) static LEVEL:        Cell<usize> = const { Cell::new(0) };
}

/// RAII guard for `LEVEL`: increments on construction, decrements on drop.
/// Guarantees the level is restored even if the callee panics.
pub(super) struct LevelGuard;

impl Drop for LevelGuard {
    fn drop(&mut self) {
        LEVEL.with(|l| l.set(l.get() - 1));
    }
}

pub(super) fn enter_level() -> LevelGuard {
    LEVEL.with(|l| l.set(l.get() + 1));
    LevelGuard
}

/// Return `true` when `data` is already rendered prototext text (fast-path).
pub fn is_prototext_text(data: &[u8]) -> bool {
    data.starts_with(PROTOTEXT_MAGIC)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Decode raw protobuf binary and render as protoc-style text in one pass.
///
/// Writes the `#@ prototext: protoc\n` header followed by field lines directly
/// into a pre-allocated `Vec<u8>`.
///
/// Parameters mirror `format_as_text` in `lib.rs`.
pub fn decode_and_render(
    buf: &[u8],
    schema: Option<&ParsedSchema>,
    annotations: bool,
    indent_size: usize,
) -> Vec<u8> {
    let capacity = buf.len() * 8;
    let mut out = Vec::with_capacity(capacity);

    // Header
    out.extend_from_slice(b"#@ prototext: protoc\n");
    // Initialise render-mode state.
    // CBL_START past the end so the first write_close_brace always takes
    // the fresh-write path.
    CBL_START.with(|c| c.set(out.len()));
    ANNOTATIONS.with(|c| c.set(annotations));
    INDENT_SIZE.with(|c| c.set(indent_size));
    LEVEL.with(|c| c.set(0));

    // Build a flat name→MessageDescriptor map for nested-type lookups.
    // Keyed by bare FQN (no leading dot), matching prost-reflect's convention.
    let all_descriptors: Option<HashMap<String, Arc<MessageDescriptor>>> =
        schema.map(build_descriptor_map);
    let all_schemas = all_descriptors.as_ref();

    let root_desc: Option<MessageDescriptor> = schema.and_then(|s| s.root_descriptor());

    render_message(buf, 0, None, root_desc.as_ref(), all_schemas, &mut out);

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

/// Build a `HashMap<bare_fqn, Arc<MessageDescriptor>>` from a `ParsedSchema`.
fn build_descriptor_map(schema: &ParsedSchema) -> HashMap<String, Arc<MessageDescriptor>> {
    schema
        .pool()
        .all_messages()
        .map(|msg| (msg.full_name().to_string(), Arc::new(msg)))
        .collect()
}

// ── Core recursive render-while-decode ───────────────────────────────────────

/// Parse and render one protobuf message into `out`.
///
/// Returns `(next_pos, group_end_tag)`:
/// - `next_pos`: byte position after this message (for the caller to
///   continue its own parse loop, or for GROUP end detection).
/// - `group_end_tag`: `Some(tag)` when parsing terminated on a `WT_END_GROUP`.
pub(super) fn render_message(
    buf: &[u8],
    start: usize,
    my_group: Option<u64>,
    schema: Option<&MessageDescriptor>,
    all_schemas: Option<&HashMap<String, Arc<MessageDescriptor>>>,
    out: &mut Vec<u8>,
) -> (usize, Option<WiretagResult>) {
    let buflen = buf.len();
    let mut pos = start;

    loop {
        if pos == buflen {
            return (pos, None);
        }

        // ── Parse wire tag ────────────────────────────────────────────────────

        let tag = parse_wiretag(buf, pos);

        if let Some(ref wtag_gar) = tag.wtag_gar {
            // Invalid wire tag: consume rest of buffer as INVALID_TAG_TYPE
            render_invalid_tag_type(wtag_gar, out);
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
                    render_invalid(
                        field_number,
                        field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        "INVALID_VARINT",
                        varint_gar,
                        out,
                    );
                    return (buflen, None);
                }
                pos = vr.next_pos;
                let val_ohb = vr.varint_ohb;
                let val = vr.varint.unwrap();

                let (content_kind, typed_val) = if let Some(ref fs) = field_schema {
                    decode_varint_typed(val, fs)
                } else {
                    (VarintKind::Wire, val)
                };

                render_varint_field(
                    field_number,
                    field_schema.as_ref(),
                    tag_ohb,
                    tag_oor,
                    val_ohb,
                    content_kind,
                    typed_val,
                    out,
                );
            }

            // ── FIXED64 ──────────────────────────────────────────────────────
            WT_I64 => {
                if pos + 8 > buflen {
                    let raw = &buf[pos..];
                    render_invalid(
                        field_number,
                        field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        "INVALID_FIXED64",
                        raw,
                        out,
                    );
                    return (buflen, None);
                }
                let data = &buf[pos..pos + 8];
                pos += 8;

                let is_mismatch;
                let mut nan_bits: Option<u64> = None;
                let value_str = if let Some(ref fs) = field_schema {
                    match fs.kind() {
                        Kind::Double => {
                            is_mismatch = false;
                            let v = decode_double(data);
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
                            format_fixed64_protoc(decode_fixed64(data))
                        }
                        Kind::Sfixed64 => {
                            is_mismatch = false;
                            format_sfixed64_protoc(decode_sfixed64(data))
                        }
                        _ => {
                            is_mismatch = true;
                            format_wire_fixed64_protoc(decode_fixed64(data))
                        } // mismatch → hex
                    }
                } else {
                    is_mismatch = false;
                    format_wire_fixed64_protoc(decode_fixed64(data)) // unknown → hex
                };

                render_scalar(
                    &ScalarCtx {
                        field_number,
                        field_schema: field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        len_ohb: None,
                        wire_type_name: "fixed64",
                        nan_bits,
                    },
                    &value_str,
                    is_mismatch,
                    out,
                );
            }

            // ── LENGTH-DELIMITED ─────────────────────────────────────────────
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if let Some(ref varint_gar) = lr.varint_gar {
                    render_invalid(
                        field_number,
                        field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        "INVALID_LEN",
                        varint_gar,
                        out,
                    );
                    return (buflen, None);
                }
                let len_ohb = lr.varint_ohb;
                pos = lr.next_pos;
                let length = lr.varint.unwrap() as usize;

                if pos + length > buflen {
                    let missing = (length - (buflen - pos)) as u64;
                    let raw = &buf[pos..];
                    render_truncated_bytes(
                        field_number,
                        tag_ohb,
                        tag_oor,
                        len_ohb,
                        missing,
                        raw,
                        out,
                    );
                    return (buflen, None);
                }
                let data = &buf[pos..pos + length];
                pos += length;

                render_len_field(
                    field_number,
                    field_schema.as_ref(),
                    all_schemas,
                    tag_ohb,
                    tag_oor,
                    len_ohb,
                    data,
                    out,
                );
            }

            // ── START GROUP ──────────────────────────────────────────────────
            WT_START_GROUP => {
                render_group_field(
                    buf,
                    &mut pos,
                    field_number,
                    field_schema.as_ref(),
                    all_schemas,
                    tag_ohb,
                    tag_oor,
                    out,
                );
            }

            // ── END GROUP ────────────────────────────────────────────────────
            WT_END_GROUP => {
                if my_group.is_none() {
                    // Unexpected END_GROUP outside a group
                    let raw = &buf[pos..];
                    render_invalid(
                        field_number,
                        field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        "INVALID_GROUP_END",
                        raw,
                        out,
                    );
                    return (buflen, None);
                }
                // Valid END_GROUP: return to parent without rendering a field.
                return (pos, Some(tag));
            }

            // ── FIXED32 ──────────────────────────────────────────────────────
            WT_I32 => {
                if pos + 4 > buflen {
                    let raw = &buf[pos..];
                    render_invalid(
                        field_number,
                        field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        "INVALID_FIXED32",
                        raw,
                        out,
                    );
                    return (buflen, None);
                }
                let data = &buf[pos..pos + 4];
                pos += 4;

                let is_mismatch;
                let mut nan_bits: Option<u64> = None;
                let value_str = if let Some(ref fs) = field_schema {
                    match fs.kind() {
                        Kind::Float => {
                            is_mismatch = false;
                            let v = decode_float(data);
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
                            format_fixed32_protoc(decode_fixed32(data))
                        }
                        Kind::Sfixed32 => {
                            is_mismatch = false;
                            format_sfixed32_protoc(decode_sfixed32(data))
                        }
                        _ => {
                            is_mismatch = true;
                            format_wire_fixed32_protoc(decode_fixed32(data))
                        } // mismatch → hex (D2)
                    }
                } else {
                    is_mismatch = false;
                    format_wire_fixed32_protoc(decode_fixed32(data)) // unknown → hex
                };

                render_scalar(
                    &ScalarCtx {
                        field_number,
                        field_schema: field_schema.as_ref(),
                        tag_ohb,
                        tag_oor,
                        len_ohb: None,
                        wire_type_name: "fixed32",
                        nan_bits,
                    },
                    &value_str,
                    is_mismatch,
                    out,
                );
            }

            _ => unreachable!("wire type > 5 caught by parse_wiretag"),
        }
    }
}
