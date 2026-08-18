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
///
/// Run at three budgets (spec 0256 test-plan item 5): the budget is the
/// pane's height, so it is the one input a user varies without meaning
/// to, and G3 is what makes deferring the old document's frees safe.
#[test]
fn a_baked_document_is_the_unbounded_document() {
    let want_lines = {
        let (mut app, _) = repeated_message_fixture();
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), None)
            .expect("an unbounded splice must succeed");
        app.document_lines()
    };
    let (want_counts, want_folded) = {
        let (mut app, _) = repeated_message_fixture();
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), None)
            .unwrap();
        (counts(&app), app.folded.clone())
    };

    // From `MIN_EXPAND_ROWS` up. A budget of 1 is not a smaller case of
    // this but a different one — it buys the header and nothing else, so
    // the node stops at itself and the walk cannot move down, which is
    // why `confirm_row_budget` clamps it away before any caller sees it.
    for budget in [App::MIN_EXPAND_ROWS, 3, 7] {
        let (mut app, _) = repeated_message_fixture();
        app.bounded_confirms = true;
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), Some(budget))
            .expect("a bounded splice must succeed");
        assert_ne!(
            app.document_lines(),
            want_lines,
            "budget {budget}: the bounded document must actually be short, \
             or this proves nothing"
        );

        drain(&mut app);
        while app.discard_step() {}

        assert_eq!(
            app.document_lines(),
            want_lines,
            "budget {budget}: same text"
        );
        assert_eq!(
            counts(&app),
            want_counts,
            "budget {budget}: same counts at every node"
        );
        // Spec 0323 S2: a splice writes every bracketed slot folded, so
        // "no fold" is no longer the resting state and `is_empty` would
        // be the wrong question. The claim that survives is the one this
        // test was always making — the bake invents nothing. Whatever
        // folds the unbounded splice ends with, the bounded one plus its
        // drain must end with exactly those.
        assert_eq!(
            app.folded, want_folded,
            "budget {budget}: and the bake invented no fold of its own"
        );
    }
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
        !app.auto_folded.contains(items[2]),
        "the stop the reader is looking at is the one that got a body"
    );
    assert!(
        app.auto_folded.contains(items[0]) && app.auto_folded.contains(items[1]),
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
    assert!(!app.auto_folded.contains(items[0]));
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
    app.tree_mut()[root].rendered_as = crate::provenance::NOT_RENDERED;
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

/// `nested_any_fixture` with its root re-rendered under a row budget,
/// leaving the `Any` three plain levels down on the revealed side of the
/// cut.
///
/// A standalone `splice_override`, as the spec 0255 tests above use:
/// the confirm's surrounding `render_overrides` walk would resettle its
/// way straight back down a twelve-line document and leave no stop at
/// all. What the walk cannot reach on a real corpus is exactly what this
/// spec is about, so the splice is driven directly.
fn bounded_any_fixture(budget: usize) -> App {
    let mut app = nested_any_fixture();
    app.bounded_confirms = true;
    app.main_area = ratatui::layout::Rect::new(0, 0, 40, budget as u16);
    let root = app.first_node;
    app.splice_override(root, Some("acme.Level1".to_string()), Some(budget))
        .expect("a bounded splice must succeed");
    app
}

/// Spec 0258 G1, and the defect the spec exists for: `expand_auto_fold`
/// spliced without ever entering a `render_overrides` batch, so spec
/// 0120's `Any` expansion never ran on what the bake revealed — and
/// `descend`'s watermark (spec 0188 S4) over a fixed arena (spec 0216)
/// meant no later batch would revisit it either.
///
/// The `Any` sits three plain levels down, below any screenful this
/// fixture can be given, which is what puts it on the revealed side of
/// the cut.
#[test]
fn a_revealed_subtree_expands_any() {
    let want = nested_any_fixture().document_lines();

    let mut app = bounded_any_fixture(App::MIN_EXPAND_ROWS);
    assert!(
        !app.auto_folded.is_empty(),
        "the bounded pass must have stopped somewhere"
    );
    assert!(
        !has_node_with_type(&app, "acme.Payload"),
        "and stopped above the Any, or this proves nothing: {:?}",
        app.document_lines()
    );

    drain(&mut app);

    assert!(
        has_node_with_type(&app, "acme.Payload"),
        "the revealed Any must expand: {:?}",
        app.document_lines()
    );
    assert_eq!(
        app.document_lines(),
        want,
        "and the baked document is the unbounded one"
    );
}

/// The other half of `collect_descend_targets`. An `Any` is found by
/// `is_auto_expand_candidate`; an override entry the user placed is
/// found by its path. One route working does not imply the other, and
/// the path route is the one a `--load-overrides` file depends on.
#[test]
fn a_revealed_subtree_applies_a_path_override() {
    // The `label` leaf inside the expanded payload — five plain levels
    // down, and only reachable once the Any has expanded, so this is
    // also an override on a node the bounded render never emitted.
    //
    // `want` is the unbounded document with the same entry already
    // applied, and the assertion is an equality against it. An absence
    // assertion — "`label` is gone, so the override ran" — would be
    // vacuous here: `label` is equally absent when the `Any` never
    // expanded at all, so the test would pass with the fix removed.
    let (origin, want) = {
        let mut app = nested_any_fixture();
        let payload = node_with_type(&app, "acme.Payload").expect("the Any must have expanded");
        let deep = app
            .first_child(payload)
            .expect("the payload must have its `label` field");
        let origin = override_pane::OverrideOrigin::Path {
            path: app.positional_path(deep),
        };
        app.overrides.activate(origin.clone(), None);
        let root = app.first_node;
        app.render_overrides(root);
        (origin, app.document_lines())
    };
    assert_ne!(
        want,
        nested_any_fixture().document_lines(),
        "the entry must change the unbounded document, or the comparison \
         below proves nothing"
    );

    let mut app = bounded_any_fixture(App::MIN_EXPAND_ROWS);
    app.overrides.activate(origin, None);
    drain(&mut app);

    assert_eq!(
        app.document_lines(),
        want,
        "the revealed subtree must honor the entry the way an unbounded \
         render does"
    );
}

/// Spec 0258 S3. A bake runs thousands of passes nobody asked for, and
/// the user's own open gesture asked to see a body rather than to apply
/// an override — so a refusal from either is not an answer to a question
/// and must not take the status line.
#[test]
fn a_revealed_subtree_reports_no_refusal() {
    let origin = {
        let app = nested_any_fixture();
        let payload = node_with_type(&app, "acme.Payload").expect("the Any must have expanded");
        override_pane::OverrideOrigin::Path {
            path: app.positional_path(payload),
        }
    };

    let mut app = bounded_any_fixture(App::MIN_EXPAND_ROWS);
    app.overrides
        .activate(origin, Some("acme.NoSuchType".to_string()));
    app.message.clear();

    drain(&mut app);

    assert!(
        !app.refusals.is_empty(),
        "the override must actually have been refused, or this asserts \
         nothing"
    );
    assert_eq!(
        app.message, "",
        "and the refusal stayed off the status line"
    );
}

/// Spec 0258 S1's other caller. `expand_auto_fold` is reached from the
/// bake and from the user opening a fold over a stop (spec 0249 S8), and
/// the second is the one a reader notices — a subtree that renders one
/// way when the bake got there first and another way when they did.
///
/// This is why the fix sits in `expand_auto_fold` rather than in
/// `bake_step`. A reader who outruns the bake — which is the whole point
/// of expand-on-arrival — must not be shown a different document for it,
/// so this reaches the bottom without a single `bake_step`.
///
/// A stop whose body is itself an `Any` is expanded by the pass over its
/// *parent*, not by its own: `mark_fresh_subtree` collects strict
/// descendants, and by the time a node is a stop it has already been
/// emitted as a header by the render that stopped there.
#[test]
fn an_opened_stop_expands_any_too() {
    let want = nested_any_fixture().document_lines();

    let mut app = bounded_any_fixture(App::MIN_EXPAND_ROWS);
    let mut opened = 0;
    // `let … else` rather than `while let`, so that the borrow the
    // iterator holds on `app` ends before `open` takes it mutably.
    loop {
        let Some(stop) = app.auto_folded.iter().next() else {
            break;
        };
        app.open(stop);
        opened += 1;
        assert!(opened < 100, "opening stops by hand must terminate");
    }

    assert!(
        has_node_with_type(&app, "acme.Payload"),
        "reading down by hand must expand the Any: {:?}",
        app.document_lines()
    );
    assert_eq!(
        app.document_lines(),
        want,
        "and reach the document the bake would have"
    );
}

// ── Spec 0257: the startup render is bounded too ────────────────────────

/// Spec 0257 test-plan item 1. The startup render reports stops the same
/// way a confirm's does, and `App::new` has to do with them what
/// `splice_override` does: fold each one, queue it, and fix the row count
/// folding it changed.
///
/// The `lines_visible == 1` assertion is the one that catches the missing
/// `refresh_line_counts`. Without it a stop claims the two rows it
/// rendered while the document draws one, so every row number below the
/// first stop is wrong — and nothing else in the frame says so.
#[test]
fn a_bounded_startup_leaves_stops() {
    let (app, items) = bounded_repeated_message_fixture(3);

    assert!(
        app.bounded_confirms,
        "a budgeted startup is a bounded session (spec 0257 S3)"
    );
    assert_eq!(
        app.auto_folded.len(),
        2,
        "the budget bought the first Item's body and stopped at the other \
         two: {:?}",
        app.auto_folded
    );
    for i in [items[1], items[2]] {
        assert!(app.auto_folded.contains(i), "Item {i} must be a stop");
        assert!(
            app.bake_queue.contains(&i),
            "and must be queued for the bake"
        );
        assert_eq!(
            (app.tree[i].lines_total, app.tree[i].lines_visible),
            (2, 1),
            "a stop rendered a header and a footer, and draws one row"
        );
    }
    assert!(
        !app.auto_folded.contains(items[0]),
        "the first Item was inside the budget"
    );
}

/// Spec 0257 G3 / test-plan item 2, and the assertion the spec rests on:
/// a document reached by a bounded startup plus a full bake is the
/// document an unbounded startup produces. Not merely the same height —
/// the same text and the same counts at every node.
///
/// The corpus version is a `cmp` over 249 MB, and it is a measurement
/// rather than a test.
///
/// Parametrized from `MIN_EXPAND_ROWS` up. A budget of 1 is not a smaller
/// case of this but a different one — it buys the root's header and
/// nothing else, so the walk never descends and the drain has a raw
/// document to finish rather than a short one. That is what the clamp in
/// `main`'s `startup_row_budget` exists for.
#[test]
fn a_baked_startup_is_the_unbounded_startup() {
    let (want_lines, want_counts) = {
        let (app, _) = repeated_message_fixture();
        (app.document_lines(), counts(&app))
    };

    for budget in [App::MIN_EXPAND_ROWS, 3, 7] {
        let (mut app, _) = bounded_repeated_message_fixture(budget);
        assert_ne!(
            app.document_lines(),
            want_lines,
            "budget {budget}: the bounded document must actually be short, \
             or this proves nothing"
        );

        drain(&mut app);
        while app.discard_step() {}

        assert_eq!(
            app.document_lines(),
            want_lines,
            "budget {budget}: same text"
        );
        assert_eq!(
            counts(&app),
            want_counts,
            "budget {budget}: same counts at every node"
        );
        assert!(
            app.folded.is_empty(),
            "budget {budget}: and the startup invented no user fold: {:?}",
            app.folded
        );
    }
}

/// Spec 0257 S4. `main` passes no budget when there is no event loop to
/// bake in, and this is the whole of what "no budget" has to mean: a
/// complete document and nothing owed.
///
/// The guard between a scripted `export` and a truncated file — spec 0257
/// N5 records that `push_subtree_lines` writes an empty pair of braces
/// for a stop rather than refusing, so a regression here is silent.
#[test]
fn a_headless_startup_renders_whole() {
    let (app, items) = repeated_message_fixture();

    assert!(
        !app.bounded_confirms,
        "no budget means no bounded confirms either (spec 0257 S3)"
    );
    assert!(
        app.auto_folded.is_empty() && app.bake_queue.is_empty(),
        "nothing owes a body: {:?}",
        app.auto_folded
    );
    for i in items {
        assert!(
            app.tree[i].lines_total > 2,
            "Item {i} rendered its body straight away"
        );
    }
}

/// Spec 0257 G2 / test-plan item 4: every row of the first frame is
/// final. A stop drawn on it would pop open a moment later under the
/// bake's `Visible` arm, which is a document that moves while it is being
/// read.
///
/// This is also the check that S2's arithmetic agrees with the
/// renderer's, and it is what found the off-by-one that arithmetic now
/// carries: the budget counts lines *emitted*, and a stop's own header is
/// one of them, so the first stop lands on document line `budget`
/// exactly. `main` therefore asks for one row more than the pane, and the
/// pane here is `budget - 1` document rows — plus the command row and the
/// statusline, which is why the terminal is `budget + 1` tall.
///
/// Driven at three budgets because the bound has to hold for *every*
/// stop, not just the first, and how many stops there are is what the
/// budget varies.
#[test]
fn a_first_frame_over_a_bounded_document_has_no_visible_stop() {
    for budget in [App::MIN_EXPAND_ROWS, 3, 4] {
        let (mut app, _) = bounded_repeated_message_fixture(budget);
        assert!(
            !app.auto_folded.is_empty(),
            "budget {budget}: there must be a stop somewhere to miss"
        );

        draw(&mut app, budget as u16 + 1);

        assert!(
            app.visible_stops.is_empty(),
            "budget {budget}: the first frame drew a stop: {:?}",
            app.visible_stops
        );
    }
}

// ── Spec 0260: a fold nobody has read says so ───────────────────────────

/// The foreground colors of the `{ ... }` collapse summary on `node`'s
/// row, one per character.
///
/// Read character-wise rather than off the span list's shape: whether
/// the summary arrives as one span or three is an implementation detail
/// of `spans_with_insertions`, and a test that asserted on it would
/// break for the wrong reason. Searched over `char`s, not bytes — the
/// fold margin ahead of it holds a multi-byte glyph.
fn summary_colors(app: &mut App, node: usize) -> Vec<Option<Color>> {
    let row = visible_row_of(app, node);
    let window = app.build_window(row, 1);
    app.refresh_window_styles(&window);
    let drawn: Vec<(char, Option<Color>)> = app
        .row_spans(window[0], 0, Modifier::empty())
        .iter()
        .flat_map(|s| {
            let fg = s.style.fg;
            s.content.chars().map(move |c| (c, fg)).collect::<Vec<_>>()
        })
        .collect();
    let want: Vec<char> = "{ ... }".chars().collect();
    let at = drawn
        .windows(want.len())
        .position(|w| w.iter().map(|(c, _)| *c).eq(want.iter().copied()))
        .unwrap_or_else(|| {
            let text: String = drawn.iter().map(|(c, _)| *c).collect();
            panic!("no collapse summary on {text:?}")
        });
    drawn[at..at + want.len()].iter().map(|(_, c)| *c).collect()
}

/// Spec 0260 G2: a node the bake has not reached draws its whole brace
/// pair in the `Unbaked` violet — including the opening brace, which
/// would otherwise keep its grammar color and split the cue in two.
#[test]
fn an_unbaked_fold_is_violet() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let violet = theme::status_color(Status::Unbaked, app.theme);
    assert!(violet.is_some(), "the fixture must run on an RGB palette");

    assert!(app.auto_folded.contains(items[1]), "the stop the bake owes");
    assert_eq!(
        summary_colors(&mut app, items[1]),
        vec![violet; 7],
        "every character of `{{ ... }}`, the brace included"
    );

    app.expand_auto_fold(items[1], App::BAKE_ROW_BUDGET);
    let row = visible_row_of(&app, items[1]);
    assert!(
        !app.row_content(app.build_window(row, 1)[0]).contains("..."),
        "the debt is paid, so there is nothing left to summarize"
    );
}

