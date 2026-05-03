// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use prost_reflect::Kind;

use super::FieldOrExt;

use crate::helpers::{
    decode_bool, decode_int32, decode_int64, decode_sint32, decode_sint64, decode_uint32,
    decode_uint64,
};
use crate::serialize::common::{
    format_bool_protoc, format_enum_protoc, format_int32_protoc, format_int64_protoc,
    format_sint32_protoc, format_sint64_protoc, format_uint32_protoc, format_uint64_protoc,
    format_wire_varint_protoc,
};

use super::helpers::{wfl_prefix_n, AnnWriter};
use super::{ANNOTATIONS, CBL_START};

/// Classify a varint value against the schema's expected type.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum VarintKind {
    Wire,     // unknown schema → WireVarint
    Mismatch, // schema known but type not a varint type → proto2_has_type_mismatch
    Int64,
    Uint64,
    Int32,      // non-negative or spec-correct 10-byte sign-extended negative
    TruncInt32, // truncated 5-byte negative int32  ([0x80000000, 0xFFFFFFFF])
    Uint32,
    Bool,
    Enum,      // non-negative or spec-correct 10-byte sign-extended negative enum
    TruncEnum, // truncated 5-byte negative enum   ([0x80000000, 0xFFFFFFFF])
    Sint32,
    Sint64,
}

#[inline]
pub(super) fn decode_varint_typed(val: u64, fs: &FieldOrExt) -> (VarintKind, u64) {
    match fs.kind() {
        Kind::Int64 => (VarintKind::Int64, val),
        Kind::Uint64 => (VarintKind::Uint64, val),
        Kind::Int32 => {
            if val <= 0x7FFF_FFFF {
                (VarintKind::Int32, val) // non-negative
            } else if val <= 0xFFFF_FFFF {
                (VarintKind::TruncInt32, val) // truncated 5-byte negative
            } else if val >= 0xFFFF_FFFF_8000_0000 {
                (VarintKind::Int32, val) // spec-correct 10-byte negative
            } else {
                (VarintKind::Mismatch, val)
            }
        }
        Kind::Bool => {
            if val > 1 {
                (VarintKind::Mismatch, val)
            } else {
                (VarintKind::Bool, val)
            }
        }
        Kind::Uint32 => {
            if val >= (1 << 32) {
                (VarintKind::Mismatch, val)
            } else {
                (VarintKind::Uint32, val)
            }
        }
        Kind::Enum(_) => {
            if val <= 0x7FFF_FFFF {
                (VarintKind::Enum, val) // non-negative
            } else if val <= 0xFFFF_FFFF {
                (VarintKind::TruncEnum, val) // truncated 5-byte negative
            } else if val >= 0xFFFF_FFFF_8000_0000 {
                (VarintKind::Enum, val) // spec-correct 10-byte negative
            } else {
                (VarintKind::Mismatch, val)
            }
        }
        Kind::Sint32 => {
            if val >= (1 << 32) {
                (VarintKind::Mismatch, val)
            } else {
                (VarintKind::Sint32, val)
            }
        }
        Kind::Sint64 => (VarintKind::Sint64, val),
        _ => (VarintKind::Mismatch, val),
    }
}

