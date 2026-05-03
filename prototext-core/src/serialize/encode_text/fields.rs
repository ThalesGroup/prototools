// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use crate::helpers::{write_varint_ohb, WT_END_GROUP, WT_I32, WT_I64, WT_LEN, WT_VARINT};

use super::encode_annotation::Ann;

// ── Low-level tag helper ──────────────────────────────────────────────────────

#[inline]
pub(super) fn write_tag_ohb_local(
    field_number: u64,
    wire_type: u32,
    ohb: Option<u64>,
    out: &mut Vec<u8>,
) {
    let tag_val = (field_number << 3) | (wire_type as u64);
    write_varint_ohb(tag_val, ohb, out);
}

// ── Number value ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub(super) enum Num {
    Int(i64),
    Float(f64),
    /// Raw NaN bit pattern from a `nan_bits: 0x…` annotation — bypasses float
    /// conversion to preserve the exact bit pattern for float and double fields.
    NanBits(u64),
}

impl Num {
    #[inline]
    pub(super) fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
            Num::NanBits(b) => f64::from_bits(b),
        }
    }
    #[inline]
    pub(super) fn as_i64(self) -> i64 {
        match self {
            Num::Int(i) => i,
            Num::Float(f) => f as i64,
            Num::NanBits(_) => 0,
        }
    }
    #[inline]
    pub(super) fn as_u64(self) -> u64 {
        self.as_i64() as u64
    }
}

pub(super) fn parse_num(s: &str) -> Option<Num> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        return u64::from_str_radix(&s[2..], 16)
            .ok()
            .map(|v| Num::Int(v as i64));
    }
    if s == "nan" {
        return Some(Num::Float(f64::NAN));
    }
    if s == "inf" {
        return Some(Num::Float(f64::INFINITY));
    }
    if s == "-inf" {
        return Some(Num::Float(f64::NEG_INFINITY));
    }
    if s == "-0" {
        return Some(Num::Float(-0.0_f64));
    }
    if s.contains('.') || s.to_ascii_lowercase().contains('e') {
        return s.parse::<f64>().ok().map(Num::Float);
    }
    if let Some(neg) = s.strip_prefix('-') {
        return neg
            .parse::<u64>()
            .ok()
            .map(|v| Num::Int((v as i64).wrapping_neg()))
            .or_else(|| s.parse::<i64>().ok().map(Num::Int));
    }
    s.parse::<u64>().ok().map(|v| Num::Int(v as i64))
}

// ── String / bytes unescaping ─────────────────────────────────────────────────

/// Unescape a protoc-quoted byte literal (surrounding `"` included).
pub(super) fn unescape_bytes(quoted: &str) -> Vec<u8> {
    let s = quoted
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(quoted);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'"' => {
                    out.push(b'"');
                    i += 2;
                }
                b'\'' => {
                    out.push(b'\'');
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                c if (b'0'..=b'7').contains(&c) => {
                    let j_end = (i + 4).min(bytes.len());
                    let mut j = i + 1;
                    while j < j_end && (b'0'..=b'7').contains(&bytes[j]) {
                        j += 1;
                    }
                    let oct = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("0");
                    out.push(u8::from_str_radix(oct, 8).unwrap_or(0));
                    i = j;
                }
                b'x' if i + 3 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 2..i + 4]).unwrap_or("00");
                    out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
                    i += 4;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

// ── Scalar encoder ────────────────────────────────────────────────────────────