/// Spec 0260 N4/S3: a region the reader collapsed is complete, and the
/// cue must mean "unread" rather than "folded". Fails if the predicate
/// is `is_folded`.
#[test]
fn a_hand_folded_region_is_not_violet() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;
    let violet = theme::status_color(Status::Unbaked, app.theme);
    assert!(
        app.auto_folded.is_empty(),
        "nothing is owed in this fixture"
    );

    app.toggle_fold(items[1]);
    let colors = summary_colors(&mut app, items[1]);
    assert_eq!(colors.len(), 7);
    assert!(
        colors.iter().all(|c| *c != violet),
        "a collapsed region must not claim to be an unread one: {colors:?}"
    );
}

/// Spec 0260 S3: the two fold sets are not exclusive — the user can fold
/// a stop — and such a node is still one nobody has looked inside, so
/// `auto_folded` membership is the test rather than which set folded it
/// last.
#[test]
fn a_folded_stop_the_user_also_folded_stays_violet() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let violet = theme::status_color(Status::Unbaked, app.theme);

    assert!(app.auto_folded.contains(items[1]));
    app.folded.insert(items[1]);

    assert_eq!(summary_colors(&mut app, items[1]), vec![violet; 7]);
}

// ── Spec 0259: the rows on screen stay where they are ───────────────────

