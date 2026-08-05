// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0114 §4: the `/` and `?` search prompt — where a pattern
//! matches, which way `n` repeats, and how case is folded.
//!
//! Both panes' searches are here rather than one each with its own pane:
//! they share `jump_to_match`'s casing rules, and the smartcase tests
//! only convince side by side.

use super::super::*;
use super::support::*;

/// Spec 0114 §4, extended to the main pane: `/`/`?` open a search
/// prompt on the shared command-line row, `n` repeats the last search
/// in the same direction, and matches wrap around the document.
#[test]
fn main_pane_search_forward_backward_and_repeat_with_n() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;
    assert_eq!(app.cursor, 0); // alpha

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert_eq!(app.command_buffer.as_deref(), Some(""));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.cursor, 1); // beta

    // `n` repeats forward, wrapping to the next match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // Wraps back around to the first match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta

    // `?` searches backward from the cursor (beta) — skips itself,
    // wraps to beta2.
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // No match leaves the cursor unchanged and sets a message.
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "nope".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 3);
    assert!(app.message.contains("not found"));
}

/// `N` repeats the last main-pane search in the opposite direction.
#[test]
fn main_pane_search_repeat_with_capital_n_reverses_direction() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta

    // `N` repeats backward, wrapping to beta2.
    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // A second `N` continues backward, wrapping to beta.
    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta
}

/// Spec 0114 §4 (vim convention), extended to the main pane:
/// confirming `/` or `?` with an empty pattern re-uses the last
/// active search pattern, searching in the newly chosen direction.
#[test]
fn main_pane_search_with_no_argument_reuses_the_active_pattern() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "gamma: 3", "beta2: 4"]);
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta

    // `/<Enter>` with no typed pattern re-uses "beta", forward.
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 3); // beta2

    // `?<Enter>` with no typed pattern re-uses "beta" too, backward.
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // beta
}

/// Spec 0114 §4, extended to the main pane: `Esc` cancels an
/// in-progress search without moving the cursor, and `Backspace` on an
/// empty buffer also cancels it.
#[test]
fn main_pane_search_esc_and_empty_backspace_cancel() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.cursor, 0);

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
}

/// Spec 0114 §4's main-pane search directive: "search in main pane
/// requires main pane to be in focus" — while the override pane has
/// focus, `/`/`?`/`n` share the same `command_buffer` as main-pane
/// search (spec-0133-adjacent rework), but `Enter` dispatches to the
/// override pane's own `jump_to_override_match`, not the main pane's
/// `jump_to_match` — the main-pane cursor never moves.
#[test]
fn main_pane_search_keys_are_inert_while_override_pane_has_focus() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    let cursor_before = app.cursor;

    app.override_candidates = vec![("pkg.Alpha".to_string(), None)];
    app.override_highlight = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.command_buffer.is_some());
    for c in "alpha".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.command_buffer.is_none());
    assert_eq!(app.override_highlight, 0); // pkg.Alpha
    assert_eq!(app.cursor, cursor_before);
}

/// Spec 0114 §4's main-pane search directive: search matches against
/// the *current* rendered text the nodes hold, so a range whose type
/// has been overridden is matched post-override, not against the
/// original rendering — there is no separate "original text" cache to
/// special-case.
#[test]
fn main_pane_search_matches_the_current_not_original_rendering() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    // Simulate an already-applied override splice (spec 0114 §5):
    // node 1's rendered line no longer contains "beta" at all.
    app.node_text[1] = Some(Box::from("pkg.Overridden { x: 1 }"));

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "beta".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 0); // unchanged — "beta" no longer present
    assert!(app.message.contains("not found"));

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "overridden".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 1); // matches the overridden text
}

/// Spec 0195 S2, at the matcher itself: vim's smartcase. An
/// all-lowercase pattern folds; a pattern carrying any uppercase
/// character matches exactly. "Any uppercase character" is the rule, not
/// "mixed case" — an all-uppercase pattern is case-sensitive too, which
/// is the case a `pattern != pattern.to_lowercase()` test gets right by
/// luck and a mixed-case test would get wrong.
#[test]
fn smartcase_folds_only_an_all_lowercase_pattern() {
    let lower = SearchPattern::new("beta");
    assert!(lower.is_match("beta: 1"));
    assert!(lower.is_match("Beta: 1"));
    assert!(lower.is_match("BETA: 1"));

    let mixed = SearchPattern::new("Beta");
    assert!(mixed.is_match("Beta: 1"));
    assert!(!mixed.is_match("beta: 1"));
    assert!(!mixed.is_match("BETA: 1"));

    let upper = SearchPattern::new("BETA");
    assert!(upper.is_match("BETA: 1"));
    assert!(!upper.is_match("beta: 1"));
    assert!(!upper.is_match("Beta: 1"));
}

/// Spec 0195 S2: the case fold runs per `char` and streams, because a
/// few characters lowercase to more than one — `İ` becomes `i` plus a
/// combining dot. A one-to-one fold would misalign the comparison from
/// that character onwards rather than merely missing this match.
#[test]
fn the_case_fold_handles_a_multi_character_lowercase_mapping() {
    let dotted = SearchPattern::new("i\u{307}d");
    assert!(dotted.is_match("\u{130}d: 7"));
}

/// Spec 0195 Background 4 / test plan 8: nothing trims the pattern, so a
/// leading space is part of it and does distinguish an indented token
/// from one that starts its own line. The 2026-07-27 report read as
/// though the space were dropped; it is not, and this pins that.
#[test]
fn a_leading_space_is_part_of_the_pattern() {
    let spaced = SearchPattern::new(" id");
    assert!(spaced.is_match("  id: 7"));
    assert!(!spaced.is_match("id: 7"));
}

