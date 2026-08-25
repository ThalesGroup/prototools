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
    fold: all
    unfold: [/2]
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