/// Which line the main pane draws on its top row, as the node that owns
/// it — the quantity spec 0259 is about, and the one a row number cannot
/// express across a splice.
fn top_row(app: &App) -> LinePos {
    match app.build_window(app.scroll.index, 1).first() {
        Some(DisplayRow::Committed(c)) => c.pos,
        _ => panic!("the pane must have a committed top row"),
    }
}

/// Park the viewport so that `node`'s header is the pane's top row, with
/// the caret on it, and draw a frame — which is what captures the anchor.
fn park_on(app: &mut App, node: usize, height: u16) {
    app.scroll.index = visible_row_of(app, node);
    app.cursor = node;
    app.cursor_line_in_node = 0;
    draw(app, height);
}

/// Park the viewport at `row` with the caret left behind on the root's
/// header — the state an `Alt` pan produces, which spec 0244 supports
/// deliberately and which `last_cursor_row` exists to keep the caret
/// clamp from fighting.
///
/// The discriminating setup for every anchor test but the first: with the
/// caret on the anchored row, `clamp_scroll_to_cursor` restores the
/// viewport all by itself and the anchor is never exercised. With the
/// caret parked on row 0 — a row no splice below it can move — the anchor
/// is the only thing that can hold the viewport.
fn park_viewport_at(app: &mut App, row: usize, height: u16) {
    app.cursor = app.first_node;
    app.cursor_line_in_node = 0;
    app.scroll.index = row;
    app.last_cursor_row = Some(0);
    draw(app, height);
    assert_eq!(app.scroll.index, row, "the caret clamp must not have moved");
}

