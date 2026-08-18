// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0323: a document opens closed.
//!
//! Every other fixture in the suite is unfolded by `fixture_app` before
//! a test sees it, precisely so that the fold is not an incidental part
//! of what those tests assert. These are the tests that do assert it, so
//! they go through `closed_fixture_*` and read `App::new`'s own output.

use super::super::bake::BakeStep;
use super::super::*;
use super::support::*;
use crate::node_status::Status;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::FileDescriptorSet;

/// `Root { Mid a = 1; Mid b = 2; }`, `Mid { Leaf x = 1; }`,
/// `Leaf { int32 v = 1; }` — three levels below the wrapper, and two
/// siblings at the top so that "the root's children" is plural.
fn nested_fds() -> FileDescriptorSet {
    let mid = |name: &str, number: i32| {
        field_of(name, number, Label::Optional, Type::Message, ".test.Mid")
    };
    proto3_fds(
        "test_folding.proto",
        vec![
            message("Root", vec![mid("a", 1), mid("b", 2)]),
            message(
                "Mid",
                vec![field_of(
                    "x",
                    1,
                    Label::Optional,
                    Type::Message,
                    ".test.Leaf",
                )],
            ),
            message("Leaf", vec![field("v", 1, Label::Optional, Type::Int32)]),
        ],
    )
}

/// `a: Mid { x: Leaf { v: 7 } }`, then `b: Mid { x: Leaf { v: 9 } }`.
const NESTED_BLOB: &[u8] = &[
    0x0Au8, 0x04, 0x0A, 0x02, 0x08, 0x07, //
    0x12, 0x04, 0x0A, 0x02, 0x08, 0x09,
];

fn nested_closed() -> App {
    closed_fixture_under("folding-nested", &nested_fds(), "test.Root", NESTED_BLOB)
}

/// [`nested_closed`] under spec 0257's startup row budget, so that the
/// nodes past the budget are bake stops as well as folded.
fn nested_bounded(budget: usize) -> App {
    closed_bounded_fixture_under(
        "folding-nested-bounded",
        &nested_fds(),
        "test.Root",
        NESTED_BLOB,
        budget,
    )
}

/// Every row the document currently draws, in order — the fold's whole
/// observable effect, and the level of detail the spec is written at.
fn visible_rows(app: &App) -> Vec<String> {
    (0..app.visible_row_count())
        .map(|row| {
            let (pos, line) = app.visible_row_pos(row).expect("row is within the count");
            app.row_text(app.committed_row_at(line, pos))
        })
        .collect()
}

/// Spec 0323 test-plan item 1 (G1, S2, S3).
#[test]
fn a_document_opens_closed() {
    let app = nested_closed();
    let root = app.first_node;

    assert_eq!(visible_rows(&app), closed_rows());
    assert_eq!(
        app.visible_row_count(),
        2 + app.child_count(root),
        "the root's own two rows, plus one collapsed row per child"
    );
    assert!(!app.is_folded(root), "the root alone is open");
}

/// The nested fixture as `App::new` leaves it.
fn closed_rows() -> Vec<String> {
    [
        "1 {  #@ Root = 1",
        "  a { ... }  #@ Mid = 1",
        "  b { ... }  #@ Mid = 2",
        "}",
    ]
    .map(String::from)
    .to_vec()
}

/// Spec 0323 test-plan item 2 (G1, S4): disclosure is one level per
/// gesture — the thing that separates this from "fold the root's
/// children", which the Alternatives section rejects.
#[test]
fn unfolding_reveals_one_level() {
    let mut app = nested_closed();
    let a = app.nth_child(app.first_node, 0).expect("Root.a");

    app.set_cursor(a);
    app.toggle_cursor_fold();

    assert_eq!(
        visible_rows(&app),
        one_level_open(),
        "`a`'s body appears, and `a`'s own child stays closed"
    );
}

/// The nested fixture with `a` open and everything below it closed —
/// what one gesture on `a` must produce, however that gesture is
/// spelled.
fn one_level_open() -> Vec<String> {
    [
        "1 {  #@ Root = 1",
        "  a {  #@ Mid = 1",
        "    x { ... }  #@ Leaf = 1",
        "  }",
        "  b { ... }  #@ Mid = 2",
        "}",
    ]
    .map(String::from)
    .to_vec()
}

/// Spec 0323 test-plan item 3: the spec's definition of the wanted
/// state, asserted directly rather than paraphrased.
///
/// The comparison is over the drawn rows and both fold sets. A frame
/// buffer would say the same thing and more, but it would also compare
/// the statusline, which quotes a cursor position `Z` moves and
/// `App::new` does not.
#[test]
fn startup_matches_z_then_z() {
    let opened_closed = nested_closed();

    let mut by_gesture = nested_closed();
    unfold_every_node(&mut by_gesture);
    by_gesture.set_cursor(by_gesture.first_node);
    by_gesture.toggle_cursor_fold_recursive(); // `Z`
    by_gesture.toggle_cursor_fold(); // `z`

    assert_eq!(visible_rows(&by_gesture), visible_rows(&opened_closed));
    assert_eq!(by_gesture.folded, opened_closed.folded);
    assert_eq!(by_gesture.auto_folded, opened_closed.auto_folded);
}

