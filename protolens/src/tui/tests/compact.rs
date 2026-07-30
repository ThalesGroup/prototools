// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0203: the node arena is compacted incrementally.
//!
//! Everything here exists to attack one claim, on which the whole
//! design rests:
//!
//! > the arena is a completely valid document after *every individual
//! > node move*, not merely at the end of a pass.
//!
//! That is what allows a slice to be cut off after a few thousand moves
//! and the event loop to resume drawing, folding, scrolling and
//! searching on a half-compacted arena. If it is false, protolens shows
//! a wrong document — or panics on an index — at some arbitrary moment
//! seconds after a keystroke, with nothing on screen connecting the two.
//!
//! So the checks are content equality and `verify_arena`, never "does it
//! still walk": a missed back-reference leaves the tree perfectly
//! walkable and merely pointing at the wrong node.

use super::super::compact::CompactStep;
use super::super::*;
use super::support::*;

/// Apply a real override, leaving the arena holding both the abandoned
/// subtree and its replacement — the state compaction exists for.
fn app_with_garbage() -> App {
    let (mut app, _outer, inner) = type_as_fixture();
    let inner_path = app.positional_path(inner);
    app.overrides.activate(
        override_pane::OverrideOrigin::Path { path: inner_path },
        None,
    );
    let root = app.first_node;
    app.render_overrides(root);
    assert!(
        app.dead_count > 0,
        "the fixture must actually produce garbage, or every test here \
         is vacuous"
    );
    app
}

fn check(app: &App, what: &str) {
    if let Err(e) = app.verify_arena() {
        panic!("{what}: {e}");
    }
}

/// Everything a reader can observe, projected so that renumbering — the
/// entire point of the operation — does not register as a difference.
fn observable(app: &App) -> (Vec<String>, Vec<Shape>, Vec<(usize, Shape, bool)>) {
    (app.lines.clone(), live_shapes(app), line_owners(app))
}

/// `Shape` carries a `Range`, which is not `Ord`, so index-keyed
/// collections (which have no inherent order) are compared through
/// this.
fn sorted(mut v: Vec<Shape>) -> Vec<Shape> {
    v.sort_by(|a, b| {
        (a.0, a.1, &a.2, a.3.start, a.3.end).cmp(&(b.0, b.1, &b.2, b.3.start, b.3.end))
    });
    v
}

/// Run to completion, returning the number of slices it took.
fn compact_fully(app: &mut App, budget: usize) -> usize {
    let mut slices = 0;
    loop {
        match app.compact_slice(budget) {
            CompactStep::Finished => return slices + 1,
            CompactStep::Progressed => {
                slices += 1;
                assert!(slices < 100_000, "compaction is not terminating");
            }
            CompactStep::Idle => panic!("the pass stalled after {slices} slices"),
        }
    }
}

/// The endpoint: a completed pass reclaims the garbage and changes
/// nothing a reader can see.
#[test]
fn a_completed_pass_reclaims_the_garbage_and_preserves_the_document() {
    let mut app = app_with_garbage();
    check(&app, "before compaction");

    let before = observable(&app);
    let before_len = app.tree.len();
    let cursor = shape_of(&app, app.cursor);
    let first = shape_of(&app, app.first_node);

    compact_fully(&mut app, 4_096);

    assert!(
        app.tree.len() < before_len,
        "the arena must shrink: {before_len} -> {}",
        app.tree.len()
    );
    assert_eq!(app.dead_count, 0, "a finished pass leaves no garbage");
    assert!(!app.dead.iter().any(|d| *d));
    check(&app, "after compaction");

    assert_eq!(observable(&app), before, "the document must be unchanged");
    assert_eq!(shape_of(&app, app.cursor), cursor, "the cursor must follow");
    assert_eq!(shape_of(&app, app.first_node), first, "the viewport too");
}

/// The claim the design actually rests on, and the one the test above
/// cannot make: consistency *between* slices, not just at the end.
///
/// A budget of one re-checks the full arena after every single node
/// move, which is the granularity the event loop is allowed to
/// interrupt at.
#[test]
fn the_arena_is_consistent_after_every_single_move() {
    let mut app = app_with_garbage();
    let before = observable(&app);

    let mut moves = 0;
    loop {
        let step = app.compact_slice(1);
        check(&app, &format!("after move {moves}"));
        assert_eq!(
            observable(&app),
            before,
            "the document changed at move {moves}"
        );
        match step {
            CompactStep::Finished => break,
            CompactStep::Progressed => moves += 1,
            CompactStep::Idle => panic!("the pass stalled at move {moves}"),
        }
        assert!(moves < 100_000, "compaction is not terminating");
    }
    assert!(moves > 0, "the fixture must take more than one move");
    assert_eq!(app.dead_count, 0);
}

