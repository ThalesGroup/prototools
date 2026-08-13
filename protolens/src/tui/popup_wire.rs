// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0282: what one part of one wire row says, in words.
//!
//! The wire row draws `2|08:05[68 65 6c 6c 6f]` — correct, dense, and
//! silent. This is the half that speaks: the pointer rests on a part,
//! [`App::wire_part_at`] says which one and what the painter accused it
//! of, and this module turns that into the four or five lines of a box.
//!
//! The division of labor is spec 0282 G5. `wire.rs` reports *structure
//! and its own keywords* and stays schema-free; everything that needs a
//! descriptor — the field's name, the declared type a value is read as,
//! an enum's spelling of a number — is resolved here. Every reading
//! goes through `prototext_core`'s own decoders and protoc-compatible
//! formatters, so a value in the box and the same value in the document
//! are the same digits.

use super::wire::{Region, WireHit, WirePart, WIRE_ROW_MAX_BYTES};
use super::*;
use crate::annotation::{clause, PACK_SIZE};
use prototext_core::helpers::{
    decode_bool, decode_double, decode_fixed32, decode_fixed64, decode_float, decode_int32,
    decode_int64, decode_sfixed32, decode_sfixed64, decode_sint32, decode_sint64, decode_uint32,
    decode_uint64, parse_varint, parse_wiretag, WT_END_GROUP, WT_I32, WT_I64, WT_LEN,
    WT_START_GROUP, WT_VARINT,
};
use prototext_core::serialize::common::{
    escape_bytes, escape_string, format_bool_protoc, format_double_protoc, format_fixed32_protoc,
    format_fixed64_protoc, format_float_protoc, format_int32_protoc, format_int64_protoc,
    format_sfixed32_protoc, format_sfixed64_protoc, format_sint32_protoc, format_sint64_protoc,
    format_uint32_protoc, format_uint64_protoc,
};

/// The protobuf spelling of a wire type (spec 0282 S6).
///
/// 6 and 7 exist as bit patterns and as nothing else, which is
/// `INVALID_TAG_TYPE` seen from the other side.
fn wire_type_name(wtype: u32) -> &'static str {
    match wtype {
        WT_VARINT => "VARINT",
        WT_I64 => "I64",
        WT_LEN => "LEN",
        WT_START_GROUP => "SGROUP",
        WT_END_GROUP => "EGROUP",
        WT_I32 => "I32",
        _ => "invalid — no such wire type",
    }
}

/// Readings with nothing to point at (spec 0283 N1): no substring of
/// `1.4142135623730951` is the byte the pointer is on.
fn unmarked(
    readings: Vec<(&'static str, String)>,
) -> Vec<(&'static str, String, Option<Range<usize>>)> {
    readings
        .into_iter()
        .map(|(name, value)| (name, value, None))
        .collect()
}

/// Spec 0283 S5: the characters `payload[at]` produced in
/// `"{escape_bytes(payload)}"`.
///
/// Exact, and cheaply so. `escape_bytes` maps one input byte to one
/// escape group with no dependence on its neighbors — the octal form is
/// always three digits, so nothing a later byte does can change how an
/// earlier one was spelled. The escape of a prefix is therefore the
/// prefix of the escape. Its output is ASCII throughout, so counting
/// bytes and counting characters agree.
fn bytes_mark(payload: &[u8], at: usize) -> Option<Range<usize>> {
    let byte = *payload.get(at)?;
    // The opening quote.
    let start = 1 + escape_bytes(&payload[..at]).len();
    Some(start..start + escape_bytes(&[byte]).len())
}

/// Spec 0283 S6: the same, one level up — the characters the *character*
/// containing `payload[at]` produced.
///
/// `escape_string` maps one character to one escape group, so the same
/// prefix argument holds. A hovered continuation byte marks a character
/// wider than itself, which is the honest answer: those bytes spell that
/// character, and no shorter piece of the string is the byte's own
/// contribution.
fn string_mark(text: &str, at: usize) -> Option<Range<usize>> {
    // At most four steps back: the start of the character `at` is in.
    let offset = (0..=at.min(text.len()))
        .rev()
        .find(|at| text.is_char_boundary(*at))?;
    let ch = text[offset..].chars().next()?;
    let start = 1 + escape_string(&text[..offset]).chars().count();
    Some(start..start + escape_string(ch.encode_utf8(&mut [0; 4])).chars().count())
}

fn bytes_word(n: usize) -> &'static str {
    if n == 1 {
        "byte"
    } else {
        "bytes"
    }
}

impl App {
    /// Opens the wire box for `hit` at `anchor`.
    ///
    /// The same guard `open_score_popup` carries: the menu is the
    /// innermost modal and a box cannot be opened underneath one.
    pub(super) fn open_wire_popup(&mut self, hit: &WireHit, anchor: (u16, u16)) {
        if self.menu.is_some() {
            return;
        }
        let body = self.wire_box(hit);
        self.popup = Some(Popup {
            body: PopupBody::Wire(body),
            anchor,
        });
    }

