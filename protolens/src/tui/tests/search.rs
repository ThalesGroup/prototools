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
/// the *current* rendered text (`self.lines`), so a range whose type
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
    app.lines[1] = "pkg.Overridden { x: 1 }".to_string();

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
