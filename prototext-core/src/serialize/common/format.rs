// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use crate::decoder::{ProtoTextContent, ProtoTextField};

use super::escape::{escape_bytes, escape_string};
use super::floats::{format_double_protoc, format_float_protoc};
use super::scalars::{
    format_bool_protoc, format_enum_protoc, format_fixed32_protoc, format_fixed64_protoc,
    format_int32_protoc, format_int64_protoc, format_sfixed32_protoc, format_sfixed64_protoc,
    format_sint32_protoc, format_sint64_protoc, format_uint32_protoc, format_uint64_protoc,
    format_wire_fixed32_protoc, format_wire_fixed64_protoc, format_wire_varint_protoc,
};

// ── format_protoc_value ───────────────────────────────────────────────────────
//
// Mirrors `format_protoc_value()` in common.py.
// Returns `None` for fields that should be skipped (INVALID_*, packed repeats,
// untyped BYTES/message when handling is done as nested).
// `include_wire_types`: true for protoc, false for protoc_meticulous.

pub fn format_protoc_value(field: &ProtoTextField, include_wire_types: bool) -> Option<String> {
    match &field.content {
        // Always skip invalid fields
        ProtoTextContent::InvalidTagType(_)
        | ProtoTextContent::InvalidVarint(_)
        | ProtoTextContent::InvalidFixed64(_)
        | ProtoTextContent::InvalidFixed32(_)
        | ProtoTextContent::InvalidBytesLength(_)
        | ProtoTextContent::TruncatedBytes(_)
        | ProtoTextContent::InvalidPackedRecords(_)
        | ProtoTextContent::InvalidString(_)
        | ProtoTextContent::InvalidGroupEnd(_) => None,

        // Generic wire types: only in protoc (include_wire_types=true)
        ProtoTextContent::WireVarint(v) => {
            if include_wire_types {
                Some(format_wire_varint_protoc(*v))
            } else {
                None
            }
        }
        ProtoTextContent::WireFixed64(v) => {
            if include_wire_types {
                Some(format_wire_fixed64_protoc(*v))
            } else {
                None
            }
        }
        ProtoTextContent::WireFixed32(v) => {
            if include_wire_types {
                Some(format_wire_fixed32_protoc(*v))
            } else {
                None
            }
        }
        ProtoTextContent::WireBytes(b) => {
            if include_wire_types {
                Some(format!("\"{}\"", escape_bytes(b)))
            } else {
                None
            }
        }

        // Typed VARINT
        ProtoTextContent::Int64(v) => Some(format_int64_protoc(*v)),
        ProtoTextContent::Uint64(v) => Some(format_uint64_protoc(*v)),
        ProtoTextContent::Int32(v) => Some(format_int32_protoc(*v)),
        ProtoTextContent::Uint32(v) => Some(format_uint32_protoc(*v)),
        ProtoTextContent::Bool(v) => Some(format_bool_protoc(*v).to_owned()),
        ProtoTextContent::Enum(v) => Some(format_enum_protoc(*v)),
        ProtoTextContent::Sint32(v) => Some(format_sint32_protoc(*v)),
        ProtoTextContent::Sint64(v) => Some(format_sint64_protoc(*v)),

        // Typed FIXED64
        ProtoTextContent::Double(v) => Some(format_double_protoc(*v)),
        ProtoTextContent::PFixed64(v) => Some(format_fixed64_protoc(*v)),
        ProtoTextContent::Sfixed64(v) => Some(format_sfixed64_protoc(*v)),

        // Typed FIXED32
        ProtoTextContent::Float(v) => Some(format_float_protoc(*v)),
        ProtoTextContent::PFixed32(v) => Some(format_fixed32_protoc(*v)),
        ProtoTextContent::Sfixed32(v) => Some(format_sfixed32_protoc(*v)),

        // Length-delimited scalars
        ProtoTextContent::StringVal(s) => Some(format!("\"{}\"", escape_string(s))),
        ProtoTextContent::BytesVal(b) => Some(format!("\"{}\"", escape_bytes(b))),

        // Nested (handled as child block, not here)
        ProtoTextContent::MessageVal(_)
        | ProtoTextContent::Group(_)
        | ProtoTextContent::WireGroup(_) => None,

        // Packed repeated: render as "[val1, val2, ...]" (matching Python's protoc output)
        ProtoTextContent::Doubles(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_double_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Floats(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_float_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Int64s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_int64_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Uint64s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_uint64_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Int32s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_int32_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Fixed64s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_fixed64_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Fixed32s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_fixed32_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Bools(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_bool_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Uint32s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_uint32_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Enums(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_enum_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Sfixed32s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_sfixed32_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Sfixed64s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_sfixed64_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Sint32s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_sint32_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ProtoTextContent::Sint64s(vs) => Some(format!(
            "[{}]",
            vs.iter()
                .map(|v| format_sint64_protoc(*v))
                .collect::<Vec<_>>()
                .join(", ")
        )),

        ProtoTextContent::Unset => None,
    }
}

// ── is_nested / is_invalid helpers ───────────────────────────────────────────

#[inline]
pub fn is_nested(content: &ProtoTextContent) -> bool {
    matches!(
        content,
        ProtoTextContent::MessageVal(_)
            | ProtoTextContent::Group(_)
            | ProtoTextContent::WireGroup(_)
    )
}

#[inline]
pub fn is_invalid(content: &ProtoTextContent) -> bool {
    matches!(
        content,
        ProtoTextContent::InvalidTagType(_)
            | ProtoTextContent::InvalidVarint(_)
            | ProtoTextContent::InvalidFixed64(_)
            | ProtoTextContent::InvalidFixed32(_)
            | ProtoTextContent::InvalidBytesLength(_)
            | ProtoTextContent::TruncatedBytes(_)
            | ProtoTextContent::InvalidPackedRecords(_)
            | ProtoTextContent::InvalidString(_)
            | ProtoTextContent::InvalidGroupEnd(_)
    )
}

// ── Modifier lines ────────────────────────────────────────────────────────────
//
// Mirrors `get_modifier_strings(include_type=True)` in common.py.

pub struct Modifier {
    pub text: String,
}

pub fn get_modifiers(field: &ProtoTextField) -> Vec<Modifier> {
    let mut out = Vec::new();

    if let Some(v) = field.tag_overhang_count {
        out.push(Modifier {
            text: format!("tag_overhang_count: {}", v),
        });
    }
    if field.tag_is_out_of_range {
        out.push(Modifier {
            text: "tag_is_out_of_range: true".to_owned(),
        });
    }
    if let Some(v) = field.value_overhang_count {
        out.push(Modifier {
            text: format!("value_overhang_count: {}", v),
        });
    }
    if let Some(v) = field.length_overhang_count {
        out.push(Modifier {
            text: format!("length_overhang_count: {}", v),
        });
    }
    if let Some(v) = field.missing_bytes_count {
        out.push(Modifier {
            text: format!("missing_bytes_count: {}", v),
        });
    }
    if let Some(v) = field.mismatched_group_end {
        out.push(Modifier {
            text: format!("mismatched_group_end: {}", v),
        });
    }
    if field.open_ended_group {
        out.push(Modifier {
            text: "open_ended_group: true".to_owned(),
        });
    }
    if let Some(v) = field.end_tag_overhang_count {
        out.push(Modifier {
            text: format!("end_tag_overhang_count: {}", v),
        });
    }
    if field.end_tag_is_out_of_range {
        out.push(Modifier {
            text: "end_tag_is_out_of_range: true".to_owned(),
        });
    }
    if field.proto2_has_type_mismatch {
        out.push(Modifier {
            text: "proto2_has_type_mismatch: true".to_owned(),
        });
    }
    if !field.records_overhung_count.is_empty() {
        let vals: Vec<String> = field
            .records_overhung_count
            .iter()
            .map(|v| v.to_string())
            .collect();
        out.push(Modifier {
            text: format!("records_overhung_count: [{}]", vals.join(", ")),
        });
    }
    out
}
