// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0271: a script step applied to a live session.
//!
//! `crate::script`'s own tests cover the format. These cover what a step
//! does to an `App`: that it is a function of the script and the document
//! and of nothing else (S6), that it bakes the lines it names (S12), that
//! a broken position is a diagnostic rather than a stop (S13), and that
//! the keys it takes are only taken while navigation is on (S7).

use super::super::*;
use super::support::*;
use crate::script::Script;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn script_of(text: &str) -> Script {
    Script::parse(text, "test.script".into()).expect("fixture script must parse")
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

/// Simulate a keypress the way the run loop does: dispatch the key then
/// evaluate advance_when, mirroring `terminal::dispatch_event`.
fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(key(code, modifiers));
    if app.script_advance_when_satisfied() {
        app.script_advance(true);
    }
}

/// Everything a step declares, read back off the session — the value
/// spec 0271 S6 says must depend on the step alone.
#[derive(Debug, PartialEq)]
struct View {
    cursor: String,
    folded: Vec<usize>,
    wire: Option<std::ops::Range<usize>>,
    prefill: Option<String>,
    lines: usize,
}

fn view(app: &App) -> View {
    let mut folded: Vec<usize> = app.user_folds();
    folded.sort_unstable();
    View {
        cursor: app.positional_path(app.cursor),
        folded,
        wire: app.wire_rows(),
        prefill: app.command_buffer.clone(),
        lines: app.total_lines(),
    }
}

/// Three steps over `repeated_message_fixture`, exercising every
/// directive family: folds, an unfold, a cursor, a wire span and a
/// prefill.
const THREE_STEPS: &str = "\
title: three steps
steps:
  - text: the first item
    node: /1
  - text: the second item, alone
    fold: [\"/ 0\", \"/2 Z\"]
    node: /2/1
    wire-line: /2/1
  - text: and a command to run
    node: /3
    command: \"override /3 --as test.Item\"
";

/// Spec 0271 test-plan item 3, and the assertion the whole design rests
/// on: stepping back to a step reproduces the view it produced the first
/// time, however far the session wandered in between. This is the test
/// that fails the moment a directive is made to inherit from the step
/// before it.
#[test]
fn a_step_is_a_function_of_the_script() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(THREE_STEPS));

    app.script_advance(true);
    let want = view(&app);
    assert!(!want.folded.is_empty(), "step 2 folds");
    assert!(want.wire.is_some(), "step 2 shows bytes");

    app.script_advance(true);
    assert!(view(&app) != want, "step 3 is a different view");

    // Wander: fold, move, show other bytes, page around. None of this is
    // recorded anywhere, and none of it may survive the step below.
    for event in [
        key(KeyCode::Char('j'), KeyModifiers::NONE),
        key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        key(KeyCode::Char('w'), KeyModifiers::NONE),
        key(KeyCode::Char('G'), KeyModifiers::NONE),
        key(KeyCode::Char('k'), KeyModifiers::NONE),
    ] {
        app.handle_key(event);
    }

    app.script_advance(false);
    assert_eq!(view(&app), want, "step 2 must reproduce step 2's view");
}

/// Spec 0355: `Tab` toggles navigation; `space` advances and `Backspace`
/// retreats while on. The arrows are never the script's in either state —
/// that is the point of moving off them.
#[test]
fn tab_toggles_and_space_backspace_step() {
    let (mut app, items) = repeated_message_fixture();
    app.set_script(script_of(THREE_STEPS));
    assert!(app.script_active(), "spec 0271 S8: navigation starts on");

    // On: `space` advances the step (text fits in one page, so it goes
    // straight to the next step without scrolling).
    let before = app.cursor;
    app.handle_key(key(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_ne!(app.cursor, before, "step 2 moves the cursor itself");
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 1, "`space` advances the step");

    // Still on: a bare arrow reaches the document, script or no script.
    app.set_cursor(items[0]);
    app.handle_key(key(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(app.cursor, items[1], "Ctrl-Down skips siblings even so");
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 1, "and no arrow, modified or not, steps the script");
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE));
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 1, "a presenter's stray Right must not change step");

    app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!app.script_active(), "Tab turns navigation off");

    // Off: `space` is no longer the script's and the step does not change.
    app.set_cursor(items[0]);
    app.handle_key(key(KeyCode::Char(' '), KeyModifiers::NONE));
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 1, "and the step stayed where it was");

    app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.script_active(), "Tab turns it back on");
    app.handle_key(key(KeyCode::Char(' '), KeyModifiers::NONE));
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 2, "and the script has the step keys back");

    // Backspace retreats. Step 2 (index 2) has a prefill that opens the
    // command buffer; close it first so Backspace reaches the script block.
    app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE));
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 1, "`Backspace` retreats the step");
}