fn visible_row_of(app: &App, node: usize) -> usize {
    app.visible_row_of_line(app.absolute_start(node))
        .expect("the node must be on a drawn row")
}

/// Spec 0259 G1, and the Background's measurement as an assertion: a bake
/// landing above the viewport must not move the rows in it.
///
/// The discriminator is that the viewport is *scrolled*. With the pane at
/// the top of the document there is nothing above it to grow, so the
/// defect cannot show and neither can the fix.
#[test]
fn a_bake_above_the_viewport_holds_the_rows() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;

    // The last Item, which sits below the stop at `items[1]`.
    park_on(&mut app, items[2], 5);
    let before_top = top_row(&app);
    let before_caret = app.terminal_row_of(app.cursor_display_row());
    assert_eq!(before_top.node, items[2], "the setup must have scrolled");

    assert!(
        app.auto_folded.contains(items[1]),
        "there must be a stop above the viewport to bake"
    );
    app.expand_auto_fold(items[1], App::BAKE_ROW_BUDGET);

    assert_eq!(
        top_row(&app),
        before_top,
        "the row at the top of the pane must be the row it was"
    );
    assert_eq!(
        app.terminal_row_of(app.cursor_display_row()),
        before_caret,
        "and the caret must keep its terminal row"
    );
}

