// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use crate::helpers::{write_varint_ohb, WT_END_GROUP, WT_LEN, WT_START_GROUP};
use memchr::memrchr;

mod encode_annotation;
mod fields;
mod frame;
mod placeholder;

#[cfg(test)]
use encode_annotation::parse_field_decl_into;
use encode_annotation::{parse_annotation, Ann};
use fields::{encode_packed_elem, encode_scalar_line, write_tag_ohb_local};
use frame::Frame;
use placeholder::{compact, fill_placeholder, write_placeholder};

// ── Helpers: field number and line classification ─────────────────────────────

/// Extract the field number from the LHS of a line and/or annotation.
///
/// Precedence: annotation's field_decl (`= N`) > numeric LHS.
#[inline]
fn extract_field_number(lhs: &str, ann: &Ann<'_>) -> u64 {
    if let Some(fn_) = ann.field_number {
        return fn_;
    }
    lhs.trim().parse::<u64>().unwrap_or(0)
}

/// Split a line into `(value_part, annotation_str)`.
///
/// The separator is `  #@ ` (2 spaces + `#` + `@` + space).  We scan right-to-left
/// so that quoted string values containing `  #@ ` don't confuse the split.
#[inline]
fn split_at_annotation(line: &str) -> (&str, &str) {
    // Find the rightmost "  #@ " separator using SIMD-accelerated memrchr for '#',
    // then verify the surrounding bytes.  Falls back leftward on false positives
    // (a bare '#' inside a string value).
    let b = line.as_bytes();
    let mut end = b.len();
    while let Some(p) = memrchr(b'#', &b[..end]) {
        if p >= 2
            && b[p - 1] == b' '
            && b[p - 2] == b' '
            && p + 2 < b.len()
            && b[p + 1] == b'@'
            && b[p + 2] == b' '
        {
            // "  #@ " confirmed: field part ends at p-2, annotation starts at p+3
            return (&line[..p - 2], &line[p + 3..]);
        }
        // Also recognize a line whose non-whitespace content starts with "#@ "
        // (comment-only annotation line, no value token before it).
        if b[..p].iter().all(|c| *c == b' ' || *c == b'\t')
            && p + 2 < b.len()
            && b[p + 1] == b'@'
            && b[p + 2] == b' '
        {
            return ("", &line[p + 3..]);
        }
        end = p; // keep searching leftward
    }
    (line, "")
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Decode a textual prototext byte string directly to binary wire bytes.
///
/// Input must start with `b"#@ prototext:"`.
/// The line-by-line format must have been produced with `include_annotations=true`
/// (the annotation comment on each line is required to reconstruct field numbers
/// and types when field names are used on the LHS).
///
/// Implements Proposal F — Strategy C2 for MESSAGE frames.
pub fn encode_text_to_binary(text: &[u8]) -> Vec<u8> {
    let capacity = (text.len() / 6).max(64);
    let mut out = Vec::with_capacity(capacity);

    let mut stack: Vec<Frame> = Vec::new();
    let mut first_placeholder: Option<usize> = None;
    let mut last_placeholder: Option<usize> = None;

    // ── Per-line packed state ─────────────────────────────────────────────────
    // When non-None, we are buffering elements for a per-line packed record.
    // `packed_field_number`: the field number of the active record.
    // `packed_tag_ohb`: tag overhang for the record's wire tag.
    // `packed_len_ohb`: length overhang for the record's LEN prefix.
    // `packed_remaining`: how many more element lines to consume.
    // `packed_payload`: accumulated payload bytes.
    let mut packed_field_number: u64 = 0;
    let mut packed_tag_ohb: Option<u64> = None;
    let mut packed_len_ohb: Option<u64> = None;
    let mut packed_remaining: usize = 0;
    let mut packed_payload: Vec<u8> = Vec::new();

    // The text is always valid ASCII (a subset of UTF-8).
    let text_str = match std::str::from_utf8(text) {
        Ok(s) => s,
        Err(_) => return out,
    };

    let mut lines = text_str.lines();

    // Skip the first line: "#@ prototext: protoc"
    lines.next();

    for line in lines {
        let line = line.trim_end(); // strip trailing CR/spaces

        if line.is_empty() {
            continue;
        }

        // ── Close brace ───────────────────────────────────────────────────────
        //
        // Brace-folding may place multiple `}` on one line, separated by spaces
        // (e.g. `}}` for indent_size=1, `} } }` for indent_size=2).  A close-
        // brace line consists solely of `}` and space characters after the
        // leading indentation.  Walk the trimmed line byte-by-byte and pop the
        // stack once per `}` found.

        let trimmed = line.trim_start();
        if !trimmed.is_empty() && trimmed.bytes().all(|b| b == b'}' || b == b' ') {
            for b in trimmed.bytes() {
                if b == b'}' {
                    match stack.pop() {
                        Some(Frame::Message {
                            placeholder_start,
                            ohb,
                            content_start,
                            acw,
                        }) => {
                            let total_waste = fill_placeholder(
                                &mut out,
                                placeholder_start,
                                ohb,
                                content_start,
                                acw,
                            );
                            // Propagate waste to parent frame.
                            if let Some(parent) = stack.last_mut() {
                                *parent.acw_mut() += total_waste;
                            }
                        }
                        Some(Frame::Group {
                            field_number,
                            open_ended,
                            mismatched_end,
                            end_tag_ohb,
                            acw,
                        }) => {
                            if !open_ended {
                                let end_fn = mismatched_end.unwrap_or(field_number);
                                write_tag_ohb_local(end_fn, WT_END_GROUP, end_tag_ohb, &mut out);
                            }
                            // Propagate accumulated waste from inner MESSAGE placeholders.
                            if acw > 0 {
                                if let Some(parent) = stack.last_mut() {
                                    *parent.acw_mut() += acw;
                                }
                            }
                        }
                        None => { /* unmatched `}` — ignore */ }
                    }
                }
            }
            continue;
        }

        // Skip plain comments (`# ...`) that carry no wire semantics.
        // Only `#@ ...` lines (handled via split_at_annotation) have meaning.
        if trimmed.starts_with('#') && !trimmed.starts_with("#@") {
            continue;
        }

        // Split value part from annotation.
        let (value_part, ann_str) = split_at_annotation(line);

        // ── Open brace ────────────────────────────────────────────────────────

        // Detect `name {` (possibly indented, before the annotation).
        let vp_trimmed = value_part.trim_end();
        let is_open_brace = vp_trimmed.ends_with(" {") || vp_trimmed == "{";

        if is_open_brace {
            let ann = parse_annotation(ann_str);

            // Extract the field name (LHS of `name {`).
            let lhs = vp_trimmed.trim_start().trim_end_matches('{').trim_end();

            let field_number = extract_field_number(lhs, &ann);
            let tag_ohb = ann.tag_overhang_count;

            if ann.wire_type == "group" {
                write_tag_ohb_local(field_number, WT_START_GROUP, tag_ohb, &mut out);
                stack.push(Frame::Group {
                    field_number,
                    open_ended: ann.open_ended_group,
                    mismatched_end: ann.mismatched_group_end,
                    end_tag_ohb: ann.end_tag_overhang_count,
                    acw: 0,
                });
            } else {
                // MESSAGE (wire type BYTES or unspecified).
                write_tag_ohb_local(field_number, WT_LEN, tag_ohb, &mut out);
                let ohb = ann.length_overhang_count.unwrap_or(0) as usize;
                let (ph_start, content_start) =
                    write_placeholder(&mut out, ohb, &mut first_placeholder, &mut last_placeholder);
                stack.push(Frame::Message {
                    placeholder_start: ph_start,
                    ohb,
                    content_start,
                    acw: 0,
                });
            }
            continue;
        }

        // ── Scalar field line ─────────────────────────────────────────────────

        // Detect a comment-only annotation line (no LHS colon, starts with `#@ `).
        // This is used for empty packed records: `pack_size: 0`.
        let trimmed_vp = value_part.trim();
        if trimmed_vp.is_empty() && !ann_str.is_empty() {
            // Comment-only line — parse annotation to handle pack_size: 0.
            let ann = parse_annotation(ann_str);
            if let Some(0) = ann.pack_size {
                // Empty packed record: emit tag + len=0.
                write_tag_ohb_local(
                    ann.field_number.unwrap_or(0),
                    WT_LEN,
                    ann.tag_overhang_count,
                    &mut out,
                );
                write_varint_ohb(0, ann.length_overhang_count, &mut out);
            }
            continue;
        }

        // Find the colon separating LHS from value.
        let Some(colon_pos) = value_part.find(':') else {
            continue;
        };
        let lhs = value_part[..colon_pos].trim_start(); // may be indented
        let value_str = value_part[colon_pos + 1..].trim();

        let ann = parse_annotation(ann_str);
        let field_number = extract_field_number(lhs, &ann);

        // ── Per-line packed: continuation element ─────────────────────────────
        if packed_remaining > 0 {
            encode_packed_elem(value_str, &ann, &mut packed_payload);
            packed_remaining -= 1;
            if packed_remaining == 0 {
                // Flush the completed wire record.
                write_tag_ohb_local(packed_field_number, WT_LEN, packed_tag_ohb, &mut out);
                write_varint_ohb(packed_payload.len() as u64, packed_len_ohb, &mut out);
                out.extend_from_slice(&packed_payload);
                packed_payload.clear();
            }
            continue;
        }

        // ── Per-line packed: first element (pack_size: N) ─────────────────────
        if ann.is_packed {
            if let Some(n) = ann.pack_size {
                if n == 0 {
                    // Empty record — emit immediately.
                    write_tag_ohb_local(field_number, WT_LEN, ann.tag_overhang_count, &mut out);
                    write_varint_ohb(0, ann.length_overhang_count, &mut out);
                } else {
                    // Start buffering.
                    packed_field_number = field_number;
                    packed_tag_ohb = ann.tag_overhang_count;
                    packed_len_ohb = ann.length_overhang_count;
                    packed_remaining = n - 1; // this line is element 0
                    packed_payload.clear();
                    encode_packed_elem(value_str, &ann, &mut packed_payload);
                    if packed_remaining == 0 {
                        // Single-element record — flush immediately.
                        write_tag_ohb_local(packed_field_number, WT_LEN, packed_tag_ohb, &mut out);
                        write_varint_ohb(packed_payload.len() as u64, packed_len_ohb, &mut out);
                        out.extend_from_slice(&packed_payload);
                        packed_payload.clear();
                    }
                }
                continue;
            }
        }

        encode_scalar_line(field_number, value_str, &ann, &mut out);
    }

    // ── Forward compaction pass ───────────────────────────────────────────────

    if let Some(first_ph) = first_placeholder {
        compact(&mut out, first_ph);
    }

    // Development instrumentation — size ratio
    #[cfg(debug_assertions)]
    {
        let ratio = out.len() as f64 / text.len().max(1) as f64;
        eprintln!(
            "[encode_text] input_len={} output_len={} ratio={:.2}",
            text.len(),
            out.len(),
            ratio
        );
    }

    out
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_at_annotation ───────────────────────────────────────────────────

    #[test]
    fn split_bare() {
        let (field, ann) = split_at_annotation("name: 42");
        assert_eq!(field, "name: 42");
        assert_eq!(ann, "");
    }

    #[test]
    fn split_hash_at_space() {
        let (field, ann) = split_at_annotation("name: 42  #@ varint = 1");
        assert_eq!(field, "name: 42");
        assert_eq!(ann, "varint = 1");
    }

    #[test]
    fn split_hash_only() {
        // Bare '#' without '@': not a separator.
        let (field, ann) = split_at_annotation("name: 42  #");
        assert_eq!(field, "name: 42  #");
        assert_eq!(ann, "");
    }

    #[test]
    fn split_hash_at_end() {
        // "#@" at end with no space after '@': not a separator.
        let (field, ann) = split_at_annotation("name: 42  #@");
        assert_eq!(field, "name: 42  #@");
        assert_eq!(ann, "");
    }

    #[test]
    fn split_hash_at_no_space() {
        // "#@x" — '@' not followed by space: not a separator.
        let (field, ann) = split_at_annotation("name: 42  #@x");
        assert_eq!(field, "name: 42  #@x");
        assert_eq!(ann, "");
    }

    // ── parse_field_decl_into — enum suffix forms ─────────────────────────────

    fn make_ann() -> Ann<'static> {
        Ann {
            wire_type: "",
            field_type: "",
            field_number: None,
            is_packed: false,
            tag_overhang_count: None,
            value_overhang_count: None,
            length_overhang_count: None,
            missing_bytes_count: None,
            mismatched_group_end: None,
            open_ended_group: false,
            end_tag_overhang_count: None,
            records_overhung_count: vec![],
            neg_int32_truncated: false,
            records_neg_int32_truncated: vec![],
            enum_scalar_value: None,
            enum_packed_values: vec![],
            nan_bits: None,
            pack_size: None,
            elem_ohb: None,
            elem_neg_trunc: false,
        }
    }

    #[test]
    fn parse_scalar_enum() {
        let mut ann = make_ann();
        parse_field_decl_into("Type(9) = 5", &mut ann);
        assert_eq!(ann.field_type, "enum");
        assert_eq!(ann.enum_scalar_value, Some(9));
        assert_eq!(ann.field_number, Some(5));
    }

    #[test]
    fn parse_scalar_enum_neg() {
        let mut ann = make_ann();
        parse_field_decl_into("Color(-1) = 3", &mut ann);
        assert_eq!(ann.field_type, "enum");
        assert_eq!(ann.enum_scalar_value, Some(-1));
        assert_eq!(ann.field_number, Some(3));
    }

    #[test]
    fn parse_packed_enum() {
        let mut ann = make_ann();
        parse_field_decl_into("Label([1, 2, 3]) [packed=true] = 4", &mut ann);
        assert_eq!(ann.field_type, "enum");
        assert!(ann.is_packed);
        assert_eq!(ann.enum_packed_values, vec![1, 2, 3]);
        assert_eq!(ann.field_number, Some(4));
    }

    #[test]
    fn parse_primitive_int32() {
        let mut ann = make_ann();
        parse_field_decl_into("int32 = 25", &mut ann);
        assert_eq!(ann.field_type, "int32");
        assert_eq!(ann.field_number, Some(25));
        assert_eq!(ann.enum_scalar_value, None);
    }

    #[test]
    fn parse_enum_named_float() {
        // Latent-bug regression (spec 0004 §5.1): an enum whose type name
        // collides with the 'float' primitive must route to varint, not fixed32.
        let mut ann = make_ann();
        parse_field_decl_into("float(1) = 1", &mut ann);
        assert_eq!(
            ann.field_type, "enum",
            "enum named 'float' must set field_type='enum', not 'float'"
        );
        assert_eq!(ann.enum_scalar_value, Some(1));
    }

    // ── ENUM_UNKNOWN silencing ────────────────────────────────────────────────

    #[test]
    fn enum_unknown_encodes_correctly() {
        // A field annotated with ENUM_UNKNOWN must encode the varint from the
        // annotation's EnumType(N) suffix, not fail or produce wrong bytes.
        // Field 1, value 99 → tag 0x08 (field=1, wire=varint), varint 0x63.
        let input = b"#@ prototext: protoc\nkind: 99  #@ Type(99) = 1; ENUM_UNKNOWN\n";
        let wire = encode_text_to_binary(input);
        assert_eq!(
            wire,
            vec![0x08, 0x63],
            "ENUM_UNKNOWN field 1 value 99: expected [0x08, 0x63]"
        );
    }
}
