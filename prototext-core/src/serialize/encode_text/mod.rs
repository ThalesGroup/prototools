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
use placeholder::{compact, fill_placeholder, write_placeholder, BASE_OVERHEAD};

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

/// Locate a line's trailing `  #@ ...` annotation, as
/// `(end of the value part, start of the annotation's own text)`.
///
/// The separator is `  #@ ` (2 spaces + `#` + `@` + space).  We scan right-to-left
/// so that quoted string values containing `  #@ ` don't confuse the split.
///
/// Two branches, and both matter to callers:
///
/// 1. a confirmed `  #@ ` separator — the value part ends at `p - 2`, i.e.
///    *before* the two separator spaces;
/// 2. a line whose content before the `#` is entirely whitespace — the whole
///    line is annotation, its leading indentation included, so the value part
///    is empty.
///
/// Branch 1 is tried first and is the wider net: a comment-only line indented
/// by two or more spaces satisfies it too, and yields a value part of that
/// indentation *less its last two columns* rather than an empty one. Branch 2
/// therefore only ever fires on a comment-only line with under two columns of
/// leading whitespace (or with a tab in them) — in this renderer's output,
/// exactly the un-indented `#@ prototext:` header. An empty packed record,
/// though also comment-only, is indented, so it goes down branch 1.
#[inline]
fn annotation_bounds(line: &str) -> Option<(usize, usize)> {
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
            return Some((p - 2, p + 3));
        }
        if b[..p].iter().all(|c| *c == b' ' || *c == b'\t')
            && p + 2 < b.len()
            && b[p + 1] == b'@'
            && b[p + 2] == b' '
        {
            return Some((0, p + 3));
        }
        end = p; // keep searching leftward
    }
    None
}

/// Split a line into `(value_part, annotation_str)`.
#[inline]
fn split_at_annotation(line: &str) -> (&str, &str) {
    match annotation_bounds(line) {
        Some((value_end, ann_start)) => (&line[..value_end], &line[ann_start..]),
        None => (line, ""),
    }
}

/// Byte offset in `line` at which its trailing `  #@ ...` annotation starts,
/// or `None` if it has none.
///
/// The inverse view of `split_at_annotation`'s first return value, exposed for
/// callers that need to *hide* an annotation rather than parse it: `&line[..
/// annotation_start(line)?]` is exactly the value part `split_at_annotation`
/// would hand the encoder. The returned offset therefore **excludes the two
/// separator spaces** — the caller needs no `trim_end` of its own — and it is
/// `Some(0)` for an un-indented comment-only line, whose whole content is
/// annotation (branch 2 above). An *indented* comment-only line goes down
/// branch 1 instead and keeps its indentation less two columns; see
/// `annotation_bounds`.
#[inline]
pub fn annotation_start(line: &str) -> Option<usize> {
    annotation_bounds(line).map(|(value_end, _)| value_end)
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
    let mut out = Vec::with_capacity(encoded_capacity(text));
    encode_text_to_binary_into(text, &mut out);
    out
}

/// An upper bound on the buffer `text` needs, for one reservation.
///
/// Two terms, because the buffer is transiently larger than the output.
///
/// The output itself never exceeds `text.len()`: every wire byte costs at
/// least one character of the annotated form, and most cost several — a
/// scalar spends a name, a value and a `#@` annotation to produce one or
/// two bytes, and even a raw byte inside a string literal is a character
/// of its own. Six is the ratio a realistic document shows.
///
/// On top of that each `{` opens a MESSAGE placeholder of [`BASE_OVERHEAD`]
/// bytes that lives in the buffer until [`compact`] removes it, and a
/// minimal `a {  #@ 1` line is about that long — so the placeholders are
/// the one term the output bound does not already cover. Counting every
/// `{`, including those inside string literals, over-counts, which is what
/// an upper bound wants.
///
/// The bound holds unless a line explicitly asks for over-long varints
/// (`len_ohb=`), which buys arbitrarily many placeholder bytes for a few
/// characters. That is a deliberate pathology, and being wrong there costs
/// only the [`Vec`] growth it would otherwise have taken anyway.
///
/// Over-reserving is close to free: an allocation this size is served by
/// `mmap`, and the pages past the encode's own high-water mark are never
/// touched, so they never become resident.
fn encoded_capacity(text: &[u8]) -> usize {
    let messages = text.iter().filter(|&&b| b == b'{').count();
    (text.len() + BASE_OVERHEAD * messages).max(64)
}

