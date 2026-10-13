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
    wire_line: /2/1
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

    // Spec 0400 S2: narrower than `three`, and still two rows of slack —
    // lines are cut, not wrapped, so the bound follows the line count
    // whatever the width.
    app.script_area = Rect::new(0, 0, 4, 4);
    for _ in 0..20 {
        app.script_scroll_by(true);
    }
    assert_eq!(scroll(&app), 2, "nothing wraps, so no row is added");
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
        "steps:\n- text: the last item's value\n  node: /3/1\n  wire_line: /3/1\n",
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

/// Spec 0398 S3: a `node:` reveals its own visible subtree, not its
/// ancestors — the spec 0279 climb and its preceding-sibling caption are
/// gone. Step 1 places `/1` at the top; step 2 aims at `/3/1`, deep in
/// the last item. The view scrolls the minimum to bring `/3/1` on screen
/// (so it lands on the pane's last row, spec 0398 S2), and does *not*
/// pull `/3`'s header, or the sibling above it, to the top.
#[test]
fn a_node_reveal_no_longer_climbs_to_an_ancestor() {
    let (mut app, _) = repeated_message_fixture();
    app.main_area = Rect::new(0, 0, 40, 4);
    app.set_script(script_of(
        "steps:\n- text: the first item\n  node: /1\n\
         - text: the last item's value\n  node: /3/1\n",
    ));

    app.script_advance(true);
    assert_eq!(app.positional_path(app.cursor), "/3/1");

    // `/3/1` itself is on screen — its single `v: 7` line is drawn inside
    // the pane.
    let target_row = app
        .visible_row_of_line(app.absolute_start(app.cursor))
        .expect("the target is on screen");
    let term = app.terminal_row_of(target_row);
    assert!(
        term >= 0 && term < app.main_area.height as isize,
        "the node's own line is visible (terminal row {term})"
    );

    // But the enclosing item `/3` is *not* opened at the top — no climb.
    let item = app.parent(app.cursor).expect("/3/1 has a parent");
    let item_top = app
        .visible_row_of_line(app.absolute_start(item))
        .expect("the item header is drawn");
    assert_ne!(
        app.terminal_row_of(item_top),
        0,
        "the ancestor is not pulled to the pane's top"
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

/// Spec 0397 S6: `select_lines: {from, to}` selects a contiguous range
/// of nodes — from `from`'s first line through `to`'s last line. The
/// smuggle beat uses it to select a byte's worth of grid cells at once.
#[test]
fn select_lines_directive_spans_a_range_of_nodes() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n\
         - text: first\n  node: /1\n  select_lines:\n    from: /1\n    to: /3\n\
         - text: second\n  node: /2\n",
    ));

    assert!(app.select_engaged, "select_lines must engage the selection");
    let (lo_line, lo_col, hi_line, _hi_col) = app.selection_span().expect("a span must be present");
    assert_eq!(lo_col, 0, "the range starts at column 0");

    // `/1` opens the range, `/3` closes it. The span runs from `/1`'s
    // header through the last line of `/3`.
    let first = app.resolve_path("/1").expect("/1 resolves");
    let last = app.resolve_path("/3").expect("/3 resolves");
    assert_eq!(
        lo_line,
        app.absolute_start(first),
        "the range starts on /1's first line"
    );
    assert_eq!(
        hi_line,
        app.absolute_start(last) + app.tree[last].lines_total as usize - 1,
        "the range ends on /3's last line"
    );

    // Advancing clears it, like every other selection directive.
    app.script_advance(true);
    assert!(!app.select_engaged, "the range clears at the next step");
    assert!(app.selection_span().is_none());
}

/// Spec 0397 S6: an unresolved `select_lines` end is a step diagnostic,
/// and nothing is selected.
#[test]
fn select_lines_with_an_unresolved_end_is_a_diagnostic() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n- text: t\n  node: /1\n  select_lines:\n    from: /1\n    to: /99\n",
    ));
    assert!(
        !app.select_engaged,
        "an unresolvable range must select nothing"
    );
    let diagnostics = app
        .script
        .as_ref()
        .map(|s| s.diagnostics.join("; "))
        .unwrap_or_default();
    assert!(
        diagnostics.contains("/99"),
        "the unresolved end is reported: {diagnostics:?}"
    );
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