/// Spec 0195 G2 in the main pane: a lowercase pattern reaches a
/// capitalized node, and a capitalized pattern walks straight past the
/// lowercase one.
#[test]
fn main_pane_search_is_smartcase() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2", "Beta: 3"]);

    app.jump_to_match(SearchDir::Forward, "beta");
    assert_eq!(app.cursor, 1);
    // Folds onto the capitalized node rather than stopping at "beta".
    app.jump_to_match(SearchDir::Forward, "beta");
    assert_eq!(app.cursor, 2);

    app.set_cursor(0);
    app.jump_to_match(SearchDir::Forward, "Beta");
    assert_eq!(app.cursor, 2); // skipped the lowercase node entirely
}

/// Spec 0195 G2 in the override pane. FQDNs are where case carries the
/// most meaning: `pkg.Beta` is a message and `pkg.beta` would be a
/// field, and the pane's whole job is telling them apart.
#[test]
fn override_pane_search_is_smartcase() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![
        ("pkg.alpha".to_string(), None),
        ("pkg.beta".to_string(), None),
        ("pkg.Beta".to_string(), None),
    ];

    app.override_highlight = 0;
    app.jump_to_override_match(SearchDir::Forward, "beta");
    assert_eq!(app.override_highlight, 1);

    app.override_highlight = 0;
    app.jump_to_override_match(SearchDir::Forward, "Beta");
    assert_eq!(app.override_highlight, 2);
}

/// The caret's byte offset within its own row — spec 0246 S8's
/// conversion, which the oracle needs because it holds match positions
/// in bytes and `cursor_column` counts characters.
fn caret_byte_offset(app: &App) -> usize {
    let Some(pos) = app.line_pos(app.cursor_line()) else {
        return 0;
    };
    let text = app.line_text(pos);
    text.char_indices()
        .nth(app.cursor_column)
        .map_or(text.len(), |(i, _)| i)
}

/// Every stop a linear scan of the whole rendered text would make, in
/// document order, as `(line, byte offset)` — the `App::lines` scan spec
/// 0222 S6 replaced, written out so the walk can be held against it.
///
/// Deliberately the crudest possible implementation: a `Vec<String>`,
/// enumerated front to back, with no structure at all. It shares the
/// matcher with the walk, and — since spec 0246 S4 made a stop a match
/// rather than a row — S5's one-character step as well; both have their
/// own tests above and neither is what is on trial here, which is the
/// *order* a node-to-node step visits the document in.
///
/// A closing brace is skipped because it draws no content of its own —
/// it is the one line whose characters the nodes do not store, and the
/// search has never matched one.
///
/// Spec 0235 S19/0246 S9: a line's owning node's positional path is a
/// second haystack, tried only when the text offered nothing and worth
/// exactly one stop. Without it the oracle would disagree with the walk
/// on every row whose path happens to contain the pattern — and `1` is a
/// pattern most paths contain.
fn every_match_in_text_order(app: &App, pattern: &str) -> Vec<(usize, usize)> {
    let lines = app.document_lines();
    let needle = SearchPattern::new(pattern);
    let mut out = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let Some(pos) = app.line_pos(line) else {
            continue;
        };
        if app.is_footer(pos) {
            continue;
        }
        // Spec 0246 S3a: a stop sits at the column its caret lands on,
        // never left of the row's first non-blank.
        let indent = text.len() - text.trim_start().len();
        let before = out.len();
        let mut from = 0;
        while let Some(range) = needle.find_range_from(text, from) {
            out.push((line, range.start.max(indent)));
            from = range.start + text[range.start..].chars().next().map_or(1, char::len_utf8);
        }
        out.dedup();
        if out.len() == before && needle.is_match(&app.positional_path(pos.node)) {
            out.push((line, indent));
        }
    }
    out
}

/// Spec 0222, test-plan item 7: the walk-based search visits the
/// document in the order a scan of its text does.
///
/// S6 gave up the one thing the old search had for free — a flat array
/// of every line, in order — and replaced it with a step from node to
/// node. Every way that can go wrong produces a *plausible* answer: a
/// step that descends into a node's own children from its closing
/// brace, or one that skips a packed run's later elements, still lands
/// the cursor on a line containing the pattern. Only the order gives it
/// away.
///
/// Repeating the search rather than doing it once is what covers the
/// wrap: the sequence is longer than the number of matches, so both
/// directions run off the end of the document and back.
///
/// Backward is the half that can break, since it is the direction with
/// no natural "descend first" reading, so it gets a first-node match
/// explicitly: from the top, one step back is the wrap, and the wrap is
/// where an endpoint resolved eagerly or off by one shows up.
#[test]
fn search_finds_the_same_hits_in_the_same_order() {
    let (packed, ..) = packed_run_with_tail_fixture();
    let (mut folded, items) = repeated_message_fixture();
    folded.toggle_fold(items[1]);

    for (name, mut app) in [("packed_run", packed), ("folded", folded)] {
        // `:` is on every scalar row, `{` on every message header, and
        // `1` lands inside a packed run's elements — between them the
        // patterns reach all three of the shapes a step has to handle.
        for pattern in [":", "{", "1"] {
            for dir in [SearchDir::Forward, SearchDir::Backward] {
                app.set_cursor(app.first_node);
                let all = every_match_in_text_order(&app, pattern);
                // The sequence below is twelve long whatever this is, so
                // the wrap is covered either way; what has to hold is
                // that the pattern matches more than once, or an order
                // proves nothing.
                assert!(
                    all.len() > 1,
                    "{name}/{pattern:?}: the fixture must offer several matches"
                );
                // Spec 0246 S3: the caret's own position is the split, so
                // the first stop is the nearest match strictly past it —
                // which for a `Backward` start at the top of the document
                // is the wrap onto the very last one.
                let start = (app.cursor_line(), caret_byte_offset(&app));
                let mut i = match dir {
                    SearchDir::Forward => all.iter().position(|&p| p > start).unwrap_or(0),
                    SearchDir::Backward => all
                        .iter()
                        .rposition(|&p| p < start)
                        .unwrap_or(all.len() - 1),
                };
                let mut want = Vec::new();
                for _ in 0..12 {
                    want.push(all[i].0);
                    i = match dir {
                        SearchDir::Forward => (i + 1) % all.len(),
                        SearchDir::Backward => (i + all.len() - 1) % all.len(),
                    };
                }

                app.set_cursor(app.first_node);
                let got: Vec<usize> = (0..want.len())
                    .map(|_| {
                        app.jump_to_match(dir, pattern);
                        app.cursor_line()
                    })
                    .collect();
                assert_eq!(got, want, "{name}/{pattern:?}/{dir:?}");
            }
        }
    }
}