    /// The box's three groups for one hit (spec 0282 S6-S13).
    pub(super) fn wire_box(&self, hit: &WireHit) -> WireBox {
        let mut body = WireBox::default();
        match hit.part {
            // S12: the row ran out of columns; the message did not run
            // out of bytes. So no flaw line, and the one instruction any
            // of these boxes gives — which has to name something that
            // works. `w`/`W` do not: the cap is enforced per *row*, so
            // opening a subtree's wire spans yields more rows that are
            // each capped the same way.
            WirePart::Elided { hidden } => {
                body.head.push(BoxLine::plain(format!(
                    "…×{hidden} — {hidden} bytes not shown"
                )));
                body.head.push(BoxLine::plain(format!(
                    "a wire row draws at most {WIRE_ROW_MAX_BYTES} bytes; \
                     :export --binary writes them all"
                )));
                return body;
            }
            // S11: `??` is drawn at two sites for one accusation
            // (0279), and this is where the reader is told which. The
            // second line is the arithmetic the `cut()` site already
            // did — or, with no declared length to subtract from, what
            // happened instead.
            WirePart::Truncated { missing, .. } => {
                body.head.push(BoxLine::plain(
                    "?? — bytes the message does not contain".to_string(),
                ));
                body.head.push(BoxLine::plain(match missing {
                    Some(n) => format!(
                        "this record needs {n} more {}; the blob ends here",
                        bytes_word(n)
                    ),
                    None => "this varint has no final byte".to_string(),
                }));
            }
            WirePart::Region(region) => self.wire_region_lines(hit, region, &mut body),
        }

        body.flaws = hit
            .flaws
            .iter()
            .filter(|keyword| **keyword != PACK_SIZE)
            .map(|keyword| {
                BoxLine::plain(match clause(keyword) {
                    Some(clause) => format!("{keyword} — {clause}"),
                    None => (*keyword).to_string(),
                })
            })
            .collect();
        body
    }

    /// The head lines for the four parts of a record (S6-S9).
    fn wire_region_lines(&self, hit: &WireHit, region: Region, body: &mut WireBox) {
        let node = hit.pos.node;
        match region {
            Region::Type => {
                let wtype = self
                    .blob
                    .get(hit.bytes.start)
                    .map_or(u32::from(self.tree[node].span.wire_type), |byte| {
                        u32::from(byte & 0x07)
                    });
                body.head.push(BoxLine::plain(format!(
                    "wire type {wtype} — {}",
                    wire_type_name(wtype)
                )));
                // The *effective* type, so an override is reflected —
                // and omitted rather than filled with a placeholder when
                // the node has none.
                if let Some(key) = self.current_type_key(node) {
                    body.head.push(BoxLine::plain(format!("reads as: {key}")));
                }
            }
            Region::Tag => {
                let number = parse_wiretag(&self.blob, hit.bytes.start)
                    .wfield
                    .unwrap_or_else(|| u64::from(self.tree[node].span.field_number));
                // S7: no name rather than a placeholder. The line below
                // says whether that is because no schema declares the
                // field, or because there was no schema to ask.
                body.head
                    .push(BoxLine::plain(match self.parent_field(node) {
                        Some(field) => format!("field {number} — \"{}\"", field.name()),
                        None => format!("field {number}"),
                    }));
                if self.parent_field(node).is_none() && self.parent_is_typed(node) {
                    body.head
                        .push(BoxLine::plain("no schema declares this field".to_string()));
                }
            }
            Region::Len => {
                let declared = parse_varint(&self.blob, hit.bytes.start)
                    .varint
                    .unwrap_or(0) as usize;
                let packed = hit.flaws.contains(&PACK_SIZE);
                body.head.push(BoxLine::plain(format!(
                    "length {declared} {}{}",
                    bytes_word(declared),
                    if packed { " — packed run" } else { "" }
                )));
            }
            Region::Payload => self.wire_value_lines(hit, body),
            // Bytes no framing claimed. Nothing honest can be said about
            // what they mean, which is exactly what the row's subdued
            // banding already says without words.
            Region::Unclaimed => body.head.push(BoxLine::plain(
                "no framing accounts for these bytes".to_string(),
            )),
        }
    }

    /// S9: the declared reading first and marked, then every other proto
    /// type the wire type admits, one per line.
    fn wire_value_lines(&self, hit: &WireHit, body: &mut WireBox) {
        let node = hit.pos.node;
        let declared = self.current_type_key(node);
        // An enum reads as a name the schema gives rather than as one of
        // the primitive spellings, so it is its own line and is already
        // the declared one.
        let mut marked = match self.enum_reading(hit) {
            Some(line) => {
                body.head.push(BoxLine::plain(line));
                true
            }
            None => false,
        };

        for (name, value, at) in self.wire_readings(hit) {
            // The reading is indented past its type name, and the
            // `← declared` suffix is appended after it, so shifting by
            // that prefix is the whole of placing the mark on the line.
            let prefix = format!("{name:<8} ");
            let shift = prefix.chars().count();
            let mark = at.map(|at| at.start + shift..at.end + shift);
            let text = format!("{prefix}{value}");
            if !marked && Some(name) == declared.as_deref() {
                marked = true;
                body.head.push(BoxLine {
                    text: format!("{text}  ← declared"),
                    mark,
                });
            } else {
                body.alts.push(BoxLine { text, mark });
            }
        }

        if !marked && !body.alts.is_empty() {
            // Nothing in the family is what this node is read as. The
            // first reading leads instead: a box whose every line S10
            // may drop has no answer left to keep.
            body.head.push(body.alts.remove(0));
        }
    }

