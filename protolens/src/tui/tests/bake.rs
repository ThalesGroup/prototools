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
    let want_counts = {
        let (mut app, _) = repeated_message_fixture();
        let root = app.first_node;
        app.splice_override(root, Some("test.Outer".to_string()), None)
            .unwrap();
        counts(&app)
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
        assert!(
            app.folded.is_empty(),
            "budget {budget}: and the bake invented no user fold: {:?}",
            app.folded
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
    while let Some(&stop) = app.auto_folded.iter().next() {
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
        assert!(app.auto_folded.contains(&i), "Item {i} must be a stop");
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
        !app.auto_folded.contains(&items[0]),
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