/// Spec 0259 S2. A reader who has pressed `G` is sitting on a stack of
/// closing braces, and a closing brace lives at `lines_total - 1` — a
/// coordinate that moves the moment the body it closes is baked in.
///
/// Anchoring it from the start instead lands inside the new body, which
/// is a silent wrong answer rather than a crash, so this asserts on the
/// identity of the row and not merely on its existence.
#[test]
fn an_anchor_on_a_footer_survives_its_body_arriving() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let root = app.first_node;

    // The document's last row is the root's own closing brace — where `G`
    // puts a reader of a bounded document.
    let last_row = app.visible_row_count() - 1;
    park_viewport_at(&mut app, last_row, 3);

    let before_top = top_row(&app);
    assert!(
        app.is_footer(before_top),
        "the setup must park the pane on a closing brace: {before_top:?}"
    );
    assert_eq!(before_top.node, root);

    assert!(app.auto_folded.contains(items[1]));
    app.expand_auto_fold(items[1], App::BAKE_ROW_BUDGET);

    let after_top = top_row(&app);
    assert_eq!(
        (after_top.node, app.is_footer(after_top)),
        (root, true),
        "the closing brace must still be the top row, not a line of the \
         body that just arrived inside it"
    );
    // And it is a *different* coordinate than the one captured, which is
    // exactly why the anchor cannot store one.
    assert_ne!(
        after_top.line_in_node, before_top.line_in_node,
        "the brace must have moved, or this proves nothing"
    );
}

