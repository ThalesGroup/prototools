// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `App::bake_step` — finishing a bounded confirm's document (spec 0255).

use super::super::bake::BakeStep;
use super::super::*;
use super::support::*;

/// Drive the bake to exhaustion, returning how many steps it took.
///
/// The cap is not a tuning knob but the assertion that the drain
/// terminates: every step must either render a body (removing its node
/// from `auto_folded`) or discard a stale queue entry, so a bake that
/// loops is a bake that has stopped making progress.
fn drain(app: &mut App) -> usize {
    let mut steps = 0;
    while app.bake_step() == BakeStep::Progressed {
        steps += 1;
        assert!(steps < 1000, "the bake must terminate");
    }
    steps
}

/// Spec 0255 test-plan item 3. The resting state, asserted because this
/// runs on every idle iteration of the event loop forever.
#[test]
fn bake_step_is_idle_with_nothing_queued() {
    let (mut app, _) = repeated_message_fixture();
    assert!(app.auto_folded.is_empty(), "fixture starts fully rendered");
    assert_eq!(app.bake_step(), BakeStep::Idle);
}

/// Spec 0255 test-plan item 1: a bounded confirm leaves stops, and the
/// bake pays them off.
#[test]
fn a_bounded_confirm_leaves_stops_and_the_bake_drains_them() {
    let (mut app, items) = repeated_message_fixture();
    let root = app.first_node;

    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    assert_eq!(
        app.auto_folded.len(),
        items.len(),
        "each Item is one stop: {:?}",
        app.auto_folded
    );
    assert_eq!(
        app.bake_queue.len(),
        items.len(),
        "and one queue entry each: {:?}",
        app.bake_queue
    );

    let steps = drain(&mut app);

    assert_eq!(steps, items.len(), "one step per stop");
    assert!(
        app.auto_folded.is_empty(),
        "nothing still owes a body: {:?}",
        app.auto_folded
    );
    assert!(app.bake_queue.is_empty(), "and the order is spent too");
    for i in &items {
        assert!(
            app.tree[*i].lines_total > 2,
            "Item {i} must have a body after the bake"
        );
    }
}

/// Spec 0255 G2 / test-plan item 2, and the assertion the whole design
/// rests on: a document reached by a bounded confirm plus a bake is the
/// document an unbounded confirm would have produced. Not merely the
/// same height — the same text, and the same counts at every node.
///
/// The 232 MB corpus version of this is a `cmp`, and it is a
/// measurement rather than a test.
#[test]
fn a_baked_document_is_the_unbounded_document() {
    let want_lines = {
        let (mut app, _) = repeated_message_fixture();
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), None)
            .expect("an unbounded splice must succeed");
        app.document_lines()
    };
    let want_counts = {
        let (mut app, _) = repeated_message_fixture();
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), None)
            .unwrap();
        counts(&app)
    };

    let (mut app, _) = repeated_message_fixture();
    let root = app.first_node;
    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    assert_ne!(
        app.document_lines(),
        want_lines,
        "the bounded document must actually be short, or this proves nothing"
    );

    drain(&mut app);

    assert_eq!(app.document_lines(), want_lines, "same text");
    assert_eq!(counts(&app), want_counts, "same counts at every node");
    assert!(
        app.folded.is_empty(),
        "and the bake invented no user fold: {:?}",
        app.folded
    );
}

/// Every node's two line counts, in slot order — the structural half of
/// the equality above. `document_lines` alone would pass with the folds
/// still in place if the folded rows happened to match.
fn counts(app: &App) -> Vec<(u32, u32)> {
    app.tree
        .iter()
        .map(|n| (n.lines_total, n.lines_visible))
        .collect()
}

/// Spec 0255 S3: the queue is a hint and `auto_folded` is the truth, so
/// an entry whose node has been rendered since is discarded rather than
/// guarded against at every site that could render one.
///
/// Exercised through the gesture that actually causes it — spec 0249
/// S8's expand-on-arrival, which renders a stop the bake had queued.
/// Without the `contains` check this trips `expand_auto_fold`'s own
/// debug assertion.
#[test]
fn a_stale_bake_queue_entry_is_skipped() {
    let (mut app, items) = repeated_message_fixture();
    let root = app.first_node;
    app.main_area = ratatui::layout::Rect::new(0, 0, 40, 20);

    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    let queued = app.bake_queue.len();
    assert!(queued > 1, "need more than one entry to skip one");

    // The user arrives at the first stop and opens it by hand. It leaves
    // `auto_folded`, but the bake's queue still names it.
    app.open(items[0]);
    assert!(!app.auto_folded.contains(&items[0]));
    assert_eq!(app.bake_queue.len(), queued, "the queue is not repaired");

    let steps = drain(&mut app);

    assert_eq!(
        steps,
        queued - 1,
        "the entry the user already paid off costs no step"
    );
    assert!(app.auto_folded.is_empty());
}

/// Spec 0255 S2 / N6 / test-plan item 5: nothing bounds a confirm unless
/// there is an event loop to bake the remainder in.
///
/// This is what keeps a headless `export` complete, and it is asserted
/// on the *document* rather than on the budget alone. Making
/// `confirm_row_budget` unconditional — the obvious way to get this
/// wrong — leaves `document_pane_height()` at zero, so every export
/// would render two rows per node and silently write a truncated file.
/// No existing export test catches that, because their fixtures are
/// shallower than the floor.
#[test]
fn a_confirm_is_unbounded_without_an_event_loop() {
    let (mut app, _) = repeated_message_fixture();
    assert!(
        !app.bounded_confirms,
        "the default is the safe one: no bake, no budget"
    );
    assert_eq!(app.confirm_row_budget(), None);

    // A whole override pass, as `App::new` and `run_export` run it —
    // driven through `resettle_node`, which is where the policy is
    // read, rather than by calling `splice_override` directly. The
    // provenance reset is what makes the pass do work at all; a settled
    // node is skipped.
    let root = app.first_node;
    app.tree[root].rendered_as = crate::provenance::NOT_RENDERED;
    app.render_overrides(root);

    assert!(
        app.auto_folded.is_empty(),
        "a pass with no bake behind it must leave nothing folded: {:?}",
        app.auto_folded
    );
    assert!(
        app.bake_queue.is_empty(),
        "and nothing queued: {:?}",
        app.bake_queue
    );
    let full = app.document_lines();

    // Turning it on is what shortens the document, and the budget is
    // the pane's own height rather than anything derived from the
    // document.
    app.main_area = ratatui::layout::Rect::new(0, 0, 40, 20);
    assert_eq!(
        app.confirm_row_budget(),
        None,
        "a known pane size is not what turns it on"
    );
    app.bounded_confirms = true;
    assert_eq!(app.confirm_row_budget(), Some(20));

    assert!(
        full.len() > 3,
        "the fixture must have a body to leave out: {full:?}"
    );
}