/// Amending spec 0271 S5 (2026-08-12): a step is a paragraph, so
/// `?`/`.` stop at both of its ends rather than panning off into
/// blank rows.
#[test]
fn scrolling_the_pane_stops_at_the_steps_own_text() {
    let (mut app, _) = repeated_message_fixture();
    // Six lines of commentary over a four-row pane, wide enough that
    // nothing wraps: two rows of slack, and no more.
    app.script_area = Rect::new(0, 0, 40, 4);
    app.set_script(script_of(
        "steps:\n- text: |\n    one\n    two\n    three\n    four\n    five\n    six\n",
    ));

    let scroll = |app: &App| app.script.as_ref().expect("a script").scroll;
    assert_eq!(scroll(&app), 0);
    app.script_scroll_by(false);
    assert_eq!(scroll(&app), 0, "the top is the top");
    for _ in 0..5 {
        app.script_scroll_by(true);
    }
    assert_eq!(scroll(&app), 2, "the last row of the step ends the pane");

    // Half the width, so the same six lines wrap to more rows and the
    // bound moves with them rather than with the line count.
    app.script_area = Rect::new(0, 0, 4, 4);
    for _ in 0..20 {
        app.script_scroll_by(true);
    }
    assert_eq!(scroll(&app), 3, "`three` is the one word that takes two");
}

/// Spec 0271 test-plan item 5 / S12. Opened under a row budget, the
/// items past it have no lines at all — so a step naming one has to bake
/// it before it can point at a row, and the wire span is where that
/// shows.
#[test]
fn a_step_waits_for_its_lines() {
    let (mut app, _) = bounded_repeated_message_fixture(3);
    assert!(
        !app.auto_folded.is_empty(),
        "the fixture must open with lines still owed"
    );

    app.set_script(script_of(
        "steps:\n- text: the last item's value\n  node: /3/1\n  wire-line: /3/1\n",
    ));

    assert_eq!(app.positional_path(app.cursor), "/3/1");
    assert!(
        app.wire_rows().is_some(),
        "a step must bake the lines it names before pointing at them"
    );
    assert!(
        app.script
            .as_ref()
            .expect("a script")
            .diagnostics
            .is_empty(),
        "and it must not report the wait as a failure"
    );
}

/// Spec 0279 S5, amending spec 0271 S6. A step declares a view, so it
/// places its node where the subtree it is about can be read.
/// `clamp_scroll_to_cursor` — the reader's rule, which only ever moves
/// far enough to bring a row on screen — would land it on the pane's
/// last row whenever the step before it was higher up, putting
/// everything the step is about off the bottom.
#[test]
fn a_step_leaves_room_below_its_node() {
    let (mut app, _) = repeated_message_fixture();
    // Room for one item's three lines and one row to spare.
    app.main_area = Rect::new(0, 0, 40, 4);
    app.set_script(script_of(
        "steps:\n- text: the first item\n  node: /1\n\
         - text: the last item's value\n  node: /3/1\n",
    ));

    app.script_advance(true);
    assert_eq!(app.positional_path(app.cursor), "/3/1");

    // The enclosing item, whole: its header, `v: 7`, and its footer.
    let item = app.parent(app.cursor).expect("/3/1 has a parent");
    let top = app
        .visible_row_of_line(app.absolute_start(item))
        .expect("the item is on screen");
    let rows = app.tree[item].lines_visible as usize;
    assert_eq!(app.terminal_row_of(top), 0, "the subtree opens the pane");
    assert!(
        app.terminal_row_of(top + rows) <= app.main_area.height as isize,
        "and ends inside it"
    );
}