/// Spec 0194 test-plan item 10 (S8). A search hit is the one jump that
/// knows a better column than the row's start: the match's own first
/// character. A match reaching back into the indentation still clamps,
/// since the indentation is unreachable (S3).
#[test]
fn a_search_hit_puts_the_caret_on_the_match() {
    let mut app = sibling_leaves_app(&["alpha: 1", "  beta: 2"]);

    app.jump_to_match(SearchDir::Forward, "beta");
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, 2, "on the match's first character");
    assert_eq!(app.desired_column, 2);

    // Within the row the caret still follows the match.
    app.jump_to_match(SearchDir::Forward, "2");
    assert_eq!(app.cursor, 1);
    assert_eq!(app.cursor_column, "  beta: ".chars().count());

    // A pattern whose first character is in the indentation clamps.
    app.jump_to_match(SearchDir::Forward, " beta");
    assert_eq!(app.cursor_column, 2, "clamped onto the first non-blank");
}

// ---------------------------------------------------------------------
// Spec 0235: the incremental search.
// ---------------------------------------------------------------------

/// Type `keys` one keystroke at a time. A leading `/` opens the prompt
/// rather than being inserted into it, which is exactly what the real
/// keyboard does.
fn type_keys(app: &mut App, keys: &str) {
    for c in keys.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

/// Run the live sweep to the end, the way `run_loop`'s idle arm does,
/// and report how many slices it took.
fn settle_sweep(app: &mut App) -> usize {
    for slices in 0..10_000 {
        if app.search_sweep_step() == SweepStep::Idle {
            return slices;
        }
    }
    panic!("a sweep must converge");
}

/// The main pane's area, at the size the centering tests reason about.
fn pane() -> Rect {
    Rect::new(0, 0, 40, 10)
}

/// A long document whose only occurrence of `target` is on `line`,
/// prefixed by `indent` spaces and followed by enough filler for the
/// horizontal centering to have somewhere to put it.
fn target_at(n: usize, line: usize, indent: usize) -> App {
    let mut app = wide_sibling_scalars_app(n);
    app.node_text[line] = Some(Box::from(
        format!("{}target: 0{}", " ".repeat(indent), "x".repeat(indent)).as_str(),
    ));
    app.splash = false;
    app.term_width = 120;
    app.message.clear();
    app
}

/// Spec 0235 test-plan item 1 (G1, G3). The keystrokes alone bring the
/// match on screen — no `Enter`, and the cursor has not moved, because
/// a sweep's whole effect on the pane is the viewport.
#[test]
fn typing_into_a_search_prompt_scrolls_to_the_match() {
    let mut app = target_at(200, 150, 0);
    let before = (app.cursor, app.cursor_line_in_node, app.cursor_column);

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    app.center_search_match(pane());

    let row = app.visible_row_of_line(150).expect("the match's row");
    assert!(
        (app.scroll.index..app.scroll.index + pane().height as usize).contains(&row),
        "the match must be on screen: row {row} against scroll {}",
        app.scroll.index
    );
    assert_eq!(
        (app.cursor, app.cursor_line_in_node, app.cursor_column),
        before,
        "a preview never moves the cursor"
    );
}

/// Spec 0235 test-plan item 2 (S7). Abandoning a sweep is a struct
/// assignment: the keystroke leaves a sweep carrying the *new* pattern
/// and walking from the origin again, so the work the old one had done
/// buys the new one nothing.
#[test]
fn a_keystroke_abandons_the_sweep_in_flight() {
    let mut app = wide_sibling_scalars_app(2100);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/zz");
    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);

    type_keys(&mut app, "z");
    let sweep = app
        .search_sweep
        .as_ref()
        .expect("the keystroke must leave a sweep behind");
    assert_eq!(sweep.pattern.needle, "zzz");
    assert_eq!(
        settle_sweep(&mut app),
        3,
        "the walk must cover the whole document again, not resume"
    );
}

/// Spec 0235 test-plan item 3 (S2, S4). A step visits exactly one
/// slice — 2100 candidates over a slice of 1000 is three steps and not
/// one more — and the walk reports `Idle` only once it is over.
#[test]
fn a_sweep_step_visits_at_most_one_slice() {
    let n = 2 * search::SEARCH_SWEEP_SLICE + 100;
    let mut app = wide_sibling_scalars_app(n);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/zzz");
    assert_eq!(
        settle_sweep(&mut app),
        n.div_ceil(search::SEARCH_SWEEP_SLICE)
    );
}

/// Spec 0235 test-plan item 4 (S3). `Idle` is the condition `run_loop`
/// needs to reach `recv_timeout`, and it holds both before a search and
/// after one — a finished sweep is kept for its answer, not for more
/// work.
#[test]
fn a_finished_sweep_lets_the_loop_sleep_again() {
    let mut app = wide_sibling_scalars_app(10);
    app.splash = false;
    app.term_width = 120;

    assert_eq!(app.search_sweep_step(), SweepStep::Idle);
    type_keys(&mut app, "/field_3");
    settle_sweep(&mut app);
    assert_eq!(app.search_sweep_step(), SweepStep::Idle);
}

