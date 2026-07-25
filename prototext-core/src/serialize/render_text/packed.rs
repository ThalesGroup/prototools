// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::ops::Range;

use prost_reflect::Kind;

use super::FieldOrExt;

use crate::helpers::{
    decode_double, decode_fixed32, decode_fixed64, decode_float, decode_int32, decode_sfixed32,
    decode_sfixed64, decode_sint32, decode_sint64, decode_uint32, parse_varint,
};
use crate::serialize::common::{
    format_bool_protoc, format_double_protoc, format_enum_protoc, format_fixed32_protoc,
    format_fixed64_protoc, format_float_protoc, format_int32_protoc, format_int64_protoc,
    format_sfixed32_protoc, format_sfixed64_protoc, format_sint32_protoc, format_sint64_protoc,
    format_uint32_protoc, format_uint64_protoc,
};

use super::helpers::{push_indent, wfl_prefix_n, AnnWriter};
use super::sink::TextSink;
use super::{ANNOTATIONS, CBL_START};

/// Write `bits` as zero-padded lowercase hex into `out` without heap allocation.
/// Uses 16 digits for doubles (bits > u32::MAX), 8 digits for floats.
#[inline]
fn write_nan_hex(bits: u64, out: &mut Vec<u8>) {
    use std::io::Write as IoWrite;
    if bits > 0xFFFF_FFFF {
        let mut buf = [0u8; 16];
        write!(&mut buf[..], "{:016x}", bits).expect("write to stack buffer");
        out.extend_from_slice(&buf);
    } else {
        let mut buf = [0u8; 8];
        write!(&mut buf[..], "{:08x}", bits).expect("write to stack buffer");
        out.extend_from_slice(&buf);
    }
}

// ── Per-element data ──────────────────────────────────────────────────────────

/// Data for one decoded element of a packed field.
pub(super) struct PackedElem {
    value_str: String,
    /// Per-element varint overhang bytes (for varint-packed types).
    ohb: u64,
    /// True when a negative int32/enum is encoded in the truncated 5-byte form.
    neg_trunc: bool,
    /// Non-canonical NaN bit pattern (float: u32 as u64; double: u64).
    /// `None` for canonical NaN or non-NaN values.
    nan_bits: Option<u64>,
    /// For ENUM: raw i32 numeric value (for the EnumName(N) annotation).
    enum_num: Option<i32>,
    /// This element's own byte span *within the packed payload* (`data`,
    /// the field's LEN-delimited contents after tag+length) — not yet
    /// translated to absolute document coordinates (spec 0115 §1).
    pub(super) byte_range: Range<usize>,
}

// ── Fixed-width packed decoder ────────────────────────────────────────────────

fn decode_packed_fixed_elems(
    data: &[u8],
    elem_size: usize,
    kind: &Kind,
) -> Result<Vec<PackedElem>, ()> {
    let n = data.len();
    if !n.is_multiple_of(elem_size) {
        return Err(());
    }
    let count = n / elem_size;
    let mut elems = Vec::with_capacity(count);
    let mut i = 0;
    while i < n {
        if i + elem_size > n {
            return Err(());
        }
        let b = &data[i..i + elem_size];
        let (value_str, nan_bits) = match kind {
            Kind::Double => {
                let v = decode_double(b);
                let nb = if v.is_nan() {
                    let bits = v.to_bits();
                    if bits != f64::NAN.to_bits() {
                        Some(bits)
                    } else {
                        None
                    }
                } else {
                    None
                };
                (format_double_protoc(v), nb)
            }
            Kind::Float => {
                let v = decode_float(b);
                let nb = if v.is_nan() {
                    let bits = v.to_bits();
                    if bits != f32::NAN.to_bits() {
                        Some(bits as u64)
                    } else {
                        None
                    }
                } else {
                    None
                };
                (format_float_protoc(v), nb)
            }
            Kind::Fixed64 => (format_fixed64_protoc(decode_fixed64(b)), None),
            Kind::Sfixed64 => (format_sfixed64_protoc(decode_sfixed64(b)), None),
            Kind::Fixed32 => (format_fixed32_protoc(decode_fixed32(b)), None),
            Kind::Sfixed32 => (format_sfixed32_protoc(decode_sfixed32(b)), None),
            _ => return Err(()),
        };
        elems.push(PackedElem {
            value_str,
            ohb: 0,
            neg_trunc: false,
            nan_bits,
            enum_num: None,
            byte_range: i..i + elem_size,
        });
        i += elem_size;
    }
    Ok(elems)
}