/// `select_node: true` selects the whole subtree of the node: target,
/// spanning from its first line to its last descendant line.
#[test]
fn select_node_covers_the_whole_subtree() {
    let (mut app, _) = repeated_message_fixture();
    // /1 is an Item message with one scalar child — 2 lines total.
    app.set_script(script_of(
        "steps:\n- text: first\n  node: /1\n  select_node: true\n",
    ));

    assert!(app.select_engaged, "select_node must engage the selection");
    let span = app.selection_span().expect("a span must be present");
    let (lo_line, lo_col, hi_line, _hi_col) = span;

    let node = app.cursor;
    let subtree_start = app.absolute_start(node);
    let subtree_lines = app.tree[node].lines_total as usize;
    assert_eq!(lo_col, 0, "selection starts at column 0");
    assert_eq!(
        lo_line, subtree_start,
        "selection starts at the node header"
    );
    assert_eq!(
        hi_line,
        subtree_start + subtree_lines - 1,
        "selection ends at the last line of the subtree"
    );
}

/// `select_node: true` on a step with `search:` selects the node: target's
/// subtree, not the line the search moved the cursor to.
#[test]
fn select_node_is_not_affected_by_search() {
    let (mut app, _) = repeated_message_fixture();
    // node: /1 (Item), search: moves the caret into /1's child scalar.
    // select_node must still cover /1's whole subtree.
    app.set_script(script_of(
        "steps:\n- text: both\n  node: /1\n  select_node: true\n  search: \"v:\"\n",
    ));

    assert!(app.select_engaged, "selection must be engaged");
    assert!(app.search_highlight, "search must also be active");

    let span = app.selection_span().expect("span must exist");
    let (lo_line, _lo_col, hi_line, _hi_col) = span;

    // Resolve /1 to confirm the expected subtree bounds.
    let node_1 = app.resolve_path("/1").expect("/1 must resolve");
    let subtree_start = app.absolute_start(node_1);
    let subtree_lines = app.tree[node_1].lines_total as usize;
    assert_eq!(lo_line, subtree_start, "selection starts at /1 header");
    assert_eq!(
        hi_line,
        subtree_start + subtree_lines - 1,
        "selection ends at /1's last descendant, not the search match"
    );
}

/// Spec 0366: `select_node:` written before `search:` selects the `node:`
/// target's subtree, not the search-match node.
#[test]
fn select_node_before_search_selects_node_subtree() {
    let (mut app, _) = repeated_message_fixture();
    // select_node comes first in the YAML, so it fires before search.
    app.set_script(script_of(
        "steps:\n- text: x\n  node: /1\n  select_node: true\n  search: \"v:\"\n",
    ));

    let span = app.selection_span().expect("span must exist");
    let (lo_line, _lo_col, hi_line, _hi_col) = span;
    let node_1 = app.resolve_path("/1").expect("/1 must resolve");
    let subtree_start = app.absolute_start(node_1);
    let subtree_lines = app.tree[node_1].lines_total as usize;
    assert_eq!(lo_line, subtree_start, "selection starts at /1 header");
    assert_eq!(
        hi_line,
        subtree_start + subtree_lines - 1,
        "select_node selects /1's subtree, not the search match"
    );
}