/// Spec 0235 test-plan item 5 (S5). A sweep steps hundreds of times a
/// second; a frame each would be the stall the whole design is against.
/// Only the slice that changes the *answer* owes one.
#[test]
fn a_sweep_forces_a_frame_only_when_its_result_changes() {
    let mut app = target_at(2500, 1500, 0);

    type_keys(&mut app, "/target");
    app.take_search_dirty(); // the prompt opening owes its own frame

    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);
    assert!(
        !app.take_search_dirty(),
        "a slice that found nothing changed nothing"
    );
    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);
    assert!(app.take_search_dirty(), "the slice that found the match");
}

/// Spec 0235 test-plan item 6 (G2, S7). Because every edit restarts from
/// the origin rather than resuming, `Backspace` puts the view back
/// exactly where the keystroke before it had left it.
#[test]
fn backspace_undoes_the_keystroke_before_it() {
    let mut app = wide_sibling_scalars_app(200);
    app.node_text[50] = Some(Box::from("fo: 0"));
    app.node_text[150] = Some(Box::from("foo: 0"));
    app.splash = false;
    app.term_width = 120;

    let settle = |app: &mut App| {
        settle_sweep(app);
        app.center_search_match(pane());
        (app.scroll.index, app.pan_offset)
    };

    type_keys(&mut app, "/fo");
    let after_fo = settle(&mut app);
    type_keys(&mut app, "o");
    let after_foo = settle(&mut app);
    assert_ne!(after_foo, after_fo, "the fixture must move the view");

    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(settle(&mut app), after_fo);
}

/// Spec 0235 test-plan item 7 (G3, S6, S8, S11). `Esc` restores the view
/// the prompt was opened over, and the cursor was never anywhere else to
/// restore.
#[test]
fn esc_puts_the_view_back_and_never_moved_the_cursor() {
    let mut app = target_at(200, 150, 0);
    let view = (app.scroll.index, app.pan_offset);
    let cursor = (app.cursor, app.cursor_line_in_node, app.cursor_column);

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    app.center_search_match(pane());
    assert_ne!((app.scroll.index, app.pan_offset), view);
    assert_eq!(
        (app.cursor, app.cursor_line_in_node, app.cursor_column),
        cursor
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!((app.scroll.index, app.pan_offset), view);
    assert_eq!(
        (app.cursor, app.cursor_line_in_node, app.cursor_column),
        cursor
    );
}

/// Spec 0235 test-plan item 8 (S8, S9). The jumplist records where the
/// user *was*, so a preview must leave no trace in it: a whole `/foo`
/// plus `Enter` is worth one entry, and a `/foo` plus `Esc` none.
#[test]
fn a_preview_records_no_jumplist_entry() {
    let mut app = target_at(200, 150, 0);

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    assert!(app.back_stack.is_empty(), "not while the prompt is open");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.back_stack.len(), 1);

    type_keys(&mut app, "/field_10");
    settle_sweep(&mut app);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.back_stack.len(), 1, "a canceled search records nothing");
}

/// Spec 0235 test-plan item 9 (S9). `Enter` on a sweep that is still
/// mid-document runs it to completion rather than committing what it
/// happens to have found — the wait is now the answer's cost, not a
/// keystroke's.
#[test]
fn enter_finishes_an_unfinished_sweep() {
    let mut app = target_at(2500, 2000, 0);

    type_keys(&mut app, "/target");
    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.cursor, 2000);
}

/// Spec 0235 test-plan item 10 (S10). The prompt and the message share
/// one row, so while the prompt is open the pattern itself carries the
/// "no match" news; the message only returns at `Enter`.
#[test]
fn an_unmatched_pattern_tints_itself_and_sets_no_message() {
    let mut app = target_at(200, 150, 0);

    type_keys(&mut app, "/zzz");
    assert!(app.message.is_empty(), "not while still looking");
    settle_sweep(&mut app);
    assert!(app.message.is_empty(), "and not once it has finished");
    assert_eq!(app.command_buffer.as_deref(), Some("zzz"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.message.contains("not found"));
}

/// Spec 0235 test-plan item 11 (S12). An axis the match already fits in
/// does not move — a minimum nudge would put every match on the same
/// edge row, and an unconditional recenter would make the text swim.
#[test]
fn a_match_already_on_screen_moves_nothing() {
    let mut app = target_at(200, 3, 0);

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    let before = (app.scroll.index, app.pan_offset);
    app.center_search_match(pane());
    assert_eq!((app.scroll.index, app.pan_offset), before);
}

/// Spec 0235 test-plan item 12 (G4, S12). An axis the match does *not*
/// fit in centers it, clamped to the document's own bounds.
#[test]
fn a_match_off_screen_is_centered_on_both_axes() {
    let mut app = target_at(200, 150, 100);
    app.main_area = pane();

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    app.center_search_match(pane());

    // 150 - (10 - 1) / 2, the topmost row that leaves the match in the
    // middle of a ten-row pane.
    assert_eq!(app.scroll.index, 146);
    // 100 - (38 - 6) / 2, over the 38 columns left once the fold field
    // has taken its two.
    assert!(
        app.max_pan_offset() >= 84,
        "the fixture's widest visible row must leave room to center, \
         or the clamp is what is being tested"
    );
    assert_eq!(app.pan_offset, 84);
}

/// Spec 0235 test-plan item 17 (S17, S18). `Ctrl-a`/`Ctrl-e` are
/// readline's line ends, and the point of having them is that they move
/// the cursor without disturbing what has been typed.
#[test]
fn ctrl_a_and_ctrl_e_move_the_command_cursor() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/abc");
    assert_eq!(app.command_cursor, 3);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 0);
    assert_eq!(app.command_buffer.as_deref(), Some("abc"));

    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(app.command_cursor, 3);
    assert_eq!(app.command_buffer.as_deref(), Some("abc"));
}

