// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

mod codec;
mod packed;
mod types;

pub use types::*;

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

use crate::helpers::{
    decode_double, decode_fixed32, decode_fixed64, decode_float, decode_sfixed32, decode_sfixed64,
    parse_varint, parse_wiretag, WiretagResult, WT_END_GROUP, WT_I32, WT_I64, WT_LEN,
    WT_START_GROUP, WT_VARINT,
};
use crate::schema::ParsedSchema;

use codec::{decode_varint_by_kind, format_annotation, TypeMismatch};
use packed::decode_len_field;

// ── Public entry point ────────────────────────────────────────────────────────

/// Decode a binary protobuf payload into a lossless `ProtoTextMessage`.
///
/// Mirrors `decode_pb()` in `decode.py`.
pub fn ingest_pb(
    pb_bytes: &[u8],
    full_schema: &ParsedSchema,
    annotations: bool,
) -> ProtoTextMessage {
    let root = full_schema.root_descriptor();
    let (msg, _, _) = parse_message(pb_bytes, 0, None, root.as_ref(), full_schema, annotations);
    msg
}

// ── Core recursive parser ─────────────────────────────────────────────────────

/// Parse one protobuf message starting at `start`.
///
/// Returns `(message, next_pos, group_end_tag)`.
/// `group_end_tag` is `Some(tag)` when the parse terminated on an END_GROUP
/// wire type — the caller uses this to detect mismatched group numbers.
///
/// Mirrors `parse_message()` in `decode.py`.
pub fn parse_message(
    buf: &[u8],
    start: usize,
    my_group: Option<u64>, // Some(field_number) when inside a group
    schema: Option<&MessageDescriptor>,
    full_schema: &ParsedSchema, // full registry for nested type lookups
    annotations: bool,
) -> (ProtoTextMessage, usize, Option<WiretagResult>) {
    let buflen = buf.len();
    let mut pos = start;
    let mut message = ProtoTextMessage::default();

    loop {
        if pos == buflen {
            return (message, pos, None);
        }

        let mut field = ProtoTextField::default();

        // ── Parse wire tag ────────────────────────────────────────────────────

        let tag = parse_wiretag(buf, pos);

        if let Some(wtag_gar) = tag.wtag_gar {
            // Invalid wire type: consume rest of buffer
            if annotations {
                field.annotations.push("invalid field".to_string());
            }
            field.field_number = Some(0);
            field.content = ProtoTextContent::InvalidTagType(wtag_gar);
            pos = buflen;
            message.fields.push(field);
            continue;
        }

        let field_number = tag.wfield.unwrap();
        let wire_type = tag.wtype.unwrap();
        field.field_number = Some(field_number);
        if let Some(ohb) = tag.wfield_ohb {
            field.tag_overhang_count = Some(ohb);
        }
        if tag.wfield_oor.is_some() {
            field.tag_is_out_of_range = true;
        }
        pos = tag.next_pos;

        // ── Schema lookup ─────────────────────────────────────────────────────

        let field_schema: Option<FieldDescriptor> =
            schema.and_then(|s| s.get_field(field_number as u32));

        if annotations {
            if schema.is_none() {
                field.annotations.push("no schema".to_string());
            } else if let Some(ref fs) = field_schema {
                field.annotations.push(format_annotation(fs));
            } else {
                field.annotations.push("unknown field".to_string());
            }
        }

        // ── Wire-type dispatch ────────────────────────────────────────────────

        match wire_type {
            // ── VARINT ───────────────────────────────────────────────────────
            WT_VARINT => {
                let vr = parse_varint(buf, pos);
                if let Some(varint_gar) = vr.varint_gar {
                    field.content = ProtoTextContent::InvalidVarint(varint_gar);
                    pos = buflen;
                    message.fields.push(field);
                    continue;
                }
                pos = vr.next_pos;
                if let Some(ohb) = vr.varint_ohb {
                    field.value_overhang_count = Some(ohb);
                }
                let val = vr.varint.unwrap();

                if let Some(ref fs) = field_schema {
                    match decode_varint_by_kind(val, fs.kind()) {
                        Ok(content) => field.content = content,
                        Err(TypeMismatch) => {
                            field.proto2_has_type_mismatch = true;
                            field.content = ProtoTextContent::WireVarint(val);
                        }
                    }
                } else {
                    field.content = ProtoTextContent::WireVarint(val);
                }
            }

            // ── FIXED64 ──────────────────────────────────────────────────────
            WT_I64 => {
                if pos + 8 > buflen {
                    field.content = ProtoTextContent::InvalidFixed64(buf[pos..].to_vec());
                    pos = buflen;
                    message.fields.push(field);
                    continue;
                }
                let data = &buf[pos..pos + 8];
                pos += 8;

                if let Some(ref fs) = field_schema {
                    match fs.kind() {
                        Kind::Double => {
                            field.content = ProtoTextContent::Double(decode_double(data));
                        }
                        Kind::Fixed64 => {
                            field.content = ProtoTextContent::PFixed64(decode_fixed64(data));
                        }
                        Kind::Sfixed64 => {
                            field.content = ProtoTextContent::Sfixed64(decode_sfixed64(data));
                        }
                        _ => {
                            field.proto2_has_type_mismatch = true;
                            field.content = ProtoTextContent::WireFixed64(decode_fixed64(data));
                        }
                    }
                } else {
                    field.content = ProtoTextContent::WireFixed64(decode_fixed64(data));
                }
            }

            // ── LENGTH-DELIMITED ─────────────────────────────────────────────
            WT_LEN => {
                let lr = parse_varint(buf, pos);
                if lr.varint_gar.is_some() {
                    field.content = ProtoTextContent::InvalidBytesLength(buf[pos..].to_vec());
                    pos = buflen;
                    message.fields.push(field);
                    continue;
                }
                pos = lr.next_pos;
                if let Some(ohb) = lr.varint_ohb {
                    field.length_overhang_count = Some(ohb);
                }
                let length = lr.varint.unwrap() as usize;

                if pos + length > buflen {
                    field.missing_bytes_count = Some((length - (buflen - pos)) as u64);
                    field.content = ProtoTextContent::TruncatedBytes(buf[pos..].to_vec());
                    pos = buflen;
                    message.fields.push(field);
                    continue;
                }
                let data = &buf[pos..pos + length];
                pos += length;

                decode_len_field(
                    data,
                    field_schema.as_ref(),
                    full_schema,
                    annotations,
                    &mut field,
                );
            }

            // ── START GROUP ──────────────────────────────────────────────────
            WT_START_GROUP => {
                // Resolve nested schema via the full registry.
                let nested_desc: Option<MessageDescriptor> = field_schema
                    .as_ref()
                    .filter(|fs| fs.is_group())
                    .and_then(|fs| {
                        if let Kind::Message(msg_desc) = fs.kind() {
                            Some(msg_desc)
                        } else {
                            None
                        }
                    });

                let (nested_msg, new_pos, end_tag) = parse_message(
                    buf,
                    pos,
                    Some(field_number),
                    nested_desc.as_ref(),
                    full_schema,
                    annotations,
                );
                pos = new_pos;

                if end_tag.is_none() {
                    field.open_ended_group = true;
                } else if let Some(ref et) = end_tag {
                    if let Some(ohb) = et.wfield_ohb {
                        field.end_tag_overhang_count = Some(ohb);
                    }
                    if et.wfield_oor.is_some() {
                        field.end_tag_is_out_of_range = true;
                    }
                    let end_field = et.wfield.unwrap_or(0);
                    if end_field != field_number {
                        field.mismatched_group_end = Some(end_field);
                    }
                }
                // Always store as proto2-level group (field 40), matching Python.
                field.content = ProtoTextContent::Group(Box::new(nested_msg));
            }

            // ── END GROUP ────────────────────────────────────────────────────
            WT_END_GROUP => {
                if my_group.is_none() {
                    // Unexpected END_GROUP outside a group
                    field.content = ProtoTextContent::InvalidGroupEnd(buf[pos..].to_vec());
                    pos = buflen;
                    message.fields.push(field);
                    continue;
                }
                // Valid END_GROUP: return WITHOUT pushing the tag as a field.
                return (message, pos, Some(tag));
            }

            // ── FIXED32 ──────────────────────────────────────────────────────
            WT_I32 => {
                if pos + 4 > buflen {
                    field.content = ProtoTextContent::InvalidFixed32(buf[pos..].to_vec());
                    pos = buflen;
                    message.fields.push(field);
                    continue;
                }
                let data = &buf[pos..pos + 4];
                pos += 4;

                if let Some(ref fs) = field_schema {
                    match fs.kind() {
                        Kind::Float => {
                            field.content = ProtoTextContent::Float(decode_float(data));
                        }
                        Kind::Fixed32 => {
                            field.content = ProtoTextContent::PFixed32(decode_fixed32(data));
                        }
                        Kind::Sfixed32 => {
                            field.content = ProtoTextContent::Sfixed32(decode_sfixed32(data));
                        }
                        _ => {
                            field.proto2_has_type_mismatch = true;
                            // Fallback: Python uses field.fixed32 (proto2-level, field 37)
                            field.content = ProtoTextContent::PFixed32(decode_fixed32(data));
                        }
                    }
                } else {
                    // No schema fallback: Python uses field.fixed32 (proto2-level, field 37)
                    field.content = ProtoTextContent::PFixed32(decode_fixed32(data));
                }
            }

            _ => {
                // Wire types 0–5 are the only valid ones; wiretag parser rejects >5.
                unreachable!("wire type > 5 should have been caught by parse_wiretag");
            }
        }

        message.fields.push(field);
    }
}