/// Spec 0366: `search:` written before `select_node:` means search runs
/// first — the selection then covers the search-match node's subtree.
#[test]
fn search_before_select_node_selects_match_subtree() {
    let (mut app, _) = repeated_message_fixture();
    // search comes first, so the cursor moves to the match before select_node.
    // "v:" matches the scalar child of /1, so the cursor lands there.
    // select_node then selects that scalar's subtree (1 line).
    app.set_script(script_of(
        "steps:\n- text: x\n  node: /1\n  search: \"v:\"\n  select_node: true\n",
    ));

    let span = app.selection_span().expect("span must exist");
    let (lo_line, _lo_col, hi_line, _hi_col) = span;
    // The cursor is now on the search-match node (the scalar child), not /1.
    let match_node = app.cursor;
    let match_start = app.absolute_start(match_node);
    let match_lines = app.tree[match_node].lines_total as usize;
    assert_eq!(lo_line, match_start, "selection starts at the match node");
    assert_eq!(
        hi_line,
        match_start + match_lines - 1,
        "select_node selects the match node's subtree"
    );
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
    wire_line: /1
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
    // Step 1 keeps wire open via wire_line: so not: starts false;
    // pressing `w` closes it, making not: true.
    let script = "\
steps:
  - text: skip
    node: /1
  - text: close wire
    node: /1
    wire_line: /1
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
    wire_line: /1
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

/// Spec 0368: `node: <path>` fires when the caret is on that node
/// (any line within it), matching the positional-path form.
#[test]
fn advance_when_node_predicate_path_form() {
    let script = "\
steps:
  - text: move to /1
    node: /
    fold: [\"/ 1\"]
    advance_when:
      - node: /1
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "node: predicate fired on reaching /1"
    );
}

/// Spec 0368: `node: <path>:N` fires when the caret is on a child of the
/// named node whose proto field number is N.
///
/// The fixture is `Outer { repeated Item items = 1; }` — all three `/1`,
/// `/2`, `/3` are children of `/` with field_number 1.  So `node: /:1`
/// fires as soon as the cursor reaches `/1` (the first child of `/` with
/// field_number 1).  `node: /:2` never fires because no child of `/` has
/// field_number 2.
#[test]
fn advance_when_node_predicate_with_field_number() {
    // `node: /:1` fires when `j` moves to /1 (field_number 1 on Outer).
    let script_fires = "\
steps:
  - text: move to /1
    node: /
    fold: [\"/ 1\"]
    advance_when:
      - node: /:1
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script_fires));
    assert_eq!(app.script.as_ref().unwrap().current, 0);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "node: /:1 fired when cursor moved to /1 (field_number 1)"
    );

    // `node: /:2` never fires — Outer has no field numbered 2.
    let script_never = "\
steps:
  - text: wait for field 2
    node: /
    fold: [\"/ 1\"]
    advance_when:
      - node: /:2
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script_never));
    assert_eq!(app.script.as_ref().unwrap().current, 0);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        0,
        "node: /:2 must not fire — Outer has no field_number 2"
    );
}

/// Spec 0368: `line: n` fires when the caret is on the given 1-based
/// absolute document line.
#[test]
fn advance_when_line_predicate() {
    // The root `/` is line 1. `/1` header is line 2 (root line + 1).
    let script = "\
steps:
  - text: move to /1
    node: /
    fold: [\"/ 1\"]
    advance_when:
      - line: 2
  - text: done
    node: /2
";
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(script));
    assert_eq!(app.script.as_ref().unwrap().current, 0);

    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        app.script.as_ref().unwrap().current,
        1,
        "line: 2 predicate fired when caret reached absolute line 2"
    );
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

/// Spec 0398 S2: a reveal of something already fully on screen does not
/// scroll. Step 1 aims at `/3`, the last item, scrolling it into view in
/// full; step 2 aims at `/3/1`, a line *inside* `/3` that step 1 already
/// shows — so the scroll must not move.
#[test]
fn a_step_does_not_scroll_when_its_node_is_already_visible() {
    let (mut app, _) = repeated_message_fixture();
    // Tall enough that `/3` fits whole once scrolled to, with room, but
    // the whole document (root + three 3-line items) does not — so step 1
    // genuinely scrolls.
    app.main_area = Rect::new(0, 0, 40, 7);
    app.set_script(script_of(
        "steps:\n- text: the last item\n  node: /3\n\
         - text: a line inside it\n  node: /3/1\n",
    ));
    // Step 1 scrolled `/3` into view.
    assert_eq!(app.positional_path(app.cursor), "/3");
    let after_step1 = app.scroll_top();

    // Step 2: `/3/1` is already fully visible inside `/3`.
    app.script_advance(true);
    assert_eq!(app.positional_path(app.cursor), "/3/1");
    assert_eq!(
        app.scroll_top(),
        after_step1,
        "an already-visible node leaves the scroll untouched"
    );
}

