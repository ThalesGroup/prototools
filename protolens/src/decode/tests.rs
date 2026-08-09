// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `decode`'s tests, in a file of their own because they were most of it:
//! inline, they were the larger half of a 2647-line module, and anyone
//! reading the production code had to page past them to reach it.
//!
//! Only the tests moved. The `#[cfg(test)]` helpers that sit among the
//! production items stayed there, `arena_gap` above all — it is called
//! from `render_resolved`, which is not test code.

use prost::Message as _;
use prost_reflect::prost_types::FileDescriptorSet;
use prototext_core::helpers::{write_tag, write_varint, WT_LEN};
use prototext_graph::build_scoring_graph::build_from_strings;

use super::*;
use crate::blob::wrapped;

/// `determine_root_type_meanwhile` with nothing to overlap and one
/// thread — the shape these tests care about, none of which is about
/// either.
fn determine_root_type(
    blob: &[u8],
    ctx: &mut DescriptorContext,
    root_type: RootType<'_>,
) -> Result<(Option<MessageDescriptor>, RankedCandidates), DecodeError> {
    determine_root_type_meanwhile(blob, ctx, root_type, 1, |_| ()).map(|(d, c, ())| (d, c))
}

#[test]
fn determine_root_type_returns_none_without_override_or_graph() {
    let mut ctx = DescriptorContext::empty_for_test();
    let blob = [0x08u8, 0x05];
    let (resolved, candidates) = determine_root_type(&blob, &mut ctx, RootType::Infer).unwrap();
    assert!(resolved.is_none());
    assert!(candidates.is_empty(), "no graph means no sweep ran");
}

/// A minimal real scoring graph, built in memory with no file I/O,
/// so the `Raw`/`Named` branches can be shown to short-circuit
/// *despite* a graph being available — which is the only
/// interesting case: with no graph they would return `None`
/// regardless and prove nothing.
fn one_entry_graph() -> LoadedGraph {
    let yaml = "entries:\n- Msg\nmessages:\n  Msg:\n    fields:\n    - number: 1\n      \
                type: uint64\n"
        .to_string();
    let (bytes, _, _) =
        build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
    LoadedGraph::from_static_bytes(Box::leak(bytes.into_boxed_slice()))
        .expect("test graph must load")
}

/// The three primitive-keyword lists — `primitive_type_for_keyword`'s
/// match arms, `primitive_keywords_for_wire_type`'s per-wire-type
/// slices, and `ALL_PRIMITIVE_KEYWORDS` — held against each other.
///
/// They are one list written out three times, and their own doc
/// comments say so ("Must stay in sync"), which until now was the
/// only thing enforcing it. Each drifts differently and none of them
/// noisily: a keyword missing from `ALL_PRIMITIVE_KEYWORDS` cannot
/// be picked from the override pane's alphabetic list but still
/// works when typed; one missing from
/// `primitive_keywords_for_wire_type` is refused as
/// wire-incompatible on a field it fits perfectly; one missing from
/// `primitive_type_for_keyword` is offered by both of the others and
/// then falls through to an FQDN lookup that cannot resolve it.
#[test]
fn the_three_primitive_keyword_lists_agree() {
    use prototext_core::helpers::{WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT};

    // Spec 0135 G4's count, pinned: a sixteenth keyword has to be
    // added in three places, and this is what says so.
    assert_eq!(ALL_PRIMITIVE_KEYWORDS.len(), 15);
    assert!(
        ALL_PRIMITIVE_KEYWORDS.windows(2).all(|w| w[0] < w[1]),
        "spec 0137 G1: the override pane presents this list as it \
         stands, so it must be sorted and free of duplicates",
    );
    for keyword in ALL_PRIMITIVE_KEYWORDS {
        assert!(
            primitive_type_for_keyword(keyword).is_some(),
            "{keyword} is offered but does not resolve",
        );
    }

    // Every keyword belongs to exactly one wire type — a keyword
    // under two of them would be accepted on a field it cannot
    // decode.
    let mut filed: Vec<&str> = Vec::new();
    for wire_type in [WT_VARINT, WT_I32, WT_I64, WT_LEN, WT_START_GROUP] {
        for keyword in primitive_keywords_for_wire_type(wire_type) {
            assert!(
                !filed.contains(keyword),
                "{keyword} is filed under two wire types",
            );
            filed.push(keyword);

            // The wire type each keyword *must* have, stated as
            // protobuf's own rule rather than as a copy of the table
            // above: the fixed-width types carry their width in
            // their name, `float`/`double` are the two that spell it
            // out in words, `string`/`bytes` are length-delimited,
            // and everything else is a varint.
            let want = match *keyword {
                "float" => WT_I32,
                "double" => WT_I64,
                "string" | "bytes" => WT_LEN,
                k if k.ends_with("32") && k.starts_with("fixed") => WT_I32,
                k if k.ends_with("32") && k.starts_with("sfixed") => WT_I32,
                k if k.ends_with("64") && k.starts_with("fixed") => WT_I64,
                k if k.ends_with("64") && k.starts_with("sfixed") => WT_I64,
                _ => WT_VARINT,
            };
            assert_eq!(want, wire_type, "{keyword} is under the wrong wire type");
        }
    }
    filed.sort_unstable();
    assert_eq!(
        filed, ALL_PRIMITIVE_KEYWORDS,
        "the wire-type table and the alphabetic list name different \
         keywords",
    );

    // Spec 0135 Background: group framing is never a primitive.
    assert!(primitive_keywords_for_wire_type(WT_START_GROUP).is_empty());
    // And `enum` is deliberately not a keyword anywhere yet (spec
    // 0135 Non-goals) — offering it would promise a target path that
    // does not exist.
    assert!(primitive_type_for_keyword("enum").is_none());
}

