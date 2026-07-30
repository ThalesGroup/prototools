// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Extract the node under the cursor to a file — spec 0111 Goal 4, Open
//! Issue 2. Two formats: raw binary (`NodeSpan::raw_range` byte slice) or
//! `#@ prototext` text (`NodeSpan::text_range` rendered lines, dedented so
//! the extracted snippet is self-contained regardless of its on-screen
//! nesting depth — same transform as spec 0111's "Copy with relative
//! indentation" — plus the `#@ prototext: protoc` header line, so the file
//! is directly re-decodable by `prototext_core::render_as_bytes`/
//! `encode_text_to_binary`, which unconditionally skip the first line
//! expecting it to be that header).
//!
//! Deliberately ignores fold state entirely (0113 D11): always slices the
//! full underlying `blob`/`lines`, never the currently-folded on-screen
//! view.

use std::io;
use std::ops::Range;
use std::path::Path;

use prototext_core::helpers::{
    parse_varint, parse_wiretag, write_tag, WT_END_GROUP, WT_LEN, WT_START_GROUP,
};

use crate::decode::TreeNode;

/// Same header line `prototext_core::serialize::render_text` writes for a
/// full-document render (`emit_header`) — not exposed as a public constant
/// there, so duplicated here. Required for the extracted text to be
/// recognized by `is_prototext_text`/round-tripped by
/// `encode_text_to_binary` (which unconditionally discards the first line).
const PROTOTEXT_HEADER: &str = "#@ prototext: protoc\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractFormat {
    Binary,
    Text,
}

/// Raw byte sub-slice of `blob` for `node`'s own encoded value.
///
/// For a *message* node (`is_message`: `NodeSpan::is_message` — a
/// nested message/group, not a scalar field), the leading wire tag (and,
/// for length-delimited fields, the length-prefix varint) is stripped, so
/// the result is that message's raw payload on its own — directly
/// decodable by a fresh top-level `--type <MessageType>` decode.
/// Extracting a *message*, not a *field*: without this, a fresh decode of
/// the full `tag+length+payload` span would misinterpret the original
/// field's own tag as some unrelated field of the extracted message's own
/// type (reported crash — spec 0113 bug report).
///
/// For a scalar leaf field (`is_message: false`), `range` is returned
/// unchanged (tag, length prefix if any, and value all included) — that's
/// simply the field's own wire-format bytes, not a standalone message.
pub fn extract_binary<'a>(blob: &'a [u8], range: &Range<usize>, is_message: bool) -> &'a [u8] {
    if !is_message {
        return &blob[range.clone()];
    }
    &blob[message_payload_range(blob, range, None)]
}

/// For any field's full `tag[+length]+payload` span, return just the
/// `payload` sub-range — generic over wire type, not node kind: a
/// length-delimited field (`WT_LEN` — messages, groups, strings, bytes,
/// packed-repeated scalars) has both tag and length stripped; a group
/// (`WT_START_GROUP`) has both its leading START_GROUP tag and its
/// trailing END_GROUP tag stripped (the latter fixed post-spec-0120: see
/// below); any other wire type (varint, fixed32, fixed64) has only its
/// tag stripped. `pub(crate)`: also reused by `tui.rs`'s payload-only
/// range display for every node, message/group or scalar alike (spec
/// 0114 §1.1, extended).
///
/// `packed_record_start` is `NodeSpan::packed_record_start` (spec 0115
/// §3): `Some` means `range` is one element of a packed-repeated field —
/// it has no wire tag of its own (it's a bare value inside the record's
/// shared LEN payload), so `range` is already the payload and is returned
/// unstripped; stripping it would misparse the element's own first byte
/// as a fake tag.
///
/// The trailing END_GROUP tag is stripped by re-encoding it from the
/// leading tag's own field number (`write_tag(field_number, WT_END_GROUP,
/// ..)`) rather than parsing backward from `range.end` (varints can't be
/// parsed right-to-left) — correct because a group's END_GROUP tag always
/// shares the *same* field number as its own START_GROUP tag (spec 0120
/// bugfix: previously left in place, corrupting a fresh top-level
/// re-decode of the payload — e.g. `splice_override`'s wrap-and-redecode
/// trick — since wire type 4 is invalid at top level, producing a
/// spurious `INVALID_GROUP_END` entry).
pub(crate) fn message_payload_range(
    blob: &[u8],
    range: &Range<usize>,
    packed_record_start: Option<usize>,
) -> Range<usize> {
    if packed_record_start.is_some() {
        return range.clone();
    }
    let tag = parse_wiretag(blob, range.start);
    let Some(wtype) = tag.wtype else {
        return range.clone();
    };
    if wtype == WT_LEN {
        let len = parse_varint(blob, tag.next_pos);
        if len.varint.is_some() {
            return len.next_pos..range.end;
        }
    }
    if wtype == WT_START_GROUP {
        if let Some(field_number) = tag.wfield {
            let mut end_tag = Vec::new();
            write_tag(field_number as u32, WT_END_GROUP, &mut end_tag);
            let end = range.end.saturating_sub(end_tag.len()).max(tag.next_pos);
            return tag.next_pos..end;
        }
    }
    // Any other wire type: no length prefix — strip only the leading tag.
    tag.next_pos..range.end
}

