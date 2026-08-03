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

/// Where a linear scan of the whole rendered text would land, starting
/// one line off `from` and moving in `dir` — the `App::lines` scan spec
/// 0222 S6 replaced, written out so the walk can be held against it.
///
/// Deliberately the crudest possible implementation: a `Vec<String>`,
/// modular arithmetic, and no structure at all. It shares nothing with
/// the walk but the matcher, which has its own tests above and is not
/// what is on trial here.
///
/// A closing brace is skipped because it draws no content of its own —
/// it is the one line whose characters the nodes do not store, and the
/// search has never matched one.
///
/// Spec 0235 S19: a line's owning node's positional path is a second
/// haystack, so the oracle tries it too. Without it the oracle would
/// disagree with the walk on every row whose path happens to contain
/// the pattern — and `1` is a pattern most paths contain.
fn scan_the_whole_text(app: &App, from: usize, dir: SearchDir, pattern: &str) -> Option<usize> {
    let lines = app.document_lines();
    let n = lines.len();
    let needle = SearchPattern::new(pattern);
    (1..=n)
        .map(|k| match dir {
            SearchDir::Forward => (from + k) % n,
            SearchDir::Backward => (from + n - k) % n,
        })
        .find(|&line| {
            let Some(pos) = app.line_pos(line) else {
                return false;
            };
            !app.is_footer(pos)
                && (needle.find(&lines[line]).is_some()
                    || needle.is_match(&app.positional_path(pos.node)))
        })
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
                let mut want = Vec::new();
                let mut at = app.cursor_line();
                for _ in 0..12 {
                    let Some(next) = scan_the_whole_text(&app, at, dir, pattern) else {
                        break;
                    };
                    want.push(next);
                    at = next;
                }
                assert!(
                    want.len() > 3,
                    "{name}/{pattern:?}: the fixture must offer several \
                     matches, or the order proves nothing"
                );

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
        (app.scroll_offset..app.scroll_offset + pane().height as usize).contains(&row),
        "the match must be on screen: row {row} against scroll {}",
        app.scroll_offset
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

    let mut settle = |app: &mut App| {
        settle_sweep(app);
        app.center_search_match(pane());
        (app.scroll_offset, app.pan_offset)
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
    let view = (app.scroll_offset, app.pan_offset);
    let cursor = (app.cursor, app.cursor_line_in_node, app.cursor_column);

    type_keys(&mut app, "/target");
    settle_sweep(&mut app);
    app.center_search_match(pane());
    assert_ne!((app.scroll_offset, app.pan_offset), view);
    assert_eq!(
        (app.cursor, app.cursor_line_in_node, app.cursor_column),
        cursor
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!((app.scroll_offset, app.pan_offset), view);
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
    let before = (app.scroll_offset, app.pan_offset);
    app.center_search_match(pane());
    assert_eq!((app.scroll_offset, app.pan_offset), before);
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
    assert_eq!(app.scroll_offset, 146);
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

/// Spec 0235 test-plan item 18 (S18). An unbound control chord is
/// ignored, not typed: `Ctrl-u` used to insert a bare `u`.
#[test]
fn a_control_modified_letter_is_not_inserted() {
    let mut app = sibling_leaves_app(&["alpha: 1"]);
    app.splash = false;
    app.term_width = 120;

    type_keys(&mut app, "/abc");
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(app.command_buffer.as_deref(), Some("abc"));
    assert_eq!(app.command_cursor, 3);
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