/// Fold state, the heat-recheck queue and the override target are all
/// keyed by index and so are repaired by equality/hash lookup rather
/// than by any link — a separate mechanism from the pointer repair,
/// and the one most easily forgotten when a field is added.
#[test]
fn index_keyed_state_follows_the_node_it_names() {
    let mut app = app_with_garbage();

    // Fold everything foldable, so the sets are not trivially empty and
    // span nodes on both sides of the holes.
    let mut n = Some(app.first_node);
    let mut folded_shapes = Vec::new();
    while let Some(i) = n {
        if app.tree[i].first_child.is_some() {
            app.folded.insert(i);
            app.refresh_line_counts(i);
            folded_shapes.push(shape_of(&app, i));
            app.pending_heat_recheck.insert(i);
        }
        n = app.tree[i].doc_next;
    }
    assert!(
        !folded_shapes.is_empty(),
        "the fixture must have something to fold"
    );
    let folded_shapes = sorted(folded_shapes);
    app.override_target = Some(app.first_node);
    let target = shape_of(&app, app.first_node);

    compact_fully(&mut app, 1);
    check(&app, "after compaction");

    let after = sorted(app.folded.iter().map(|i| shape_of(&app, *i)).collect());
    assert_eq!(after, folded_shapes, "folds must follow their nodes");

    let rechecks = sorted(
        app.pending_heat_recheck
            .iter()
            .map(|i| shape_of(&app, *i))
            .collect(),
    );
    assert_eq!(rechecks, folded_shapes, "heat rechecks must follow too");

    assert_eq!(
        shape_of(&app, app.override_target.unwrap()),
        target,
        "the override target must follow"
    );
}

/// A splice is the arena's only mutator, and it invalidates a pass in
/// flight: it appends past the end and can abandon nodes inside the
/// prefix a pass has already packed. The reset lives in
/// `splice_override` rather than in `render_overrides` precisely
/// because the live preview reaches the former without the latter.
#[test]
fn a_splice_abandons_the_pass_in_flight() {
    let mut app = app_with_garbage();

    assert_eq!(app.compact_slice(1), CompactStep::Progressed);
    assert_ne!(
        (app.compact_dst, app.compact_src),
        (0, 0),
        "a pass must be in flight for this test to mean anything"
    );

    let root = app.first_node;
    app.overrides.activate(
        override_pane::OverrideOrigin::Path {
            path: "/".to_string(),
        },
        None,
    );
    app.render_overrides(root);

    assert_eq!(
        (app.compact_dst, app.compact_src),
        (0, 0),
        "the splice must have abandoned the pass"
    );
    check(&app, "after a splice interrupted a pass");

    // And the arena is still compactable afterwards — abandoning loses
    // the scanning, not the ability to reclaim.
    let before = observable(&app);
    compact_fully(&mut app, 4_096);
    check(&app, "after the restarted pass");
    assert_eq!(observable(&app), before);
    assert_eq!(app.dead_count, 0);
}

/// The gate is a counter read, not a scan, because the event loop asks
/// on every idle iteration. A clean arena must cost nothing and must
/// not truncate or shrink anything.
#[test]
fn a_clean_arena_is_left_alone() {
    let (mut app, _outer, _inner) = type_as_fixture();
    assert_eq!(app.dead_count, 0);
    let len = app.tree.len();
    assert_eq!(app.compact_slice(4_096), CompactStep::Idle);
    assert_eq!(app.tree.len(), len);
    assert_eq!((app.compact_dst, app.compact_src), (0, 0));
    check(&app, "an untouched fixture");
}

/// `verify_arena` is load-bearing for every test above, so it has to be
/// shown to fail when the arena is actually broken — otherwise the
/// suite passes by checking nothing.
#[test]
fn the_verifier_rejects_a_broken_arena() {
    check(&app_with_garbage(), "the fixture itself");

    // A dangling reference — the failure mode compaction newly makes
    // fatal, and so the one most worth having a witness for.
    let mut app = app_with_garbage();
    let victim = app.dead.iter().position(|d| *d).unwrap();
    app.tree[app.first_node].doc_next = Some(victim);
    assert!(
        app.verify_arena().is_err(),
        "a live node pointing at a dead one must be rejected"
    );

    let mut app = app_with_garbage();
    let root = app.first_node;
    app.mark_dead(root);
    assert!(
        app.verify_arena().is_err(),
        "marking a live node dead must be rejected"
    );

    let mut app = app_with_garbage();
    app.dead_count += 1;
    assert!(
        app.verify_arena().is_err(),
        "a dead_count out of step with the flags must be rejected"
    );

    let mut app = app_with_garbage();
    let root = app.first_node;
    app.tree[root].last_child = None;
    assert!(
        app.verify_arena().is_err(),
        "a child chain disagreeing with last_child must be rejected"
    );
}