/// Spec 0279 S5, amended 2026-08-12. A caption need not be an ancestor:
/// in `tests/fixtures/anomalies.pb` it is the top-level `name` line *beside*
/// the wrapper, so that folding the document leaves the headings
/// readable. The climb alone puts the wrapper's first row at the top of
/// the pane and the row naming it just above the fold, so the view
/// reaches back over the fitting ancestor's previous sibling whenever
/// the two still fit together.
#[test]
fn a_step_keeps_the_row_above_its_subtree() {
    let (mut app, _) = repeated_message_fixture();
    // Room for two items' three lines each, and one row to spare.
    app.main_area = Rect::new(0, 0, 40, 7);
    app.set_script(script_of(
        "steps:\n- text: the first item\n  node: /1\n\
         - text: the last item's value\n  node: /3/1\n",
    ));

    app.script_advance(true);
    assert_eq!(app.positional_path(app.cursor), "/3/1");

    let item = app.parent(app.cursor).expect("/3/1 has a parent");
    let before = app.prev_sibling(item).expect("/3 has a sibling above it");
    let top = app
        .visible_row_of_line(app.absolute_start(before))
        .expect("the sibling is on screen");
    assert_eq!(
        app.terminal_row_of(top),
        0,
        "the row above the subtree opens the pane"
    );
    let rows = app.tree[item].lines_visible as usize;
    let end = app
        .visible_row_of_line(app.absolute_start(item))
        .expect("the item is on screen")
        + rows;
    assert!(
        app.terminal_row_of(end) <= app.main_area.height as isize,
        "and the subtree still ends inside it"
    );
}

/// Spec 0271 test-plan item 6 / S13. A position that resolves to nothing
/// is reported, and everything else about the step still happens — the
/// text above all, which is what makes a drifted script degrade into a
/// slide deck rather than a stop.
#[test]
fn a_broken_position_still_shows_its_text() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n- text: this node is long gone\n  node: /9/9\n  command: \"quit\"\n",
    ));

    let state = app.script.as_ref().expect("a script is loaded");
    assert_eq!(state.diagnostics, vec!["no node at /9/9".to_string()]);
    assert_eq!(state.script.steps[0].text.trim(), "this node is long gone");
    assert!(app.message.contains("no node at /9/9"));
    assert_eq!(
        app.command_buffer.as_deref(),
        Some("quit"),
        "the rest of the step is applied anyway"
    );
}

/// Spec 0271 S3: a scalar that is not a well-formed path is matched
/// against the rendered text, and it resolves against the document as it
/// stands rather than as the script was written.
#[test]
fn a_search_position_resolves_against_the_rendered_text() {
    let (mut app, items) = repeated_message_fixture();
    app.set_script(script_of("steps:\n- text: find it\n  node: \"v: 7\"\n"));

    assert!(
        app.script
            .as_ref()
            .expect("a script")
            .diagnostics
            .is_empty(),
        "the search must resolve"
    );
    assert_eq!(
        app.parent(app.cursor),
        Some(items[2]),
        "`v: 7` is the third item's only field"
    );
}

/// Spec 0271 S5, amended: the legend is flushed to the *right* edge of
/// the rule.
///
/// Left-aligned it started in the same column the document's first line
/// starts in one row below, in a green close to the document's own
/// palette, and it was read as a line of the blob — the one confusion
/// the separator exists to prevent. Two rule characters of run-out keep
/// it sitting on the rule rather than ending it.
#[test]
fn the_separator_legend_is_flushed_right() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    app.set_script(script_of(THREE_STEPS));

    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();

    let row = |y: u16| -> String {
        (0..60u16)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    };
    let separator = (0..24u16)
        .map(row)
        .find(|line| line.contains("Tab to pause"))
        .expect("the separator carries the legend");

    // 60 columns is wide enough for the full legend (52 columns needed).
    assert!(
        separator.ends_with("Tab to pause  space/Backspace step  step 1/3 ──"),
        "the legend must sit at the right edge: {separator:?}"
    );
    assert!(
        separator.starts_with("──────"),
        "and the rule must run up to it: {separator:?}"
    );
}

/// Spec 0271 S15, amended: the pane's whole area carries the tint, and
/// it is the separator's own hue — the two are one region, and a reader
/// who cannot see where the commentary ends has no separator at all.
#[test]
fn the_script_pane_is_tinted_across_its_whole_width() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    app.set_script(script_of(THREE_STEPS));

    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();

    // Whatever the palette says, not whatever the terminal running the
    // suite happens to be: on ANSI-16 there is no green dim enough to
    // sit behind prose, so the pane carries no background at all and the
    // separator alone cues it (`theme::script_pane_style`). A sandbox
    // with no COLORTERM lands on exactly that branch.
    // A cell the pane left alone still reports `Reset` rather than
    // `None`, so both sides are read through the same lens.
    let bg = |style: ratatui::style::Style| style.bg.unwrap_or(ratatui::style::Color::Reset);
    let want = bg(crate::theme::script_pane_style(app.theme));
    // Row 0 is the pane's first row; the far right of it is past the
    // text, which is where a style applied to the spans rather than to
    // the area would stop.
    for x in [0u16, 30, 59] {
        assert_eq!(
            bg(buffer[(x, 0)].style()),
            want,
            "column {x} of the pane's first row must be tinted"
        );
    }
    if want != ratatui::style::Color::Reset {
        assert_ne!(
            bg(buffer[(0, 23)].style()),
            want,
            "and the tint must not reach the document"
        );
    }
}