/// The caret's own `line_in_node` is stale for exactly as long as the
/// footer anchor's is, and for the same reason: a bake grows the node
/// whose closing brace it sits on. `splice_override` repairs it — but
/// `finalize_override_batch` *reads* it, through `clamp_pan_offset`, to
/// decide whether the caret needs scrolling into view.
///
/// So the repair has to happen first. With the two in the other order the
/// clamp reads a coordinate pointing into the body of the node the brace
/// closes and scrolls the viewport by however far the two differ — a
/// defect that predates this spec and that the anchor cannot cover for,
/// because the anchor runs inside the same finalizer.
#[test]
fn a_caret_on_a_brace_does_not_drag_the_viewport() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let root = app.first_node;

    // Caret on the root's closing brace, viewport at the bottom of the
    // document — where `G` leaves a reader.
    app.cursor = root;
    app.cursor_line_in_node = app.tree[root].lines_total - 1;
    app.scroll.index = app.visible_row_count() - 1;
    draw(&mut app, 3);

    let before_top = top_row(&app);
    assert!(app.is_footer(before_top), "the setup must park on a brace");

    assert!(app.auto_folded.contains(items[1]));
    app.expand_auto_fold(items[1], App::BAKE_ROW_BUDGET);

    assert_eq!(
        app.cursor_line_in_node,
        app.tree[root].lines_total - 1,
        "the caret must still be on the brace it was on"
    );
    assert_eq!(
        (top_row(&app).node, app.is_footer(top_row(&app))),
        (root, true),
        "a stale caret coordinate must not have scrolled the pane off the \
         brace"
    );
}

/// The negative case. Without it the restore could pass every test above
/// by scrolling to a fixed place rather than by holding a node still.
#[test]
fn a_bake_below_the_viewport_moves_nothing() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;

    park_on(&mut app, items[0], 5);
    let before_top = top_row(&app);
    let before_scroll = app.scroll;

    assert!(
        app.auto_folded.contains(items[2]),
        "the stop must be below the viewport"
    );
    app.expand_auto_fold(items[2], App::BAKE_ROW_BUDGET);

    assert_eq!(top_row(&app), before_top);
    assert_eq!(
        app.scroll, before_scroll,
        "a bake below the fold must not move the viewport at all"
    );
}

/// Spec 0259 G2: a confirm is a splice batch like any other. The user
/// asked for a new interpretation, not to be scrolled somewhere else.
#[test]
fn a_confirm_holds_the_rows_too() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;

    let row = visible_row_of(&app, items[2]);
    park_viewport_at(&mut app, row, 5);
    let before_top = top_row(&app);
    let before_rows = app.visible_row_count();
    assert_eq!(before_top.node, items[2], "the setup must have scrolled");

    // Flattening the *first* Item to a scalar drops every row it drew,
    // which renumbers everything below it — the row the pane is on
    // included.
    app.splice_override(items[0], Some("bytes".to_string()), None)
        .expect("flattening to a scalar must succeed");
    assert_ne!(
        app.visible_row_count(),
        before_rows,
        "the confirm must actually have changed the document's height, or \
         this proves nothing"
    );

    assert_eq!(
        top_row(&app),
        before_top,
        "confirming an override elsewhere must not scroll the reader"
    );
}

/// Spec 0259 S4 / spec 0249 S10a. The anchor's node can stop being
/// rendered outright: an override on an ancestor can flatten it to
/// `bytes`, after which it has no row of its own to return to.
///
/// The rule is to climb to the nearest rendered ancestor, which is the
/// node that swallowed it — where the override the user just made
/// actually took effect. What must not happen is a panic or a silent
/// jump to row 0.
#[test]
fn an_anchor_climbs_out_of_a_flattened_subtree() {
    let (mut app, items) = repeated_message_fixture();
    app.splash = false;

    // Park on a row *inside* the first Item, then flatten the Item.
    let inner = app
        .first_child(items[0])
        .expect("an Item must have a field to park on");
    park_on(&mut app, inner, 8);
    assert_eq!(top_row(&app).node, inner);

    app.splice_override(items[0], Some("bytes".to_string()), None)
        .expect("flattening to a scalar must succeed");

    assert_eq!(
        app.tree[inner].lines_total, 0,
        "the anchored node must actually have lost its rendering, or this \
         proves nothing"
    );
    assert_eq!(
        top_row(&app).node,
        items[0],
        "the anchor climbs to the node that swallowed it"
    );
}