/// `encode_text_to_binary`, appending to `out` instead of returning a fresh
/// `Vec` (spec 0216 S28).
///
/// The caller that wants this is one building a larger buffer around the
/// wire bytes — protolens reserves a wrapper-prefix headroom and encodes
/// straight into the tail — for which the owned-`Vec` form costs a copy of
/// the whole payload for nothing.
///
/// Such a caller need not reserve: whatever `out` already holds is kept,
/// and the reservation made here is [`encoded_capacity`], which is a bound
/// rather than a guess precisely so that the append cannot turn into a
/// reallocation and undo the copy it was meant to save.
pub fn encode_text_to_binary_into(text: &[u8], out: &mut Vec<u8>) {
    let base = out.len();
    out.reserve(encoded_capacity(text));

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

    // Rendered prototext is ASCII, so this holds for anything this crate
    // produced. It is not a guarantee about the argument, though, and
    // this signature has nowhere to report a violation: a non-UTF-8
    // `text` appends nothing and is indistinguishable from an empty
    // document. Validating is therefore the caller's job — protolens'
    // `Blob::load` does it and names the offending byte offset.
    let text_str = match std::str::from_utf8(text) {
        Ok(s) => s,
        Err(_) => return,
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
                            let total_waste =
                                fill_placeholder(out, placeholder_start, ohb, content_start, acw);
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
                                write_tag_ohb_local(end_fn, WT_END_GROUP, end_tag_ohb, out);
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
                write_tag_ohb_local(field_number, WT_START_GROUP, tag_ohb, out);
                stack.push(Frame::Group {
                    field_number,
                    open_ended: ann.open_ended_group,
                    mismatched_end: ann.mismatched_group_end,
                    end_tag_ohb: ann.end_tag_overhang_count,
                    acw: 0,
                });
            } else {
                // MESSAGE (wire type BYTES or unspecified).
                write_tag_ohb_local(field_number, WT_LEN, tag_ohb, out);
                let ohb = ann.length_overhang_count.unwrap_or(0) as usize;
                let (ph_start, content_start) =
                    write_placeholder(out, ohb, &mut first_placeholder, &mut last_placeholder);
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
                    out,
                );
                write_varint_ohb(0, ann.length_overhang_count, out);
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
                write_tag_ohb_local(packed_field_number, WT_LEN, packed_tag_ohb, out);
                write_varint_ohb(packed_payload.len() as u64, packed_len_ohb, out);
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
                    write_tag_ohb_local(field_number, WT_LEN, ann.tag_overhang_count, out);
                    write_varint_ohb(0, ann.length_overhang_count, out);
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
                        write_tag_ohb_local(packed_field_number, WT_LEN, packed_tag_ohb, out);
                        write_varint_ohb(packed_payload.len() as u64, packed_len_ohb, out);
                        out.extend_from_slice(&packed_payload);
                        packed_payload.clear();
                    }
                }
                continue;
            }
        }

        encode_scalar_line(field_number, value_str, &ann, out);
    }

    // ── Forward compaction pass ───────────────────────────────────────────────

    if let Some(first_ph) = first_placeholder {
        compact(out, first_ph, base);
    }

    // Development instrumentation — size ratio
    #[cfg(debug_assertions)]
    {
        let written = out.len() - base;
        let ratio = written as f64 / text.len().max(1) as f64;
        eprintln!(
            "[encode_text] input_len={} output_len={} ratio={:.2}",
            text.len(),
            written,
            ratio
        );
    }
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

    // ── annotation_start ──────────────────────────────────────────────────────

    /// `annotation_start` is the same rule as `split_at_annotation`, read
    /// from the other end: truncating there must reproduce the value part
    /// exactly, separator spaces included, with no `trim_end` of the
    /// caller's own.
    #[test]
    fn annotation_start_agrees_with_split_at_annotation() {
        for line in [
            "name: 42",
            "name: 42  #@ varint = 1",
            "  name: \"a  #@ b\"  #@ string = 2",
            "  name: \"a  #@ b\"",
            "  }  #@ close",
            "#@ prototext: protoc",
            "    #@ packed = 7",
            "name: 42  #",
            "name: 42  #@",
            "name: 42  #@x",
        ] {
            let (value, _) = split_at_annotation(line);
            let truncated = match annotation_start(line) {
                Some(p) => &line[..p],
                None => line,
            };
            assert_eq!(truncated, value, "line {line:?}");
        }
    }

    /// Branch 2: an un-indented comment-only line is annotation all the
    /// way to column zero, so hiding annotations leaves nothing at all.
    /// This is the `#@ prototext:` header's case.
    #[test]
    fn annotation_start_is_zero_for_an_unindented_comment_only_line() {
        assert_eq!(annotation_start("#@ prototext: protoc"), Some(0));
        assert_eq!(annotation_start(" #@ packed = 7"), Some(0));
        assert_eq!(annotation_start("\t#@ packed = 7"), Some(0));
    }

    /// ...but branch 1 is tried first and claims any comment-only line
    /// indented by two or more spaces, leaving that indentation *less
    /// two columns* as the value part. Pinned because it is a genuine
    /// asymmetry between two lines that look alike, and because
    /// protolens's annotation-hiding display must reproduce whatever
    /// the encoder's own split does, not a tidier rule of its own.
    #[test]
    fn an_indented_comment_only_line_keeps_its_indentation_less_two() {
        assert_eq!(annotation_start("    #@ packed = 7"), Some(2));
        assert_eq!(split_at_annotation("    #@ packed = 7").0, "  ");
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

    // ── Appending into a caller's buffer (spec 0216 S28) ──────────────────────

    /// The appending flavor must not care what is already in the buffer.
    /// The one thing that could go wrong is the placeholder machinery, whose
    /// offsets are absolute: a nested message forces `write_placeholder`,
    /// `fill_placeholder` and the `compact` sweep all to run, and compaction
    /// is where an assumption that the encode owns the whole buffer would
    /// eat the caller's prefix.
    #[test]
    fn encoding_into_a_buffer_that_is_not_empty_appends() {
        let input = b"#@ prototext: protoc\nouter {  #@ 1\n  inner {  #@ 2\n    n: 7  #@ int32 = 3\n  }\n}\n";
        let standalone = encode_text_to_binary(input);
        assert!(
            standalone.len() > 4,
            "the fixture must nest, or this proves nothing"
        );

        let prefix: Vec<u8> = (0..11u8).collect();
        let mut out = prefix.clone();
        encode_text_to_binary_into(input, &mut out);

        assert_eq!(&out[..prefix.len()], &prefix[..], "the prefix must survive");
        assert_eq!(&out[prefix.len()..], &standalone[..]);
    }

    /// `encoded_capacity` claims to be a bound, not a guess, and the whole
    /// point of appending into a caller's buffer is to save a copy — which
    /// a reallocation mid-encode would hand straight back. So: reserve
    /// exactly the claim, and require the capacity to be untouched
    /// afterwards.
    ///
    /// The interesting input is not the biggest but the most
    /// message-dense, because the term the output bound does not cover is
    /// the placeholder each `{` transiently costs. `a{` is about as short
    /// as a message-open line can be made, so this is close to the worst
    /// ratio the format admits.
    #[test]
    fn the_reservation_is_never_outgrown() {
        let mut dense = b"#@ prototext: protoc\n".to_vec();
        for i in 0..500 {
            dense.extend_from_slice(format!("a {{  #@ {}\n}}\n", i % 100 + 1).as_bytes());
        }
        let fixture = include_bytes!("../../../fixtures/descriptor_protoc.txt");

        for (name, text) in [("message-dense", &dense[..]), ("descriptor", &fixture[..])] {
            let claimed = encoded_capacity(text);
            let mut out = Vec::with_capacity(claimed);
            let before = out.capacity();
            encode_text_to_binary_into(text, &mut out);
            assert_eq!(
                out.capacity(),
                before,
                "{name}: the encode outgrew a reservation of {claimed} bytes, \
                 reaching {} for an output of {}",
                out.capacity(),
                out.len(),
            );
        }
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