/// Spec 0168 (`--raw`): the sweep is skipped outright, not merely
/// ignored. Asserted via the empty candidate list — a sweep that
/// ran would have produced entries for the graph's one message —
/// since skipping it is the entire point of the flag on a large
/// descriptor pool.
#[test]
fn raw_never_sweeps_even_when_a_graph_is_loaded() {
    let mut ctx = DescriptorContext::for_test_with_graph(one_entry_graph());
    let blob = [0x08u8, 0x05];

    let (resolved, candidates) = determine_root_type(&blob, &mut ctx, RootType::Raw).unwrap();

    assert!(resolved.is_none());
    assert!(candidates.is_empty(), "--raw must not run the sweep");
}

/// Spec 0168 G6: `--type` is an O(1) pool lookup with no sweep. A
/// name the pool does not know is an error, not a quiet fallback to
/// inference — otherwise a typo would silently open the wrong type
/// and look like a scoring bug.
#[test]
fn a_named_type_is_looked_up_not_swept_and_a_bad_name_errors() {
    let mut ctx = DescriptorContext::for_test_with_graph(one_entry_graph());
    let blob = [0x08u8, 0x05];

    let err = determine_root_type(&blob, &mut ctx, RootType::Named("no.such.Type")).unwrap_err();

    assert!(
        matches!(err, DecodeError::Determination(_)),
        "unknown --type must error: {err:?}"
    );
}