/// Spec 0235 test-plan item 18 (S18), extended by spec 0237 S9. An
/// unbound control chord is ignored, not typed: `Ctrl-u` used to insert
/// a bare `u`. `Ctrl-d` is now bound, and must not insert a `d` either.
#[test]
fn a_control_modified_letter_is_not_inserted() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/abc");
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(app.command_buffer.as_deref(), Some("abc"));
    assert_eq!(app.command_cursor, 3);

    // Bound, and at the end of the buffer, so it deletes nothing —
    // which is exactly how a stray `d` would show up.
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.command_buffer.as_deref(), Some("abc"));
    assert_eq!(app.command_cursor, 3);
}

/// Spec 0237 S9/S10. `Ctrl-d` is readline's forward-delete — the one
/// hole left in the `Ctrl-a`/`Ctrl-e`/`Ctrl-b`/`Ctrl-f` set — and
/// nothing more: it is *not* readline's delete-or-EOF, since an empty
/// buffer is not a quit here (spec 0236 G8).
#[test]
fn ctrl_d_forward_deletes_in_the_command_line() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/abc");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.command_buffer.as_deref(), Some("bc"));
    assert_eq!(app.command_cursor, 0, "forward-delete does not move");

    for _ in 0..2 {
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    }
    assert_eq!(app.command_buffer.as_deref(), Some(""));

    // An empty buffer is not a quit, and the prompt stays open.
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.command_buffer.as_deref(), Some(""));
    assert!(!app.should_quit);
}

/// Spec 0237 S11/S12. The prompt's pattern has three states, not two:
/// orange while the sweep is still running, default once it has a hit,
/// red only once it has finished with nothing. Spec 0235 tinted the
/// first and the third alike, which trained the user to ignore red on
/// any document big enough for the sweep to be visible.
#[test]
fn search_prompt_is_orange_while_sweeping_and_red_when_finished() {
    let mut app = target_at(2500, 2000, 0);
    app.splash = false;
    app.term_width = 120;

    let pattern_style = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let area = app.cmd_area.expect("the prompt must be on screen");
        // Column 0 is the `/` sigil; the pattern starts at column 1.
        terminal.backend().buffer()[(area.x + 1, area.y)].style().fg
    };

    type_keys(&mut app, "/target");
    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);
    assert_eq!(
        pattern_style(&mut app),
        theme::search_running_style(app.theme).fg,
        "still looking is not the same news as not there"
    );

    settle_sweep(&mut app);
    assert!(app.search_sweep.as_ref().unwrap().found.is_some());
    assert_eq!(
        pattern_style(&mut app),
        Some(Color::Reset),
        "a found pattern carries no news at all"
    );

    type_keys(&mut app, "zzz");
    settle_sweep(&mut app);
    assert_eq!(
        pattern_style(&mut app),
        theme::search_unmatched_style(app.theme).fg,
        "finished with nothing is the only red"
    );
}

/// Spec 0235 test-plan item 19 (G8, S19). A line offers two haystacks
/// with no shape test: its own text and its owning node's positional
/// path. The path reaches a node no line's text names, and a document
/// carrying the pattern both ways yields both lines in document order.
#[test]
fn a_pattern_is_tried_against_the_path_and_the_text() {
    let (mut app, _run, tail, a, _b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");
    assert_eq!(app.positional_path(id), "/2/1");
    assert!(
        app.document_lines().iter().all(|l| !l.contains("/2/1")),
        "the path must be the only haystack that can reach it"
    );

    app.set_cursor(app.first_node);
    app.jump_to_match(SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id);

    // Now a second line carries the same pattern in its *text*.
    app.node_text[a] = Some(Box::from("  a: \"/2/1\""));
    app.set_cursor(app.first_node);
    app.jump_to_match(SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id, "the path match comes first in the document");
    app.jump_to_match(SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, a);
}

/// Spec 0235 test-plan item 20 (S20). A path is not on screen, so a path
/// match has no column of its own: it lands on the row's Home anchor. A
/// text match keeps the matched character and `Free`, and a line
/// matching both ways takes the text column.
#[test]
fn a_path_match_lands_on_the_home_anchor() {
    let (mut app, _run, tail, a, _b) = packed_run_with_tail_fixture();
    let id = app.nth_child(tail, 0).expect("tail has one child");

    app.set_cursor(app.first_node);
    app.jump_to_match(SearchDir::Forward, "/2/1");
    assert_eq!(app.cursor, id);
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
    assert_eq!(app.cursor_column, app.caret_bounds().0);

    app.set_cursor(app.first_node);
    app.jump_to_match(SearchDir::Forward, "vals");
    assert_eq!(app.caret_anchor, CaretAnchor::Free);
    assert_eq!(app.cursor_column, 2);

    // `a`'s path is `/3` and its text now holds `/3` too.
    app.node_text[a] = Some(Box::from("  a: \"/3\""));
    app.set_cursor(app.first_node);
    app.jump_to_match(SearchDir::Forward, "/3");
    assert_eq!(app.cursor, a);
    assert_eq!(app.caret_anchor, CaretAnchor::Free);
    assert_eq!(app.cursor_column, 6, "the column the user can see");
}

/// Spec 0235 test-plan item 22 (S23). The side panes list FQDNs, not
/// nodes: they have one haystack, so a positional path finds nothing
/// there however well it matches the document.
#[test]
fn the_side_panes_match_text_only() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

    app.override_candidates = vec![
        ("pkg.Alpha".to_string(), None),
        ("pkg.Beta".to_string(), None),
    ];
    app.override_highlight = 0;
    app.message.clear();

    app.jump_to_override_match(SearchDir::Forward, "/1");
    assert_eq!(app.override_highlight, 0);
    assert!(app.message.contains("not found"));
}

