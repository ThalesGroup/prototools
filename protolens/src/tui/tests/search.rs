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
            app.node_at_footer_line(line).is_none() && needle.find(&lines[line]).is_some()
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