/// Spec 0257 test-plan item 5, and why spec 0258 is a prerequisite
/// rather than a nice-to-have.
///
/// Before this spec the startup render was followed by one
/// `render_overrides` pass over the whole document, so every `Any`
/// anywhere in the file expanded. Bounding the render moves everything
/// below the first screenful onto the bake's side of the cut, where spec
/// 0258's fix is the only thing that expands it — on every file, in every
/// session, rather than only after a root override plus scrolling.
#[test]
fn a_bounded_startup_expands_any_below_the_screenful() {
    let want = nested_any_fixture().document_lines();

    let mut app = bounded_nested_any_fixture(App::MIN_EXPAND_ROWS);
    assert!(
        !app.auto_folded.is_empty(),
        "the budget must have stopped above the Any"
    );
    assert!(
        !has_node_with_type(&app, "acme.Payload"),
        "and the Any must still be unexpanded, or this proves nothing: {:?}",
        app.document_lines()
    );

    drain(&mut app);

    assert!(
        has_node_with_type(&app, "acme.Payload"),
        "the bake must expand an Any the startup never reached: {:?}",
        app.document_lines()
    );
    assert_eq!(
        app.document_lines(),
        want,
        "and land on the document an unbounded startup produces"
    );
}

// ── Spec 0261: an export waits for the lines it names ──────────────────────

/// Export the cursor to a scratch file and read the bytes back.
///
/// `run_export` writes to a path rather than returning anything, so a
/// file is not optional. It is removed before the caller's assertion, not
/// after it, so a red run leaves nothing behind.
fn exported(app: &mut App, tag: &str, flags: &[&str]) -> Option<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("protolens-0261-{tag}.pb"));
    let _ = std::fs::remove_file(&path);
    let as_arg = path.to_string_lossy().into_owned();
    let mut argv: Vec<&str> = flags.to_vec();
    argv.push(&as_arg);
    app.run_export(argv);
    let written = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(&path);
    written
}

/// Spec 0261 test-plan item 1, and the defect the spec exists for: a stop
/// exported as text used to come out as its header and its closing brace
/// with nothing in between — an empty message rather than an unread one.
///
/// Also the scope: an export names one node, and the stops it pays off
/// are the ones under that node. A sibling stop is nobody's business
/// here and must be left for the bake.
#[test]
fn an_export_of_a_stop_is_whole() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let target = items[2];
    let sibling = items[1];
    assert!(
        app.auto_folded.contains(target) && app.auto_folded.contains(sibling),
        "the fixture must leave both of the last two items unbaked"
    );

    app.set_cursor(target);
    let got = exported(&mut app, "stop", &[]).expect("the export must write a file");
    assert!(
        !app.auto_folded.contains(target),
        "the export must have paid off the node it names"
    );
    assert!(
        app.auto_folded.contains(sibling),
        "and only that node — a sibling stop is the bake's business"
    );

    let (mut whole, items) = repeated_message_fixture();
    whole.splash = false;
    whole.set_cursor(items[2]);
    let want = exported(&mut whole, "stop-whole", &[]).expect("a file");

    let got = String::from_utf8(got).unwrap();
    assert_eq!(got, String::from_utf8(want).unwrap());
    assert!(
        got.contains("v: 7"),
        "the item's body must be in the file, got {got:?}"
    );
}

/// Test-plan item 2: the root, which has every stop in the document
/// under it — the case with the most to wait for.
#[test]
fn an_export_of_the_root_is_whole() {
    let (mut app, _) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let root = app.first_node;
    app.set_cursor(root);
    let got = exported(&mut app, "root", &[]).expect("the export must write a file");
    assert!(
        app.auto_folded.is_empty(),
        "an export of the root has every stop under it"
    );

    let (mut whole, _) = repeated_message_fixture();
    whole.splash = false;
    let root = whole.first_node;
    whole.set_cursor(root);
    let want = exported(&mut whole, "root-whole", &[]).expect("a file");

    let got = String::from_utf8(got).unwrap();
    assert_eq!(got, String::from_utf8(want).unwrap());
    for v in ["v: 5", "v: 6", "v: 7"] {
        assert!(
            got.contains(v),
            "the whole document must be in the file: {v}"
        );
    }
}

