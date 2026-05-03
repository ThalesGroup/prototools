// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use prost_reflect::{Cardinality, FieldDescriptor, Kind};

use crate::helpers::{
    decode_bool, decode_int32, decode_int64, decode_sint32, decode_sint64, decode_uint32,
    decode_uint64,
};

use super::types::ProtoTextContent;

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(super) struct TypeMismatch;

/// Map a varint value to the appropriate `ProtoTextContent` variant given
/// the field's proto Kind.  Returns `Err(TypeMismatch)` when the value is out
/// of range for the declared type, mirroring the Python `WireTypeMismatch`.
pub(super) fn decode_varint_by_kind(
    val: u64,
    kind: Kind,
) -> Result<ProtoTextContent, TypeMismatch> {
    match kind {
        Kind::Int64 => {
            // val is u64 so it is always < 2^64; parse_varint already returns
            // varint_gar for values that would overflow u64.  No range check needed.
            Ok(ProtoTextContent::Int64(decode_int64(val)))
        }
        Kind::Uint64 => Ok(ProtoTextContent::Uint64(decode_uint64(val))),
        Kind::Int32 => {
            if val >= (1u64 << 32) {
                return Err(TypeMismatch);
            }
            Ok(ProtoTextContent::Int32(decode_int32(val)))
        }
        Kind::Bool => {
            if val > 1 {
                return Err(TypeMismatch);
            }
            Ok(ProtoTextContent::Bool(decode_bool(val)))
        }
        Kind::Uint32 => {
            if val >= (1u64 << 32) {
                return Err(TypeMismatch);
            }
            Ok(ProtoTextContent::Uint32(decode_uint32(val)))
        }
        Kind::Enum(_) => {
            if val >= (1u64 << 32) {
                return Err(TypeMismatch);
            }
            Ok(ProtoTextContent::Enum(decode_int32(val)))
        }
        Kind::Sint32 => {
            if val >= (1u64 << 32) {
                return Err(TypeMismatch);
            }
            Ok(ProtoTextContent::Sint32(decode_sint32(val)))
        }
        Kind::Sint64 => Ok(ProtoTextContent::Sint64(decode_sint64(val))),
        _ => Err(TypeMismatch),
    }
}

/// Format the annotation string for a field, mirroring the Python code.
pub(super) fn format_annotation(fs: &FieldDescriptor) -> String {
    let label = match fs.cardinality() {
        Cardinality::Optional => "optional",
        Cardinality::Required => "required",
        Cardinality::Repeated => "repeated",
    };
    let (_type_str, type_display) = kind_to_annotation_strs(fs);
    let packed_suffix = if fs.is_packed() { " [packed=true]" } else { "" };
    format!("{}: {} {}{}", fs.name(), label, type_display, packed_suffix)
}

/// Return `(type_str, type_display)` for a field's kind.
///
/// For message/group/enum fields, `type_display` is the short type name.
/// For all other fields, `type_display == type_str`.
fn kind_to_annotation_strs(fs: &FieldDescriptor) -> (&'static str, String) {
    match fs.kind() {
        Kind::Double => ("double", "double".to_string()),
        Kind::Float => ("float", "float".to_string()),
        Kind::Int64 => ("int64", "int64".to_string()),
        Kind::Uint64 => ("uint64", "uint64".to_string()),
        Kind::Int32 => ("int32", "int32".to_string()),
        Kind::Fixed64 => ("fixed64", "fixed64".to_string()),
        Kind::Fixed32 => ("fixed32", "fixed32".to_string()),
        Kind::Bool => ("bool", "bool".to_string()),
        Kind::String => ("string", "string".to_string()),
        Kind::Bytes => ("bytes", "bytes".to_string()),
        Kind::Uint32 => ("uint32", "uint32".to_string()),
        Kind::Sfixed32 => ("sfixed32", "sfixed32".to_string()),
        Kind::Sfixed64 => ("sfixed64", "sfixed64".to_string()),
        Kind::Sint32 => ("sint32", "sint32".to_string()),
        Kind::Sint64 => ("sint64", "sint64".to_string()),
        Kind::Message(msg_desc) => ("message", msg_desc.name().to_string()),
        Kind::Enum(enum_desc) => ("enum", enum_desc.name().to_string()),
    }
}
