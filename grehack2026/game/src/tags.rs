// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The value channel of spec 0384 (replacing the tag channel of spec 0377):
//! the client hides a bit field in a request's VARINT field *values*
//! (canonical, or padded with one redundant continuation byte), and the
//! server reads it back.
//!
//! Both sides go through the prototext text render, so the library is the one
//! source of truth for which rendered lines are in-scope field records (0384
//! S1) and how a value's non-canonicity shows (`val_ohb`). The client appends
//! `; val_ohb: 1` to the lines a set bit selects and re-encodes (0384 S2); the
//! server reads which lines carry `val_ohb`. A terminator `1` bit frames a
//! message inside the bit field (0377 S3a).
//!
//! Only VARINT field records carry a bit (0384 S1): in a life message those
//! are `cells` (a `CellState` enum), `generation`, and the `Range` scalars.
//! The length-delimited headers (`Grid`/`Row`/`Rules`) are skipped.

use prototext_core::schema::Message;
use prototext_core::{parse_schema, render_as_bytes, render_as_text, ParsedSchema, RenderOpts};

/// The two message roots the value channel reads and writes: requests, from the
/// server's decode and the client's encode; responses, the other way round
/// (spec 0379 S1).
pub const REQUEST: &str = "grehack.life.v1.StepRequest";
pub const RESPONSE: &str = "grehack.life.v1.StepResponse";

/// A boolean per field record, packed into bytes (spec 0377 S4).
///
/// Bit *i* is record *i*'s flag; bit 0 is the most significant bit of byte 0,
/// so the bytes read left to right as the fields were sent. `n` bits occupy
/// `n.div_ceil(8)` bytes, the last byte's unused low bits zero.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BitField {
    bytes: Vec<u8>,
    bits: usize,
}

impl BitField {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, set: bool) {
        if self.bits.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if set {
            self.bytes[self.bits / 8] |= 1 << (7 - (self.bits % 8));
        }
        self.bits += 1;
    }

    pub fn bit(&self, i: usize) -> bool {
        i < self.bits && (self.bytes[i / 8] >> (7 - (i % 8))) & 1 == 1
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bits
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// A message framed for the tag channel (spec 0377 S3a): its bits, then a
    /// single `1` terminator. The empty message frames to an empty bit field
    /// — no terminator — so an all-canonical request carries nothing.
    pub fn frame_message(message: &[u8]) -> Self {
        let mut bits = BitField::new();
        if message.is_empty() {
            return bits;
        }
        for &byte in message {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1 == 1);
            }
        }
        bits.push(true); // terminator
        bits
    }

    /// The message this bit field frames (spec 0377 S3a): the bits strictly
    /// before the last `1`, regrouped into bytes. No `1` bit — the empty bit
    /// field included — means the empty message. A trailing partial byte
    /// (the convention does not require whole bytes) is dropped; the demo's
    /// payload is byte-aligned.
    pub fn recover_message(&self) -> Vec<u8> {
        let Some(terminator) = (0..self.bits).rev().find(|&i| self.bit(i)) else {
            return Vec::new();
        };
        let mut message = Vec::with_capacity(terminator / 8);
        for byte_start in (0..terminator).step_by(8) {
            if byte_start + 8 > terminator {
                break; // a partial final byte
            }
            let mut byte = 0u8;
            for bit in 0..8 {
                byte = (byte << 1) | u8::from(self.bit(byte_start + bit));
            }
            message.push(byte);
        }
        message
    }
}

/// The life schema for a root message (`REQUEST` or `RESPONSE`), built once
/// from the embedded descriptor (a lone `FileDescriptorProto`, which a
/// `FileDescriptorSet` wraps for `parse_schema`) and cached per root.
fn schema(root: &str) -> &'static ParsedSchema {
    use prost::Message;
    use prost_types::{FileDescriptorProto, FileDescriptorSet};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static CACHE: OnceLock<Mutex<HashMap<String, &'static ParsedSchema>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(s) = cache.lock().unwrap().get(root) {
        return s;
    }
    let file = FileDescriptorProto::decode(crate::DESCRIPTOR)
        .expect("the embedded descriptor is written by build.rs");
    let set = FileDescriptorSet { file: vec![file] }.encode_to_vec();
    let parsed: &'static ParsedSchema = Box::leak(Box::new(
        parse_schema(&set, root).expect("the life schema holds the root"),
    ));
    cache.lock().unwrap().insert(root.to_string(), parsed);
    parsed
}

