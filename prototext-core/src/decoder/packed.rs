// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use prost_reflect::{Cardinality, FieldDescriptor, Kind};

use crate::helpers::{
    decode_double, decode_fixed32, decode_fixed64, decode_float, decode_int32, decode_int64,
    decode_sfixed32, decode_sfixed64, decode_sint32, decode_sint64, decode_uint32, parse_varint,
};
use crate::schema::ParsedSchema;

use super::types::{ProtoTextContent, ProtoTextField};

// ── Packed helpers ────────────────────────────────────────────────────────────

/// Decode a slice of fixed-width packed elements.
///
/// Returns `Err(())` when `data.len()` is not a multiple of `elem_size`.
fn decode_fixed_vec<T>(
    data: &[u8],
    elem_size: usize,
    f: impl Fn(&[u8]) -> T,
) -> Result<Vec<T>, ()> {
    if !data.len().is_multiple_of(elem_size) {
        return Err(());
    }
    Ok(data.chunks_exact(elem_size).map(f).collect())
}

/// Decode a packed repeated field, mirroring the Python packed-records block.
pub(super) fn decode_packed(data: &[u8], fs: &FieldDescriptor, field: &mut ProtoTextField) {
    // When length == 0, set content to an empty vector (matching Python behavior:
    // field.sfixed64s.extend([]) still marks the field as "set", producing "sfixed64Pk: []")
    macro_rules! fixed {
        ($size:expr, $decode:expr, $variant:ident) => {
            match decode_fixed_vec(data, $size, $decode) {
                Ok(vals) => field.content = ProtoTextContent::$variant(vals),
                Err(()) => {
                    field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                }
            }
        };
    }
    match fs.kind() {
        Kind::Double => fixed!(8, decode_double, Doubles),
        Kind::Float => fixed!(4, decode_float, Floats),
        Kind::Fixed64 => fixed!(8, decode_fixed64, Fixed64s),
        Kind::Sfixed64 => fixed!(8, decode_sfixed64, Sfixed64s),
        Kind::Fixed32 => fixed!(4, decode_fixed32, Fixed32s),
        Kind::Sfixed32 => fixed!(4, decode_sfixed32, Sfixed32s),
        // Varint-packed types
        _ => decode_packed_varints(data, fs, field),
    }
}

/// Decode varint-packed repeated fields (int32, int64, uint32, uint64, bool,
/// enum, sint32, sint64).  Tracks overhang bytes per record.
fn decode_packed_varints(data: &[u8], fs: &FieldDescriptor, field: &mut ProtoTextField) {
    let length = data.len();

    // When length == 0, fall through to set content to an empty vector
    // (matching Python: field.int64s.extend([]) marks the field as set → "int64Pk: []")

    let mut vals_i64: Vec<i64> = Vec::new();
    let mut vals_u64: Vec<u64> = Vec::new();
    let mut vals_i32: Vec<i32> = Vec::new();
    let mut vals_u32: Vec<u32> = Vec::new();
    let mut vals_bool: Vec<bool> = Vec::new();
    let mut vals_enum: Vec<i32> = Vec::new();
    let mut ohbs: Vec<u64> = Vec::new();
    let mut i = 0;

    while i < length {
        let vr = parse_varint(data, i);
        if vr.varint_gar.is_some() {
            field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
            return;
        }
        i = vr.next_pos;
        ohbs.push(vr.varint_ohb.unwrap_or(0));
        let v = vr.varint.unwrap();

        match fs.kind() {
            Kind::Int64 => {
                vals_i64.push(decode_int64(v));
            }
            Kind::Uint64 => {
                vals_u64.push(v);
            }
            Kind::Int32 => {
                if v >= (1u64 << 32) {
                    field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                    return;
                }
                vals_i32.push(decode_int32(v));
            }
            Kind::Bool => {
                if v > 1 {
                    field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                    return;
                }
                vals_bool.push(v != 0);
            }
            Kind::Uint32 => {
                if v >= (1u64 << 32) {
                    field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                    return;
                }
                vals_u32.push(decode_uint32(v));
            }
            Kind::Enum(_) => {
                if v >= (1u64 << 32) {
                    field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                    return;
                }
                vals_enum.push(decode_int32(v));
            }
            Kind::Sint32 => {
                if v >= (1u64 << 32) {
                    field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                    return;
                }
                vals_i32.push(decode_sint32(v));
            }
            Kind::Sint64 => {
                vals_i64.push(decode_sint64(v));
            }
            _ => {
                field.content = ProtoTextContent::InvalidPackedRecords(data.to_vec());
                return;
            }
        }
    }

    // Record overhang bytes
    if ohbs.iter().any(|&x| x > 0) {
        field.records_overhung_count = ohbs;
    }

    field.content = match fs.kind() {
        Kind::Int64 => ProtoTextContent::Int64s(vals_i64),
        Kind::Uint64 => ProtoTextContent::Uint64s(vals_u64),
        Kind::Int32 => ProtoTextContent::Int32s(vals_i32),
        Kind::Bool => ProtoTextContent::Bools(vals_bool),
        Kind::Uint32 => ProtoTextContent::Uint32s(vals_u32),
        Kind::Enum(_) => ProtoTextContent::Enums(vals_enum),
        Kind::Sint32 => ProtoTextContent::Sint32s(vals_i32),
        Kind::Sint64 => ProtoTextContent::Sint64s(vals_i64),
        _ => ProtoTextContent::InvalidPackedRecords(data.to_vec()),
    };
}

/// Decode a length-delimited field payload, updating `field.content`.
///
/// Mirrors the `case w.BYTES:` block in `parse_message()`.
pub(super) fn decode_len_field(
    data: &[u8],
    field_schema: Option<&FieldDescriptor>,
    full_schema: &ParsedSchema,
    annotations: bool,
    field: &mut ProtoTextField,
) {
    let Some(fs) = field_schema else {
        field.content = ProtoTextContent::WireBytes(data.to_vec());
        return;
    };

    let is_repeated = fs.cardinality() == Cardinality::Repeated;

    // ── Packed repeated ───────────────────────────────────────────────────────
    if is_repeated && fs.is_packed() {
        decode_packed(data, fs, field);
        return;
    }

    // ── String ────────────────────────────────────────────────────────────────
    if fs.kind() == Kind::String {
        match std::str::from_utf8(data) {
            Ok(s) => field.content = ProtoTextContent::StringVal(s.to_string()),
            Err(_) => field.content = ProtoTextContent::InvalidString(data.to_vec()),
        }
        return;
    }

    // ── Bytes ─────────────────────────────────────────────────────────────────
    if fs.kind() == Kind::Bytes {
        field.content = ProtoTextContent::BytesVal(data.to_vec());
        return;
    }

    // ── Nested message ────────────────────────────────────────────────────────
    if let Kind::Message(nested_msg_desc) = fs.kind() {
        let (nested_msg, _, _) = super::parse_message(
            data,
            0,
            None,
            Some(&nested_msg_desc),
            full_schema,
            annotations,
        );
        field.content = ProtoTextContent::MessageVal(Box::new(nested_msg));
        return;
    }

    // ── Wire-type mismatch fallback ───────────────────────────────────────────
    // The field is declared in the schema but its proto-type expects a different
    // wire type (e.g. int32 arriving as WT_LEN).  Mirror the varint path in
    // codec.rs: emit WireBytes for lossless roundtrip AND flag the conflict.
    field.proto2_has_type_mismatch = true;
    field.content = ProtoTextContent::WireBytes(data.to_vec());
}