/// Spec 0323 test-plan item 4 (S4, G2): a subtree the bake reveals
/// arrives folded, so opening a stop discloses one level like every
/// other gesture rather than dumping a screenful.
#[test]
fn a_baked_subtree_arrives_folded() {
    let mut app = nested_bounded(2);
    assert!(!app.auto_folded.is_empty(), "the budget must leave stops");

    while app.bake_step() != BakeStep::Idle {}
    assert!(app.auto_folded.is_empty(), "the bake owes nothing");

    for k in 0..app.child_count(app.first_node) {
        let mid = app.nth_child(app.first_node, k).expect("k is a child");
        let x = app.nth_child(mid, 0).expect("Mid.x");
        assert!(
            app.folded.contains(x),
            "the bake produced {x}, which nobody asked to see"
        );
    }
    assert_eq!(
        visible_rows(&app),
        closed_rows(),
        "a finished bake changes nothing the reader can see"
    );
    // `assert_line_counts_are_exact` needs no call here: it is hung off
    // `finalize_override_batch`, which every bake step goes through.
    app.assert_status_is_exact();
}

/// Spec 0323 test-plan item 5 (S4): `splice_override` is not a gesture.
/// It leaves the node it retyped in the fold state it found it in —
/// either way round — and the children it produces arrive closed.
#[test]
fn an_override_subtree_arrives_folded() {
    for open_first in [false, true] {
        let mut app = nested_closed();
        let a = app.nth_child(app.first_node, 0).expect("Root.a");
        if open_first {
            app.open(a);
            app.refresh_line_counts(a);
        }

        app.splice_override(a, Some("test.Mid".to_string()), None)
            .expect("retyping `a` to its own declared type must succeed");

        assert_eq!(
            app.is_folded(a),
            !open_first,
            "a splice must neither open nor close the node it retyped"
        );
        let x = app.nth_child(a, 0).expect("Mid.x");
        assert!(app.is_folded(x), "and its new child arrives closed");
        assert_eq!(
            visible_rows(&app),
            if open_first {
                one_level_open()
            } else {
                closed_rows()
            }
        );
    }
}

/// Spec 0323 test-plan item 6 (S6): a preview draws the candidate
/// unfolded, and committing the same candidate collapses it. The
/// asymmetry is deliberate — a preview answers "is this type right",
/// which a `{ ... }` answers not at all.
#[test]
fn a_preview_draws_the_candidate_unfolded() {
    let mut app = nested_closed();
    let a = app.nth_child(app.first_node, 0).expect("Root.a");

    app.override_target = Some(a);
    app.override_candidates = vec![("test.Mid".to_string(), None)];
    app.override_highlight = 0;
    app.preview_override_highlight();

    let overlay = app
        .preview_overlay
        .as_ref()
        .unwrap_or_else(|| panic!("test.Mid must preview: {}", app.message));
    assert_eq!(
        overlay.lines,
        vec![
            "  a {  #@ Mid = 1",
            "    x {  #@ Leaf = 1",
            "      v: 7  #@ int32 = 1",
            "    }",
            "  }",
        ],
        "the preview shows every level of the candidate's body"
    );

    app.close_override();
    app.splice_override(a, Some("test.Mid".to_string()), None)
        .expect("the committed splice must succeed");
    assert!(
        app.is_folded(app.nth_child(a, 0).expect("Mid.x")),
        "the same candidate, committed, is drawn collapsed"
    );
}

/// Spec 0323 G4/S5: a stop is now normally in *both* sets, and the
/// violet margin — which reads `auto_folded` alone, via the `Unbaked`
/// rung — must still mean "nobody has looked inside" rather than
/// "collapsed".
#[test]
fn the_violet_margin_still_means_unread() {
    let mut app = nested_bounded(2);
    let stop = app.auto_folded.iter().next().expect("the budget stopped");
    assert!(app.folded.contains(stop), "and spec 0323 also folded it");
    assert_eq!(
        app.status_of(stop),
        Status::Unbaked,
        "a stop reads as unread whatever the user fold says"
    );

    while app.bake_step() != BakeStep::Idle {}

    assert!(app.folded.contains(stop), "still collapsed after the bake");
    assert_eq!(
        app.status_of(stop),
        Status::Ok,
        "but no longer unread: a user fold over a baked node is not violet"
    );
}
