// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0226 — `grpconf/anomalies.pb` covers the whole annotation vocabulary.
//!
//! The fixture is `#@` prototext text committed under a `.pb` name: both tools
//! detect the format from the first thirteen bytes, so there is no build step
//! and no derived artifact to go stale.

use prototext_core::{parse_schema, render_as_bytes, render_as_text, RenderOpts};
use std::collections::BTreeSet;

const FIXTURE: &str = include_str!("../../grpconf/anomalies.pb");
const DESCRIPTOR: &[u8] = include_bytes!("../fixtures/descriptor.pb");
const ROOT_TYPE: &str = "google.protobuf.FileDescriptorProto";

/// The tokens the renderer can emit, per spec 0226 S2.
const VOCABULARY: &[&str] = &[
    // Wire types (no tier)
    "varint",
    "fixed64",
    "fixed32",
    "bytes",
    "group",
    // Landmark
    "pack_size",
    // Non-canonical
    "tag_ohb",
    "val_ohb",
    "len_ohb",
    "etag_ohb",
    "ohb",
    "nan_bits",
    "truncated_neg",
    "neg",
    "ENUM_UNKNOWN",
    // Invalid
    "TAG_OOR",
    "ETAG_OOR",
    "TYPE_MISMATCH",
    "MISSING",
    "END_MISMATCH",
    "OPEN_GROUP",
    "INVALID_TAG_TYPE",
    "INVALID_VARINT",
    "INVALID_FIXED64",
    "INVALID_FIXED32",
    "INVALID_LEN",
    "INVALID_GROUP_END",
    "TRUNCATED_BYTES",
    "INVALID_PACKED_RECORDS",
    "INVALID_STRING",
];

fn annotated_opts() -> RenderOpts {
    RenderOpts {
        include_annotations: true,
        ..RenderOpts::default()
    }
}

/// Encode the fixture, then re-render it against the root type.
fn encode_and_render() -> (Vec<u8>, String) {
    let schema = parse_schema(DESCRIPTOR, ROOT_TYPE).expect("descriptor.pb declares the root type");
    let root = schema.root_descriptor();
    let binary = render_as_bytes(FIXTURE.as_bytes(), RenderOpts::default())
        .expect("the fixture encodes")
        .into_owned();
    let text = render_as_text(&binary, root.as_ref(), annotated_opts()).expect("the bytes render");
    (
        binary,
        String::from_utf8(text).expect("the rendering is UTF-8"),
    )
}

/// Collect the annotation keywords of a rendering.
///
/// A field declaration (`repeated int32 [packed=true] = 1`) carries an `=` and
/// is not part of the vocabulary; a modifier is either `keyword: value` or a
/// bare keyword.
fn keywords(text: &str) -> BTreeSet<&str> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        let Some((_, annotation)) = line.split_once("  #@ ") else {
            continue;
        };
        for token in annotation.split(';') {
            let token = token.trim();
            if token.contains('=') {
                continue;
            }
            found.insert(token.split(':').next().unwrap_or(token).trim());
        }
    }
    found
}

#[test]
fn the_fixture_round_trips_byte_exact() {
    let (binary, text) = encode_and_render();
    let again = render_as_bytes(text.as_bytes(), RenderOpts::default())
        .expect("the rendering encodes")
        .into_owned();
    assert_eq!(
        binary, again,
        "re-encoding the fixture's own rendering changed the bytes"
    );
}

#[test]
fn the_fixture_covers_the_whole_vocabulary() {
    let (_, text) = encode_and_render();
    let found = keywords(&text);
    let expected: BTreeSet<&str> = VOCABULARY.iter().copied().collect();

    let missing: Vec<_> = expected.difference(&found).collect();
    let extra: Vec<_> = found.difference(&expected).collect();
    assert!(
        missing.is_empty(),
        "the fixture no longer exhibits: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "the renderer emitted tokens absent from spec 0226 S2: {extra:?}"
    );
}

#[test]
fn a_plain_comment_does_not_reach_the_wire() {
    let with_comments = "#@ prototext: protoc\n\
         # A whole-line comment explaining the next field.\n\
         seconds: 5  #@ varint; int64 = 1\n\
         \x20 # An indented whole-line comment.\n\
         nanos: 7  #@ varint; int32 = 2\n";
    let binary = render_as_bytes(with_comments.as_bytes(), RenderOpts::default())
        .expect("the document encodes");
    assert_eq!(binary.as_ref(), &[0x08, 0x05, 0x10, 0x07]);
}