/// Encode a scalar field line directly into `out`.
///
/// `value_str` is the trimmed text value (right-hand side of `:`).
/// `ann` carries the annotation metadata.
pub(super) fn encode_scalar_line(
    field_number: u64,
    value_str: &str,
    ann: &Ann<'_>,
    out: &mut Vec<u8>,
) {
    let tag_ohb = ann.tag_overhang_count;
    let val_ohb = ann.value_overhang_count;
    let len_ohb = ann.length_overhang_count;
    let wire = ann.wire_type;

    // ── Explicit error wire types ──────────────────────────────────────────────

    match wire {
        "INVALID_TAG_TYPE" => {
            // No tag; raw bytes as the entire field.
            let data = unescape_bytes(value_str);
            out.extend_from_slice(&data);
            return;
        }
        "INVALID_VARINT" => {
            write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
            out.extend_from_slice(&unescape_bytes(value_str));
            return;
        }
        "INVALID_FIXED64" => {
            write_tag_ohb_local(field_number, WT_I64, tag_ohb, out);
            out.extend_from_slice(&unescape_bytes(value_str));
            return;
        }
        "INVALID_FIXED32" => {
            write_tag_ohb_local(field_number, WT_I32, tag_ohb, out);
            out.extend_from_slice(&unescape_bytes(value_str));
            return;
        }
        "INVALID_LEN" => {
            write_tag_ohb_local(field_number, WT_LEN, tag_ohb, out);
            out.extend_from_slice(&unescape_bytes(value_str));
            return;
        }
        "TRUNCATED_BYTES" => {
            let data = unescape_bytes(value_str);
            let missing = ann.missing_bytes_count.unwrap_or(0) as usize;
            let declared = data.len() + missing;
            write_tag_ohb_local(field_number, WT_LEN, tag_ohb, out);
            write_varint_ohb(declared as u64, len_ohb, out);
            out.extend_from_slice(&data);
            return;
        }
        "INVALID_PACKED_RECORDS" | "INVALID_STRING" => {
            let data = unescape_bytes(value_str);
            write_tag_ohb_local(field_number, WT_LEN, tag_ohb, out);
            write_varint_ohb(data.len() as u64, len_ohb, out);
            out.extend_from_slice(&data);
            return;
        }
        "INVALID_GROUP_END" => {
            write_tag_ohb_local(field_number, WT_END_GROUP, tag_ohb, out);
            out.extend_from_slice(&unescape_bytes(value_str));
            return;
        }
        _ => {}
    }

    // ── Packed array: "[v1, v2, …]" ───────────────────────────────────────────

    if value_str.starts_with('[') {
        encode_packed_array_line(field_number, value_str, ann, out);
        return;
    }

    // ── Known wire type overrides (v2 lowercase names) ────────────────────────

    match wire {
        "varint" => {
            if let Some(num) = parse_num(value_str) {
                write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
                write_varint_ohb(num.as_u64(), val_ohb, out);
            } else {
                // bool literal
                let v: u64 = if value_str == "true" { 1 } else { 0 };
                write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
                write_varint_ohb(v, val_ohb, out);
            }
            return;
        }
        "fixed64" => {
            if let Some(num) = parse_num(value_str) {
                let v: u64 = if let Num::Float(f) = num {
                    f.to_bits()
                } else {
                    num.as_u64()
                };
                write_tag_ohb_local(field_number, WT_I64, tag_ohb, out);
                out.extend_from_slice(&v.to_le_bytes());
            }
            return;
        }
        "fixed32" => {
            if let Some(num) = parse_num(value_str) {
                let v: u32 = if let Num::Float(f) = num {
                    (f as f32).to_bits()
                } else {
                    num.as_u64() as u32
                };
                write_tag_ohb_local(field_number, WT_I32, tag_ohb, out);
                out.extend_from_slice(&v.to_le_bytes());
            }
            return;
        }
        "bytes" => {
            // Explicit wire type `bytes` (unknown field or wire-type mismatch).
            let data = unescape_bytes(value_str);
            write_tag_ohb_local(field_number, WT_LEN, tag_ohb, out);
            write_varint_ohb(data.len() as u64, len_ohb, out);
            out.extend_from_slice(&data);
            return;
        }
        _ => {}
    }

    // ── Infer encoding from field_type (no explicit wire type) ────────────────

    let ft = ann.field_type;

    // bool literal
    if value_str == "true" || value_str == "false" {
        let v: u64 = if value_str == "true" { 1 } else { 0 };
        write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
        write_varint_ohb(v, val_ohb, out);
        return;
    }

    // Quoted string / bytes
    if value_str.starts_with('"') {
        let is_bytes = matches!(ft, "bytes" | "");
        let data = if is_bytes {
            unescape_bytes(value_str)
        } else {
            // string field: unescape then re-encode as UTF-8
            unescape_bytes(value_str)
        };
        write_tag_ohb_local(field_number, WT_LEN, tag_ohb, out);
        write_varint_ohb(data.len() as u64, len_ohb, out);
        out.extend_from_slice(&data);
        return;
    }

    // For enum scalar fields: use the numeric value extracted from the annotation
    // (e.g. `Type(9)`) instead of parsing the LHS symbolic name (e.g. `TYPE_STRING`).
    // This avoids name-resolution and eliminates the primitive-keyword collision hazard.
    if let Some(enum_val) = ann.enum_scalar_value {
        encode_num(
            field_number,
            Num::Int(enum_val),
            "enum",
            tag_ohb,
            val_ohb,
            ann.neg_int32_truncated,
            out,
        );
        return;
    }

    // Numeric value — dispatch by field_type.
    // For float/double `nan` values, check for a `nan_bits` annotation modifier.
    let num_opt = if value_str == "nan" {
        if let Some(bits) = ann.nan_bits {
            Some(Num::NanBits(bits))
        } else {
            parse_num(value_str)
        }
    } else {
        parse_num(value_str)
    };
    let Some(num) = num_opt else {
        return;
    };
    encode_num(
        field_number,
        num,
        ft,
        tag_ohb,
        val_ohb,
        ann.neg_int32_truncated,
        out,
    );
}