// ── Varint-packed decoder ─────────────────────────────────────────────────────

fn decode_packed_varint_elems(data: &[u8], fs: &FieldOrExt) -> Result<Vec<PackedElem>, ()> {
    let is_enum = matches!(fs.kind(), Kind::Enum(_));
    let mut elems = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let elem_start = i;
        let vr = parse_varint(data, i);
        if vr.varint_gar.is_some() {
            return Err(());
        }
        i = vr.next_pos;
        let ohb = vr.varint_ohb.unwrap_or(0);
        let v = vr.varint.unwrap();

        let (value_str, neg_trunc, enum_num) = match fs.kind() {
            Kind::Int64 => (
                format_int64_protoc(crate::helpers::decode_int64(v)),
                false,
                None,
            ),
            Kind::Uint64 => (format_uint64_protoc(v), false, None),
            Kind::Int32 => {
                if v <= 0x7FFF_FFFF {
                    (format_int32_protoc(decode_int32(v)), false, None)
                } else if v <= 0xFFFF_FFFF {
                    (format_int32_protoc(decode_int32(v)), true, None)
                } else if v >= 0xFFFF_FFFF_8000_0000 {
                    (format_int32_protoc(decode_int32(v)), false, None)
                } else {
                    return Err(());
                }
            }
            Kind::Bool => {
                if v > 1 {
                    return Err(());
                }
                (format_bool_protoc(v != 0).to_owned(), false, None)
            }
            Kind::Uint32 => {
                if v >= (1 << 32) {
                    return Err(());
                }
                (format_uint32_protoc(decode_uint32(v)), false, None)
            }
            Kind::Enum(ref enum_desc) => {
                let (n_i32, trunc) = if v <= 0x7FFF_FFFF {
                    (decode_int32(v), false)
                } else if v <= 0xFFFF_FFFF {
                    (decode_int32(v), true)
                } else if v >= 0xFFFF_FFFF_8000_0000 {
                    (decode_int32(v), false)
                } else {
                    return Err(());
                };
                let s = match enum_desc.get_value(n_i32) {
                    Some(ev) => ev.name().to_owned(),
                    None => format_enum_protoc(n_i32),
                };
                (s, trunc, Some(n_i32))
            }
            Kind::Sint32 => {
                if v >= (1 << 32) {
                    return Err(());
                }
                (format_sint32_protoc(decode_sint32(v)), false, None)
            }
            Kind::Sint64 => (format_sint64_protoc(decode_sint64(v)), false, None),
            _ => return Err(()),
        };

        elems.push(PackedElem {
            value_str,
            ohb,
            neg_trunc,
            nan_bits: None,
            enum_num: if is_enum { enum_num } else { None },
            byte_range: elem_start..i,
        });
    }
    Ok(elems)
}

// ── Decode packed payload to per-element list ─────────────────────────────────

pub(super) fn decode_packed_elems(data: &[u8], foe: &FieldOrExt) -> Result<Vec<PackedElem>, ()> {
    match foe.kind() {
        Kind::Double
        | Kind::Float
        | Kind::Fixed64
        | Kind::Sfixed64
        | Kind::Fixed32
        | Kind::Sfixed32 => {
            let elem_size = match foe.kind() {
                Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => 8,
                _ => 4,
            };
            decode_packed_fixed_elems(data, elem_size, &foe.kind())
        }
        _ => decode_packed_varint_elems(data, foe),
    }
}

// ── Packed field renderer ─────────────────────────────────────────────────────

/// Record-level annotation data for a packed wire record.
struct PackedRecordAnn {
    /// True for the first element in the record; triggers record-level modifiers.
    is_first: bool,
    /// Total number of elements in this wire record.
    pack_size: usize,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    len_ohb: Option<u64>,
}

