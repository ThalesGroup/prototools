// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0286: the wall at either end of the content, as a reader meets
//! it. The arithmetic behind it is unit-tested beside `EdgeResistance`
//! itself; these drive it through `App` and read the screen.

use super::super::event::AppEvent;
use super::super::pane_scroll::EDGE_HOLD;
use super::super::terminal::dispatch_event;
use super::super::*;
use super::support::*;
use crossterm::event::{Event, KeyEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// A 200-line document in a 24-row terminal, drawn once so that
/// `main_area` is real.
fn tall_app() -> (App, Terminal<TestBackend>) {
    let mut app = wide_sibling_scalars_app(200);
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    (app, terminal)
}

/// The main pane's own statusline, as drawn.
fn statusline(app: &App, terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let y = app.main_area.y + app.main_area.height;
    (app.main_area.x..app.main_area.x + app.main_area.width)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Rolls the wheel down until the viewport stops moving, then steps off
/// the wall and back onto it so that the pressure that stopped it is
/// spent: the app is left resting *on* the last line with the wall
/// armed and no pushes behind it.
fn roll_to_the_wall(app: &mut App) -> isize {
    let mut last = app.scroll_top();
    for _ in 0..500 {
        app.pan_vertical_down();
        if app.scroll_top() == last {
            break;
        }
        last = app.scroll_top();
    }
    app.pan_vertical_up();
    app.pan_vertical_down();
    assert_eq!(app.scroll_top(), last, "back on the wall");
    assert!(!app.scroll_resistance.pushing(), "and at rest against it");
    last
}

/// The style of the statusline cell `back` columns from its right edge,
/// before the trailing blanks — i.e. inside the viewport label.
fn label_style(app: &App, terminal: &Terminal<TestBackend>, back: u16) -> Style {
    let buffer = terminal.backend().buffer();
    let y = app.main_area.y + app.main_area.height;
    let drawn = statusline(app, terminal).chars().count() as u16;
    let x = app.main_area.x + drawn - back;
    buffer[(x, y)].style()
}

/// Spec 0286 test plan 1 / S2: rolling the wheel at the end of the
/// document stops on the last line rather than carrying on into the
/// blank space past it, which is the reported complaint.
#[test]
fn a_pan_stops_at_the_last_line_rather_than_past_it() {
    let (mut app, _terminal) = tall_app();
    let stopped = roll_to_the_wall(&mut app);
    let content_rows = app.row_heights().offset(app.composed_row_count());
    let (_, natural_max) = natural_top_bounds(content_rows, app.main_area.height as usize);
    assert_eq!(
        stopped, natural_max,
        "the wheel stops on the last line, not past it"
    );

    // And the same going back up, from an over-panned start: the wall
    // stands at both ends.
    app.set_scroll_top(natural_max);
    let mut last = app.scroll_top();
    for _ in 0..500 {
        app.pan_vertical_up();
        if app.scroll_top() == last {
            break;
        }
        last = app.scroll_top();
    }
    assert_eq!(last, 0, "and on the first line coming back");
}

/// Spec 0286 test plan 2 / S3: the wall is soft. Holding against it gets
/// through without reaching for another key (G2).
#[test]
fn pushing_on_reaches_the_over_pan() {
    let (mut app, _terminal) = tall_app();
    let stopped = roll_to_the_wall(&mut app);

    // Several notches in quick succession are refused: what buys the
    // over-pan is the duration, not the count.
    for notch in 1..=10 {
        app.pan_vertical_down();
        assert_eq!(app.scroll_top(), stopped, "notch {notch} is refused");
    }
    // An `EDGE_HOLD` of it later, still in the same gesture.
    app.scroll_resistance.backdate(EDGE_HOLD);
    app.pan_vertical_down();
    assert!(app.scroll_top() > stopped, "and the next one is through");
}

/// Spec 0286 test plan 6 / S6: the viewport label is accented while the
/// wall is being pushed, and plain again once it is not.
///
/// Asserts a *named* color: the CI sandbox has no `COLORTERM`, so the
/// terminal renders ANSI-16 and an RGB expectation would not survive it.
#[test]
fn the_viewport_label_is_accented_while_the_wall_is_pushed() {
    let (mut app, mut terminal) = tall_app();
    roll_to_the_wall(&mut app);
    terminal.draw(|frame| app.render(frame)).unwrap();
    let at_rest = statusline(&app, &terminal);
    assert!(at_rest.ends_with("Bot"), "on the last line: {at_rest:?}");
    let plain = label_style(&app, &terminal, 2);
    assert_ne!(plain.fg, Some(theme::edge_resistance_color()));

    app.pan_vertical_down();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        statusline(&app, &terminal),
        at_rest,
        "the text is unchanged — the wall moved nothing"
    );
    for back in 1..=3 {
        assert_eq!(
            label_style(&app, &terminal, back).fg,
            Some(theme::edge_resistance_color()),
            "all three characters of `Bot` are accented, not just one"
        );
    }
    // The character in front of the label is not.
    assert_ne!(
        label_style(&app, &terminal, 4).fg,
        Some(theme::edge_resistance_color()),
        "the accent stops at the label"
    );

    // Panning back off the wall puts it out.
    app.pan_vertical_up();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_ne!(
        label_style(&app, &terminal, 2).fg,
        Some(theme::edge_resistance_color()),
        "a pan that moved spends the pressure, so the cue goes out"
    );
}

/// Spec 0286 test plan 9 / S6: the cue reports a gesture in progress,
/// so anything the reader does that is not that gesture puts it out —
/// and, since the wall's own state is what the cue draws, drops the
/// pressure with it.
///
/// Driven through `dispatch_event` rather than `handle_key`, because
/// that is where the rule lives: a pan and a non-pan have to go through
/// the same door for the difference between them to mean anything.
#[test]
fn any_other_input_ends_the_gesture() {
    let (mut app, _terminal) = tall_app();
    roll_to_the_wall(&mut app);

    let event = |key| AppEvent::Term(Event::Key(key));
    // Alt-Down is the main pane's vertical pan; a bare `j` is not.
    let pan = KeyEvent::new(KeyCode::Down, KeyModifiers::ALT);
    let other = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);

    for _ in 0..2 {
        dispatch_event(&mut app, &event(pan));
    }
    assert!(app.scroll_resistance.pushing(), "the wall is being pushed");

    // A key that is not a pan. It owes a frame, because the cue it puts
    // out was on screen.
    dispatch_event(&mut app, &event(other));
    assert!(!app.scroll_resistance.pushing(), "the gesture is over");
    assert!(
        !app.event_changed_nothing,
        "and the cue going out is a frame"
    );

    // So the pushing starts from nothing rather than resuming: two
    // pushes are not a gesture, however long ago the first one was.
    dispatch_event(&mut app, &event(pan));
    app.scroll_resistance.backdate(EDGE_HOLD);
    dispatch_event(&mut app, &event(pan));
    assert!(app.scroll_resistance.pushing(), "still against the wall");
}