/// A message like the capture's StepRequest: a tall first field (`grid`,
/// many rows) followed by small siblings. Returns the app.
fn tall_first_field_app() -> App {
    use prototext_core::helpers::{write_tag, write_varint};
    use prototext_core::schema::descriptor::field_descriptor_proto::{Label, Type};
    let fds = proto3_fds(
        "tall.proto",
        vec![
            message(
                "Outer",
                vec![
                    field_of("grid", 1, Label::Optional, Type::Message, ".test.Big"),
                    field_of("rules", 2, Label::Optional, Type::Message, ".test.Small"),
                    field("generation", 3, Label::Optional, Type::Uint64),
                ],
            ),
            message(
                "Big",
                vec![field("cells", 1, Label::Repeated, Type::Uint32)],
            ),
            message("Small", vec![field("k", 1, Label::Optional, Type::Uint32)]),
        ],
    );
    let mut big = Vec::new();
    for i in 0..40u64 {
        write_tag(1, 0, &mut big);
        write_varint(i + 1, &mut big);
    }
    let mut blob = Vec::new();
    write_tag(1, 2, &mut blob);
    write_varint(big.len() as u64, &mut blob);
    blob.extend_from_slice(&big);
    let mut small = Vec::new();
    write_tag(1, 0, &mut small);
    write_varint(7, &mut small);
    write_tag(2, 2, &mut blob);
    write_varint(small.len() as u64, &mut blob);
    blob.extend_from_slice(&small);
    write_tag(3, 0, &mut blob);
    write_varint(5, &mut blob);
    let mut app = fixture_under("tallfirst", &fds, "test.Outer", &blob);
    app.splash = false;
    app
}

/// Spec 0398 S2/S3: `node: /` + `fold: ["/ 1", "/N Z"]` + `search: /N` —
/// the capture beat's shape. The search highlights one line (the field's
/// header); folding keeps the message rooted at the top. The highlighted
/// line is already on screen, so nothing scrolls, even though `/1`
/// unfolded is far taller than the pane.
#[test]
fn a_search_hit_on_a_tall_node_does_not_scroll() {
    let root_term = |a: &App| {
        let rl = a.absolute_start(a.first_node);
        a.visible_row_of_line(rl).map(|r| a.terminal_row_of(r))
    };
    for target in ["/1", "/2", "/3"] {
        let mut app = tall_first_field_app();
        app.main_area = Rect::new(0, 0, 50, 10);
        app.set_script(script_of(&format!(
            "steps:\n- text: t\n  node: /\n  fold: [\"/ 1\", \"{target} Z\"]\n  search: {target}\n"
        )));
        assert_eq!(
            app.scroll_top(),
            0,
            "search on {target} must not scroll the rooted view"
        );
        assert_eq!(
            root_term(&app),
            Some(0),
            "the root header stays at the top for {target}"
        );
    }
}

/// Spec 0398 S2: a search whose hit is below the fold scrolls the minimum
/// that brings the hit's line on screen — it lands on the pane's last
/// row, not the first (no over-scroll, no centering).
#[test]
fn a_search_scrolls_minimally_to_an_offscreen_hit() {
    let mut app = tall_first_field_app();
    // Short pane; show the tall grid unfolded so `generation` (/3) sits
    // well below the bottom, then search it.
    app.main_area = Rect::new(0, 0, 50, 6);
    app.set_script(script_of(
        "steps:\n- text: t\n  node: /\n  fold: [\"/1 Z\"]\n  search: /3\n",
    ));
    let (hit_line, _, _, _) = app.search_current_cell().expect("the search is current");
    let row = app
        .visible_row_of_line(hit_line)
        .expect("the hit line is drawn");
    assert_eq!(
        app.terminal_row_of(row),
        app.main_area.height as isize - 1,
        "the off-screen hit is brought onto the last row, not over-scrolled"
    );
}

