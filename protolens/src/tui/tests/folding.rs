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
    assert_eq!(by_gesture.user_folds(), opened_closed.user_folds());
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
            app.is_user_folded(x),
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

/// Spec 0329 test-plan items 1 and 2 (G1, S1): the same claim as
/// `an_override_subtree_arrives_folded`, but through the key the reader
/// actually presses.
///
/// A distinct test because it is a distinct code path: `splice_override`
/// never touched the fold bit, and the rule spec 0329 removes lived in
/// `resettle_node`, which only the render pass behind a confirm reaches.
/// So the test above passed throughout and this one is the one that
/// would have failed.
///
/// Both directions, since S1 removes a rule rather than inverting one:
/// a folded node stays folded, and an open node stays open.
#[test]
fn a_commit_leaves_the_fold_alone() {
    for open_first in [false, true] {
        let mut app = nested_closed();
        app.splash = false;
        app.term_width = 120;
        let a = app.nth_child(app.first_node, 0).expect("Root.a");
        if open_first {
            app.open(a);
            app.refresh_line_counts(a);
        }
        app.set_cursor(a);

        app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert!(app.override_focus, "`t` must open the pane on `a`");
        app.override_candidates = vec![("test.Mid".to_string(), None)];
        app.override_highlight = 0;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.overrides.entries().iter().any(|e| e.active),
            "the confirm must have committed something: {}",
            app.message
        );

        assert_eq!(
            app.is_folded(a),
            !open_first,
            "a commit must neither open nor close the node it retyped"
        );
        // Item 2: spec 0323 S4's uniform rule over the *new* children is
        // untouched by item 1 — what arrives is still closed.
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
        app.assert_status_is_exact();
    }
}

// ---------------------------------------------------------------------
// Spec 0332: folding is a question about the bytes.
// ---------------------------------------------------------------------

/// [`nested_closed`] with every user fold dropped, so a test can assert
/// which bits a gesture *put* there rather than which it left.
fn nested_open() -> App {
    let mut app = nested_closed();
    app.splash = false;
    unfold_every_node(&mut app);
    app
}