/// Format a typed varint value as a string.
#[inline]
pub(super) fn fmt_varint(kind: VarintKind, raw: u64) -> String {
    match kind {
        // Unknown (no schema) or wire-type mismatch: render by wire type → decimal.
        VarintKind::Wire => format_wire_varint_protoc(raw),
        VarintKind::Mismatch => format_wire_varint_protoc(raw),
        VarintKind::Int64 => format_int64_protoc(decode_int64(raw)),
        VarintKind::Uint64 => format_uint64_protoc(decode_uint64(raw)),
        // decode_int32 does `(v as u32) as i32` — takes the low 32 bits as signed i32.
        // This correctly handles all three sub-ranges:
        //   [0, 0x7FFFFFFF]              → non-negative
        //   [0x80000000, 0xFFFFFFFF]     → truncated negative (TruncInt32/TruncEnum only)
        //   [0xFFFFFFFF80000000, …]      → spec-correct 10-byte negative (Int32/Enum only)
        VarintKind::Int32 | VarintKind::TruncInt32 => format_int32_protoc(decode_int32(raw)),
        VarintKind::Uint32 => format_uint32_protoc(decode_uint32(raw)),
        VarintKind::Bool => format_bool_protoc(decode_bool(raw)).to_owned(),
        VarintKind::Enum | VarintKind::TruncEnum => format_enum_protoc(decode_int32(raw)),
        VarintKind::Sint32 => format_sint32_protoc(decode_sint32(raw)),
        VarintKind::Sint64 => format_sint64_protoc(decode_sint64(raw)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_varint_field(
    field_number: u64,
    field_schema: Option<&FieldOrExt>,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    val_ohb: Option<u64>,
    kind: VarintKind,
    raw_val: u64,
    out: &mut Vec<u8>,
) {
    let annotations = ANNOTATIONS.with(|c| c.get());
    let is_mismatch = kind == VarintKind::Mismatch;
    let is_wire = kind == VarintKind::Wire;
    let is_trunc = kind == VarintKind::TruncInt32 || kind == VarintKind::TruncEnum;
    let is_enum = kind == VarintKind::Enum || kind == VarintKind::TruncEnum;
    let unknown = field_schema.is_none();

    // When annotations=false: skip unknown and raw-wire fields
    if !annotations && (unknown || is_wire || is_mismatch) {
        return;
    }

    // For ENUM fields: resolve symbolic name and record raw i32 for the annotation.
    // raw_i32 is decode_int32(raw_val) — takes the low 32 bits as signed i32.
    let enum_i32: i32 = decode_int32(raw_val);

    let (value_str, enum_unknown) = if is_enum {
        if let Some(fi) = field_schema {
            if let Kind::Enum(enum_desc) = fi.kind() {
                match enum_desc.get_value(enum_i32) {
                    Some(v) => (v.name().to_owned(), false),
                    None => (fmt_varint(kind, raw_val), true),
                }
            } else {
                (fmt_varint(kind, raw_val), false)
            }
        } else {
            (fmt_varint(kind, raw_val), false)
        }
    } else {
        (fmt_varint(kind, raw_val), false)
    };

    // v2 key rule:
    //   - unknown (no schema), is_wire, or is_mismatch: field NUMBER
    //   - normal known field: field NAME
    let use_numeric_key = is_wire || unknown || is_mismatch;
    wfl_prefix_n(field_number, field_schema, use_numeric_key, out);
    out.extend_from_slice(value_str.as_bytes());

    if annotations {
        let mut aw = AnnWriter::new();
        if unknown || is_wire || is_mismatch {
            // Unknown / raw-wire / type-mismatch: wire type FIRST, then modifiers, NO field_decl
            aw.push_wire(out, "varint");
            if let Some(v) = tag_ohb {
                aw.push_u64_mod(out, b"tag_ohb: ", v);
            }
            if tag_oor {
                aw.push(out, b"TAG_OOR");
            }
            if let Some(v) = val_ohb {
                aw.push_u64_mod(out, b"val_ohb: ", v);
            }
            if is_mismatch {
                aw.push(out, b"TYPE_MISMATCH");
            }
        } else {
            // Known field (no mismatch): field_decl FIRST, then modifiers
            let enum_raw = if is_enum { Some(enum_i32) } else { None };
            aw.push_field_decl(out, field_number, field_schema, enum_raw, None);
            if let Some(v) = tag_ohb {
                aw.push_u64_mod(out, b"tag_ohb: ", v);
            }
            if tag_oor {
                aw.push(out, b"TAG_OOR");
            }
            if let Some(v) = val_ohb {
                aw.push_u64_mod(out, b"val_ohb: ", v);
            }
            if is_trunc {
                aw.push(out, b"truncated_neg");
            }
            if enum_unknown {
                aw.push(out, b"ENUM_UNKNOWN");
            }
        }
    }
    out.push(b'\n');
    CBL_START.with(|c| c.set(out.len())); // content line: set past-end to inhibit folding
}