/// The two explicit modes against one blob and one pool: `--type`
/// decodes under the named type, `--raw` decodes the same bytes
/// under none. Pins `--raw`'s actual guarantee — the render stays
/// raw — which is what distinguishes it from the deleted
/// `defer_root_type` flag, whose raw render was replaced under the
/// reader seconds later.
#[test]
fn named_types_the_root_and_raw_leaves_it_untyped() {
    let file = FileDescriptorProto {
        name: Some("test_root_type_modes.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![DescriptorProto {
            name: Some("Inner".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };
    let mut ctx = ctx_from_fds("root-type-modes", &fds);

    let blob = [0x08u8, 0x05];

    let named = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Inner"), 2).unwrap();
    assert_eq!(named.root_type, "test.Inner");
    assert!(named.root_candidates.is_empty(), "no sweep for --type");

    let raw = decode(wrapped(&blob), &mut ctx, RootType::Raw, 2).unwrap();
    assert_eq!(raw.root_type, "<raw / no type>");
    assert!(raw.root_candidates.is_empty(), "no sweep for --raw");
}

/// Spec 0143: the placeholder is anchored on its exact structural
/// position (immediately after indentation, followed by `": "` or
/// `" {"`), not found by a bare `_`-anywhere-in-the-line search.
#[test]
fn patch_synthetic_field_name_replaces_a_scalar_value_line_placeholder() {
    assert_eq!(
        patch_synthetic_field_name("_: 5", "id"),
        Some("id: 5".to_string())
    );
}

#[test]
fn patch_synthetic_field_name_replaces_a_message_header_placeholder() {
    assert_eq!(
        patch_synthetic_field_name("_ {", "inner"),
        Some("inner {".to_string())
    );
}

#[test]
fn patch_synthetic_field_name_preserves_leading_indentation() {
    assert_eq!(
        patch_synthetic_field_name("    _: 5", "id"),
        Some("    id: 5".to_string())
    );
}

/// A wire-type mismatch line never writes the placeholder, so the
/// line must come back untouched rather than have a field name
/// spliced into the middle of `TYPE_MISMATCH`.
#[test]
fn patch_synthetic_field_name_leaves_a_type_mismatch_line_untouched() {
    assert_eq!(
        patch_synthetic_field_name("2: 525005305  #@ varint; TYPE_MISMATCH", "type_id"),
        None
    );
}

#[test]
fn patch_synthetic_field_name_leaves_a_plain_numeric_key_line_untouched() {
    assert_eq!(patch_synthetic_field_name("5: 3  #@ int32 = 5", "id"), None);
}

/// Spec 0114: `--type` is optional — with no graph (autoinference
/// unavailable), `decode()` must not error but instead render the
/// blob with no known type. The virtual wrapper's own top-level node
/// (spec 0114 §1.1) still probes as message-shaped even with no
/// schema (spec 0097's unknown-LEN-field cascade), so it is the sole
/// node in the tree, with `type_fqdn: None` — the same representation
/// `apply_override(None)` would produce, i.e. this initial render
/// already stands in for "the first override of the session".
#[test]
fn decode_without_type_override_or_graph_renders_raw_not_error() {
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
    let file = FileDescriptorProto {
        name: Some("test_decode_raw_fallback.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![inner_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    let mut ctx = ctx_from_fds("decode-raw-fallback", &fds);

    // A single varint field (tag 0x08, value 5) — no --type, and this
    // context has no hopcroft.rkyv, so autoinference is unavailable.
    let blob = [0x08u8, 0x05];

    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Infer, 2).unwrap();
    assert_eq!(decoded.root_type, "<raw / no type>");
    // The wrapper's own top-level field (the "virtual encompassing
    // message", spec 0114 §1.1) — level 0, no type resolved.
    let wrapper = decoded
        .tree
        .iter()
        .find(|n| n.span.level == 0)
        .expect("tree must contain the wrapper's top-level node");
    assert!(wrapper.span.is_message);
    assert_eq!(wrapper.span.type_fqdn, NO_FQDN);
}

/// A blob holding all three of spec 0222 S1's ownership shapes at
/// once: a bracketed node (`Outer`, and `inner` inside it), a flat
/// one-line node (`id`), and a packed run (`xs`), whose three
/// elements share a single slot and so are the only case where a
/// node owns more than one line.
///
/// Shared because two specs want the same shapes for two reasons —
/// 0216 because a packed element is a display row inside a slot
/// rather than a slot of its own, 0222 because that is exactly what
/// makes its owner's text multi-line.
fn packed_and_nested_fixture(ctx_name: &str) -> (DescriptorContext, [u8; 10]) {
    let inner = DescriptorProto {
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
    let outer = DescriptorProto {
        name: Some("Outer".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("xs".to_string()),
                number: Some(1),
                label: Some(Label::Repeated as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("inner".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".test.Inner".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_arena_coverage.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![inner, outer],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    // field 1: packed [1, 2, 300]; field 2: Inner { id: 7 }.
    (
        ctx_from_fds(ctx_name, &fds),
        [0x0A, 0x04, 0x01, 0x02, 0xAC, 0x02, 0x12, 0x02, 0x08, 0x07],
    )
}

/// Spec 0216, test-plan item 1: the maximal tree is a superset of
/// every interpretation's tree.
///
/// The same bytes are decoded twice, once with the schema and once
/// with none, against a single arena — which is the claim in its
/// sharpest form, since the arena is built without either. The raw
/// pass renders the packed run's very same bytes as an opaque string
/// instead.
#[test]
fn the_arena_covers_every_interpretation() {
    let (mut ctx, blob) = packed_and_nested_fixture("arena-coverage");

    // `decode` runs `arena_gap` itself under `cfg(test)` and panics
    // on a gap, so what is left to assert here is only that the run
    // rendered something — a check over nothing passes vacuously.
    for root in [RootType::Named("test.Outer"), RootType::Raw] {
        let decoded = decode(wrapped(&blob), &mut ctx, root, 2).unwrap();
        let rendered = decoded.tree.iter().filter(|n| n.is_rendered()).count();
        assert!(
            rendered > 1,
            "{root:?}: nothing was rendered, so the check would prove nothing"
        );
    }
}

/// Spec 0222, test-plan items 1-3: every rendered node owns exactly
/// the lines it draws, a footer is its header's indent and a brace,
/// and the two together reassemble the document byte for byte.
///
/// The claims themselves are asserted inside `decode`, where the
/// render output is still in hand and where they therefore also cover
/// the corpus harness. What is left — and it is the part that decides
/// whether those assertions mean anything — is that this fixture
/// really presents all three of S1's shapes, since a split that
/// mishandled packed runs would pass vacuously on a document with
/// none.
#[test]
fn a_node_owns_the_lines_it_renders() {
    let (mut ctx, blob) = packed_and_nested_fixture("node-owns-its-lines");
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Outer"), 2).unwrap();

    let mut bracketed = 0;
    let mut flat_one_line = 0;
    let mut packed_run = 0;
    for (slot, node) in decoded.tree.iter().enumerate() {
        let Some(text) = decoded.node_text[slot].as_deref() else {
            assert!(
                !node.is_rendered(),
                "slot {slot} draws {} lines but owns no text",
                node.lines_total
            );
            continue;
        };
        let own = text.split('\n').count() as u32;
        if node.is_bracketed() {
            // The header only: the children's lines are the children's,
            // and the footer is derived rather than stored.
            assert_eq!(own, 1, "slot {slot}: a bracketed node keeps its header");
            assert!(
                node.lines_total >= 2,
                "slot {slot}: a bracketed node draws at least a header and a footer"
            );
            bracketed += 1;
        } else {
            assert_eq!(
                own, node.lines_total,
                "slot {slot}: a flat node owns every line it draws"
            );
            if own == 1 {
                flat_one_line += 1;
            } else {
                packed_run += 1;
            }
        }
    }
    assert!(
        bracketed >= 2,
        "the fixture must nest a message in a message"
    );
    assert!(flat_one_line >= 1, "the fixture must draw a lone scalar");
    assert_eq!(
        packed_run, 1,
        "the fixture must draw exactly one packed run"
    );

    // The reassembly the checks inside `decode` compared against, shown
    // here so a reader can see what shape it is.
    assert_eq!(
        decoded.document_lines().len(),
        decoded.total_lines,
        "the reassembled document is the rendered document"
    );
}

/// The same claim — and spec 0222's — against a blob nobody wrote for
/// them.
///
/// A fixture proves the property on shapes chosen to exercise it,
/// which is the weaker half of the argument: the arena has to hold
/// for whatever a reader opens, and so does the ownership split that
/// decides which node draws which line. `decode` checks both itself
/// under `cfg(test)`, so running it over a real corpus is all this
/// needs to do. It is `#[ignore]`d because the corpus is not in the
/// repository. Point it at one and run it explicitly:
///
/// ```text
/// PROTOLENS_CORPUS_BLOB=/path/to/googleapis.desc \
/// PROTOLENS_CORPUS_DESCRIPTOR=/path/to/googleapis.desc \
/// PROTOLENS_CORPUS_TYPE=google.protobuf.FileDescriptorSet \
///   cargo test --release -p protolens --bin protolens \
///     -- --ignored --nocapture the_arena_covers_a_real_corpus
/// ```
///
/// `--bin protolens` is not optional: without it cargo also runs the
/// integration target, where the filter matches nothing and the run
/// reports success having checked nothing.
///
/// Omit the last two and it checks the raw interpretation instead —
/// worth doing both, since they are two different renderings of one
/// arena.
#[test]
#[ignore = "needs a corpus blob in PROTOLENS_CORPUS_BLOB"]
fn the_arena_covers_a_real_corpus() {
    let blob_path = std::env::var("PROTOLENS_CORPUS_BLOB")
        .expect("set PROTOLENS_CORPUS_BLOB to the blob to check");
    let bytes = std::fs::read(&blob_path).expect("corpus blob is readable");

    let descriptor = std::env::var("PROTOLENS_CORPUS_DESCRIPTOR").ok();
    let type_name = std::env::var("PROTOLENS_CORPUS_TYPE").ok();
    let mut ctx = match &descriptor {
        Some(path) => DescriptorContext::load(Path::new(path)).expect("descriptor set loads"),
        None => DescriptorContext::empty_for_test(),
    };
    let root = match &type_name {
        Some(name) => RootType::Named(name),
        None => RootType::Raw,
    };

    let decoded = decode(wrapped(&bytes), &mut ctx, root, 2).unwrap();
    let rendered = decoded.tree.iter().filter(|n| n.is_rendered()).count();
    assert!(rendered > 1, "nothing was rendered");
    eprintln!(
        "the arena covers, and agrees with, all {rendered} rendered nodes, in {} slots; \
         those nodes reassemble the whole {} lines byte for byte",
        decoded.arena.len(),
        decoded.total_lines
    );
}

/// The document root is field number 1 of the virtual encompassing
/// message, and its field number is always shown, same as any other
/// unnamed field — the root is not special-cased.
#[test]
fn decode_shows_the_root_field_number_in_the_header_line() {
    let msg_desc = DescriptorProto {
        name: Some("Msg".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("id".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let file = FileDescriptorProto {
        name: Some("test_decode_root_name.proto".to_string()),
        package: Some("test".to_string()),
        message_type: vec![msg_desc],
        syntax: Some("proto3".to_string()),
        ..Default::default()
    };
    let fds = FileDescriptorSet { file: vec![file] };

    let mut ctx = ctx_from_fds("decode-root-name", &fds);

    let blob = [0x08u8, 0x05];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Msg"), 2).unwrap();
    assert!(
        decoded.document_lines()[0].starts_with("1 "),
        "root header line must show the root field number: {:?}",
        decoded.document_lines()[0]
    );
}

/// Spec 0120: `decode()` disables `expand_any`/`expand_message_set`,
/// so a `google.protobuf.Any` field is *not* auto-expanded at this
/// layer (that's `tui.rs`'s `render_overrides`/`auto_expand_type`'s
/// job, spec 0120 §G1) — instead it falls through to ordinary
/// nested-message rendering under Any's own real 2-field descriptor,
/// giving `type_url` (field 1) and `value` (field 2) real,
/// correctly-ordered `NodeSpan`s of their own (no virtual wrapper, no
/// fabricated `field_number: 0`). Fixture mirrors `prototext/tests/
/// node_span.rs`'s own `any_schema`/`any_wire_bytes`.
#[test]
fn decode_leaves_any_fields_unexpanded_with_real_type_url_and_value_spans() {
    let any_msg = DescriptorProto {
        name: Some("Any".to_string()),
        field: vec![
            FieldDescriptorProto {
                name: Some("type_url".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::String as i32),
                ..Default::default()
            },
            FieldDescriptorProto {
                name: Some("value".to_string()),
                number: Some(2),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Bytes as i32),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let any_file = FileDescriptorProto {
        name: Some("google/protobuf/any.proto".to_string()),
        syntax: Some("proto3".to_string()),
        package: Some("google.protobuf".to_string()),
        message_type: vec![any_msg],
        ..Default::default()
    };

    let payload_msg = DescriptorProto {
        name: Some("Payload".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("label".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::String as i32),
            ..Default::default()
        }],
        ..Default::default()
    };
    let container_msg = DescriptorProto {
        name: Some("Container".to_string()),
        field: vec![FieldDescriptorProto {
            name: Some("payload".to_string()),
            number: Some(1),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Message as i32),
            type_name: Some(".google.protobuf.Any".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let acme_file = FileDescriptorProto {
        name: Some("acme.proto".to_string()),
        syntax: Some("proto2".to_string()),
        package: Some("acme".to_string()),
        dependency: vec!["google/protobuf/any.proto".to_string()],
        message_type: vec![payload_msg, container_msg],
        ..Default::default()
    };
    let fds = FileDescriptorSet {
        file: vec![any_file, acme_file],
    };

    let mut ctx = ctx_from_fds("decode-any-expansion", &fds);

    // Container { payload: Any { type_url:
    // "type.googleapis.com/acme.Payload", value: Payload { label:
    // "hello" } } }.
    let label = b"hello";
    let mut payload_bytes = vec![0x0au8, label.len() as u8];
    payload_bytes.extend_from_slice(label);
    let type_url = b"type.googleapis.com/acme.Payload";
    let mut any_bytes = vec![0x0au8, type_url.len() as u8];
    any_bytes.extend_from_slice(type_url);
    any_bytes.push(0x12);
    any_bytes.push(payload_bytes.len() as u8);
    any_bytes.extend_from_slice(&payload_bytes);
    let mut blob = vec![0x0au8, any_bytes.len() as u8];
    blob.extend_from_slice(&any_bytes);

    let decoded = decode(
        wrapped(&blob),
        &mut ctx,
        RootType::Named("acme.Container"),
        2,
    )
    .unwrap();
    let any = decoded.fqdns.id_of("google.protobuf.Any");
    let any_idx = decoded
        .tree
        .iter()
        .position(|n| n.span.type_fqdn == any)
        .expect("tree must contain the unexpanded Any node itself");
    // Spec 0216: the structure is the arena's, and `Decoded` has no
    // `App` to read it through, so the two child slots are the
    // arithmetic itself — `Any`'s block starts at `first_child[any]`.
    let first_child = decoded.arena.first_child();
    let type_url_idx = first_child[any_idx] as usize;
    let value_idx = type_url_idx + 1;
    assert!(
        value_idx < first_child[any_idx + 1] as usize,
        "Any node must have type_url and value as its two children"
    );
    assert_eq!(decoded.tree[type_url_idx].span.field_number, 1);
    assert_eq!(decoded.tree[value_idx].span.field_number, 2);
    assert!(
        decoded.tree[value_idx].span.type_fqdn == NO_FQDN,
        "value must stay unexpanded (plain bytes) at decode() layer: {:#?}",
        decoded.tree[value_idx].span
    );
    let payload = decoded.fqdns.id_of("acme.Payload");
    assert!(
        !decoded.tree.iter().any(|n| n.span.type_fqdn == payload),
        "acme.Payload must not appear — auto-expansion is tui.rs's job, \
         not decode()'s: {:#?}",
        decoded.document_lines()
    );
    // Real tag/length-backed ranges, in document order: type_url's
    // range must end before value's own range starts.
    assert!(
        decoded.tree[type_url_idx].span.raw_range.end
            <= decoded.tree[value_idx].span.raw_range.start,
        "type_url and value must have real, non-overlapping, correctly \
         ordered raw ranges: {:#?}",
        (
            &decoded.tree[type_url_idx].span,
            &decoded.tree[value_idx].span
        )
    );
}

// ── Spec 0197: on-demand descriptor loading ──────────────────────────

/// A four-file descriptor set shaped so the on-demand branch can be
/// told apart from the eager one by observation alone:
///
/// - `leaf.proto` — `t.Leaf`, in `t.Root`'s closure, with an extension
///   range so an extension can be hung off it.
/// - `root.proto` — `t.Root`, importing `leaf.proto`, carrying one
///   nested message and one nested enum (spec 0137's namespace).
/// - `stray.proto` — `t.Stray` and `t.Mood`, imported by nobody, so
///   they are *outside* `t.Root`'s closure and must not appear in a
///   lazy pool until asked for.
/// - `ext.proto` — extends `t.Leaf` at field 100, likewise outside the
///   closure, so `ext_to_file` is the only way to find it.
///
/// proto2 throughout: proto3 has no extension ranges.
fn fixture_files() -> Vec<FileDescriptorProto> {
    use prost_reflect::prost_types::{
        descriptor_proto::ExtensionRange, EnumDescriptorProto, EnumValueDescriptorProto,
    };

    let scalar = |name: &str, number: i32| FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(Type::Int32 as i32),
        ..Default::default()
    };

    let leaf = FileDescriptorProto {
        name: Some("leaf.proto".to_string()),
        package: Some("t".to_string()),
        message_type: vec![DescriptorProto {
            name: Some("Leaf".to_string()),
            field: vec![scalar("id", 1)],
            extension_range: vec![ExtensionRange {
                start: Some(100),
                end: Some(200),
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    let root = FileDescriptorProto {
        name: Some("root.proto".to_string()),
        package: Some("t".to_string()),
        dependency: vec!["leaf.proto".to_string()],
        message_type: vec![DescriptorProto {
            name: Some("Root".to_string()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("leaf".to_string()),
                    number: Some(1),
                    label: Some(Label::Optional as i32),
                    r#type: Some(Type::Message as i32),
                    type_name: Some(".t.Leaf".to_string()),
                    ..Default::default()
                },
                scalar("n", 2),
            ],
            nested_type: vec![DescriptorProto {
                name: Some("Nested".to_string()),
                field: vec![scalar("k", 1)],
                ..Default::default()
            }],
            enum_type: vec![EnumDescriptorProto {
                name: Some("Color".to_string()),
                value: vec![EnumValueDescriptorProto {
                    name: Some("RED".to_string()),
                    number: Some(0),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    let stray = FileDescriptorProto {
        name: Some("stray.proto".to_string()),
        package: Some("t".to_string()),
        message_type: vec![DescriptorProto {
            name: Some("Stray".to_string()),
            field: vec![scalar("s", 1)],
            ..Default::default()
        }],
        enum_type: vec![EnumDescriptorProto {
            name: Some("Mood".to_string()),
            value: vec![EnumValueDescriptorProto {
                name: Some("CALM".to_string()),
                number: Some(0),
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    let ext = FileDescriptorProto {
        name: Some("ext.proto".to_string()),
        package: Some("t".to_string()),
        dependency: vec!["leaf.proto".to_string()],
        extension: vec![FieldDescriptorProto {
            name: Some("tag".to_string()),
            number: Some(100),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            extendee: Some(".t.Leaf".to_string()),
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    vec![leaf, root, stray, ext]
}

/// Each file's `(start, end)` byte span within an encoded FDS,
/// keyed by file name.
type FdsSpans = Vec<(String, (u64, u64))>;

/// The FDS wire encoding plus each file's span within it. Laid out
/// by hand rather than via `FileDescriptorSet::encode` because the
/// spans only exist while the records are being written, and they
/// are precisely what `LazyPool` slices FDPs out of.
fn encode_fds(files: &[FileDescriptorProto]) -> (Vec<u8>, FdsSpans) {
    let mut buf = Vec::new();
    let mut spans = Vec::new();
    for file in files {
        let body = file.encode_to_vec();
        write_tag(1, WT_LEN, &mut buf);
        write_varint(body.len() as u64, &mut buf);
        let start = buf.len() as u64;
        buf.extend_from_slice(&body);
        spans.push((file.name().to_owned(), (start, buf.len() as u64)));
    }
    (buf, spans)
}

fn collect_types(prefix: &str, msg: &DescriptorProto, file: &str, out: &mut Vec<(String, String)>) {
    let fqdn = format!("{prefix}{}", msg.name());
    out.push((fqdn.clone(), file.to_owned()));
    let inner = format!("{fqdn}.");
    for nested in &msg.nested_type {
        collect_types(&inner, nested, file, out);
    }
    for e in &msg.enum_type {
        out.push((format!("{inner}{}", e.name()), file.to_owned()));
    }
}

/// Build the `FdsIndex` `reproto` would have written for `files`.
///
/// There is no Rust-side index builder to reuse — `reproto` assembles
/// the maps in Python across the pyo3 boundary — so this reproduces
/// the four maps `fds_index.rs:57-79` documents, through
/// `canonical_map` so the archive is laid out the same way.
fn build_index(
    files: &[FileDescriptorProto],
    spans: Vec<(String, (u64, u64))>,
) -> prototext_graph::fds_index::FdsIndex {
    use prototext_graph::fds_index::{canonical_map, FdsIndex};

    let mut types = Vec::new();
    let mut deps = Vec::new();
    let mut exts = Vec::new();
    for file in files {
        let fname = file.name().to_owned();
        let pkg = file.package();
        let prefix = if pkg.is_empty() {
            String::new()
        } else {
            format!("{pkg}.")
        };
        for msg in &file.message_type {
            collect_types(&prefix, msg, &fname, &mut types);
        }
        for e in &file.enum_type {
            types.push((format!("{prefix}{}", e.name()), fname.clone()));
        }
        deps.push((fname.clone(), file.dependency.clone()));
        for x in &file.extension {
            let extendee = x.extendee().trim_start_matches('.');
            exts.push((format!("{extendee}/{}", x.number()), fname.clone()));
        }
    }

    FdsIndex {
        type_to_file: canonical_map(types),
        file_to_span: canonical_map(spans),
        dep_graph: canonical_map(deps),
        ext_to_file: canonical_map(exts),
    }
}

/// `<dir>/schema.pb` plus the `<dir>/schema/` sidecar directory — the
/// layout `DescriptorContext::load`'s probe expects, since it derives
/// the sidecar directory from `path.with_extension("")`.
struct Fixture {
    dir: PathBuf,
    pb: PathBuf,
    index: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    /// A binary descriptor with no sidecar: the eager branch.
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("protolens-0197-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("schema")).unwrap();
        let pb = dir.join("schema.pb");
        std::fs::write(&pb, encode_fds(&fixture_files()).0).unwrap();
        Fixture {
            index: dir.join("schema").join("index.rkyv"),
            pb,
            dir,
        }
    }

    /// Add the sidecar, enabling the on-demand branch.
    fn with_index(self) -> Self {
        let (_, spans) = encode_fds(&fixture_files());
        let index = build_index(&fixture_files(), spans);
        prototext_graph::fds_index::write(&index, &self.index).unwrap();
        self
    }

    /// Add a sidecar whose PTSGRAPH version field is one the reader
    /// does not accept — the state every existing schema-db is in the
    /// moment this ships.
    fn with_version_skewed_index(self) -> Self {
        let (_, spans) = encode_fds(&fixture_files());
        let index = build_index(&fixture_files(), spans);
        let mut bytes = prototext_graph::fds_index::to_bytes(&index).unwrap();
        bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
        std::fs::write(&self.index, &bytes).unwrap();
        self
    }

    /// Rewrite the descriptor as `#@ prototext` text. Round-tripped
    /// through the codec so `read_descriptor_file` recovers exactly
    /// the binary the sidecar's spans were measured against.
    fn with_prototext_descriptor(self) -> Self {
        let binary = std::fs::read(&self.pb).unwrap();
        let text = prototext_core::render_as_text(
            &binary,
            None,
            RenderOpts {
                assume_binary: true,
                include_annotations: true,
                indent: 1,
                expand_any: false,
                ..RenderOpts::default()
            },
        )
        .unwrap();
        std::fs::write(&self.pb, &text).unwrap();
        self
    }

    fn load(&self) -> DescriptorContext {
        DescriptorContext::load(&self.pb).unwrap()
    }
}

/// `Root { leaf { id: 5 } n: 7 }`.
const ROOT_BLOB: &[u8] = &[0x0a, 0x02, 0x08, 0x05, 0x10, 0x07];

fn message_names(ctx: &DescriptorContext) -> Vec<String> {
    let mut names: Vec<String> = ctx
        .pool()
        .all_messages()
        .map(|m| m.full_name().to_string())
        .collect();
    names.sort_unstable();
    names
}

/// Spec 0197 test 1 + G5. The branch is an implementation detail: the
/// same blob under the same root type must render the same text
/// whether the schema arrived one file at a time or all at once.
#[test]
fn both_branches_render_the_same_document() {
    let lazy = Fixture::new("same-render-lazy").with_index();
    let eager = Fixture::new("same-render-eager");

    let mut lazy_ctx = lazy.load();
    let mut eager_ctx = eager.load();
    assert!(lazy_ctx.lazy.is_some(), "sidecar present: on-demand branch");
    assert!(eager_ctx.lazy.is_none(), "no sidecar: eager branch");

    let from_lazy = decode(
        wrapped(ROOT_BLOB),
        &mut lazy_ctx,
        RootType::Named("t.Root"),
        2,
    )
    .unwrap();
    let from_eager = decode(
        wrapped(ROOT_BLOB),
        &mut eager_ctx,
        RootType::Named("t.Root"),
        2,
    )
    .unwrap();

    assert_eq!(from_lazy.document_lines(), from_eager.document_lines());
    assert_eq!(from_lazy.root_type, from_eager.root_type);
}

/// Spec 0197 tests 2 and 3. The whole point of the branch — the pool
/// starts with nothing in it, grows to exactly the root's file
/// closure, and grows again only when a name outside that closure is
/// asked for.
#[test]
fn the_lazy_pool_starts_empty_and_grows_only_on_demand() {
    let fixture = Fixture::new("grows-on-demand").with_index();
    let mut ctx = fixture.load();

    assert!(
        message_names(&ctx).is_empty(),
        "a freshly opened lazy pool holds no types at all"
    );

    ctx.message("t.Root").expect("root must resolve");
    assert_eq!(
        message_names(&ctx),
        vec![
            "t.Leaf".to_string(),
            "t.Root".to_string(),
            "t.Root.Nested".to_string()
        ],
        "exactly root.proto's closure — leaf.proto came in as its import, \
         stray.proto did not come in at all"
    );

    ctx.message("t.Stray")
        .expect("a name outside the closure must still resolve");
    assert!(
        message_names(&ctx).contains(&"t.Stray".to_string()),
        "the on-demand load must have added stray.proto"
    );
}

/// Spec 0197 tests 4 and 5, and the measured property of §5: the
/// index's key set *is* the pool's type namespace, enums included.
/// Equality of the ordered lists, not just the sets — the override
/// pane's lexicographic mode indexes into this by row.
#[test]
fn all_type_fqdns_agrees_across_branches_and_keeps_enums() {
    let lazy = Fixture::new("fqdns-lazy").with_index();
    let eager = Fixture::new("fqdns-eager");

    let from_index = lazy.load().all_type_fqdns();
    let from_pool = eager.load().all_type_fqdns();

    assert_eq!(from_index, from_pool);
    // Spec 0137's enums, one nested and one top-level, both present.
    assert!(from_index.contains(&"t.Root.Color".to_string()));
    assert!(from_index.contains(&"t.Mood".to_string()));
}

/// Spec 0197 test 6 (§S3, first cause). A missing sidecar is the
/// ordinary case for a hand-built descriptor: eager, and the user is
/// told which file to make and how.
#[test]
fn a_missing_index_falls_back_and_names_the_missing_sidecar() {
    let fixture = Fixture::new("missing-index");
    let ctx = fixture.load();

    assert!(ctx.lazy.is_none());
    let warning = &ctx
        .fallback
        .as_ref()
        .expect("fallback must be recorded")
        .message;
    assert!(
        warning.contains("no index.rkyv beside") && warning.contains("re-run reproto"),
        "{warning}"
    );
}

/// Spec 0197 test 7 (§S3, second cause). The load must *degrade*, not
/// fail: a version-skewed sidecar is what every existing schema-db
/// looks like on the day this ships, and erroring there would make
/// the feature a breaking change.
#[test]
fn a_version_skewed_index_falls_back_without_erroring() {
    let fixture = Fixture::new("skewed-index").with_version_skewed_index();
    let ctx = fixture.load();

    assert!(ctx.lazy.is_none());
    let warning = &ctx
        .fallback
        .as_ref()
        .expect("fallback must be recorded")
        .message;
    assert!(
        warning.contains("unsupported version 99") && warning.contains("re-run reproto"),
        "{warning}"
    );
}

/// Spec 0197 test 8 (§S3, third cause / §S7). The sidecar's spans
/// index the binary encoding; a `#@` descriptor on disk is not that
/// encoding, so a present and perfectly valid sidecar must still be
/// declined.
#[test]
fn a_prototext_descriptor_falls_back_even_with_a_valid_index() {
    let fixture = Fixture::new("prototext-descriptor")
        .with_index()
        .with_prototext_descriptor();
    let mut ctx = fixture.load();

    assert!(ctx.lazy.is_none());
    let warning = ctx
        .fallback
        .as_ref()
        .expect("fallback must be recorded")
        .message
        .clone();
    assert!(warning.contains("is #@ prototext"), "{warning}");
    // And the eager pool it fell back to is a working one.
    assert!(ctx.message("t.Root").is_some());
}

/// Spec 0197 test 11 (§S6). The lazy branch does not retain the
/// bytes, so it recomputes the hash from `source` — and must reach
/// the same value as the eager branch, since spec 0117's
/// `descriptor_set_sha256` is written into override files that
/// already exist.
#[test]
fn the_descriptor_hash_is_identical_across_branches() {
    let lazy = Fixture::new("hash-lazy").with_index();
    let eager = Fixture::new("hash-eager");

    let from_lazy = lazy.load().descriptor_sha256().unwrap();
    let from_eager = eager.load().descriptor_sha256().unwrap();

    assert_eq!(from_lazy, from_eager);
    assert_eq!(
        from_lazy,
        crate::override_pane::sha256_hex(&encode_fds(&fixture_files()).0),
        "the hash is still over the canonicalized descriptor bytes"
    );
}

/// Spec 0197 test 12 (§S6, last paragraph). No `--descriptor-set` at
/// all: nothing to be lazy about, nothing to warn about, and the hash
/// keeps the value it has always had.
#[test]
fn the_schemaless_context_has_no_lazy_branch_and_hashes_the_empty_string() {
    let ctx = DescriptorContext::schemaless();

    assert!(ctx.lazy.is_none());
    assert!(ctx.fallback.is_none());
    assert!(ctx.pool().all_messages().next().is_none());
    assert_eq!(
        ctx.descriptor_sha256().unwrap(),
        crate::override_pane::sha256_hex(&[])
    );
}

/// Spec 0197 test 14 (§S5, the staleness rule). Overriding a range to
/// a type from outside the root's closure loads a file into a pool
/// that already has live descriptors handed out of it, which is
/// exactly when prost-reflect's `Arc::make_mut` forks it. The wrapper
/// registration and the re-render that follow must see the new
/// symbol, not the pre-fork snapshot.
#[test]
fn a_type_loaded_after_the_root_can_still_be_rendered_through() {
    let fixture = Fixture::new("override-fresh-type").with_index();
    let mut ctx = fixture.load();

    decode(wrapped(ROOT_BLOB), &mut ctx, RootType::Named("t.Root"), 2).unwrap();

    // `Stray { s: 9 }`, the payload an override would splice.
    let stray_blob = [0x08u8, 0x09];
    let desc = ctx.message("t.Stray").expect("must load on demand");
    let stray = wrapped(&stray_blob);
    let arena = build_arena(stray.as_ref()).expect("stray blob is walkable");
    let decoded = render_resolved(stray, &mut ctx, Some(desc), Vec::new(), arena, 2, None).unwrap();

    assert_eq!(decoded.root_type, "t.Stray");
    assert!(
        decoded.document_lines().iter().any(|l| l.contains("s: 9")),
        "the freshly loaded type must render by name: {:?}",
        decoded.document_lines()
    );
}

/// Spec 0197 test 13 (§S5). MessageSet expansion resolves a payload
/// through an *extension* on the enclosing type, and the file
/// declaring that extension need never be in the root's closure —
/// `ext_to_file` is the only route to it, which is why
/// `load_extension` exists alongside `message`.
#[test]
fn an_extension_jit_loads_from_outside_the_root_closure() {
    let fixture = Fixture::new("extension-jit").with_index();
    let mut ctx = fixture.load();

    ctx.message("t.Root").expect("root must resolve");
    assert!(
        ctx.pool()
            .get_message_by_name("t.Leaf")
            .expect("Leaf is in the closure")
            .get_extension(100)
            .is_none(),
        "ext.proto is outside the closure, so its extension is not there yet"
    );

    ctx.load_extension("t.Leaf", 100);

    assert!(
        ctx.pool()
            .get_message_by_name("t.Leaf")
            .expect("Leaf is still there")
            .get_extension(100)
            .is_some(),
        "load_extension must have pulled ext.proto in"
    );
}

/// `Root { leaf { id: 5, [t.tag]: 42 } }` — field 100 is the
/// extension `ext.proto` declares, and `ext.proto` is in nobody's
/// dependency closure.
const EXT_BLOB: &[u8] = &[0x0a, 0x05, 0x08, 0x05, 0xa0, 0x06, 0x2a];

/// Spec 0248 test 1 (G1). Rendering — not MessageSet expansion, not an
/// explicit override — is what has to reach `ext_to_file` here. Before
/// the `EXT_LOADER` hook the field rendered as `100: 42`, because the
/// schema lookup only ever asked the descriptor it already held.
#[test]
fn an_extension_resolves_through_the_lazy_pool() {
    let lazy = Fixture::new("ext-render-lazy").with_index();
    let eager = Fixture::new("ext-render-eager");

    let mut lazy_ctx = lazy.load();
    let mut eager_ctx = eager.load();
    assert!(lazy_ctx.lazy.is_some(), "sidecar present: on-demand branch");
    assert!(eager_ctx.lazy.is_none(), "no sidecar: eager branch");

    let from_lazy = decode(
        wrapped(EXT_BLOB),
        &mut lazy_ctx,
        RootType::Named("t.Root"),
        2,
    )
    .unwrap();
    let from_eager = decode(
        wrapped(EXT_BLOB),
        &mut eager_ctx,
        RootType::Named("t.Root"),
        2,
    )
    .unwrap();

    assert!(
        from_lazy
            .document_lines()
            .iter()
            .any(|l| l.contains("[t.tag]: 42")),
        "the extension must render by name: {:?}",
        from_lazy.document_lines()
    );
    assert_eq!(
        from_lazy.document_lines(),
        from_eager.document_lines(),
        "the branch is not observable in the rendered text"
    );
}

/// Spec 0248 test 2 (G2). The hook is reached only by a field number
/// that missed twice, so a blob carrying no extension must leave the
/// declaring file exactly where it was. This is the assertion every
/// preloading alternative fails.
#[test]
fn a_blob_without_an_extension_leaves_the_declaring_file_unloaded() {
    let fixture = Fixture::new("ext-render-untouched").with_index();
    let mut ctx = fixture.load();

    decode(wrapped(ROOT_BLOB), &mut ctx, RootType::Named("t.Root"), 2).unwrap();

    assert!(
        ctx.pool()
            .get_message_by_name("t.Leaf")
            .expect("Leaf is in the closure")
            .get_extension(100)
            .is_none(),
        "nothing on the wire asked for field 100, so ext.proto must still be unloaded"
    );
}

/// Spec 0253 S3. The wrapper's name is the pool key `register_wrapper`
/// returns early on, so a label that is not part of that name makes two
/// genuinely different declarations collide: the second call hands back
/// the first's descriptor and the node renders under the wrong label.
#[test]
fn two_cardinalities_are_two_wrappers() {
    let mut ctx = DescriptorContext::empty_for_test();
    let wrapper = |ctx: &mut DescriptorContext, packed, cardinality| {
        register_wrapper(ctx.pool_mut(), 1, Type::Int32, None, packed, cardinality).unwrap()
    };

    let optional = wrapper(&mut ctx, false, Cardinality::Optional);
    let required = wrapper(&mut ctx, false, Cardinality::Required);
    let repeated = wrapper(&mut ctx, false, Cardinality::Repeated);

    let label = |m: &MessageDescriptor| {
        m.get_field(1)
            .expect("field 1 is the wrapper's")
            .cardinality()
    };
    assert_eq!(label(&optional), Cardinality::Optional);
    assert_eq!(label(&required), Cardinality::Required);
    assert_eq!(label(&repeated), Cardinality::Repeated);

    let names = [
        optional.full_name().to_string(),
        required.full_name().to_string(),
        repeated.full_name().to_string(),
    ];
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            assert_ne!(a, b, "each label must key its own wrapper");
        }
    }

    // And asking twice still returns the one already registered.
    assert_eq!(
        wrapper(&mut ctx, false, Cardinality::Required).full_name(),
        required.full_name()
    );
}

/// Spec 0253 S1. Protobuf requires a packed field to be repeated, so
/// that one rule stays inside `register_wrapper` and overrules whatever
/// the caller derived from the parent's schema.
#[test]
fn a_packed_wrapper_is_repeated_whatever_the_caller_asked_for() {
    let mut ctx = DescriptorContext::empty_for_test();
    let packed = register_wrapper(
        ctx.pool_mut(),
        1,
        Type::Int32,
        None,
        true,
        Cardinality::Optional,
    )
    .unwrap();
    assert_eq!(
        packed.get_field(1).unwrap().cardinality(),
        Cardinality::Repeated
    );
    assert!(packed.get_field(1).unwrap().is_packed());
}