fn root_descriptor(root: &str) -> Option<Message> {
    schema(root).root_descriptor()
}

fn opts() -> RenderOpts {
    RenderOpts {
        assume_binary: true,
        include_annotations: true,
        indent: 1,
        expand_any: false,
        hide_unknown_fields: false,
        expand_message_set: false,
    }
}

/// One bit per in-scope VARINT field record in `message` (a `root` message),
/// in render order: 1 when the field's value was non-canonical (spec 0384 S2).
pub fn read_values(message: &[u8], root: &str) -> BitField {
    let text = render_as_text(message, root_descriptor(root).as_ref(), opts())
        .expect("a received message renders");
    let text = String::from_utf8(text).expect("prototext renders UTF-8");

    let mut bits = BitField::new();
    for line in text.lines() {
        if let Some(annotation) = in_scope(line) {
            bits.push(has_val_ohb(annotation));
        }
    }
    bits
}

/// `message` (a `root` message) re-encoded so in-scope VARINT record *i*'s
/// value is non-canonical when `bits.bit(i)` is set (spec 0384 S2). Bits past
/// the in-scope-record count are dropped (0377 N5). Returns the modified wire
/// bytes; the decoded message is unchanged (0384 S3).
pub fn encode_values(message: &[u8], bits: &BitField, root: &str) -> Vec<u8> {
    let text = render_as_text(message, root_descriptor(root).as_ref(), opts())
        .expect("the message renders");
    let text = String::from_utf8(text).expect("prototext renders UTF-8");

    let mut field = 0usize;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if in_scope(line).is_some() {
            if bits.bit(field) && !line.contains("val_ohb") {
                out.push_str(line);
                out.push_str("; val_ohb: 1");
            } else {
                out.push_str(line);
            }
            field += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    render_as_bytes(out.as_bytes(), {
        let mut o = opts();
        o.assume_binary = false; // the input is now prototext text, not binary
        o
    })
    .expect("the modified text re-encodes")
    .into_owned()
}

/// The `#@` annotation of a line that is an in-scope VARINT field record, or
/// `None` for a line that is not one (spec 0384 S1): a `}`, the header, a blank
/// line, a malformed record, a packed continuation, or a non-VARINT field.
fn in_scope(line: &str) -> Option<&str> {
    let annotation = line.split_once("#@")?.1;
    if !has_field_tag(annotation) {
        return None;
    }
    // A packed record's first line shows pack_size and is in scope; its later
    // element lines carry [packed=true] without pack_size and are values under
    // a tag already counted (spec 0377 N2). (A life message has no packed
    // VARINT field — `cells` is unpacked, 0377 S1 — but the rule is kept.)
    if annotation.contains("[packed=true]") && !annotation.contains("pack_size") {
        return None;
    }
    // Only VARINT fields carry a bit now (spec 0384 S1). The value channel
    // rides value varints, so a length-delimited or fixed-width field is out.
    if !is_varint_field(annotation) {
        return None;
    }
    Some(annotation)
}

/// Whether a known field's annotation names a VARINT wire type (spec 0384 S1).
///
/// A known field renders its type in the annotation, before the ` = <num>`
/// field tag; the wire type is implied by that type, not spelled out (the
/// literal `varint`/`bytes` token appears only on unknown/raw-wire/mismatch
/// lines, which a life message does not produce). The rendered forms:
///
/// - a scalar is a lowercase keyword: `uint64 = 3`, `uint32 = 1`;
/// - an **enum** is its type name with the value in parens: `CellState(1) = 1`,
///   `repeated CellState(0) = 1` — VARINT on the wire;
/// - a **nested message / group** header is its bare type name, no parens:
///   `Grid = 1`, `repeated Row = 1`, `Range = 2` — length-delimited.
///
/// So the VARINT scalars and any enum (a named type with a `(value)` suffix)
/// are in scope; the length-delimited (`string`, `bytes`, a bare message name)
/// and fixed-width (`fixed*`/`sfixed*`/`float`/`double`) are out.
fn is_varint_field(annotation: &str) -> bool {
    // The declaration is everything before the ` = <num>` field tag. A trailing
    // field option like `[packed=true]` sits between the type and the ` = `, so
    // drop it; the type word is then the last whitespace-separated token.
    let Some((decl, _)) = annotation.split_once(" = ") else {
        return false;
    };
    let decl = decl.split('[').next().unwrap_or(decl);
    let Some(type_word) = decl.split_whitespace().next_back() else {
        return false;
    };
    match type_word {
        // Explicit VARINT scalars.
        "int32" | "int64" | "uint32" | "uint64" | "sint32" | "sint64" | "bool" => true,
        // Length-delimited and fixed-width scalars are not VARINT.
        "string" | "bytes" | "double" | "float" | "fixed32" | "fixed64" | "sfixed32"
        | "sfixed64" => false,
        // A named type: an enum renders with a `(value)` suffix and is VARINT;
        // a bare message/group name is length-delimited. `group` itself (an
        // unknown-group marker) is not a VARINT value field.
        other => other != "group" && other.ends_with(')'),
    }
}

/// Whether the annotation carries a `= <number>` field tag. Guards against a
/// `[packed=true]` `=` counting as the tag: the field tag is ` = `, set off by
/// spaces (annotation format), and followed by digits.
fn has_field_tag(annotation: &str) -> bool {
    annotation.split(" = ").skip(1).any(|rest| {
        rest.trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

/// Whether the annotation reports value overhang (spec 0384 S2: the value
/// varint carries one redundant continuation byte, `val_ohb`). Only `val_ohb`
/// counts, not `tag_ohb`, `len_ohb`, `etag_ohb` or the per-element `ohb`.
fn has_val_ohb(annotation: &str) -> bool {
    annotation
        .split(';')
        .any(|m| m.trim().starts_with("val_ohb"))
}

// ── The smuggled command channel (specs 0382, 0383, 0385) ─────────────────────

/// The server's half, as sent on the wire: the operator's command bytes
/// verbatim, no wrapper (spec 0385 G1). The client runs them directly (the
/// smuggled channel).
pub fn command_message(command: &[u8]) -> Vec<u8> {
    command.to_vec()
}

/// The client's half: the command's output bytes, exactly (spec 0385 S1, G1).
/// No prefix is added. Newlines are kept, so the server displays several
/// lines; the other control bytes (tabs, a stray carriage return) become
/// spaces, and the ends are trimmed (spec 0383 S1, kept: it truncates text,
/// not structure). The empty output is the empty payload.
pub fn command_output(output: &[u8]) -> Vec<u8> {
    let kept: Vec<u8> = output
        .iter()
        .map(|&b| {
            if b != b'\n' && b.is_ascii_control() {
                b' '
            } else {
                b
            }
        })
        .collect();
    kept.trim_ascii().to_vec()
}

/// The command output a reply message carries (spec 0385 S1): the bytes as
/// sent, which — with no prefix or framing word — are exactly the payload.
/// A reply is just its bytes, so this is the identity; it exists so callers
/// read the intent (`parse_command_output`) rather than touching the bytes
/// raw, and so a future framing change has one place to live.
pub fn parse_command_output(message: &[u8]) -> &[u8] {
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn a_request() -> Vec<u8> {
        crate::pb::StepRequest {
            grid: Some(crate::pb::Grid {
                rows: vec![crate::pb::Row { cells: vec![1, 0] }],
            }),
            rules: Some(crate::pb::Rules {
                birth: Some(crate::pb::Range { min: 3, max: 3 }),
                survival: Some(crate::pb::Range { min: 2, max: 3 }),
                topology: 0,
            }),
            generation: 7,
        }
        .encode_to_vec()
    }

    fn rendered_fields(bytes: &[u8]) -> usize {
        let text = render_as_text(bytes, root_descriptor(REQUEST).as_ref(), opts()).unwrap();
        String::from_utf8(text)
            .unwrap()
            .lines()
            .filter(|l| in_scope(l).is_some())
            .count()
    }

    #[test]
    fn bits_pack_big_endian() {
        let mut b = BitField::new();
        assert!(b.is_empty());
        b.push(true);
        b.push(false);
        b.push(true);
        assert_eq!(b.len(), 3);
        assert_eq!(b.as_bytes(), &[0b1010_0000]);
        assert!(b.bit(0) && !b.bit(1) && b.bit(2) && !b.bit(3));
    }

    #[test]
    fn a_new_byte_opens_every_eight_bits() {
        for n in [0usize, 1, 7, 8, 9] {
            let mut b = BitField::new();
            for _ in 0..n {
                b.push(false);
            }
            assert_eq!(b.len(), n);
            assert_eq!(b.as_bytes().len(), n.div_ceil(8));
            assert!(b.as_bytes().iter().all(|&byte| byte == 0), "n={n}");
        }
    }

    #[test]
    fn a_message_frames_with_a_terminator_and_round_trips() {
        let framed = BitField::frame_message(b"Hi");
        assert_eq!(framed.len(), 8 * 2 + 1, "two bytes and a terminator");
        assert!(framed.bit(16), "terminator is the last bit");
        assert_eq!(framed.recover_message(), b"Hi");
    }

    #[test]
    fn the_empty_message_is_an_all_false_field() {
        assert_eq!(BitField::frame_message(b""), BitField::new());
        assert_eq!(BitField::new().recover_message(), b"");
        // Padding after the terminator is dropped.
        let mut padded = BitField::frame_message(b"x");
        for _ in 0..20 {
            padded.push(false);
        }
        assert_eq!(padded.recover_message(), b"x");
    }

    #[test]
    fn a_message_whose_last_bit_is_one_still_gets_a_terminator() {
        // 0xFF ends in a 1 bit; without a terminator it would be ambiguous.
        let framed = BitField::frame_message(&[0xFF]);
        assert_eq!(framed.len(), 9);
        assert_eq!(framed.recover_message(), &[0xFF]);
    }

    #[test]
    fn reading_a_canonical_request_is_all_zero() {
        let bytes = a_request();
        let bits = read_values(&bytes, REQUEST);
        // VARINT records only (spec 0384 S1): two `cells`, `generation`, and
        // the four `Range` scalars (birth/survival min/max) = 7. The
        // `Grid`/`Row`/`Rules`/`Range` headers are length-delimited, out.
        assert_eq!(bits.len(), 7);
        assert_eq!(bits.len(), rendered_fields(&bytes));
        assert!(bits.as_bytes().iter().all(|&b| b == 0));
    }

    /// A request with a 5×5 grid: about 36 field records, enough tags to
    /// carry a short message with its terminator.
    fn a_roomy_request() -> Vec<u8> {
        // A 20x20 grid: ~420 field records, enough tags for a short message
        // and its terminator (spec 0379 N4).
        crate::pb::StepRequest {
            grid: Some(crate::pb::Grid {
                rows: vec![crate::pb::Row { cells: vec![1; 20] }; 20],
            }),
            rules: Some(crate::pb::Rules {
                birth: Some(crate::pb::Range { min: 3, max: 3 }),
                survival: Some(crate::pb::Range { min: 2, max: 3 }),
                topology: 0,
            }),
            generation: 1,
        }
        .encode_to_vec()
    }

    #[test]
    fn encode_then_read_round_trips_a_message() {
        let request = a_roomy_request();
        let message = b"Hi";
        let framed = BitField::frame_message(message);
        assert!(
            framed.len() <= rendered_fields(&request),
            "message must fit"
        );
        let spoiled = encode_values(&request, &framed, REQUEST);

        // Value-preserving: the spoiled bytes decode to the same StepRequest.
        assert_eq!(
            crate::pb::StepRequest::decode(&spoiled[..]).unwrap(),
            crate::pb::StepRequest::decode(&request[..]).unwrap()
        );
        // And the server's read recovers the message.
        assert_eq!(read_values(&spoiled, REQUEST).recover_message(), message);
    }

    #[test]
    fn a_non_canonical_value_sets_exactly_its_bit() {
        // `generation: 7` is the last VARINT record (bit 6 of 7: two cells,
        // four Range scalars, then generation). Pad its value `18 07` to
        // `18 87 00` — a redundant continuation byte on the value (val_ohb),
        // which re-encodes to the same 7 (spec 0384 S3).
        let bytes = a_request();
        let at = bytes
            .windows(2)
            .rposition(|w| w == [0x18, 0x07])
            .expect("generation: 7 is `18 07`");
        let mut spoiled = bytes.clone();
        spoiled.splice(at + 1..at + 2, [0x87, 0x00]);
        // Value-preserving: still decodes to generation 7.
        assert_eq!(
            crate::pb::StepRequest::decode(&spoiled[..])
                .unwrap()
                .generation,
            7
        );
        assert_eq!(read_values(&bytes, REQUEST).as_bytes(), &[0x00]);
        // bit 6 set: 0b0000_0010.
        assert_eq!(read_values(&spoiled, REQUEST).as_bytes(), &[0b0000_0010]);
    }

    #[test]
    fn a_bit_field_longer_than_the_fields_is_truncated() {
        let request = a_request();
        let n = rendered_fields(&request);
        let mut too_long = BitField::new();
        for _ in 0..n + 10 {
            too_long.push(true);
        }
        // encode_values drops the excess; every one of the n in-scope VARINT
        // records gets val_ohb, so the read-back has exactly n bits, all set.
        let spoiled = encode_values(&request, &too_long, REQUEST);
        let read = read_values(&spoiled, REQUEST);
        assert_eq!(read.len(), n);
        assert!((0..n).all(|i| read.bit(i)));
    }

    #[test]
    fn in_scope_picks_varint_fields_and_val_ohb_classifies_lines() {
        // Not field records.
        assert_eq!(in_scope("#@ prototext: protoc"), None);
        assert_eq!(in_scope("}"), None);
        // VARINT scalars and enums are in scope.
        assert!(in_scope("generation: 5  #@ uint64 = 3").is_some());
        assert!(in_scope("min: 3  #@ uint32 = 1").is_some());
        // An enum renders its type name with the value in parens.
        assert!(in_scope("cells: CELL_STATE_ALIVE  #@ repeated CellState(1) = 1").is_some());
        assert!(in_scope("cells: CELL_STATE_DEAD  #@ CellState(0) = 1").is_some());
        // Length-delimited fields (nested messages, strings, bytes) are out.
        assert_eq!(in_scope("grid {  #@ Grid = 1"), None);
        assert_eq!(in_scope("birth {  #@ Range = 1"), None);
        assert_eq!(in_scope("name: \"x\"  #@ string = 2"), None);
        assert_eq!(in_scope("blob: \"..\"  #@ bytes = 4"), None);
        // Fixed-width scalars are out (not VARINT).
        assert_eq!(in_scope("t: 1.5  #@ double = 7"), None);
        assert_eq!(in_scope("u: 3  #@ fixed32 = 8"), None);
        // A packed continuation stays out (spec 0377 N2); its header is in.
        assert!(in_scope("x: 2  #@ repeated int32 [packed=true] = 5").is_none());
        assert!(in_scope("x: 1  #@ repeated int32 [packed=true] = 5; pack_size: 3").is_some());
        // val_ohb classification: only val_ohb counts, not the others.
        assert!(has_val_ohb("uint64 = 3; val_ohb: 1"));
        assert!(!has_val_ohb("group; GroupOp = 30; tag_ohb: 1"));
        assert!(!has_val_ohb("uint32 = 1; len_ohb: 2"));
        assert!(!has_val_ohb("group; GroupOp = 30; etag_ohb: 1"));
    }

    // ── The smuggled command channel (specs 0382, 0383, 0385) ─────────────────

    #[test]
    fn the_command_message_rides_verbatim() {
        // The server's half carries the operator's command bytes verbatim
        // (spec 0385 G1): no wrapper is added.
        for cmd in [b"whoami".as_slice(), b"echo hi", b"ls -la /"] {
            assert_eq!(command_message(cmd), cmd);
        }
    }

    #[test]
    fn command_output_is_exactly_the_bytes_with_no_prefix() {
        // Newlines are kept; a tab (and any other control byte) becomes a
        // space; the ends are trimmed. No prefix (spec 0385 S1, G1).
        let message = command_output(b"  a line\nand another\t-- tabbed  ");
        assert_eq!(message, b"a line\nand another -- tabbed");
        assert_eq!(parse_command_output(&message), message);
        // Whitespace-only output trims to the empty payload.
        assert_eq!(command_output(b"\n  \n"), b"");
        assert_eq!(parse_command_output(b""), b"");
    }

    #[test]
    fn the_command_channel_rides_the_values_both_ways() {
        // The server's half, in a response's values. A 20x20 grid has plenty
        // of VARINT records for a short command and its terminator.
        let response = crate::pb::StepResponse {
            grid: Some(crate::pb::Grid {
                rows: vec![crate::pb::Row { cells: vec![1; 20] }; 20],
            }),
            generation: 2,
        }
        .encode_to_vec();
        let message = command_message(b"whoami");
        let framed = BitField::frame_message(&message);
        let spoiled = encode_values(&response, &framed, RESPONSE);
        assert_eq!(read_values(&spoiled, RESPONSE).recover_message(), message);
        // Value-preserving: the response still decodes the same (spec 0384 S3).
        assert_eq!(
            crate::pb::StepResponse::decode(&spoiled[..]).unwrap(),
            crate::pb::StepResponse::decode(&response[..]).unwrap()
        );

        // The client's half, in a request's values: the command's output.
        let out = command_output(b"experiment");
        let reply = BitField::frame_message(&out);
        let spoiled = encode_values(&a_roomy_request(), &reply, REQUEST);
        assert_eq!(
            parse_command_output(&read_values(&spoiled, REQUEST).recover_message()),
            b"experiment"
        );
    }
}