/// Test-plan item 3 / S4 / N4. `--binary` slices the blob by the node's
/// raw range and never reads a rendered line, so it must not pay for a
/// drain it has no use for — on a real document that is the difference
/// between an instant root export and a multi-second one.
#[test]
fn a_binary_export_does_not_wait() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    app.set_cursor(items[2]);
    let stops = app.auto_folded.len();
    assert!(
        stops > 0,
        "the fixture must have something to skip waiting for"
    );

    let got = exported(&mut app, "binary", &["--binary"]).expect("a file");
    assert_eq!(
        app.auto_folded.len(),
        stops,
        "--binary must not have expanded anything"
    );

    let (mut whole, items) = repeated_message_fixture();
    whole.splash = false;
    whole.set_cursor(items[2]);
    let want = exported(&mut whole, "binary-whole", &["--binary"]).expect("a file");
    assert_eq!(got, want, "the bytes are the blob's, baked or not");
}

/// Test-plan item 4: the second truncation, which spec 0257 N5 does not
/// name. The descriptor formats read the *shape* of the cursor's
/// children through `child_slots`, which reports a node whose first
/// child is unrendered as having none — so an unbaked message exported
/// as a `FileDescriptorSet` used to come out with no fields at all.
#[test]
fn a_descriptor_export_of_a_stop_has_its_fields() {
    use prost_reflect::prost::Message as _;

    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    assert!(app.auto_folded.contains(items[2]));
    app.set_cursor(items[2]);

    let bytes =
        exported(&mut app, "descriptor", &["--descriptor-binary"]).expect("the export must write");
    let fds = prost_types::FileDescriptorSet::decode(&bytes[..]).expect("a descriptor set");
    let message = &fds.file[0].message_type[0];
    assert_eq!(
        message.field.len(),
        1,
        "the stop's own field must be described, got {:#?}",
        message.field
    );
    assert_eq!(message.field[0].number, Some(1));
}

/// Test-plan item 5 / S5. The one case the drain cannot fix is the one
/// case the user has to be told about: an export that would truncate
/// writes nothing at all and says why.
#[test]
fn a_refused_expansion_refuses_the_export() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let stop = items[2];
    assert!(app.auto_folded.contains(stop));

    // A splice refuses when its target does not resolve, and
    // `expand_auto_fold` re-renders a node under the target its own
    // provenance records — so naming a type that is not in the pool is
    // enough to make the expansion fail the way a real one would.
    let bogus = app
        .provenance
        .intern(&(Some(Some("no.such.Type".to_string())), "items".to_string()));
    app.tree_mut()[stop].rendered_as = bogus;
    app.set_cursor(stop);

    assert!(
        exported(&mut app, "refused", &[]).is_none(),
        "a refused export must write no file at all"
    );
    assert!(
        app.message.starts_with("export refused:"),
        "the refusal must say so, got {:?}",
        app.message
    );
}

/// Test-plan item 6 / S2. A refused splice leaves its node in
/// `auto_folded` on purpose, so a drain that retried until the set
/// emptied would never return. One attempt per node — and the refusal of
/// one node must not stop the descent from clearing the others.
#[test]
fn bake_subtree_attempts_each_node_once() {
    let (mut app, items) = bounded_repeated_message_fixture(3);
    app.splash = false;
    let stop = items[2];
    let bogus = app
        .provenance
        .intern(&(Some(Some("no.such.Type".to_string())), "items".to_string()));
    app.tree_mut()[stop].rendered_as = bogus;

    let others: Vec<usize> = app.auto_folded.iter().filter(|&i| i != stop).collect();
    assert!(
        !others.is_empty(),
        "the fixture must have a second stop, or the descent proves nothing"
    );

    assert!(
        !app.bake_subtree(app.first_node),
        "the refusal must be reported"
    );
    assert_eq!(
        app.auto_folded.iter().collect::<Vec<_>>(),
        vec![stop],
        "every stop but the refusing one must have been paid off"
    );
}