// ---------------------------------------------------------------------
// Spec 0246: match granularity, the history, and the rotation.
// ---------------------------------------------------------------------

/// Press `code` with no modifier.
fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// Press `code` with `Control` — spec 0246 S17's two rotation keys.
fn press_ctrl(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::CONTROL));
}

/// Type `/pattern` and confirm it, the way a user records a history
/// entry (spec 0246 S12).
fn commit_search_by_key(app: &mut App, pattern: &str) {
    type_keys(app, "/");
    type_keys(app, pattern);
    press(app, KeyCode::Enter);
}

/// Spec 0246 test-plan item 1 (G5, S2, S3). `n` over a row carrying
/// three occurrences stops at each of them before the row changes, and
/// the cycle closes on the one it started from.
#[test]
fn n_stops_at_every_match_on_a_line() {
    let mut app = sibling_leaves_app(&["ab ab ab", "zz"]);

    let got: Vec<(usize, usize)> = (0..4)
        .map(|_| {
            app.jump_to_match(SearchDir::Forward, "ab");
            (app.cursor, app.cursor_column)
        })
        .collect();
    // Starting on the match at column 0, the caret steps off it onto the
    // row's other two, and the wrap brings it back to where it began.
    assert_eq!(got, vec![(0, 3), (0, 6), (0, 0), (0, 3)]);
}

/// Spec 0246 test-plan item 2 (S4) — Background 4's first consequence.
/// A backward search arriving at a row takes its *rightmost* match; the
/// leftmost would land right of where the user is looking.
#[test]
fn a_backward_search_lands_on_the_last_match_of_the_row() {
    let mut app = sibling_leaves_app(&["zz", "ab ab ab"]);

    app.jump_to_match(SearchDir::Backward, "ab");
    assert_eq!((app.cursor, app.cursor_column), (1, 6));
}

/// Spec 0246 test-plan item 3 (S2, S3). The origin row is visited twice
/// and the two halves partition it, so a document with one match sends
/// `n` back to that match rather than reporting "not found".
#[test]
fn a_search_wraps_back_to_the_match_it_started_on() {
    let mut app = sibling_leaves_app(&["ab", "zz"]);
    app.message.clear();

    app.jump_to_match(SearchDir::Forward, "ab");
    assert_eq!((app.cursor, app.cursor_column), (0, 0));
    assert!(
        app.message.is_empty(),
        "the match is still there: {}",
        app.message
    );
}

/// Spec 0246 test-plan item 4 (S5). The scan for the next eligible start
/// resumes one character past the previous *start*, not past its end, so
/// `aa` has two stops in `aaa` — vim's rule, and the only one under
/// which S3's two halves partition the row.
#[test]
fn overlapping_matches_are_separate_stops() {
    let mut app = sibling_leaves_app(&["aaa", "zz"]);

    app.jump_to_match(SearchDir::Forward, "aa");
    assert_eq!((app.cursor, app.cursor_column), (0, 1));
    app.jump_to_match(SearchDir::Forward, "aa");
    assert_eq!((app.cursor, app.cursor_column), (0, 0));
}

/// Spec 0246 test-plan item 5 (S2). Spec 0235 skipped the caret's own
/// row entirely, so a match later on it was unreachable without a wrap
/// through the whole document; the first visit of the two now finds it.
#[test]
fn the_caret_row_is_searched_ahead_of_the_caret() {
    let mut app = sibling_leaves_app(&["ab ab", "zz"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/ab");
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), Some((0, 3, 2, false)));
}

/// Spec 0246 test-plan item 6 (S9). A path match is one stop per row
/// however many times the pattern occurs in the path, and the bound
/// still admits it exactly once per cycle — so a run of rows matching
/// only on their paths visits each of them once.
#[test]
fn a_path_match_is_one_stop_per_row() {
    let mut app = sibling_leaves_app(&["alpha: 0", "beta: 0", "gamma: 0"]);
    // The pattern must be reachable *only* through the path, or the
    // text branch is what is being measured.
    assert!(app.document_lines().iter().all(|l| !l.contains('/')));

    let got: Vec<usize> = (0..4)
        .map(|_| {
            app.jump_to_match(SearchDir::Forward, "/");
            app.cursor
        })
        .collect();
    assert_eq!(got, vec![1, 2, 0, 1]);
    assert_eq!(app.caret_anchor, CaretAnchor::Home);
}

/// Spec 0246 test-plan item 7 (N4). A side pane highlights whole
/// entries, so a second stop inside one entry would draw as nothing
/// having happened: its stop count stays its entry count, whatever the
/// pattern does inside an entry's text.
#[test]
fn the_manage_pane_still_stops_once_per_entry() {
    let (mut app, items) = repeated_message_fixture();
    app.manage_focus = true;
    app.manage_open = true;

    // Every type carries the pattern twice, so a match-granular walk
    // would take six stops to come back round instead of three.
    for (item, ty) in items.iter().zip(["pkg.zz1", "pkg.zz2", "pkg.zz3"]) {
        let origin = OverrideOrigin::Path {
            path: app.positional_path(*item),
        };
        app.overrides.activate(origin, Some(ty.to_string()));
    }
    // Entry 0 is the auto-derived root (`/ test.Outer`), which carries
    // no `z`; the three activated ones are 1, 2 and 3.
    app.manage_highlight = 1;
    app.message.clear();

    let got: Vec<usize> = (0..3)
        .map(|_| {
            app.jump_to_manage_match(SearchDir::Forward, "z");
            app.manage_highlight
        })
        .collect();
    assert_eq!(got, vec![2, 3, 1], "{}", app.message);
}