    /// The value as the declared enum spells it, when the declared type
    /// is one (S9).
    fn enum_reading(&self, hit: &WireHit) -> Option<String> {
        if u32::from(self.tree[hit.pos.node].span.wire_type) != WT_VARINT {
            return None;
        }
        let prost_reflect::Kind::Enum(desc) = self.parent_field(hit.pos.node)?.kind() else {
            return None;
        };
        let number = decode_int32(parse_varint(&self.blob, hit.bytes.start).varint?);
        Some(match desc.get_value(number) {
            Some(value) => format!("{:<8} {} ({number})  ← declared", desc.name(), value.name()),
            None => format!("{:<8} {number}  ← declared", desc.name()),
        })
    }

    /// Every proto type the record's wire type admits, in the order S9
    /// lists them.
    ///
    /// The bytes are the ones the painter *recorded*, which is the ones
    /// it drew — so a LEN payload past `WIRE_ROW_MAX_BYTES` is elided
    /// here exactly the way the row elides it, rather than the box
    /// quietly knowing more than the row it explains.
    /// The third element is spec 0283's mark: where in *that reading*
    /// the hovered byte went. Only the two LEN spellings have one — N1.
    fn wire_readings(&self, hit: &WireHit) -> Vec<(&'static str, String, Option<Range<usize>>)> {
        let end = hit.bytes.end.min(self.blob.len());
        let bytes = &self.blob[hit.bytes.start.min(end)..end];
        match u32::from(self.tree[hit.pos.node].span.wire_type) {
            WT_VARINT => {
                let Some(v) = parse_varint(&self.blob, hit.bytes.start).varint else {
                    return Vec::new();
                };
                unmarked(vec![
                    ("int32", format_int32_protoc(decode_int32(v))),
                    ("int64", format_int64_protoc(decode_int64(v))),
                    ("uint32", format_uint32_protoc(decode_uint32(v))),
                    ("uint64", format_uint64_protoc(decode_uint64(v))),
                    ("sint32", format_sint32_protoc(decode_sint32(v))),
                    ("sint64", format_sint64_protoc(decode_sint64(v))),
                    ("bool", format_bool_protoc(decode_bool(v)).to_string()),
                ])
            }
            WT_I64 if bytes.len() >= 8 => unmarked(vec![
                ("fixed64", format_fixed64_protoc(decode_fixed64(bytes))),
                ("sfixed64", format_sfixed64_protoc(decode_sfixed64(bytes))),
                ("double", format_double_protoc(decode_double(bytes))),
            ]),
            WT_I32 if bytes.len() >= 4 => unmarked(vec![
                ("fixed32", format_fixed32_protoc(decode_fixed32(bytes))),
                ("sfixed32", format_sfixed32_protoc(decode_sfixed32(bytes))),
                ("float", format_float_protoc(decode_float(bytes))),
            ]),
            WT_LEN => {
                // Spec 0283 S4: the byte within the payload, which is
                // the only part whose readings spell their bytes out.
                let at = hit.byte.and_then(|at| at.checked_sub(hit.bytes.start));
                let (text, mark) = match std::str::from_utf8(bytes) {
                    Ok(s) => (
                        format!("\"{}\"", escape_string(s)),
                        at.and_then(|at| string_mark(s, at)),
                    ),
                    // Not lossy text: a replacement character would be a
                    // reading this payload does not have — and there is
                    // nothing to point at in the words that say so.
                    Err(_) => ("not valid UTF-8".to_string(), None),
                };
                vec![
                    ("string", text, mark),
                    (
                        "bytes",
                        format!("\"{}\"", escape_bytes(bytes)),
                        at.and_then(|at| bytes_mark(bytes, at)),
                    ),
                ]
            }
            // A truncated fixed-width payload, or a wire type that has
            // no readings at all. The flaw lines say why.
            _ => Vec::new(),
        }
    }

    /// Whether `idx`'s parent has a resolved message type at all — the
    /// difference between "no schema declares this field" and "there was
    /// no schema to ask", which is the whole of what S7's second line
    /// distinguishes.
    fn parent_is_typed(&self, idx: usize) -> bool {
        self.parent(idx)
            .and_then(|parent| self.fqdns.get(self.tree[parent].span.type_fqdn))
            .and_then(|fqdn| self.ctx.pool().get_message_by_name(fqdn))
            .is_some()
    }
}