/// Encode a numeric value according to `field_type`.
///
/// `truncated`: when `true` and `field_type` is `"int32"` or `"enum"`, produce the
/// 5-byte truncated varint form (`value as i32 as u32 as u64`).  When `false`,
/// produce the spec-correct 10-byte sign-extended form (`value as i64 as u64`).
pub(super) fn encode_num(
    field_number: u64,
    num: Num,
    field_type: &str,
    tag_ohb: Option<u64>,
    val_ohb: Option<u64>,
    truncated: bool,
    out: &mut Vec<u8>,
) {
    match field_type {
        "double" => {
            write_tag_ohb_local(field_number, WT_I64, tag_ohb, out);
            let bits = if let Num::NanBits(b) = num {
                b | 0x7FF0000000000000 // force exponent to all-ones
            } else {
                num.as_f64().to_bits()
            };
            out.extend_from_slice(&bits.to_le_bytes());
        }
        "fixed64" | "sfixed64" => {
            write_tag_ohb_local(field_number, WT_I64, tag_ohb, out);
            out.extend_from_slice(&num.as_u64().to_le_bytes());
        }
        "float" => {
            write_tag_ohb_local(field_number, WT_I32, tag_ohb, out);
            let bits = if let Num::NanBits(b) = num {
                (b as u32) | 0x7F800000 // force exponent to all-ones
            } else {
                (num.as_f64() as f32).to_bits()
            };
            out.extend_from_slice(&bits.to_le_bytes());
        }
        "fixed32" | "sfixed32" => {
            write_tag_ohb_local(field_number, WT_I32, tag_ohb, out);
            out.extend_from_slice(&(num.as_u64() as u32).to_le_bytes());
        }
        "sint32" => {
            let v = num.as_i64() as i32;
            let enc = ((v << 1) ^ (v >> 31)) as u32 as u64;
            write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
            write_varint_ohb(enc, val_ohb, out);
        }
        "sint64" => {
            let v = num.as_i64();
            let enc = ((v << 1) ^ (v >> 63)) as u64;
            write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
            write_varint_ohb(enc, val_ohb, out);
        }
        "bool" => {
            write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
            write_varint_ohb(num.as_u64() & 1, val_ohb, out);
        }
        // int32 / enum: two valid wire forms for negative values.
        //   truncated=true  → 5-byte form  (value as i32 as u32 as u64)
        //   truncated=false → 10-byte spec-correct form (value as i64 as u64)
        "int32" | "enum" => {
            let v = if truncated {
                num.as_i64() as i32 as u32 as u64 // 5-byte truncated
            } else {
                num.as_i64() as u64 // 10-byte spec-correct
            };
            write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
            write_varint_ohb(v, val_ohb, out);
        }
        // uint32, int64, uint64, and fallback (unknown named types)
        _ => {
            write_tag_ohb_local(field_number, WT_VARINT, tag_ohb, out);
            write_varint_ohb(num.as_u64(), val_ohb, out);
        }
    }
}