/// Spec 0246 test-plan item 8 (G1, S14, S15). `Up` recalls the newest
/// committed pattern and leaves the command cursor at its end, ready to
/// be edited; the oldest entry is where `Up` stops, without wrapping and
/// without a message.
#[test]
fn up_at_a_search_prompt_recalls_the_last_committed_pattern() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "alpha");
    commit_search_by_key(&mut app, "beta");

    type_keys(&mut app, "/");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    assert_eq!(app.command_cursor, 4);

    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("alpha"));
    assert_eq!(app.command_cursor, 5);

    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("alpha"), "no wrap");
}

/// Spec 0246 test-plan item 9 (S14). The draft is stashed, not
/// discarded: `Down` past the newest entry gives back what was typed.
/// `Down` when no browse is under way does nothing at all — there is no
/// "future" to walk into.
#[test]
fn down_past_the_newest_history_entry_restores_what_was_typed() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "alpha");

    type_keys(&mut app, "/be");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("alpha"));

    press(&mut app, KeyCode::Down);
    assert_eq!(app.command_buffer.as_deref(), Some("be"));
    assert_eq!(app.command_cursor, 2);

    press(&mut app, KeyCode::Down);
    assert_eq!(app.command_buffer.as_deref(), Some("be"));
}

/// Spec 0246 test-plan item 10 (S16). Editing a recalled entry ends the
/// browse, so the edited text is the user's own again: `Down` no longer
/// answers, and the next `Up` starts a fresh walk whose stash is the
/// edit rather than the empty buffer the first one saw.
#[test]
fn editing_after_a_history_recall_ends_the_browse() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "alpha");
    commit_search_by_key(&mut app, "beta");

    type_keys(&mut app, "/");
    press(&mut app, KeyCode::Up);
    type_keys(&mut app, "x");
    assert_eq!(app.command_buffer.as_deref(), Some("betax"));

    press(&mut app, KeyCode::Down);
    assert_eq!(app.command_buffer.as_deref(), Some("betax"));

    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    press(&mut app, KeyCode::Down);
    assert_eq!(app.command_buffer.as_deref(), Some("betax"));
}

/// Spec 0246 test-plan item 11 (S11). One history across all three
/// panes, as vim keeps one across buffers — a per-pane history would
/// make `Up` in a side pane skip a pattern typed seconds earlier in the
/// main one.
#[test]
fn the_search_history_is_shared_across_panes() {
    let mut app = message_node_app();
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "message");

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(app.override_focus);
    app.override_candidates = vec![("pkg.Alpha".to_string(), None)];
    app.override_highlight = 0;

    type_keys(&mut app, "/");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("message"));
}

/// Spec 0246 test-plan item 12 (S12). A history of one pattern typed
/// three times is a history of one pattern: the repeat moves to the end
/// rather than being stored again, so `Up` never offers the same text
/// twice in a row.
#[test]
fn a_repeated_pattern_moves_to_the_end_of_the_history() {
    let mut app = sibling_leaves_app(&["alpha: 1", "beta: 2"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "alpha");
    commit_search_by_key(&mut app, "beta");
    commit_search_by_key(&mut app, "alpha");

    type_keys(&mut app, "/");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("alpha"));
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("beta"));
    press(&mut app, KeyCode::Up);
    assert_eq!(
        app.command_buffer.as_deref(),
        Some("beta"),
        "the repeat moved rather than adding a third entry"
    );
}

/// Spec 0246 test-plan item 13 (G2, S18, S19). `Ctrl-Right` moves the
/// preview on and nothing else: the cursor is where it was, the
/// jumplist is empty, and `search_origin` still names the row the prompt
/// opened over.
#[test]
fn ctrl_right_previews_the_next_match_without_moving_the_cursor() {
    let mut app = sibling_leaves_app(&["ab", "ab", "ab"]);
    app.splash = false;
    app.term_width = 120;
    let before = (app.cursor, app.cursor_line_in_node, app.cursor_column);

    type_keys(&mut app, "/ab");
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), Some((1, 0, 2, false)));

    press_ctrl(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), Some((2, 0, 2, false)));

    assert_eq!(
        (app.cursor, app.cursor_line_in_node, app.cursor_column),
        before
    );
    assert!(app.back_stack.is_empty(), "a preview is not a jump");
}

/// Spec 0246 test-plan item 14 (S17, S20). The arrow means what it
/// draws: `Ctrl-Left` goes backward through the document even inside a
/// `/` prompt, and leaves the prompt a `/` prompt — so `Enter` still
/// records a forward search and `n` still means forward.
#[test]
fn ctrl_left_rotates_backward_in_a_forward_prompt() {
    let mut app = sibling_leaves_app(&["ab", "ab", "ab"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/ab");
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), Some((1, 0, 2, false)));

    press_ctrl(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), Some((0, 0, 2, false)));
    assert_eq!(
        app.command_kind,
        CommandLineKind::Search(SearchDir::Forward)
    );
}

/// Spec 0246 test-plan item 15 (S18). The rotation's fresh sweep starts
/// *at* the shown match, so S2's two-visit walk steps off it and cycles
/// back onto it: a pattern with one match rotates to itself, either way,
/// rather than falling to "not found".
#[test]
fn rotation_wraps_back_to_the_only_match() {
    let mut app = sibling_leaves_app(&["ab", "zz"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/ab");
    settle_sweep(&mut app);
    let only = Some((0, 0, 2, false));
    assert_eq!(app.search_current_cell(), only);

    press_ctrl(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), only);

    press_ctrl(&mut app, KeyCode::Left);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), only);
}