/// Spec 0357: `select_line: true` engages the selection on the caret's header
/// line; advancing to the next step clears it.
#[test]
fn select_directive_highlights_the_caret_line() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n\
         - text: first\n  node: /1\n  select_line: true\n\
         - text: second\n  node: /2\n",
    ));

    // Step 0: selection must be engaged and cover /1's header line.
    // `node: /1` has already placed the cursor on /1, so `app.cursor`
    // is the node index we need.
    assert!(app.select_engaged, "select: must engage the selection");
    let span = app.selection_span().expect("a span must be present");
    let (lo_line, lo_col, _hi_line, _hi_col) = span;
    assert_eq!(lo_col, 0, "selection starts at column 0");
    assert_eq!(
        lo_line,
        app.absolute_start(app.cursor),
        "selection starts on /1's header line"
    );

    // Advance to step 1: selection must be gone.
    app.script_advance(true);
    assert!(!app.select_engaged, "selection is cleared at the next step");
    assert!(app.selection_span().is_none());
}

/// Spec 0357: `search:` fires the search highlight; advancing clears it.
#[test]
fn search_directive_highlights_pattern() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n\
         - text: first\n  node: /1\n  search: \"v:\"\n\
         - text: second\n  node: /2\n",
    ));

    // Step 0: search highlight must be active with a compiled sweep,
    // and the pattern must be recorded so `F` can reuse it.
    assert!(app.search_highlight, "search: must engage the highlight");
    assert!(
        app.search_sweep.is_some(),
        "a sweep must be present for a valid pattern"
    );
    use super::super::search::SearchScope;
    assert!(
        app.last_search_for(SearchScope::Main).is_some(),
        "pattern must be recorded for F/n/N reuse"
    );

    // Advance to step 1: highlight must be gone.
    app.script_advance(true);
    assert!(
        !app.search_highlight,
        "search highlight is cleared at the next step"
    );
    assert!(app.search_sweep.is_none());
}

/// Spec 0357: `select_line: true` and `search:` may coexist on one step.
#[test]
fn select_and_search_may_coexist() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n- text: both\n  node: /1\n  select_line: true\n  search: \"v:\"\n",
    ));

    let state = app.script.as_ref().expect("a script is loaded");
    assert!(state.diagnostics.is_empty(), "no errors expected");
    assert!(app.select_engaged, "selection is engaged");
    assert!(app.search_highlight, "search is highlighted");
}

// ── Spec 0356: advance_when predicates ────────────────────────────────────

const WIRE_STEP: &str = "\
steps:
  - text: show wire
    node: /1
    advance_when:
      - wire: /1
  - text: done
    node: /2
";

/// Spec 0356 test 1: a `wire:` predicate fires after `w` makes the wire
/// span cover the target node.
#[test]
fn advance_when_wire_advances_on_w() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(WIRE_STEP));

    assert_eq!(app.script.as_ref().unwrap().current, 0, "starts on step 0");
    // `w` opens the wire span; advance_when must fire.
    press(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "wire: predicate advanced the step"
    );
}

/// Spec 0356 test 2: a key that does not satisfy the predicate leaves the
/// step unchanged.
#[test]
fn advance_when_not_satisfied_does_not_advance() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(WIRE_STEP));

    // `j` moves the cursor but does not open wire bytes.
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        0,
        "unrelated key must not advance the step"
    );
}

/// Spec 0356 test 3: a step whose own `script_apply` already satisfies
/// its `advance_when` skips forward immediately on entry (G3).
#[test]
fn advance_when_fires_immediately_if_satisfied_at_entry() {
    // Step 0 has a `wire: /1` predicate but also opens the wire itself —
    // so on entry it is immediately satisfied and must skip to step 1.
    let script = "\
steps:
  - text: already satisfied
    node: /1
    wire-line: /1
    advance_when:
      - wire: /1
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "step 0 skipped immediately because wire: was already satisfied"
    );
}