/// Spec 0397 S6: the view follows the whole `select_lines:` range, not
/// just the cursor node at its head. Step 1 parks the view over item
/// `/16`; step 2 then aims at `/18`, which lands mid-pane — already on
/// screen on its own, so the head-only rule (S2) would hold the scroll
/// and leave the tail of a `/18..24` range just below the bottom edge.
/// The range fits the pane, so following it scrolls the few rows needed
/// to show `/24` too, which is exactly what the byte-selection steps of
/// the smuggle beat need: reveal the whole selected byte, do not stop at
/// its first cell. (Confirmed to fail without the extension: the end
/// stays off the bottom and the scroll sits three rows higher.)
#[test]
fn a_step_reveals_the_whole_selection_range_not_just_its_head() {
    let on_screen = |app: &super::super::App, path: &str| {
        let node = app.resolve_path(path).expect("path resolves");
        let last_line = app.absolute_start(node) + app.tree[node].lines_total as usize - 1;
        (app.absolute_start(node)..=last_line).all(|line| {
            let Some(row) = app.visible_row_of_line(line) else {
                return false;
            };
            let term = app.terminal_row_of(row);
            term >= 0 && term < app.main_area.height as isize
        })
    };

    let (mut app, _) = super::bake::opaque_items_fixture(40);
    app.splash = false;
    app.main_area = Rect::new(0, 0, 50, 8);
    app.set_script(script_of(
        "steps:\n- text: tail\n  node: /16\n\
         - text: the byte\n  node: /18\n  select_lines:\n    from: /18\n    to: /24\n",
    ));
    app.script_advance(true);
    assert_eq!(app.positional_path(app.cursor), "/18");

    assert!(on_screen(&app, "/18"), "the range head /18 is on screen");
    assert!(
        on_screen(&app, "/24"),
        "the range end /24 is on screen, not just its head"
    );
}

/// Spec 0397 S3: a step whose node is taller than the pane puts the
/// node's own first line at the top of the viewport — no climb, no
/// caption, which would only push that line off the top.
#[test]
fn a_step_too_tall_to_fit_opens_on_its_own_first_line() {
    let (mut app, _) = repeated_message_fixture();
    // The root (ten lines) cannot fit a four-row pane.
    app.main_area = Rect::new(0, 0, 40, 4);
    app.set_script(script_of("steps:\n- text: the whole message\n  node: /\n"));

    assert_eq!(app.positional_path(app.cursor), "/");
    let top = app
        .visible_row_of_line(app.absolute_start(app.cursor))
        .expect("the root is on screen");
    assert_eq!(
        app.terminal_row_of(top),
        0,
        "the too-tall node's first line opens the pane"
    );
}

/// Spec 0397 S1, at the pixels: a step's path search tints the *whole*
/// matched line, not one cell. Driving it through a script step is what
/// populates the highlight pattern the renderer needs (an earlier version
/// set the hit width but still painted a single cell).
#[test]
fn a_scripted_path_search_tints_the_whole_line() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    // `/3/1` is the last item's scalar `v: 7` — a single short line.
    app.set_script(script_of("steps:\n- text: the value\n  search: /3/1\n"));

    let (line, _col, width, on_path) = app
        .search_current_cell()
        .expect("the path search is current");
    assert!(on_path, "`/3/1` is a path match");

    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();

    let bg = crate::theme::search_current_style(app.theme).bg;
    let y = app.main_area.y + line as u16;
    let tinted: usize = (app.main_area.x..app.main_area.x + app.main_area.width)
        .filter(|&x| buffer[(x, y)].style().bg == bg)
        .count();
    assert_eq!(
        tinted, width,
        "the whole matched line is tinted ({tinted} cells), not one"
    );
    assert!(tinted > 1, "more than the old single cell");
}
/// Spec 0397 S5: `set_wire_span` does not scroll when the span and the
/// cursor are already on screen. This is the smuggle beat's exact shape
/// (spec 0396): two steps hold the *same* wire span open while the search
/// moves the caret from one cell to the contiguous next. The wire rows do
/// not change, and both cells stay in view, so the viewport must not shift
/// — the old unconditional re-anchor slid it by the one row the caret
/// dropped.
#[test]
fn two_steps_sharing_a_wire_span_do_not_scroll_between_cells() {
    // `/3` (a) and `/4` (b) are two contiguous scalar siblings with no
    // header between them — the beat's shape, a grid row's two cells. The
    // span `/3`..`/4` and either caret fit the pane with room to spare, so
    // nothing the steps show leaves the screen — the view must hold still.
    let (mut app, ..) = packed_run_with_tail_fixture();
    app.splash = false;
    app.main_area = Rect::new(0, 0, 60, 14);
    app.set_script(script_of(
        "steps:\n\
         - text: first cell\n  node: /3\n  wire_lines:\n    from: /3\n    to: /4\n  search: /3\n\
         - text: next cell\n  node: /4\n  wire_lines:\n    from: /3\n    to: /4\n  search: /4\n",
    ));
    assert_eq!(app.positional_path(app.cursor), "/3");
    let after_step1 = app.scroll_top();

    app.script_advance(true);
    assert_eq!(app.positional_path(app.cursor), "/4");
    assert_eq!(
        app.scroll_top(),
        after_step1,
        "an unchanged wire span with both cells visible must not scroll"
    );
}

