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
    let mut folded: Vec<usize> = app.folded.iter().copied().collect();
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
    override: \"override /3 --as test.Item\"
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

/// Spec 0271 test-plan item 4. `space` toggles in both states; the
/// Ctrl-arrows only belong to the script while navigation is on, and go
/// back to moving between siblings the moment it is off.
#[test]
fn space_toggles_and_ctrl_arrows_are_conditional() {
    let (mut app, items) = repeated_message_fixture();
    app.set_script(script_of(THREE_STEPS));
    assert!(!app.script_active(), "spec 0271 S8: navigation starts off");

    // Off: Ctrl-Down is still the sibling move it has always been, and
    // the step does not change.
    app.set_cursor(items[0]);
    app.handle_key(key(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(app.cursor, items[1], "Ctrl-Down still skips siblings");
    assert_eq!(app.positional_path(app.cursor), "/2");

    app.handle_key(key(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(app.script_active(), "space turns navigation on");

    // On: the same key is the script's, and the document does not move.
    let before = app.cursor;
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL));
    assert_ne!(app.cursor, before, "step 2 moves the cursor itself");
    let step = app.script.as_ref().expect("a script is loaded").current;
    assert_eq!(step, 1, "Ctrl-Right advances the step");

    app.handle_key(key(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(!app.script_active(), "space turns it back off");
    app.set_cursor(items[0]);
    app.handle_key(key(KeyCode::Down, KeyModifiers::CONTROL));
    assert_eq!(app.cursor, items[1], "and the sibling move is back");
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

/// Spec 0271 test-plan item 6 / S13. A position that resolves to nothing
/// is reported, and everything else about the step still happens — the
/// text above all, which is what makes a drifted script degrade into a
/// slide deck rather than a stop.
#[test]
fn a_broken_position_still_shows_its_text() {
    let (mut app, _) = repeated_message_fixture();
    app.set_script(script_of(
        "steps:\n- text: this node is long gone\n  node: /9/9\n  override: \"quit\"\n",
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