/// For a message/group node's full `text_range` (its opening `field {`
/// line through its closing `}` line), return just the inner lines —
/// same "extract the message's own contents, not the field wrapping it"
/// rule as `extract_binary`'s tag/length stripping (spec 0113 bug
/// report): otherwise the extracted text still starts with the
/// original field's own `field {` line, which isn't valid standalone
/// prototext for that message's own type.
fn message_text_range(range: &Range<usize>) -> Range<usize> {
    if range.end.saturating_sub(range.start) < 2 {
        return range.clone();
    }
    (range.start + 1)..(range.end - 1)
}

/// Strip the minimum common leading-whitespace width across all non-blank
/// lines, so the least-indented line lands at column 0 while deeper lines
/// keep their relative indentation to each other — same transform as
/// Python's `textwrap.dedent()`.
pub fn dedent(lines: &[String]) -> String {
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                l.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render `node` (any tree node, not just the cursor's) from `blob`/
/// `lines` to a self-contained byte vector, in the requested format — the
/// same bytes `extract` writes to a file, factored out so a caller with
/// no `Path` to write to (spec 0123's batch mode, writing to stdout) can
/// reuse the exact same rendering.
///
/// Spec 0210 S1: `text_range` is passed in rather than read off
/// `node.span`, which holds the range the node had when the tree was
/// built and is not repaired by a splice. The tui derives it from the
/// line counters (`App::node_lines`).
pub fn extract_bytes(
    format: ExtractFormat,
    blob: &[u8],
    lines: &[String],
    node: &TreeNode,
    text_range: Range<usize>,
) -> Vec<u8> {
    match format {
        ExtractFormat::Binary => {
            let is_message = node.span.is_message;
            extract_binary(blob, &node.span.raw_range, is_message).to_vec()
        }
        ExtractFormat::Text => {
            let r = if node.span.is_message {
                message_text_range(&text_range)
            } else {
                text_range
            };
            let end = r.end.min(lines.len());
            format!("{PROTOTEXT_HEADER}{}", dedent(&lines[r.start..end])).into_bytes()
        }
    }
}

/// Extract `node` (the cursor's current node) from `blob`/`lines` to
/// `path`, in the requested format.
pub fn extract(
    path: &Path,
    format: ExtractFormat,
    blob: &[u8],
    lines: &[String],
    node: &TreeNode,
    text_range: Range<usize>,
) -> io::Result<()> {
    std::fs::write(path, extract_bytes(format, blob, lines, node, text_range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_binary_field_keeps_tag_and_length() {
        let blob = b"hello world";
        assert_eq!(extract_binary(blob, &(6..11), false), b"world");
    }

    #[test]
    fn extract_binary_message_strips_tag_and_length() {
        // Inner message: field 1 varint = 5 -> tag 0x08, value 0x05.
        let inner = [0x08u8, 0x05];
        // Outer wraps it as field 2 (LEN): tag (2<<3)|2 = 0x12, len 2.
        let blob = [0x12u8, 0x02, inner[0], inner[1]];
        assert_eq!(extract_binary(&blob, &(0..4), true), &inner);
    }

    #[test]
    fn extract_binary_message_with_no_length_prefix_strips_only_tag() {
        // A malformed/garbage tag (invalid wire type 7): falls back to the
        // full range unchanged, same as a scalar field.
        let blob = [0x07u8, 0xAB, 0xCD];
        assert_eq!(extract_binary(&blob, &(0..3), true), &blob[..]);
    }

    #[test]
    fn extract_binary_group_strips_leading_and_trailing_tags() {
        // Group field 5: START_GROUP tag (5<<3)|3 = 0x2B, inner varint
        // field 1 = 7 (tag 0x08, value 0x07), END_GROUP tag (5<<3)|4 =
        // 0x2C. Groups have no length prefix; both the leading
        // START_GROUP tag and the trailing END_GROUP tag are stripped
        // (spec 0120 bugfix — leaving the trailing tag in place corrupts
        // a fresh top-level re-decode of the extracted payload).
        let blob = [0x2Bu8, 0x08, 0x07, 0x2C];
        assert_eq!(extract_binary(&blob, &(0..4), true), &blob[1..3]);
    }

    #[test]
    fn dedent_strips_common_leading_whitespace() {
        let lines: Vec<String> = vec![
            "  message {".to_string(),
            "    name: \"x\"".to_string(),
            "  }".to_string(),
        ];
        assert_eq!(dedent(&lines), "message {\n  name: \"x\"\n}");
    }

    #[test]
    fn dedent_ignores_blank_lines_when_computing_minimum() {
        let lines: Vec<String> = vec!["  a".to_string(), "".to_string(), "    b".to_string()];
        assert_eq!(dedent(&lines), "a\n\n  b");
    }

    #[test]
    fn dedent_handles_already_flush_lines() {
        let lines: Vec<String> = vec!["a".to_string(), "  b".to_string()];
        assert_eq!(dedent(&lines), "a\n  b");
    }

    #[test]
    fn extract_binary_message_round_trips_through_a_fresh_decode() {
        // Regression test for the reported bug: extracting a nested
        // message's raw binary without stripping its tag+length header
        // corrupted a fresh top-level decode of that message (the leading
        // tag byte was misread as an unrelated field of the extracted
        // type). Builds a minimal synthetic `FileDescriptorSet` in-memory
        // (rather than reading a sibling crate's fixture file by relative
        // path, which isn't available in a sandboxed Nix build), decodes a
        // hand-crafted `Outer { inner: Inner { id: 5 } }` blob, extracts
        // the `Inner` submessage, and re-decodes it as a standalone
        // `test.Inner`.
        use prost::Message as _;
        use prost_types::field_descriptor_proto::{Label, Type};
        use prost_types::{
            DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
        };

        use crate::decode::{decode, DescriptorContext, RootType};

        let inner_desc = DescriptorProto {
            name: Some("Inner".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            }],
            ..Default::default()
        };
        let outer_desc = DescriptorProto {
            name: Some("Outer".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("inner".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Inner".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let file = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![outer_desc, inner_desc],
            syntax: Some("proto3".to_string()),
            ..Default::default()
        };
        let fds = FileDescriptorSet { file: vec![file] };

        let descriptor_path =
            std::env::temp_dir().join("protolens-extract-round-trip-descriptor.pb");
        std::fs::write(&descriptor_path, fds.encode_to_vec()).unwrap();
        let mut ctx = DescriptorContext::load(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();

        // Inner: field 1 varint 5 -> tag 0x08, value 0x05.
        let inner_bytes = [0x08u8, 0x05];
        // Outer wraps it as field 1 (LEN): tag (1<<3)|2 = 0x0A, len 2.
        let blob = [0x0Au8, 0x02, inner_bytes[0], inner_bytes[1]];

        let decoded = decode(&blob, &mut ctx, RootType::Named("test.Outer"), 2).unwrap();
        let inner_node = decoded
            .tree
            .iter()
            .find(|n| n.span.type_fqdn.as_deref() == Some("test.Inner"))
            .expect("decoded tree must contain the Inner submessage");

        // `raw_range` is relative to `decoded.blob` (the wrapped blob, spec
        // 0114 §1.1), not the local pre-wrap `blob` array.
        let extracted = extract_binary(&decoded.blob, &inner_node.span.raw_range, true);
        assert_eq!(extracted, &inner_bytes);

        // Not asserting `!reopened.tree.is_empty()` here: an extract can
        // legitimately decode to zero fields (e.g. extracting an all-default
        // submessage, or a real `google.protobuf.Empty`) — that's not a bug.
        // `extracted == inner_bytes` above already proves byte-for-byte
        // fidelity; this only additionally checks for the specific garbling
        // symptom reported (a field misread as an unrelated one).
        let reopened = decode(extracted, &mut ctx, RootType::Named("test.Inner"), 2).unwrap();
        assert!(
            !reopened.lines.join("\n").contains("INVALID_STRING"),
            "re-decoded Inner extract must not contain garbled fields: {:?}",
            reopened.lines
        );
    }

    #[test]
    fn extract_text_prepends_the_prototext_header() {
        use prototext_core::helpers::WT_LEN;
        use prototext_core::serialize::render_text::NodeSpan;

        let lines: Vec<String> = vec!["  options {".to_string(), "  }".to_string()];
        let node = TreeNode {
            span: NodeSpan {
                field_number: 8,
                raw_range: 0..2,
                text_range: 0..2,
                level: 1,
                type_fqdn: None,
                // Not exercising message-line stripping here — just header
                // prepending — so left scalar-shaped, same as before this
                // field existed.
                is_message: false,
                packed_record_start: None,
                wire_type: WT_LEN,
            },
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            doc_next: None,
            doc_prev: None,
            sibling_ordinal: 1,
            lines_total: 2,
            lines_visible: 2,
            rendered_as: None,
        };
        let path = std::env::temp_dir().join("protolens-extract-header-test.pb");
        extract(
            &path,
            ExtractFormat::Text,
            b"",
            &lines,
            &node,
            node.span.text_range.clone(),
        )
        .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written, "#@ prototext: protoc\noptions {\n}");
    }

    #[test]
    fn extract_text_message_strips_the_wrapping_field_and_brace_lines() {
        // Regression test for the reported bug: a message node's
        // `text_range` includes its own `options {` opening line and `}`
        // closing line — extracting it verbatim produced text still
        // wrapped in the original field, not standalone prototext for the
        // extracted message's own type.
        use prototext_core::helpers::WT_LEN;
        use prototext_core::serialize::render_text::NodeSpan;

        let lines: Vec<String> = vec![
            "options {  #@ FileOptions = 8".to_string(),
            "  java_package: \"x\"  #@ string = 1".to_string(),
            "}".to_string(),
        ];
        let node = TreeNode {
            span: NodeSpan {
                field_number: 8,
                raw_range: 0..2,
                text_range: 0..3,
                level: 0,
                type_fqdn: Some("google.protobuf.FileOptions".to_string()),
                is_message: true,
                packed_record_start: None,
                wire_type: WT_LEN,
            },
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            doc_next: None,
            doc_prev: None,
            sibling_ordinal: 1,
            lines_total: 3,
            lines_visible: 3,
            rendered_as: None,
        };
        let path = std::env::temp_dir().join("protolens-extract-message-text-test.pb");
        extract(
            &path,
            ExtractFormat::Text,
            b"",
            &lines,
            &node,
            node.span.text_range.clone(),
        )
        .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            written,
            "#@ prototext: protoc\njava_package: \"x\"  #@ string = 1"
        );
    }

    #[test]
    fn extract_text_group_strips_the_wrapping_field_and_brace_lines() {
        // Groups render with the same opening `... {` / closing `}` line
        // shape as messages (`Sink::begin_nested`/`end_nested`'s
        // `NestedKind::Group` branch), so `is_message` alone (regardless of
        // message vs. group) is enough to correctly trigger the same
        // wrapping-line stripping.
        use prototext_core::helpers::WT_START_GROUP;
        use prototext_core::serialize::render_text::NodeSpan;

        let lines: Vec<String> = vec![
            "MyGroup {  #@ group".to_string(),
            "  id: 7  #@ int32 = 1".to_string(),
            "}".to_string(),
        ];
        let node = TreeNode {
            span: NodeSpan {
                field_number: 5,
                raw_range: 0..4,
                text_range: 0..3,
                level: 0,
                type_fqdn: Some("pkg.MyGroup".to_string()),
                is_message: true,
                packed_record_start: None,
                wire_type: WT_START_GROUP,
            },
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            doc_next: None,
            doc_prev: None,
            sibling_ordinal: 1,
            lines_total: 3,
            lines_visible: 3,
            rendered_as: None,
        };
        let path = std::env::temp_dir().join("protolens-extract-group-text-test.pb");
        extract(
            &path,
            ExtractFormat::Text,
            b"",
            &lines,
            &node,
            node.span.text_range.clone(),
        )
        .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written, "#@ prototext: protoc\nid: 7  #@ int32 = 1");
    }
}