/// Encode a packed-array value line `[v1, v2, …]` into `out`.
pub(super) fn encode_packed_array_line(
    field_number: u64,
    value_str: &str,
    ann: &Ann<'_>,
    out: &mut Vec<u8>,
) {
    let ft = ann.field_type;
    let ohbs = &ann.records_overhung_count;

    // For packed ENUM fields: ignore the LHS symbolic-name list entirely.
    // Use the raw numeric values extracted from `EnumType([n1, n2])` in the annotation.
    if ft == "enum" && !ann.enum_packed_values.is_empty() {
        let mut payload = Vec::new();
        for (i, &raw_val) in ann.enum_packed_values.iter().enumerate() {
            let ohb = ohbs.get(i).copied().filter(|&o| o > 0);
            let trunc = ann
                .records_neg_int32_truncated
                .get(i)
                .copied()
                .unwrap_or(false);
            let v = if trunc {
                raw_val as i32 as u32 as u64 // 5-byte truncated
            } else {
                raw_val as u64 // 10-byte spec-correct for negatives
            };
            write_varint_ohb(v, ohb, &mut payload);
        }
        write_tag_ohb_local(field_number, WT_LEN, ann.tag_overhang_count, out);
        write_varint_ohb(payload.len() as u64, ann.length_overhang_count, out);
        out.extend_from_slice(&payload);
        return;
    }

    let inner = value_str.trim_start_matches('[').trim_end_matches(']');

    let mut payload = Vec::new();
    for (i, elem) in inner.split(',').enumerate() {
        let elem = elem.trim();
        if elem.is_empty() {
            continue;
        }
        let ohb = ohbs.get(i).copied().filter(|&o| o > 0);
        match ft {
            "double" => {
                if let Some(n) = parse_num(elem) {
                    let bits = if let Num::NanBits(b) = n {
                        b | 0x7FF0000000000000 // force exponent to all-ones
                    } else {
                        n.as_f64().to_bits()
                    };
                    payload.extend_from_slice(&bits.to_le_bytes());
                }
            }
            "float" => {
                if let Some(n) = parse_num(elem) {
                    let bits = if let Num::NanBits(b) = n {
                        (b as u32) | 0x7F800000 // force exponent to all-ones
                    } else {
                        (n.as_f64() as f32).to_bits()
                    };
                    payload.extend_from_slice(&bits.to_le_bytes());
                }
            }
            "fixed64" | "sfixed64" => {
                if let Some(n) = parse_num(elem) {
                    payload.extend_from_slice(&n.as_u64().to_le_bytes());
                }
            }
            "fixed32" | "sfixed32" => {
                if let Some(n) = parse_num(elem) {
                    payload.extend_from_slice(&(n.as_u64() as u32).to_le_bytes());
                }
            }
            "sint32" => {
                if let Some(n) = parse_num(elem) {
                    let v = n.as_i64() as i32;
                    let enc = ((v << 1) ^ (v >> 31)) as u32 as u64;
                    write_varint_ohb(enc, ohb, &mut payload);
                }
            }
            "sint64" => {
                if let Some(n) = parse_num(elem) {
                    let v = n.as_i64();
                    let enc = ((v << 1) ^ (v >> 63)) as u64;
                    write_varint_ohb(enc, ohb, &mut payload);
                }
            }
            "bool" => {
                let v: u64 = if elem == "true" { 1 } else { 0 };
                write_varint_ohb(v, ohb, &mut payload);
            }
            // int32 / enum: honour per-element truncation flag.
            //   truncated=true  → 5-byte form  (n as i32 as u32 as u64)
            //   truncated=false → 10-byte spec-correct form (n as i64 as u64)
            "int32" | "enum" => {
                if let Some(n) = parse_num(elem) {
                    let trunc = ann
                        .records_neg_int32_truncated
                        .get(i)
                        .copied()
                        .unwrap_or(false);
                    let v = if trunc {
                        n.as_i64() as i32 as u32 as u64 // 5-byte truncated
                    } else {
                        n.as_i64() as u64 // 10-byte spec-correct
                    };
                    write_varint_ohb(v, ohb, &mut payload);
                }
            }
            _ => {
                if let Some(n) = parse_num(elem) {
                    write_varint_ohb(n.as_u64(), ohb, &mut payload);
                }
            }
        }
    }

    write_tag_ohb_local(field_number, WT_LEN, ann.tag_overhang_count, out);
    write_varint_ohb(payload.len() as u64, ann.length_overhang_count, out);
    out.extend_from_slice(&payload);
}