fn digit(n: usize) -> KeyEvent {
    let c = char::from(b'0' + u8::try_from(n).expect("a single digit"));
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Every arena slot in `root`'s subtree with its depth below `root`.
///
/// Written out here rather than borrowed from `set_cursor_fold_depth`,
/// which is what these tests are checking: a walk that asserted itself
/// would agree with any bug it had.
fn arena_subtree(app: &App, root: usize) -> Vec<(usize, usize)> {
    let first_child = app.arena.first_child();
    let mut out = vec![(root, 0)];
    let mut k = 0;
    while k < out.len() {
        let (i, depth) = out[k];
        for child in first_child[i] as usize..first_child[i + 1] as usize {
            out.push((child, depth + 1));
        }
        k += 1;
    }
    out
}

/// Spec 0332 test-plan item 1 (G1): the digit *is* the depth, on every
/// slot of the subtree at once.
#[test]
fn a_digit_sets_the_subtree_to_that_depth() {
    for depth in 0..5 {
        let mut app = nested_open();
        let root = app.first_node;
        app.set_cursor(root);
        app.handle_key(digit(depth));

        for (idx, at) in arena_subtree(&app, root) {
            assert_eq!(
                app.is_user_folded(idx),
                at >= depth && app.is_foldable(idx),
                "slot {idx} sits at depth {at} and the digit was {depth}"
            );
        }
        app.assert_line_counts_are_exact();
    }
}

/// The same three messages as [`nested_fds`], plus a root that reads
/// [`NESTED_BLOB`]'s two fields as `bytes`.
///
/// `Mid` and `Leaf` are still in the pool so that the flat fields can be
/// overridden to them afterwards.
fn flat_fds() -> FileDescriptorSet {
    let mut fds = nested_fds();
    let flat = message(
        "RootFlat",
        vec![
            field("a", 1, Label::Optional, Type::Bytes),
            field("b", 2, Label::Optional, Type::Bytes),
        ],
    );
    fds.file[0].message_type.push(flat);
    fds
}

/// Spec 0332 test-plan item 2 (G3): the corollary, and the reason the
/// walk is over the arena rather than over `child_slots`.
///
/// `a` is drawn as a string here, so the rendered tree gives it no
/// children at all — but the greedy walk decomposed its payload, so the
/// arena does, and a digit writes bits on those slots. That the bits
/// then *survive* an override of `a` is the second half: the arena is a
/// function of the bytes, so slot `k` covers the same byte range under
/// every typing, and since spec 0338 G1 a splice writes no bit of the
/// set at all.
#[test]
fn a_digit_folds_slots_this_typing_does_not_show() {
    let mut app = closed_fixture_under("folding-flat", &flat_fds(), "test.RootFlat", NESTED_BLOB);
    app.splash = false;
    unfold_every_node(&mut app);

    let a = app.nth_child(app.first_node, 0).expect("RootFlat.a");
    assert!(!app.has_children(a), "this typing prints `a` as a string");
    assert!(
        app.child_slots(a).is_empty(),
        "so the rendered tree offers nothing below it"
    );
    let hidden: Vec<usize> = arena_subtree(&app, a)
        .into_iter()
        .filter(|&(idx, _)| idx != a && app.is_foldable(idx))
        .map(|(idx, _)| idx)
        .collect();
    assert!(!hidden.is_empty(), "but the arena does: {hidden:?}");

    app.set_cursor(a);
    app.handle_key(digit(0));
    assert!(
        hidden.iter().all(|&i| app.is_user_folded(i)),
        "a digit reaches slots no row stands for"
    );

    app.splice_override(a, Some("test.Mid".to_string()), None)
        .expect("retyping `a` to a message type must succeed");
    assert!(
        hidden.iter().all(|&i| app.is_user_folded(i)),
        "and the override does not scrub what it never showed"
    );
}

/// Spec 0332 test-plan item 9 (S3): the reverse pass, pinned on the one
/// digit that both opens and folds within a single keystroke.
///
/// `refresh_line_counts` climbs upward and stops at the first ancestor
/// whose count is unchanged, so a parent refreshed before its children
/// propagates a number that is about to move again — and the symptom is
/// silent, a fold marker one row off rather than a panic.
#[test]
fn line_counts_stay_exact_after_a_digit() {
    let mut app = nested_open();
    let root = app.first_node;
    app.set_cursor(root);
    app.handle_key(digit(3));
    app.handle_key(digit(1));
    app.assert_line_counts_are_exact();
    assert_eq!(
        visible_rows(&app),
        closed_rows(),
        "depth 1 is the root open with its children folded"
    );
}

/// Spec 0332 test-plan item 3 (G2): a digit names a shape, so pressing
/// it twice does what pressing it once did, and what came before does
/// not show through.
#[test]
fn a_digit_is_absolute_not_a_toggle() {
    let shape_after = |presses: &[usize]| {
        let mut app = nested_open();
        app.set_cursor(app.first_node);
        for &p in presses {
            app.handle_key(digit(p));
        }
        (app.user_folds(), visible_rows(&app))
    };

    assert_eq!(shape_after(&[2, 2]), shape_after(&[2]), "twice is once");
    assert_eq!(shape_after(&[3, 1]), shape_after(&[1]), "3 then 1 is 1");
    assert_eq!(shape_after(&[0, 4]), shape_after(&[4]), "0 then 4 is 4");
}

/// Spec 0332 test-plan item 4: the shallower digit's folds are not a
/// floor the deeper one has to work around.
#[test]
fn a_deeper_digit_reopens_what_a_shallower_one_closed() {
    let mut app = nested_open();
    app.set_cursor(app.first_node);

    app.handle_key(digit(1));
    let one = visible_rows(&app);
    assert_eq!(one, closed_rows());

    app.handle_key(digit(3));
    assert!(visible_rows(&app).len() > one.len(), "3 shows more");

    app.handle_key(digit(1));
    assert_eq!(visible_rows(&app), one, "and 1 puts it back");
}

/// Spec 0332 test-plan item 5 (S4, G5): `Z` is the two ends of the
/// digits' range, which is the claim that they generalize it rather
/// than reimplement it.
#[test]
fn shift_z_is_the_two_extremes_of_the_digits() {
    let by_shift_z = |folded_first: bool| {
        let mut app = nested_open();
        app.set_cursor(app.first_node);
        if folded_first {
            app.handle_key(digit(0));
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));
        app.user_folds()
    };

    let by_digit = |depth: usize| {
        let mut app = nested_open();
        app.set_cursor(app.first_node);
        app.handle_key(digit(depth));
        app.user_folds()
    };

    assert_eq!(by_shift_z(false), by_digit(0), "`Z` closing is depth 0");
    assert_eq!(
        by_shift_z(true),
        by_digit(9),
        "`Z` opening is any depth past the bottom of the subtree"
    );
}

/// Spec 0332 test-plan item 6 (G4): the whole of the baking-independence
/// claim, on all three gestures.
///
/// A bake stop is the one node where the old code rendered on a
/// keystroke. Now every one of them writes `folded` and leaves
/// `auto_folded` and the rendered rows exactly as they were, so the
/// keystroke means the same thing whenever it is pressed.
#[test]
fn a_fold_gesture_never_renders() {
    for key in [
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE),
        digit(4),
    ] {
        let mut app = nested_bounded(2);
        app.splash = false;
        let stop = app.auto_folded.iter().next().expect("the budget stopped");
        let stops_before = app.auto_folded.clone();
        let rows_before = visible_rows(&app);

        app.set_cursor(stop);
        app.handle_key(key);

        assert_eq!(
            app.auto_folded, stops_before,
            "{key:?} must not touch the bake's own bit"
        );
        assert_eq!(
            app.tree[stop].lines_total, 2,
            "{key:?} must not have spliced a body in"
        );
        assert_eq!(visible_rows(&app), rows_before, "so no row moved");
        assert!(
            !app.is_user_folded(stop),
            "and what the reader asked for is on record"
        );
    }
}

