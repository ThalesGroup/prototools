// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `tui/structure.rs` — spec 0216's shape layer.
//!
//! Every link there is arithmetic on the arena's `first_child` and
//! `parent` arrays, and every method indexes those arrays unchecked. An
//! off-by-one would not crash; it would quietly hand back the wrong
//! neighbor, and the symptom would surface somewhere else entirely — a
//! positional path that resolves to the wrong node, a fold that eats the
//! sibling after it. So most of what is below is not a table of expected
//! values but a set of *relations between the accessors*, checked over
//! every node of several real fixtures: the relations are what the rest
//! of the crate actually relies on, and they are what an off-by-one
//! breaks.

use super::super::*;
use super::support::*;

/// Every rendered node reachable from the root, in document order,
/// collected through `child_count`/`nth_child` — deliberately *not*
/// through `doc_next`, which is one of the things under test.
fn all_nodes(app: &App) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![app.first_node];
    while let Some(i) = stack.pop() {
        out.push(i);
        for k in (0..app.child_count(i)).rev() {
            stack.push(app.nth_child(i, k).expect("k is below the child count"));
        }
    }
    out
}

/// The fixtures the property tests below run over: a repeated
/// submessage, a packed run nested under a message, a `MessageSet`, an
/// `Any`, and the widest hand-built one. Between them they cover a
/// single child, several children, a packed run (one slot, several
/// rows) and several levels.
fn shaped_fixtures() -> Vec<(&'static str, App)> {
    vec![
        ("repeated_message", repeated_message_fixture().0),
        ("nested_packed_run", nested_packed_run_fixture()),
        ("message_set", message_set_fixture()),
        ("nested_any", nested_any_fixture()),
        ("export_fields", export_fields_fixture()),
    ]
}

/// The parent/child pair must agree in both directions, and the child's
/// ordinal must be the position it was fetched by. `sibling_position` is
/// what `positional_path` is built from, so a drift here is a drift in
/// every saved override.
#[test]
fn parent_child_and_ordinal_agree_for_every_node() {
    for (name, app) in shaped_fixtures() {
        for i in all_nodes(&app) {
            for k in 0..app.child_count(i) {
                let child = app.nth_child(i, k).expect("k is below the child count");
                assert_eq!(
                    app.parent(child),
                    Some(i),
                    "{name}: child {k} of node {i} must name {i} as its parent"
                );
                assert_eq!(
                    app.sibling_position(child),
                    k + 1,
                    "{name}: child {k} of node {i} must be at 1-based position {}",
                    k + 1
                );
            }
        }
    }
}

/// `first_child`/`last_child` are `nth_child` at the two ends, and both
/// are `None` exactly when there are no children. `last_child` is the
/// one that subtracts, so it is the one an empty block used to reach
/// through.
#[test]
fn first_and_last_child_are_the_ends_of_the_child_block() {
    for (name, app) in shaped_fixtures() {
        for i in all_nodes(&app) {
            let count = app.child_count(i);
            assert_eq!(
                app.first_child(i),
                app.nth_child(i, 0),
                "{name}: node {i}'s first child"
            );
            assert_eq!(
                app.last_child(i),
                count.checked_sub(1).and_then(|k| app.nth_child(i, k)),
                "{name}: node {i}'s last child"
            );
            assert_eq!(
                app.first_child(i).is_none(),
                count == 0,
                "{name}: node {i} has {count} children"
            );
        }
    }
}

/// The sibling steps must be inverses, and must stop at the ends of the
/// block rather than walking into the neighboring parent's children —
/// which, level order being what it is, sit in the very next slots.
#[test]
fn sibling_steps_are_inverses_and_stop_at_the_block_edges() {
    for (name, app) in shaped_fixtures() {
        for i in all_nodes(&app) {
            let count = app.child_count(i);
            for k in 0..count {
                let child = app.nth_child(i, k).expect("k is below the child count");
                let expected_prev = k.checked_sub(1).map(|p| child - (k - p));
                assert_eq!(
                    app.prev_sibling(child),
                    expected_prev,
                    "{name}: node {child} (child {k} of {i})"
                );
                assert_eq!(
                    app.next_sibling(child),
                    (k + 1 < count).then(|| child + 1),
                    "{name}: node {child} (child {k} of {i})"
                );
                if let Some(next) = app.next_sibling(child) {
                    assert_eq!(
                        app.prev_sibling(next),
                        Some(child),
                        "{name}: the two sibling steps must undo each other"
                    );
                }
            }
        }
    }
}