/// An override pane with 30 candidates in a 5-row list, drawn once so
/// that the statusline the wall's cue lands on is real.
fn override_app() -> (App, Terminal<TestBackend>) {
    let mut app = message_node_app();
    app.splash = false;
    app.override_focus = true;
    app.override_target = Some(0);
    app.override_candidates = (0..30).map(|i| (format!("cand.Type{i}"), None)).collect();
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    (app, terminal)
}

/// Spec 0286 S2, in the two side panes: their lists have ends worth
/// stopping at too, and spec 0244 S7's over-pan past them is reached the
/// same way — by leaning.
#[test]
fn a_side_panes_list_stops_at_its_own_last_row() {
    let (mut app, _terminal) = override_app();
    let natural_max = natural_top_bounds(app.override_candidates.len(), app.override_list_height).1;
    assert!(natural_max > 0, "a list worth panning");

    for _ in 0..30 {
        app.override_pan_vertical(PAN_STEP, false);
    }
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        natural_max,
        "the wheel stops on the last candidate, not past it"
    );
    assert!(app.override_resistance.pushing(), "and is pushing on it");

    // Held against it, it gives — into spec 0244's over-pan and no
    // further.
    app.override_resistance.backdate(EDGE_HOLD);
    app.override_pan_vertical(PAN_STEP, false);
    assert!(
        app.override_scroll.top(&FLAT_ROWS) > natural_max,
        "and the next one is through"
    );
}