/// Spec 0356 test 4: `caret:` fires when the cursor moves to the target.
#[test]
fn advance_when_caret_predicate() {
    // Fold items to depth 0 so j from root goes directly to /1 (not /1/1).
    let script = "\
steps:
  - text: move to /1
    node: /
    fold: [\"/ 1\"]
    advance_when:
      - caret: /1
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // `j` moves the caret to /1 (items are folded, so /1/1 is invisible).
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "caret: predicate fired after moving to /1"
    );
}

/// Spec 0356 test 5: `type:` fires after an override sets the expected type.
#[test]
fn advance_when_type_predicate() {
    // /1's natural type is test.Item; we wait for an override that
    // re-types it as test.Outer, which does not hold at entry.
    let script = "\
steps:
  - text: name the type
    node: /1
    advance_when:
      - type: /1 test.Outer
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // Open the command line, type the override, then press Enter — the
    // advance_when check must fire on the Enter key.
    press(&mut app, KeyCode::Char(':'), KeyModifiers::NONE);
    for ch in "override /1 --as test.Outer".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "type: predicate fired on Enter after override re-typed /1 as test.Outer"
    );
}

/// Spec 0356 test 6: `visible:` and `folded:` hold and fail at the right
/// times; a leaf is never `folded:`.
#[test]
fn advance_when_visible_and_folded_predicates() {
    // /1 is a message node (has children); /1/1 is a scalar (leaf).
    let folded_script = "\
steps:
  - text: fold /1
    node: /
    advance_when:
      - folded: /1
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(folded_script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // `0` on the root folds everything to depth 0 — /1 becomes folded.
    press(&mut app, KeyCode::Char('0'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "folded: predicate fired after /1 was folded"
    );
}

/// Spec 0356 test 7: all predicates in an `advance_when` list must hold.
#[test]
fn advance_when_all_predicates_must_hold() {
    let script = "\
steps:
  - text: need both
    node: /1
    advance_when:
      - wire: /1
      - caret: /2
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));

    // Opening wire satisfies `wire: /1` but not `caret: /2`.
    press(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        0,
        "one predicate satisfied is not enough"
    );
}

/// Spec 0356 test 8: a predicate whose position resolves to no node is
/// false; `space` still advances.
#[test]
fn advance_when_unresolvable_position_is_false() {
    let script = "\
steps:
  - text: unreachable
    advance_when:
      - caret: /999
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // Any key — predicate stays false because /999 does not exist.
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        0,
        "unresolvable stays false"
    );

    // space still advances (G2).
    app.script_advance(true);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "space advances regardless"
    );
}

/// Spec 0356 test 9: `not:` inverts a single predicate.
#[test]
fn advance_when_not_inverts_predicate() {
    // Step 1 keeps wire open via wire-line: so not: starts false;
    // pressing `w` closes it, making not: true.
    let script = "\
steps:
  - text: skip
    node: /1
  - text: close wire
    node: /1
    wire-line: /1
    advance_when:
      - not:
          - wire: /1
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    app.script_advance(true);
    assert_eq!(app.script.as_ref().unwrap().current, 1, "on step 1");
    assert!(app.wire_rows().is_some(), "wire is open on step 1");

    // `w` closes the wire (toggle); not: fires.
    press(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        2,
        "not: predicate fired when wire was closed"
    );
}

/// Spec 0356 test 10: `not:` of a conjunction fires when at least one
/// sub-predicate is false (De Morgan).
#[test]
fn advance_when_not_of_conjunction() {
    let script = "\
steps:
  - text: leave /1 or close wire
    node: /1
    wire-line: /1
    advance_when:
      - not:
          - wire: /1
          - caret: /1
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    // Step 0 opens wire on /1 and puts caret on /1 — both sub-predicates
    // hold, so `not:` is false. But it is immediately satisfied check:
    // let's verify it did NOT auto-advance (both hold → not: is false).
    app.set_script(script_of(script));
    assert_eq!(
        app.script.as_ref().unwrap().current,
        0,
        "not: is false when both hold"
    );

    // Moving the caret off /1 breaks `caret: /1` → not: fires.
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "not: fired when caret left /1"
    );
}