/// Encode one per-line packed element into `payload`.
///
/// Used by the per-line packed state machine in `encode_text_to_binary`.
pub(super) fn encode_packed_elem(value_str: &str, ann: &Ann<'_>, payload: &mut Vec<u8>) {
    let ft = ann.field_type;
    let ohb = ann.elem_ohb.filter(|&o| o > 0);

    match ft {
        "double" => {
            if let Some(n) = parse_num(value_str) {
                let bits = if let Num::NanBits(b) = n {
                    b | 0x7FF0000000000000
                } else if let Some(raw) = ann.nan_bits {
                    raw | 0x7FF0000000000000
                } else {
                    n.as_f64().to_bits()
                };
                payload.extend_from_slice(&bits.to_le_bytes());
            }
        }
        "float" => {
            if let Some(n) = parse_num(value_str) {
                let bits = if let Num::NanBits(b) = n {
                    (b as u32) | 0x7F800000
                } else if let Some(raw) = ann.nan_bits {
                    (raw as u32) | 0x7F800000
                } else {
                    (n.as_f64() as f32).to_bits()
                };
                payload.extend_from_slice(&bits.to_le_bytes());
            }
        }
        "fixed64" | "sfixed64" => {
            if let Some(n) = parse_num(value_str) {
                payload.extend_from_slice(&n.as_u64().to_le_bytes());
            }
        }
        "fixed32" | "sfixed32" => {
            if let Some(n) = parse_num(value_str) {
                payload.extend_from_slice(&(n.as_u64() as u32).to_le_bytes());
            }
        }
        "sint32" => {
            if let Some(n) = parse_num(value_str) {
                let v = n.as_i64() as i32;
                let enc = ((v << 1) ^ (v >> 31)) as u32 as u64;
                write_varint_ohb(enc, ohb, payload);
            }
        }
        "sint64" => {
            if let Some(n) = parse_num(value_str) {
                let v = n.as_i64();
                let enc = ((v << 1) ^ (v >> 63)) as u64;
                write_varint_ohb(enc, ohb, payload);
            }
        }
        "bool" => {
            let v: u64 = if value_str == "true" { 1 } else { 0 };
            write_varint_ohb(v, ohb, payload);
        }
        "int32" | "enum" => {
            // For ENUM: use enum_scalar_value from the per-element annotation if present.
            let raw_val = ann.enum_scalar_value;
            let trunc = ann.elem_neg_trunc;
            if let Some(rv) = raw_val {
                let v = if trunc {
                    rv as i32 as u32 as u64
                } else {
                    rv as u64
                };
                write_varint_ohb(v, ohb, payload);
            } else if let Some(n) = parse_num(value_str) {
                let v = if trunc {
                    n.as_i64() as i32 as u32 as u64
                } else {
                    n.as_i64() as u64
                };
                write_varint_ohb(v, ohb, payload);
            }
        }
        _ => {
            if let Some(n) = parse_num(value_str) {
                write_varint_ohb(n.as_u64(), ohb, payload);
            }
        }
    }
}
