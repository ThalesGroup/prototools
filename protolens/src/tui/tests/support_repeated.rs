// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Fixtures whose subject is repetition: repeated scalar and message
//! fields, and the packed runs that encode a repetition as a single
//! record.

use super::super::*;
use super::support_build::{
    bounded_fixture_under, field, field_of, fixture_under, message, proto3_fds,
};
use prost_types::field_descriptor_proto::{Label, Type};

/// `Outer { repeated int32 vals = 1; }`, packed, 3 elements (`5, 6,
/// 7`) — the smallest packed run, returned with the node that draws it.
///
/// Spec 0184 already had the 3 elements share one wire record and hence
/// one positional path `/1`; spec 0216 finishes the job by making them
/// one *node*, drawing three rows. So this fixture no longer offers
/// three of anything: a test that needs three siblings wants
/// `repeated_message_fixture`.
pub(super) fn repeated_scalar_fixture() -> (App, usize) {
    let fds = proto3_fds(
        "test_repeated_scalar.proto",
        vec![message(
            "Outer",
            vec![field("vals", 1, Label::Repeated, Type::Int32)],
        )],
    );

    // vals: field 1 (tag 0x0A, LEN/packed), length 3, payload
    // [0x05, 0x06, 0x07] (three one-byte varint elements).
    let app = fixture_under(
        "repeated-scalar",
        &fds,
        "test.Outer",
        &[0x0Au8, 0x03, 0x05, 0x06, 0x07],
    );

    let run = app
        .tree
        .iter()
        .position(|n| n.span.packed_record_start != NO_PACKED_RECORD)
        .expect("fixture must contain the packed run");
    assert_eq!(
        app.tree[run].lines_total, 3,
        "the run draws one row per element"
    );
    (app, run)
}

/// `Outer { repeated Item items = 1; }` with 3 `Item { int32 v = 1; }`
/// submessages — three genuinely distinct sibling nodes at `/1`, `/2`,
/// `/3` that nonetheless share a parent type and field number.
///
/// This is the shape the manage pane's ambiguity machinery (spec 0134)
/// needs: an `FqdnField`/`PathField` origin matching several nodes that
/// derive *different* `Path` origins. `repeated_scalar_fixture` used to
/// serve that role, until spec 0184 made a packed run occupy a single
/// positional ordinal — an unpacked repeated *message* field is one
/// wire record per element and so keeps distinct paths by construction.
pub(super) fn repeated_message_fixture() -> (App, Vec<usize>) {
    with_items(fixture_under(
        "repeated-message",
        &repeated_message_fds(),
        "test.Outer",
        REPEATED_MESSAGE_BLOB,
    ))
}

/// [`repeated_message_fixture`] opened under spec 0257's startup row
/// budget, so the `Item`s past the budget arrive as stops rather than as
/// rendered bodies.
pub(super) fn bounded_repeated_message_fixture(budget: usize) -> (App, Vec<usize>) {
    with_items(bounded_fixture_under(
        "repeated-message-bounded",
        &repeated_message_fds(),
        "test.Outer",
        REPEATED_MESSAGE_BLOB,
        budget,
    ))
}

/// items: field 1 (tag 0x0A, LEN), thrice, each wrapping
/// `Item { v: 5 | 6 | 7 }` (field 1, tag 0x08, one-byte varint).
const REPEATED_MESSAGE_BLOB: &[u8] = &[
    0x0Au8, 0x02, 0x08, 0x05, //
    0x0A, 0x02, 0x08, 0x06, //
    0x0A, 0x02, 0x08, 0x07,
];

fn repeated_message_fds() -> prost_types::FileDescriptorSet {
    proto3_fds(
        "test_repeated_message.proto",
        vec![
            message(
                "Outer",
                vec![field_of(
                    "items",
                    1,
                    Label::Repeated,
                    Type::Message,
                    ".test.Item",
                )],
            ),
            message("Item", vec![field("v", 1, Label::Optional, Type::Int32)]),
        ],
    )
}

/// Spec 0216: the root is slot 0 — the wrapper — and the `Item`s are its
/// children, wherever the current interpretation puts them. True of a
/// bounded render too: a stop still has its header, so it is still a
/// child.
fn with_items(app: App) -> (App, Vec<usize>) {
    let items: Vec<usize> = (0..app.child_count(app.first_node))
        .map(|k| {
            app.nth_child(app.first_node, k)
                .expect("k is below the child count")
        })
        .collect();
    assert_eq!(items.len(), 3, "fixture must contain 3 Item submessages");
    (app, items)
}

