// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `App::bake_step` — finishing a bounded confirm's document (spec 0255).

use super::super::bake::BakeStep;
use super::super::render::ACTIVITY_GLYPH;
use super::super::search::SearchScope;
use super::super::*;
use super::support::*;
use crate::node_status::Status;

/// Drive the bake to exhaustion, returning how many steps it took.
///
/// The cap is not a tuning knob but the assertion that the drain
/// terminates: every step must either render a body (removing its node
/// from `auto_folded`) or discard a stale queue entry, so a bake that
/// loops is a bake that has stopped making progress.
fn drain(app: &mut App) -> usize {
    let mut steps = 0;
    while app.bake_step() != BakeStep::Idle {
        steps += 1;
        assert!(steps < 1000, "the bake must terminate");
    }
    steps
}

/// Draw one real frame into a terminal `height` rows tall, which is what
/// populates `visible_stops` — `render_main_pane` is the only writer,
/// deliberately, so a test that wants a viewport has to draw one.
///
/// The document pane is `height - 2`: the global command/message row and
/// the main pane's own statusline each take one (spec 0147 G1).
fn draw(app: &mut App, height: u16) {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(40, height)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
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

/// Spec 0249 S8's scroll half: a reader who jumps is not in document
/// order, and the bake's queue is. A stop that is *on screen* is paid
/// off before the queue's head, however far down the document it sits.
///
/// The discriminator is the one-row pane: with the viewport on the last
/// stop, the queue still names the first, so an implementation that only
/// pops the queue expands the wrong node.
#[test]
fn a_stop_on_screen_is_baked_before_the_queue_order() {
    let (mut app, items) = repeated_message_fixture();
    let root = app.first_node;
    app.splash = false;

    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    assert_eq!(
        app.bake_queue.front(),
        Some(&items[0]),
        "the queue is in document order, so it names the first stop"
    );

    // One document row of pane, scrolled to the last stop: the bounded
    // document is the root's header, one folded row per Item, and the
    // root's footer, so the third stop is row 3.
    app.scroll.index = 3;
    draw(&mut app, 3);
    assert_eq!(
        app.visible_stops,
        vec![items[2]],
        "the frame drew exactly one stop, the third"
    );

    assert_eq!(app.bake_step(), BakeStep::Visible);
    assert!(
        !app.auto_folded.contains(&items[2]),
        "the stop the reader is looking at is the one that got a body"
    );
    assert!(
        app.auto_folded.contains(&items[0]) && app.auto_folded.contains(&items[1]),
        "and the ones off screen waited their turn: {:?}",
        app.auto_folded
    );

    // With nothing left on screen to owe, the bake goes back to the
    // queue — and says so, because the two answers buy different frames.
    assert_eq!(app.bake_step(), BakeStep::Progressed);
}

/// The same document either way (spec 0255 G2) — re-ordering the bake
/// must not change what it produces, only when. Without this, S8's
/// re-aim is a plausible source of a divergence that only shows up on a
/// 232 MB corpus.
#[test]
fn baking_what_is_on_screen_first_reaches_the_same_document() {
    let want = {
        let (mut app, _) = repeated_message_fixture();
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), None)
            .unwrap();
        (app.document_lines(), counts(&app))
    };

    let (mut app, _) = repeated_message_fixture();
    let root = app.first_node;
    app.splash = false;
    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .unwrap();

    // Drain with a frame between every step, from the bottom of the
    // document, so the visible path is taken as often as it can be.
    let mut steps = 0;
    loop {
        app.scroll.index = app.document_lines().len().saturating_sub(1);
        draw(&mut app, 3);
        if app.bake_step() == BakeStep::Idle {
            break;
        }
        steps += 1;
        assert!(steps < 1000, "the bake must still terminate");
    }

    assert!(app.auto_folded.is_empty(), "nothing still owes a body");
    assert_eq!((app.document_lines(), counts(&app)), want);
}

/// Spec 0249 S13, the ambient half. The dot spec 0190 already put in
/// column 0 is where the bake's cue lives — open question 4 — so this
/// asserts on that cell rather than on any new surface.
#[test]
fn a_bake_in_progress_lights_the_dot_in_violet() {
    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    let mut terminal = ratatui::Terminal::new(TestBackend::new(80, 10)).unwrap();
    let dot = (0u16, 9u16); // column 0 of the bottom (global) row
    let violet = theme::status_color(Status::Unbaked, app.theme);

    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[dot].symbol(),
        " ",
        "nothing is owed, so nothing is reported"
    );

    let root = app.first_node;
    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(terminal.backend().buffer()[dot].symbol(), ACTIVITY_GLYPH);
    assert_eq!(
        terminal.backend().buffer()[dot].style().fg,
        violet,
        "the same violet the unbaked fold toggles wear"
    );

    // The heat subsystem keeps the cell while it is using it: the bake
    // is ambient and a cue is about a row the user is looking at.
    app.activity_shown = Some(tiered::Tier::User);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(terminal.backend().buffer()[dot].symbol(), ACTIVITY_GLYPH);
    assert_ne!(terminal.backend().buffer()[dot].style().fg, violet);
    app.activity_shown = None;

    drain(&mut app);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[dot].symbol(),
        " ",
        "the debt is paid, so the cue goes out on its own"
    );
}

/// Spec 0249 S13, the consequential half. "Not found" is a claim about
/// the whole document, and a bake in progress is exactly the state in
/// which the search cannot make it.
///
/// Amended from the spec as written: S13 asked for the caveat on a
/// *count* of matches, and there is none to qualify — the sweep stops at
/// the first hit (spec 0246). A hit needs no caveat anyway; it is a real
/// row at a real position. The miss is the one answer an unbaked
/// remainder can falsify.
#[test]
fn a_search_that_misses_says_how_much_it_did_not_look_at() {
    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    app.main_area = ratatui::layout::Rect::new(0, 0, 40, 20);
    let root = app.first_node;

    app.splice_override(root, Some("test.Outer".to_string()), Some(2))
        .expect("a bounded splice must succeed");
    assert_eq!(app.auto_folded.len(), 3);

    app.run_search(SearchScope::Main, SearchDir::Forward, "nowhere");
    assert_eq!(
        app.message,
        "pattern not found: nowhere (3 subtrees not yet baked)"
    );

    drain(&mut app);
    app.message.clear();
    app.run_search(SearchScope::Main, SearchDir::Forward, "nowhere");
    assert_eq!(
        app.message, "pattern not found: nowhere",
        "with nothing left unread the search speaks for the whole document"
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