/// Spec 0332 test-plan item 7: the guard, and the one shape it must
/// still refuse.
///
/// A scalar has no child block in the arena and no braces on screen, so
/// nothing would ever take it back out of `folded`.
#[test]
fn a_digit_on_a_leaf_says_not_foldable() {
    let mut app = nested_open();
    let a = app.nth_child(app.first_node, 0).expect("Root.a");
    let x = app.nth_child(a, 0).expect("Mid.x");
    let v = app.nth_child(x, 0).expect("Leaf.v");
    assert!(!app.is_foldable(v), "`v` is an int32");

    let before = app.user_folds();
    app.set_cursor(v);
    app.handle_key(digit(0));

    assert_eq!(app.message, "not foldable");
    assert_eq!(app.user_folds(), before, "and nothing moved");
}

/// Spec 0332 test-plan item 8 (N6): the cursor is already on the node
/// being reshaped, and a digit only changes rows after its header.
#[test]
fn a_digit_leaves_the_cursor_where_it_was() {
    let mut app = nested_open();
    let a = app.nth_child(app.first_node, 0).expect("Root.a");
    app.set_cursor(a);
    let moves = app.cursor_moves;

    app.handle_key(digit(1));

    assert_eq!(app.cursor, a);
    assert_eq!(app.cursor_moves, moves, "not even a round trip");
}

/// Spec 0332 test-plan item 10 (S6, N5): `0` folds, and the caret's own
/// column zero is still one keystroke away.
#[test]
fn zero_folds_and_the_caret_still_reaches_column_zero() {
    let mut app = nested_open();
    let root = app.first_node;
    app.set_cursor(root);
    app.caret_to_line_end();

    app.handle_key(digit(0));
    assert!(app.is_user_folded(root), "`0` closes the cursor node");
    assert_eq!(
        visible_rows(&app),
        vec!["1 { ... }  #@ Root = 1".to_string()],
        "and everything in it"
    );

    app.caret_to_line_end();
    app.handle_key(KeyEvent::new(KeyCode::Char('^'), KeyModifiers::NONE));
    assert_eq!(app.cursor_column, app.caret_bounds().0);
}

/// Spec 0323 G4/S5: a stop is now normally in *both* sets, and the
/// grayed margin — which reads `auto_folded` alone, via the `Unbaked`
/// rung — must still mean "nobody has looked inside" rather than
/// "collapsed".
#[test]
fn the_unbaked_margin_still_means_unread() {
    let mut app = nested_bounded(2);
    let stop = app.auto_folded.iter().next().expect("the budget stopped");
    assert!(app.is_user_folded(stop), "and spec 0323 also folded it");
    assert_eq!(
        app.status_of(stop),
        Status::Unbaked,
        "a stop reads as unread whatever the user fold says"
    );

    while app.bake_step() != BakeStep::Idle {}

    assert!(app.is_user_folded(stop), "still collapsed after the bake");
    assert_eq!(
        app.status_of(stop),
        Status::Ok,
        "but no longer unread: a user fold over a baked node is not unbaked"
    );
}

/// Spec 0338 test-plan item 1: the other half of spec 0332 G3's
/// corollary, which used to run one way only.
///
/// A *fold* recorded on a slot this typing does not print survives an
/// override of the node above it —
/// `a_digit_folds_slots_this_typing_does_not_show` is that half. An
/// *unfold* did not, because the render inserted every bracketed slot it
/// wrote into `folded` (spec 0323 S2) and a vacant slot had no bit for
/// it to leave alone. Spec 0338 S1 gives every foldable slot in the
/// arena a bit at open, so there is no such thing as a slot with no
/// answer, and S2 takes the write off the splice entirely. Both halves
/// now run the same way.
#[test]
fn an_unfold_survives_an_override_of_the_node_above_it() {
    let mut app = closed_fixture_under("folding-open", &flat_fds(), "test.RootFlat", NESTED_BLOB);
    app.splash = false;
    unfold_every_node(&mut app);

    let a = app.nth_child(app.first_node, 0).expect("RootFlat.a");
    let hidden: Vec<usize> = arena_subtree(&app, a)
        .into_iter()
        .filter(|&(idx, _)| idx != a && app.is_foldable(idx))
        .map(|(idx, _)| idx)
        .collect();

    app.set_cursor(a);
    app.handle_key(digit(9));
    assert!(
        hidden.iter().all(|&i| !app.is_user_folded(i)),
        "the reader asked for every level"
    );

    app.splice_override(a, Some("test.Mid".to_string()), None)
        .expect("retyping `a` to a message type must succeed");
    assert!(
        hidden.iter().all(|&i| !app.is_user_folded(i)),
        "and the render that first draws those slots leaves the answer alone"
    );
}