/// Write the annotation for a packed element line.
fn write_packed_elem_ann(
    field_number: u64,
    foe: &FieldOrExt,
    rec: &PackedRecordAnn,
    elem: &PackedElem,
    out: &mut Vec<u8>,
) {
    let mut aw = AnnWriter::new();
    // Field declaration first (with enum per-element numeric value).
    let enum_raw = elem.enum_num;
    aw.push_field_decl(out, field_number, Some(foe), enum_raw, None);
    // Record-level anomaly modifiers on first element only.
    if rec.is_first {
        aw.push_u64_mod(out, b"pack_size: ", rec.pack_size as u64);
        if let Some(v) = rec.tag_ohb {
            aw.push_u64_mod(out, b"tag_ohb: ", v);
        }
        if rec.tag_oor {
            aw.push(out, b"TAG_OOR");
        }
        if let Some(v) = rec.len_ohb {
            aw.push_u64_mod(out, b"len_ohb: ", v);
        }
    }
    // Element-level anomaly modifiers.
    if elem.ohb > 0 {
        aw.push_u64_mod(out, b"ohb: ", elem.ohb);
    }
    if elem.neg_trunc {
        aw.push(out, b"neg");
    }
    if let Some(bits) = elem.nan_bits {
        aw.sep(out);
        out.extend_from_slice(b"nan_bits: 0x");
        write_nan_hex(bits, out);
    }
    // ENUM_UNKNOWN: when the enum value has no symbolic name.
    if let (Some(n), Kind::Enum(ref enum_desc)) = (elem.enum_num, foe.kind()) {
        if enum_desc.get_value(n).is_none() {
            aw.push(out, b"ENUM_UNKNOWN");
        }
    }
}

/// Render a packed repeated field.
pub(super) fn render_packed(
    field_number: u64,
    foe: &FieldOrExt,
    tag_ohb: Option<u64>,
    tag_oor: bool,
    len_ohb: Option<u64>,
    data: &[u8],
    sink: &mut TextSink,
) {
    use super::helpers::render_invalid;

    let annotations = ANNOTATIONS.with(|c| c.get());

    // Decode the packed payload into per-element data.
    let elems = match decode_packed_elems(data, foe) {
        Ok(e) => e,
        Err(()) => {
            render_invalid(
                field_number,
                Some(foe),
                tag_ohb,
                tag_oor,
                "INVALID_PACKED_RECORDS",
                data,
                sink,
            );
            return;
        }
    };

    let pack_size = elems.len();

    // ── Empty packed record ────────────────────────────────────────────────────
    // No elements: emit a comment-only annotation line.
    if pack_size == 0 {
        if annotations {
            let out = &mut sink.out;
            push_indent(out);
            let mut aw = AnnWriter::new_no_leading_spaces();
            aw.push_field_decl(out, field_number, Some(foe), None, None);
            aw.push_u64_mod(out, b"pack_size: ", 0);
            if let Some(v) = tag_ohb {
                aw.push_u64_mod(out, b"tag_ohb: ", v);
            }
            if tag_oor {
                aw.push(out, b"TAG_OOR");
            }
            if let Some(v) = len_ohb {
                aw.push_u64_mod(out, b"len_ohb: ", v);
            }
        }
        sink.newline();
        CBL_START.with(|c| c.set(sink.out.len()));
        return;
    }

    // ── Per-element lines ──────────────────────────────────────────────────────
    for (i, elem) in elems.iter().enumerate() {
        let is_first = i == 0;
        let out = &mut sink.out;
        wfl_prefix_n(field_number, Some(foe), false, out);
        out.extend_from_slice(elem.value_str.as_bytes());
        if annotations {
            write_packed_elem_ann(
                field_number,
                foe,
                &PackedRecordAnn {
                    is_first,
                    pack_size,
                    tag_ohb,
                    tag_oor,
                    len_ohb,
                },
                elem,
                out,
            );
        }
        sink.newline();
        CBL_START.with(|c| c.set(sink.out.len()));
    }
}