/// Spec 0286 S3+S6: a run is a repeat of the same push against the same
/// wall, so the three panes' walls are three runs and not one. Panning
/// the main pane is not a pan of the override pane's list, so it ends
/// that pane's gesture and puts its cue out rather than adding to it.
///
/// Driven through `dispatch_event`, because `settle` is what enforces
/// this and only `dispatch_event` calls it.
#[test]
fn each_panes_wall_is_pushed_on_its_own() {
    let (mut app, _terminal) = override_app();
    let key = |key| AppEvent::Term(Event::Key(key));
    // Ctrl-Down pans this pane's own list; Alt-Down reaches past it to
    // the main pane (spec 0185 S5).
    let mine = KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL);
    let theirs = KeyEvent::new(KeyCode::Down, KeyModifiers::ALT);

    for _ in 0..8 {
        dispatch_event(&mut app, &key(mine));
    }
    let stopped = app.override_scroll.top(&FLAT_ROWS);
    assert!(app.override_resistance.pushing(), "pushing this wall");
    assert!(
        !app.scroll_resistance.pushing(),
        "and not the main pane's, which was never touched"
    );

    dispatch_event(&mut app, &key(theirs));
    assert!(
        !app.override_resistance.pushing(),
        "another pane's pan is not this pane's gesture"
    );

    // So what follows is a first push and not a fourth, however old the
    // pressure it interrupted was.
    dispatch_event(&mut app, &key(mine));
    app.override_resistance.backdate(EDGE_HOLD);
    dispatch_event(&mut app, &key(mine));
    assert_eq!(
        app.override_scroll.top(&FLAT_ROWS),
        stopped,
        "two pushes are not a gesture"
    );
}

/// Spec 0286 S6 in the override pane: its own statusline carries its own
/// viewport label, and the accent reports its own wall.
///
/// A *named* color, as the main pane's own case is: the CI sandbox has
/// no `COLORTERM`.
#[test]
fn a_side_panes_viewport_label_is_accented_while_its_wall_is_pushed() {
    let (mut app, mut terminal) = override_app();
    let row = |app: &App, terminal: &Terminal<TestBackend>| -> Vec<(String, Style)> {
        let buffer = terminal.backend().buffer();
        let y = app.side_area.y + app.side_area.height;
        (app.side_area.x..app.side_area.x + app.side_area.width)
            .map(|x| (buffer[(x, y)].symbol().to_string(), buffer[(x, y)].style()))
            .collect()
    };
    let accented = |cells: &[(String, Style)]| -> String {
        cells
            .iter()
            .filter(|(_, style)| style.fg == Some(theme::edge_resistance_color()))
            .map(|(symbol, _)| symbol.as_str())
            .collect()
    };

    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(accented(&row(&app, &terminal)), "", "nothing is pushing");

    for _ in 0..30 {
        app.override_pan_vertical(PAN_STEP, false);
    }
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        accented(&row(&app, &terminal)),
        "Bot",
        "the label alone, and the one at the end of this pane's own row"
    );
}

/// Spec 0286 test plan 7 / S7: the push that lights the cue changes the
/// screen and so owes a frame; the ones behind it do not, which is what
/// keeps a held wheel at the bottom of a document as cheap as spec 0245
/// S2 made it.
#[test]
fn the_push_that_lights_the_cue_owes_a_frame() {
    let (mut app, _terminal) = tall_app();
    roll_to_the_wall(&mut app);
    assert!(
        !app.event_changed_nothing,
        "arriving on the last line is a real move"
    );

    app.pan_vertical_down();
    assert!(
        !app.event_changed_nothing,
        "the first push moves nothing but lights the cue"
    );
    app.pan_vertical_down();
    assert!(
        app.event_changed_nothing,
        "the second changes neither the viewport nor the cue"
    );
}