/// An index past the end of the child block yields nothing, rather than
/// the first child of the next parent's block.
#[test]
fn nth_child_past_the_end_is_none() {
    for (name, app) in shaped_fixtures() {
        for i in all_nodes(&app) {
            let count = app.child_count(i);
            assert_eq!(
                app.nth_child(i, count),
                None,
                "{name}: node {i} has {count} children"
            );
            assert_eq!(app.nth_child(i, count + 7), None, "{name}: node {i}");
        }
    }
}

/// `doc_next` is derived, not stored, so the check is that it agrees
/// with the recursive walk the rest of these tests use: same nodes, same
/// order, and it stops rather than cycling.
#[test]
fn doc_next_enumerates_the_tree_in_document_order() {
    for (name, app) in shaped_fixtures() {
        let expected = all_nodes(&app);
        let mut walked = vec![app.first_node];
        let mut cur = app.first_node;
        while let Some(next) = app.doc_next(cur) {
            walked.push(next);
            cur = next;
            assert!(
                walked.len() <= expected.len(),
                "{name}: doc_next must terminate, not cycle"
            );
        }
        assert_eq!(walked, expected, "{name}: document order");
    }
}

/// The root terminates every climb — it is its own arena parent (spec
/// 0216 S8), which is what makes the sentinel unnecessary — and, being
/// the only node at level 0 in a wrapped document, it has no siblings
/// either.
#[test]
fn the_root_has_no_parent_and_no_siblings() {
    for (name, app) in shaped_fixtures() {
        let root = app.first_node;
        assert_eq!(app.parent(root), None, "{name}");
        assert_eq!(app.prev_sibling(root), None, "{name}");
        assert_eq!(app.next_sibling(root), None, "{name}");
        assert_eq!(app.sibling_position(root), 1, "{name}");
        assert_eq!(app.doc_next(root), app.first_child(root), "{name}");
    }
}

/// The other branch of `sibling_block`: an *unwrapped* blob of several
/// top-level records has one root per record, and the roots are each
/// other's siblings. The wrapped documents above never take this path.
#[test]
fn several_top_level_records_are_siblings_of_each_other() {
    let app = sibling_leaves_app(&["a: 0", "b: 0", "c: 0"]);
    for i in 0..3 {
        assert_eq!(app.parent(i), None, "record {i} is a root");
        assert_eq!(app.sibling_position(i), i + 1);
    }
    assert_eq!(app.next_sibling(0), Some(1));
    assert_eq!(app.next_sibling(1), Some(2));
    assert_eq!(app.next_sibling(2), None);
    assert_eq!(app.prev_sibling(0), None);
    assert_eq!(app.prev_sibling(2), Some(1));
}

/// `positional_path` is `sibling_position` all the way up and
/// `resolve_path` is `nth_child` all the way down, so the two agreeing
/// on every node is the end-to-end statement of what this module is
/// for: it is the identity a saved override depends on to still name the
/// same node in the next session.
#[test]
fn positional_path_round_trips_through_resolve_path() {
    for (name, app) in shaped_fixtures() {
        for i in all_nodes(&app) {
            let path = app.positional_path(i);
            assert_eq!(
                app.resolve_path(&path),
                Some(i),
                "{name}: node {i} renders as '{path}', which must resolve back to it"
            );
        }
    }
}

/// A path with a segment that does not exist, or a 0 segment (the
/// positions are 1-based on the wire), resolves to nothing rather than
/// to whatever the arithmetic lands on.
#[test]
fn resolve_path_rejects_out_of_range_and_zero_segments() {
    let (app, _items) = repeated_message_fixture();
    assert_eq!(app.resolve_path("/"), Some(app.first_node));
    assert_eq!(app.resolve_path("/1"), app.nth_child(app.first_node, 0));
    assert_eq!(app.resolve_path("/0"), None);
    assert_eq!(app.resolve_path("/99"), None);
    assert_eq!(app.resolve_path("/1/0"), None);
    assert_eq!(app.resolve_path("/1/99"), None);
    assert_eq!(app.resolve_path("/x"), None);
}