/// Spec 0246 test-plan item 16 (S21). Rotating from a guess is worse
/// than not rotating, so the key is ignored while the sweep has nothing
/// to show — both while it is still walking and once it has finished
/// having missed.
#[test]
fn rotation_is_ignored_while_the_sweep_is_still_walking() {
    let mut app = target_at(2500, 2000, 0);

    type_keys(&mut app, "/target");
    assert_eq!(app.search_sweep_step(), SweepStep::Progressed);
    assert!(app.search_sweep.as_ref().unwrap().found.is_none());
    app.take_search_dirty();
    press_ctrl(&mut app, KeyCode::Right);
    assert!(
        !app.take_search_dirty(),
        "nothing shown is nothing to rotate from"
    );
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(2000));

    type_keys(&mut app, "zzz");
    settle_sweep(&mut app);
    app.take_search_dirty();
    press_ctrl(&mut app, KeyCode::Left);
    assert!(!app.take_search_dirty(), "and a miss is not a match either");
    assert!(app.search_sweep.as_ref().unwrap().is_finished());
}

/// Spec 0246 test-plan item 17 (G3, S23) — the point of the whole spec.
/// `Enter` commits what the prompt is *showing*, which after a rotation
/// is not the first match.
#[test]
fn enter_commits_the_rotated_match_not_the_first_one() {
    let mut app = sibling_leaves_app(&["ab", "ab", "ab"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/ab");
    settle_sweep(&mut app);
    press_ctrl(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell(), Some((2, 0, 2, false)));

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.cursor, 2);
}

/// Spec 0246 test-plan item 18 (G4, S19). A rotation moves the live
/// sweep and not the origin, so the next edit searches from where the
/// prompt was opened — however far the preview had wandered.
#[test]
fn typing_after_a_rotation_searches_from_the_prompts_origin() {
    let mut app = sibling_leaves_app(&["zz", "abc", "abc", "abc"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/ab");
    settle_sweep(&mut app);
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(1));
    for _ in 0..2 {
        press_ctrl(&mut app, KeyCode::Right);
        settle_sweep(&mut app);
    }
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(3));

    type_keys(&mut app, "c");
    settle_sweep(&mut app);
    assert_eq!(
        app.search_current_cell().map(|c| c.0),
        Some(1),
        "the first match after the origin, not after the rotation"
    );
}

/// Spec 0246 test-plan item 19 (S19). Because the origin never moved,
/// `Esc` is still a single restore of scroll and pan — no unwinding of
/// however many rotations happened in between, and still nothing to
/// restore about the cursor.
#[test]
fn esc_after_a_rotation_restores_the_opening_view() {
    let mut app = wide_sibling_scalars_app(400);
    app.node_text[100] = Some(Box::from("target: 0"));
    app.node_text[300] = Some(Box::from("target: 0"));
    app.splash = false;
    app.term_width = 120;
    app.message.clear();
    let view = (app.scroll.index, app.pan_offset);
    let cursor = (app.cursor, app.cursor_line_in_node, app.cursor_column);

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    app.center_search_match(pane());
    press_ctrl(&mut app, KeyCode::Right);
    settle_sweep(&mut app);
    app.center_search_match(pane());
    assert_eq!(app.search_current_cell().map(|c| c.0), Some(300));
    assert_ne!((app.scroll.index, app.pan_offset), view);

    press(&mut app, KeyCode::Esc);
    assert_eq!((app.scroll.index, app.pan_offset), view);
    assert_eq!(
        (app.cursor, app.cursor_line_in_node, app.cursor_column),
        cursor
    );
}

/// Spec 0246 test-plan item 20 (N1, S24). At a `:` prompt both pairs are
/// inert — and `Ctrl-Right` is inert rather than falling through to the
/// plain `Right` arm and moving the text cursor, which is what an arm
/// placed after it would have done.
#[test]
fn ctrl_right_at_a_colon_prompt_does_not_rotate() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.term_width = 120;

    press(&mut app, KeyCode::Char(':'));
    type_keys(&mut app, "ab");
    app.command_cursor = 0;

    press_ctrl(&mut app, KeyCode::Right);
    assert_eq!(app.command_cursor, 0, "swallowed, not a plain Right");
    press_ctrl(&mut app, KeyCode::Left);
    assert_eq!(app.command_cursor, 0);
    assert!(app.search_sweep.is_none());
}

/// Spec 0246 N1, the other half of test-plan item 20: no `:` history.
#[test]
fn up_at_a_colon_prompt_is_still_inert() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.term_width = 120;

    commit_search_by_key(&mut app, "alpha");

    press(&mut app, KeyCode::Char(':'));
    type_keys(&mut app, "q");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.command_buffer.as_deref(), Some("q"));
    press(&mut app, KeyCode::Down);
    assert_eq!(app.command_buffer.as_deref(), Some("q"));
}

/// Spec 0235 test-plan item 23 (S1). The `memchr2` prefilter is guarded
/// on the needle's first character being ASCII *and* the haystack being
/// ASCII too, because folding is only one byte to one byte inside
/// ASCII: `U+212A` KELVIN SIGN folds to `k`, which no scan for `k` or
/// `K` would ever land on.
#[test]
fn the_prefilter_preserves_smartcase() {
    // The prefilter's own case, which is the overwhelming majority.
    let ascii = SearchPattern::new("beta");
    assert!(ascii.is_match("BETA: 1"));
    assert_eq!(ascii.find("x beta"), Some(2));

    // A non-ASCII needle skips it on the first guard.
    let accented = SearchPattern::new("é");
    assert!(accented.is_match("café: 1"));
    assert!(accented.is_match("CAFÉ: 1"));

    // An ASCII needle against a non-ASCII haystack skips it on the
    // second, which is the guard a first-character test alone misses.
    let k = SearchPattern::new("k");
    assert!(k.is_match("\u{212A}elvin: 1"));
}