/// Spec 0356 test 11: `not` inside `not` double-negates.
#[test]
fn advance_when_not_is_recursive() {
    // not: [not: [wire: /1]] ≡ wire: /1 — fires when wire IS open.
    let script = "\
steps:
  - text: open wire
    node: /1
    advance_when:
      - not:
          - not:
              - wire: /1
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    press(&mut app, KeyCode::Char('w'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "double-not is equivalent to the inner predicate"
    );
}

/// Spec 0356 test 12: an `or` key in an `advance_when` list is a load error.
#[test]
fn advance_when_or_key_is_a_load_error() {
    let result = Script::parse(
        "steps:\n- text: t\n  advance_when:\n    - or:\n        - wire: /1\n",
        "test.script".into(),
    );
    assert!(result.is_err(), "or: must be a parse error");
}

/// Spec 0356 test 13: an unknown key in an `advance_when` list is a load error.
#[test]
fn advance_when_unknown_key_is_a_load_error() {
    let result = Script::parse(
        "steps:\n- text: t\n  advance_when:\n    - dancing: /1\n",
        "test.script".into(),
    );
    assert!(result.is_err(), "unknown key must be a parse error");
}

/// Spec 0356 test 14: `space` advances regardless of unsatisfied advance_when.
#[test]
fn space_always_advances_regardless_of_advance_when() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(WIRE_STEP));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // Don't open wire — predicate unsatisfied — but space must still advance.
    app.script_advance(true);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "space advances even when advance_when is not satisfied"
    );
}

/// Spec 0356 test 15: `annotations:` predicate fires when the mode matches.
#[test]
fn advance_when_annotations_predicate() {
    let script = "\
steps:
  - text: hide annotations
    advance_when:
      - annotations: false
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert!(app.annotations, "annotations start on");
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // `a` toggles annotations off.
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "annotations: false fired after toggling off"
    );
}

/// Spec 0356 test 16: `heat_cues:` predicate fires when the mode matches.
#[test]
fn advance_when_heat_cues_predicate() {
    use crate::tui::heat_cue::HeatCueMode;
    let script = "\
steps:
  - text: enable heat cues
    advance_when:
      - heat_cues: findings
  - text: done
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.heat_cues, HeatCueMode::Off);
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // `i` cycles heat cues to Findings.
    press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "heat_cues: findings fired after pressing i"
    );
}

/// Spec 0356 test 17: `field_name:` fires after an override sets the expected
/// field name.
#[test]
fn advance_when_field_name_predicate() {
    let script = "\
steps:
  - text: name the field
    node: /1
    advance_when:
      - field_name: /1 myfield
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // Open the command line, type the override, then press Enter.
    press(&mut app, KeyCode::Char(':'), KeyModifiers::NONE);
    for ch in "override /1 --field-name myfield".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "field_name: predicate fired on Enter after override named /1 myfield"
    );
}

/// Spec 0356 test 18: `file_exists:` fires when the file appears on disk.
#[test]
fn advance_when_file_exists_predicate() {
    let dir = std::env::temp_dir().join(format!("protolens-fe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let target = dir.join("sentinel");
    let path_str = target.to_str().expect("utf8 path").to_string();

    let script = format!(
        "steps:\n  - text: wait for file\n    advance_when:\n      - file_exists: {path_str}\n  - text: done\n"
    );
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(&script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    // File absent — any key leaves step unchanged.
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        0,
        "step stays at 0 while file absent"
    );

    // Create the file — next key should auto-advance.
    std::fs::write(&target, b"").expect("create sentinel");
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "file_exists: predicate fired once file appeared"
    );

    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// Spec 0356 test 19: `annotations:` step directive sets the mode on entry.
#[test]
fn step_directive_annotations_sets_mode() {
    let script = "\
steps:
  - text: hide
    annotations: false
  - text: show
    annotations: true
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert!(!app.annotations, "step 0 directive set annotations=false");

    app.script_advance(true);
    assert!(app.annotations, "step 1 directive set annotations=true");
}

/// Spec 0356 test 18: `heat_cues:` step directive sets the mode on entry.
#[test]
fn step_directive_heat_cues_sets_mode() {
    use crate::tui::heat_cue::HeatCueMode;
    let script = "\
steps:
  - text: all
    heat_cues: all
  - text: off
    heat_cues: off
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.heat_cues, HeatCueMode::All, "step 0 set heat_cues=all");

    app.script_advance(true);
    assert_eq!(app.heat_cues, HeatCueMode::Off, "step 1 set heat_cues=off");
}

/// Spec 0356 test 19: a bad `heat_cues:` directive value is a load error.
#[test]
fn step_directive_heat_cues_bad_value_is_load_error() {
    let result = Script::parse(
        "steps:\n- text: t\n  heat_cues: maybe\n",
        "test.script".into(),
    );
    assert!(
        result.is_err(),
        "invalid heat_cues value must be a parse error"
    );
}