// -----------------------------------------------------------------
// Spec 0400: the pane fits its script; lines are cut, not wrapped
// -----------------------------------------------------------------

/// `n` lines of commentary, as a YAML block scalar.
fn text_of(n: usize) -> String {
    let mut text = String::from("|\n");
    for i in 0..n {
        text.push_str(&format!("      line {i}\n"));
    }
    text
}

/// Spec 0400 test-plan item 1: one height for the whole script, the
/// tallest step's, so the document does not move while stepping.
#[test]
fn the_pane_fits_the_tallest_step() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(&format!(
        "steps:\n  - text: {}  - text: {}",
        text_of(2),
        text_of(5)
    )));
    assert_eq!(
        app.script_rows(60),
        5,
        "the two-line step gets the five-line step's pane"
    );
    app.script_advance(true);
    assert_eq!(app.script_rows(60), 5);
}

/// Spec 0400 test-plan item 2: never more than a third of the terminal,
/// and a step taller than that pages before it advances.
#[test]
fn the_pane_is_capped_at_a_third() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(&format!(
        "steps:\n  - text: {}  - text: {}",
        text_of(40),
        text_of(1)
    )));
    assert_eq!(app.script_rows(60), 20);
    app.script_area = Rect::new(0, 0, 80, 20);
    app.script_space();
    let state = app.script.as_ref().expect("a script");
    assert_eq!(state.current, 0, "space pages first");
    assert_eq!(state.scroll, 20);
}

/// Spec 0400 test-plan item 3.
#[test]
fn script_height_still_overrides() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(&format!("steps:\n  - text: {}", text_of(9))));
    app.script_height = Some(5);
    assert_eq!(app.script_rows(60), 5);
}

/// Spec 0400 test-plan item 4: a line wider than the pane takes one row,
/// says it is cut, and panning reaches its end.
#[test]
fn a_long_line_is_cut_not_wrapped() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (mut app, _) = repeated_message_fixture();
    app.splash = false;
    let long: String = (0..150)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    app.set_script(script_of(&format!("steps:\n  - text: \"{long}\"\n")));
    assert_eq!(app.script_rows(30), 1, "one line, one row");

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let pane_row = |terminal: &Terminal<TestBackend>| -> String {
        let buffer = terminal.backend().buffer();
        (0..100u16)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect()
    };
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = pane_row(&terminal);
    assert_eq!(&row[..99], &long[..99], "drawn as written up to the edge");
    assert!(row.ends_with('›'), "and marked as cut: {row:?}");

    for _ in 0..10 {
        app.script_pan(PAN_STEP, false);
    }
    assert_eq!(
        app.script.as_ref().unwrap().hscroll,
        50,
        "clamped at the line's end"
    );
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = pane_row(&terminal);
    assert_eq!(row, long[50..], "panned to the end, which is not cut");
}

/// Spec 0400 test-plan item 5 (S3): Tab resumes navigation, so the pane
/// comes back and the main pane shrinks — the step's reveal has to be
/// measured against the shrunk pane, or its target lands below the
/// bottom edge once the frame is drawn.
#[test]
fn a_reveal_uses_the_resumed_panes_main_pane() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = wide_sibling_scalars_app(60);
    app.splash = false;
    app.set_script(script_of(&format!(
        "steps:\n  - text: {}    node: field_30\n",
        text_of(10)
    )));
    let mut terminal = Terminal::new(TestBackend::new(120, 37)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    // Navigation off: the pane is gone, the main pane is ten rows
    // taller, and the reader is back at the top.
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    terminal.draw(|frame| app.render(frame)).unwrap();
    app.set_scroll_top(0);
    let tall = app.main_area.height;

    // Tab: the step is applied before the frame that brings the pane back.
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    let predicted = app.main_area.height;
    assert_eq!(predicted, tall - 10, "measured against the coming frame");
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.main_area.height, predicted, "which the frame confirms");

    let row = app.terminal_row_of(app.absolute_start(app.cursor));
    assert!(
        (0..app.main_area.height as isize).contains(&row),
        "the target is on screen: row {row} of {}",
        app.main_area.height
    );
}