/// `Outer { repeated int32 vals = 1; Inner tail = 2; int32 a = 3;
/// int32 b = 4; }` — a packed run of 3 elements followed by three
/// ordinary siblings. Returns `(app, run, tail, a, b)`.
///
/// The shape spec 0184 is about: the run must occupy exactly one
/// positional ordinal (`/1`) so that `tail`/`a`/`b` sit at `/2`/`/3`/
/// `/4` whatever the run's element count and whatever its override
/// state, while `a` and `b` — two consecutive *non-packed* scalars —
/// must still get distinct ordinals of their own (the S1 trap).
///
/// Spec 0216 S22 turns that rule into arithmetic: the run is one node
/// drawing three rows, so `run` is a single index rather than three.
pub(super) fn packed_run_with_tail_fixture() -> (App, usize, usize, usize, usize) {
    let scalar = |name: &str, number: i32| field(name, number, Label::Optional, Type::Int32);
    let fds = proto3_fds(
        "test_packed_run_with_tail.proto",
        vec![
            message(
                "Outer",
                vec![
                    field("vals", 1, Label::Repeated, Type::Int32),
                    field_of("tail", 2, Label::Optional, Type::Message, ".test.Inner"),
                    scalar("a", 3),
                    scalar("b", 4),
                ],
            ),
            message("Inner", vec![scalar("id", 3)]),
        ],
    );

    // vals: field 1 (tag 0x0A, LEN/packed), 3 one-byte elements;
    // tail: field 2 (tag 0x12, LEN) wrapping Inner { id: 5 };
    // a: field 3 (tag 0x18, varint) = 42; b: field 4 (tag 0x20) = 43.
    let app = fixture_under(
        "packed-run-with-tail",
        &fds,
        "test.Outer",
        &[
            0x0Au8, 0x03, 0x05, 0x06, 0x07, //
            0x12, 0x02, 0x18, 0x05, //
            0x18, 0x2A, //
            0x20, 0x2B,
        ],
    );

    // Spec 0216 S22: the run is one node, so the root has four children
    // — the record, then tail, a, b — where it used to have six.
    let children: Vec<usize> = (0..app.child_count(app.first_node))
        .map(|k| {
            app.nth_child(app.first_node, k)
                .expect("k is below the child count")
        })
        .collect();
    assert_eq!(
        children.len(),
        4,
        "fixture must decode the packed run plus tail, a, b"
    );
    assert!(
        app.tree[children[0]].span.packed_record_start != NO_PACKED_RECORD,
        "the first child must be the packed run"
    );
    (app, children[0], children[1], children[2], children[3])
}

/// `Holder { int32 pad = 1; bytes blob = 2; }` whose `blob` holds an
/// encoded `Payload { repeated int32 vals = 1; }` — i.e. a packed run
/// that only exists *after* an override retypes `blob`, and that the
/// splice therefore has to translate from the retyped node's own byte
/// frame into the document's.
///
/// `pad` is there to make that translation observable: the field being
/// retyped has to start somewhere other than byte 0, or the two frames
/// coincide and any missing translation is invisible.
///
/// Byte layout, which the tests assert against directly:
///
/// ```text
///   0: 0A 09        RootType::Named's synthetic wrapper field
///   2: 08 01        pad = 1
///   4: 12 05        blob, LEN 5            <- retyped node's tag
///   6: 0A 03        Payload.vals, packed   <- the packed record's tag
///   8: 05 06 07     the three elements
/// ```
pub(super) fn nested_packed_run_fixture() -> App {
    let fds = proto3_fds(
        "test_nested_packed_run.proto",
        vec![
            message(
                "Holder",
                vec![
                    field("pad", 1, Label::Optional, Type::Int32),
                    field("blob", 2, Label::Optional, Type::Bytes),
                ],
            ),
            message(
                "Payload",
                vec![field("vals", 1, Label::Repeated, Type::Int32)],
            ),
        ],
    );

    fixture_under(
        "nested-packed-run",
        &fds,
        "test.Holder",
        &[
            0x08u8, 0x01, //
            0x12, 0x05, //
            0x0A, 0x03, 0x05, 0x06, 0x07,
        ],
    )
}
