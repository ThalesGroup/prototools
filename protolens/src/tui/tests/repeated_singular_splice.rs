// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0373 — a node re-rendered alone keeps the `repeated_singular` its
//! header had in the parent's render.
//!
//! The mark is the parent frame's verdict (spec 0343 A2), so a lone render
//! — the bake, a type override, its preview — cannot recompute it and has
//! to be handed it. Each test below re-renders a node one of those ways and
//! reads its header row.

use super::super::bake::BakeStep;
use super::super::*;
use super::support::*;
use crate::override_pane::OverrideOrigin;
use prototext_core::schema::descriptor::field_descriptor_proto::{Label, Type};
use prototext_core::schema::descriptor::Encode as _;
use prototext_core::schema::descriptor::FileDescriptorSet;
use prototext_core::schema::Cardinality;

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// `message Box { optional Item item = 1; }`, `message Item { optional
/// int32 v = 1; }`, and `Other`, a second shape `Item`'s bytes also fit.
fn box_fds() -> FileDescriptorSet {
    proto2_fds(
        "box.proto",
        vec![
            message(
                "Box",
                vec![field_of(
                    "item",
                    1,
                    Label::Optional,
                    Type::Message,
                    ".test.Item",
                )],
            ),
            message("Item", vec![field("v", 1, Label::Optional, Type::Int32)]),
            message("Other", vec![field("w", 1, Label::Optional, Type::Int32)]),
        ],
    )
}

/// `item { v: 1 }` then `item { v: 2 }`: the singular `item` twice.
const TWO_ITEMS: [u8; 8] = [0x0a, 0x02, 0x08, 0x01, 0x0a, 0x02, 0x08, 0x02];

/// The Box fixture with a row budget small enough to defer both bodies,
/// and the bake drained — the state an interactive session settles in.
fn baked_box() -> App {
    let mut app = bounded_fixture_under("rs-splice", &box_fds(), "test.Box", &TWO_ITEMS, 1);
    drain_bake(&mut app);
    app
}

fn drain_bake(app: &mut App) {
    while !matches!(app.bake_step(), BakeStep::Idle) {}
}

/// The node at `path`.
fn node(app: &App, path: &str) -> usize {
    (0..app.tree.len())
        .find(|&i| app.positional_path(i) == path)
        .unwrap_or_else(|| panic!("no node at {path}"))
}

/// The header row of the node at `path`.
fn header(app: &App, path: &str) -> String {
    let idx = node(app, path);
    app.document_lines()[app.node_lines(idx).start].clone()
}

fn anomalies() -> (FileDescriptorSet, Vec<u8>) {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = root.parent().expect("protolens/ has a parent");
    let descriptor = std::fs::read(root.join("prototext-core/fixtures/descriptor.pb")).unwrap();
    let text = std::fs::read(root.join("tests/fixtures/anomalies.pb")).unwrap();
    let blob = prototext_core::render_as_bytes(
        &text,
        prototext_core::RenderOpts {
            assume_binary: false,
            include_annotations: true,
            ..Default::default()
        },
    )
    .unwrap()
    .into_owned();
    (
        FileDescriptorSet::decode(descriptor.as_slice()).unwrap(),
        blob,
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Test 1 (G2). Once the bake has drained, the document is the unbounded
/// render, line for line. Before spec 0373 exactly one line differed on
/// this fixture: 5.c's `source_code_info` header, missing its mark.
#[test]
fn baked_document_equals_the_unbounded_render() {
    let (fds, blob) = anomalies();
    let ty = "google.protobuf.FileDescriptorProto";
    let unbounded = fixture_under("rs-anom-u", &fds, ty, &blob).document_lines();
    assert!(
        unbounded.iter().any(|l| l.contains("repeated_singular")),
        "the fixture must exercise the mark at all"
    );
    for budget in [5, 41] {
        let mut app = bounded_fixture_under("rs-anom-b", &fds, ty, &blob, budget);
        drain_bake(&mut app);
        assert_eq!(app.document_lines(), unbounded, "budget {budget}");
    }
}

/// Test 2. The second `item` is deferred, then baked: its header keeps
/// the mark, and the first's never had one.
#[test]
fn a_repeated_singular_message_keeps_its_mark_after_bake() {
    let app = baked_box();
    assert!(
        !header(&app, "/1").contains("repeated_singular"),
        "{}",
        header(&app, "/1")
    );
    assert!(
        header(&app, "/2").contains("repeated_singular"),
        "{}",
        header(&app, "/2")
    );
}

/// Test 3 (S4). A type override is a splice too, committed or previewed:
/// the field still occurs again, whatever type it is now read as.
#[test]
fn a_type_override_keeps_repeated_singular() {
    let mut app = baked_box();
    let idx = node(&app, "/2");
    app.render_node_as(idx, Some("test.Other"), true, None)
        .expect("preview");
    assert!(
        header(&app, "/2").contains("repeated_singular"),
        "preview: {}",
        header(&app, "/2")
    );

    let mut app = baked_box();
    let idx = node(&app, "/2");
    app.splice_override(idx, Some("test.Other".to_string()), None)
        .expect("commit");
    let row = header(&app, "/2");
    assert!(
        row.contains("Other") && row.contains("repeated_singular"),
        "commit: {row}"
    );
}

/// Test 4 (S3). An override entry that makes the field repeated takes the
/// mark away with it.
#[test]
fn a_cardinality_override_to_repeated_drops_the_mark() {
    let mut app = baked_box();
    let origin = OverrideOrigin::Path {
        path: "/2".to_string(),
    };
    app.overrides
        .activate(origin.clone(), Some("test.Item".to_string()));
    let entry = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin)
        .expect("the entry just activated");
    app.overrides
        .set_cardinality(entry, Some(Cardinality::Repeated));
    let idx = node(&app, "/2");
    app.splice_override(idx, Some("test.Item".to_string()), None)
        .expect("commit");
    let row = header(&app, "/2");
    assert!(!row.contains("repeated_singular"), "{row}");
}

/// Test 5 (S3). A raw override keeps the mark: the verdict was made under
/// the parent's schema, which a raw override of the child leaves alone.
#[test]
fn a_raw_override_keeps_repeated_singular() {
    let mut app = baked_box();
    let idx = node(&app, "/2");
    app.splice_override(idx, None, None).expect("raw");
    let row = header(&app, "/2");
    assert!(row.contains("repeated_singular"), "{row}");
}

/// Test 6 (S2). Bake, then override: the mark survives both splices. This
/// is what pins where the verdict is paid. Paid inside `TextSink`, the row
/// would carry it but the span's bit would not, and the second splice —
/// which reads the bit — would lose it.
#[test]
fn a_second_splice_keeps_repeated_singular() {
    let mut app = baked_box();
    assert!(
        header(&app, "/2").contains("repeated_singular"),
        "after the bake"
    );
    let idx = node(&app, "/2");
    app.splice_override(idx, Some("test.Item".to_string()), None)
        .expect("first override");
    let idx = node(&app, "/2");
    app.splice_override(idx, Some("test.Other".to_string()), None)
        .expect("second override");
    let row = header(&app, "/2");
    assert!(row.contains("repeated_singular"), "{row}");
}
